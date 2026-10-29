//! Running blends (blend.rs): each model's part on its own connection, then the join on the
//! organisation's ScramDB.

use std::sync::Arc;
use std::time::Instant;

use chrono::{DateTime, Utc};

use axum::Json;
use axum::extract::{Path, Query as UrlQuery, State};
use serde_json::{Value, json};

use super::{
    Body, Outcome, Source, cache_later, cache_store, cached, compile_for, connection_for, query_failed, run_model,
};
use crate::access::{model_available, row_filters_for};
use crate::auth::{Auth, Caller};
use crate::blend::{Blend, Joined, MAX_SOURCE_ROWS, Plan, plan, prefix_columns};
use crate::cache;
use crate::compiler::{Compiled, Dialect, compile_query, compile_query_up_to, humanize};
use crate::connectors::Connector;
use crate::connectors::postgres::{col_type, row_json};
use crate::error::{ApiError, ApiResult};
use crate::materialize;
use crate::period::now_rounded;
use crate::schema::Query;
use crate::state::{AppState, Shared};

// --------------------------------------------------------------------------- definitions

/// The blends this person can use: the project's (defined in code) and the organisation's own
/// (`editable`, by power users and up).
pub async fn list(Auth(caller): Auth) -> ApiResult<Json<Value>> {
    caller.require("view")?;
    let blends: Vec<Value> = caller
        .project
        .blends
        .values()
        .filter(|b| model_available(&caller.scope, &caller.project, &b.name))
        .map(|b| {
            let mut v = json!(b);
            v["editable"] = json!(caller.project.org_blends.contains(&b.name));
            v
        })
        .collect();
    Ok(Json(json!({"blends": blends})))
}

pub async fn save(
    State(state): State<Shared>,
    Auth(caller): Auth,
    Path(name): Path<String>,
    UrlQuery(p): UrlQuery<super::dashboards::WriteParams>,
    Body(b): Body<Blend>,
) -> ApiResult<Json<Value>> {
    caller.require("edit")?;
    if b.name != name {
        return Err(ApiError::bad_request("the blend's name must match the address it's saved to"));
    }
    // models this person can't see don't exist as far as they can tell
    for m in std::iter::once(&b.base).chain(b.joins.iter().map(|j| &j.model)) {
        if !model_available(&caller.scope, &caller.project, m) {
            return Err(ApiError::bad_request(format!("unknown model {m:?}")));
        }
    }
    // the project as this person sees it, minus the blend being saved: an organisation's blend
    // never replaces a model or another blend
    let mut project = (*caller.project).clone();
    if project.org_blends.remove(&b.name) {
        project.blends.remove(&b.name);
        project.models.remove(&b.name);
    }
    project.check_new_blend(&b).map_err(ApiError::bad_request)?;
    if p.dry_run {
        return Ok(Json(json!({"ok": true, "dry_run": true})));
    }
    state.orgs.store(caller.org()).await?.put_blend(caller.org(), &b, Some(caller.email())).await?;
    state.blends.forget(caller.org());
    state.usage.record(crate::usage::Event { model: Some(name), ..caller.event("blend_saved") });
    Ok(Json(json!({"ok": true})))
}

pub async fn delete(
    State(state): State<Shared>,
    Auth(caller): Auth,
    Path(name): Path<String>,
) -> ApiResult<Json<Value>> {
    caller.require("edit")?;
    if !state.orgs.store(caller.org()).await?.delete_blend(caller.org(), &name).await? {
        return Err(ApiError::not_found(format!("no blend named {name:?}")));
    }
    state.blends.forget(caller.org());
    state.usage.record(crate::usage::Event { model: Some(name), ..caller.event("blend_deleted") });
    Ok(Json(json!({"ok": true})))
}

// --------------------------------------------------------------------------- queries

type Part = (Dialect, Arc<dyn Connector>, Compiled);

/// Every model's part (in its connection's dialect, with the person's restrictions on that
/// model), the join over placeholder tables, and all of it as one SQL text.
async fn compile(
    state: &AppState,
    caller: &Caller,
    j: &Joined,
    now: DateTime<Utc>,
) -> ApiResult<(Vec<Part>, Compiled, String)> {
    let mut parts = vec![];
    for side in &j.sides {
        let (dialect, connector) = connection_for(state, caller, &side.source).await?;
        let mandatory = row_filters_for(&caller.scope, &caller.project, &side.model)?;
        // aggregations of its columns (`sum(fare)`), and the fields behind sum_if and the like
        let (source, query) = crate::compiler::expand(&side.source, &side.query, dialect)?;
        let compiled = compile_query_up_to(&source, &query, dialect, now, &mandatory, MAX_SOURCE_ROWS)?;
        parts.push((dialect, connector, compiled));
    }
    let placeholders: Vec<String> = (0..j.sides.len()).map(|i| format!("source_{i}")).collect();
    let shown = compile_query(&j.joined_model(&placeholders), &j.query, Dialect::Postgres, now, &[])?;
    let mut sql = String::new();
    for (i, (side, (dialect, _, c))) in j.sides.iter().zip(&parts).enumerate() {
        sql.push_str(&format!(
            "-- source_{i}: {} on connection {} ({dialect:?})\n{}\n\n",
            side.model, side.source.connection, c.sql
        ));
    }
    sql.push_str(&format!("-- joined on ScramDB\n{}", shown.sql));
    Ok((parts, shown, sql))
}

/// The SQL a blend query would run (Explore's preview), without running it.
pub async fn preview(state: &AppState, caller: &Caller, blend: &Blend, q: &Query) -> ApiResult<String> {
    let now = now_rounded();
    match plan(blend, &caller.project.models, q, now)? {
        Plan::Single { model, query } => {
            let (dialect, _) = connection_for(state, caller, &caller.project.models[&model]).await?;
            Ok(compile_for(caller, &query, dialect)?.sql)
        }
        Plan::Joined(j) => Ok(compile(state, caller, &j, now).await?.2),
    }
}

pub async fn execute(
    state: &AppState,
    caller: &Caller,
    blend: &Blend,
    q: &Query,
    source: Source<'_>,
) -> ApiResult<Outcome> {
    let now = now_rounded();
    let j = match plan(blend, &caller.project.models, q, now)? {
        Plan::Single { model, query } => {
            let label = caller.project.models[&model].label.clone().unwrap_or_else(|| humanize(&model));
            let mut out = run_model(state, caller, &query, source).await?;
            prefix_columns(&model, &label, &mut out.columns);
            return Ok(out);
        }
        Plan::Joined(j) => j,
    };
    let started = Instant::now();
    let event = || crate::usage::Event {
        source: Some(source.kind),
        dashboard_id: source.dashboard.map(String::from),
        model: Some(blend.name.clone()),
        connection: Some("blend".into()),
        ..caller.event("query")
    };

    let (parts, shown, sql) = compile(state, caller, &j, now).await?;

    let ttl = blend.cache.unwrap_or(0);
    let store = cache_store(state, caller, ttl).await;
    let key = cache::key(caller.org(), &format!("blend:{}", blend.name), &sql);
    if let Some(hit) = cached(caller, &store, &key).await {
        state.usage.record(crate::usage::Event {
            row_count: Some(hit.rows.len() as i64),
            duration_ms: Some(0),
            cache_hit: Some(true),
            ..event()
        });
        let stats =
            json!({"duration_ms": 0, "cache_hit": true, "cached_at": hit.created_at.to_rfc3339(), "cache_ttl_s": ttl});
        return Ok(Outcome { columns: shown.columns, rows: hit.rows, sql, stats, period: shown.period });
    }

    // run the parts at once, each on its engine
    let mut running = tokio::task::JoinSet::new();
    for (i, (_, connector, c)) in parts.iter().enumerate() {
        let (connector, part_sql) = (connector.clone(), c.sql.clone());
        let names: Vec<String> = c.columns.iter().map(|c| c.name.clone()).collect();
        running.spawn(async move { (i, connector.run(&part_sql, &names).await) });
    }
    let mut results = vec![None; parts.len()];
    while let Some(done) = running.join_next().await {
        let (i, r) = done.map_err(|e| ApiError::Internal(anyhow::anyhow!(e)))?;
        let r = r.map_err(|e| query_failed(state, caller, &e, event(), &j.sides[i].model))?;
        if r.rows.len() as i64 >= MAX_SOURCE_ROWS {
            return Err(ApiError::bad_request(format!(
                "{} returns more than {MAX_SOURCE_ROWS} rows at the join keys: filter it, or join at a longer grain",
                j.sides[i].model
            )));
        }
        results[i] = Some(r);
    }
    let results: Vec<_> = results.into_iter().map(|r| r.expect("every part ran")).collect();
    let sources: Vec<_> = j
        .sides
        .iter()
        .zip(&results)
        .map(|(s, r)| json!({"model": s.model, "rows": r.rows.len(), "duration_ms": r.duration_ms}))
        .collect();

    // join on the organisation's ScramDB: the parts as tables, checked, joined, dropped
    let joining = Instant::now();
    let db = state.orgs.store(caller.org()).await?;
    let tables: Vec<String> = results.iter().map(|_| materialize::work_table_name()).collect();
    let joined = async {
        for (t, r) in tables.iter().zip(&results) {
            materialize::copy(&db.pool, t, &r.types, &r.rows).await?;
        }
        let c = db.pool.get().await?;
        for (check, why) in j.fanout_checks(&tables) {
            if !c.query(&check, &[]).await.map_err(anyhow::Error::from)?.is_empty() {
                return Ok(Err(ApiError::bad_request(why)));
            }
        }
        let compiled = compile_query(&j.joined_model(&tables), &j.query, Dialect::Postgres, now, &[])?;
        let stmt = c.prepare(&compiled.sql).await.map_err(anyhow::Error::from)?;
        let types: Vec<_> = stmt.columns().iter().map(|c| col_type(c.type_())).collect();
        let rows: Vec<_> = c.query(&stmt, &[]).await.map_err(anyhow::Error::from)?.iter().map(row_json).collect();
        Ok::<_, ApiError>(Ok((rows, types)))
    }
    .await;
    for t in &tables {
        if let Err(e) = materialize::drop(&db.pool, "public", t).await {
            tracing::warn!(table = t, "blend table not dropped (a cache sweep will): {e:#}");
        }
    }
    let (rows, types) = joined??;

    let duration_ms = started.elapsed().as_millis() as u64;
    state.usage.record(crate::usage::Event {
        row_count: Some(rows.len() as i64),
        duration_ms: Some(duration_ms as i64),
        cache_hit: Some(false),
        detail: Some(json!({"sources": sources})),
        ..event()
    });
    let names: Vec<String> = shown.columns.iter().map(|c| c.name.clone()).collect();
    cache_later(
        store,
        key,
        (caller.org().to_string(), blend.name.clone(), "blend".into(), ttl),
        (names, types, rows.clone()),
    );
    let mut stats = json!({"duration_ms": duration_ms, "cache_hit": false, "sources": sources,
                           "join_ms": joining.elapsed().as_millis() as u64});
    if ttl > 0 {
        stats["cache_ttl_s"] = json!(ttl);
    }
    Ok(Outcome { columns: shown.columns, rows, sql, stats, period: shown.period })
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use serde_json::{Value, json};

    use crate::api::tests::{app_with, call, scramdb_config};

    /// A project whose models read `connection` (its own: tests run in parallel and others
    /// change `warehouse`).
    fn project(connection: &str) -> String {
        let dir = std::env::temp_dir().join(format!("magnum-blend-{}", uuid::Uuid::new_v4().simple()));
        let files = [
            (
                "models/sales.yaml",
                std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../project/models/sales.yaml"))
                    .unwrap()
                    .replace("connection: warehouse", &format!("connection: {connection}")),
            ),
            (
                "models/targets.yaml",
                "name: targets\nconnection: CONN\nsql: SELECT region, 1000 AS target FROM sample_sales GROUP BY region\n\
                 dimensions: [{name: region}]\nmeasures: [{name: target, type: sum, sql: target}]\n"
                    .replace("CONN", connection),
            ),
            ("blends/sales_targets.yaml", "name: sales_targets\nbase: sales\njoins: [{model: targets, on: [region = region]}]\n".to_string()),
        ];
        for (path, text) in files {
            let p = dir.join(path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
        dir.display().to_string()
    }

    #[tokio::test]
    async fn people_define_blends_for_their_organisation() {
        let Some((app, _, cookie)) = crate::api::tests::app().await else { return };
        let name = format!("sw_{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
        let path = format!("/api/blends/{name}");
        let def = json!({"name": name, "label": "Sales and weather", "base": "sales",
                         "joins": [{"model": "weather", "on": ["region = borough"]}], "cache": "10m"});
        let (s, body) = call(&app, &cookie, "PUT", &path, Some(def.clone())).await;
        assert_eq!(s, StatusCode::OK, "{body}");

        // listed with the models, and among the blends as the organisation's own (editable)
        let (_, models) = call(&app, &cookie, "GET", "/api/models", None).await;
        let m = models["models"].as_array().unwrap().iter().find(|m| m["name"] == json!(name)).cloned().unwrap();
        assert_eq!(m["label"], json!("Sales and weather"));
        assert!(m["dimensions"].as_array().unwrap().iter().any(|d| d["name"] == json!("weather.condition")));
        let (_, list) = call(&app, &cookie, "GET", "/api/blends", None).await;
        let mine = list["blends"].as_array().unwrap().iter().find(|b| b["name"] == json!(name)).cloned().unwrap();
        assert_eq!(mine["editable"], json!(true), "{mine}");
        assert_eq!(mine["joins"][0]["on"][0], json!("region = borough"));
        assert!(
            list["blends"]
                .as_array()
                .unwrap()
                .iter()
                .any(|b| b["name"] == json!("taxi_weather") && b["editable"] == json!(false))
        );

        // checked before saving
        let bad = |patch: Value| {
            let mut d = def.clone();
            d.as_object_mut().unwrap().extend(patch.as_object().unwrap().clone());
            d
        };
        for (body, why) in [
            (bad(json!({"joins": [{"model": "weather", "on": ["region = nope"]}]})), "nope"),
            (bad(json!({"name": "other"})), "name"),
        ] {
            let (s, r) = call(&app, &cookie, "PUT", &path, Some(body)).await;
            assert_eq!(s, StatusCode::BAD_REQUEST, "{r}");
            assert!(r["detail"].as_str().unwrap().contains(why), "{r}");
        }
        let (s, r) = call(&app, &cookie, "PUT", "/api/blends/sales", Some(bad(json!({"name": "sales"})))).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "a model's name: {r}");
        let (s, r) =
            call(&app, &cookie, "PUT", "/api/blends/taxi_weather", Some(bad(json!({"name": "taxi_weather"})))).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "defined in the project: {r}");

        let (s, _) = call(&app, &cookie, "DELETE", &path, None).await;
        assert_eq!(s, StatusCode::OK);
        let (_, models) = call(&app, &cookie, "GET", "/api/models", None).await;
        assert!(!models.to_string().contains(&name));
        assert_eq!(call(&app, &cookie, "DELETE", &path, None).await.0, StatusCode::NOT_FOUND);
    }

    fn column(body: &Value, i: usize) -> Vec<Value> {
        body["rows"].as_array().unwrap().iter().map(|r| r[i].clone()).collect()
    }

    #[tokio::test]
    async fn blends_join_models_on_scramdb() {
        let conn = format!("b{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
        let Some((app, _, cookie)) = app_with(&project(&conn)).await else { return };
        let path = format!("/api/connections/{conn}");
        call(&app, &cookie, "PUT", &path, Some(json!({"config": scramdb_config()}))).await;
        let (s, direct) = call(
            &app,
            &cookie,
            "POST",
            "/api/query",
            Some(json!({"model": "sales", "dimensions": ["region"], "measures": ["revenue"], "sort": ["region"]})),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{direct}");

        // both models: each aggregated on its connection, joined on ScramDB
        let (s, body) = call(&app, &cookie, "POST", "/api/query", Some(json!({"model": "sales_targets",
            "dimensions": ["sales.region"], "measures": ["sales.revenue", "targets.target"], "sort": ["sales.region"]}))).await;
        assert_eq!(s, StatusCode::OK, "{body}");
        let names: Vec<&str> =
            body["columns"].as_array().unwrap().iter().map(|c| c["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["sales.region", "sales.revenue", "targets.target"]);
        assert_eq!(column(&body, 0), column(&direct, 0));
        for (a, b) in column(&body, 1).iter().zip(column(&direct, 1)) {
            assert!((a.as_f64().unwrap() - b.as_f64().unwrap()).abs() < 0.01, "{a} vs {b}");
        }
        assert!(column(&body, 2).iter().all(|t| t.as_f64() == Some(1000.0)), "{body}");
        assert!(body["sql"].as_str().unwrap().contains("joined on ScramDB"), "{body}");
        assert_eq!(body["stats"]["sources"].as_array().unwrap().len(), 2, "{body}");

        // one model: straight to it, under the blend's names
        let (s, one) = call(
            &app,
            &cookie,
            "POST",
            "/api/query",
            Some(json!({"model": "sales_targets",
            "dimensions": ["sales.region"], "measures": ["sales.revenue"], "sort": ["sales.region"]})),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{one}");
        assert_eq!(one["columns"][1]["name"], json!("sales.revenue"));
        assert_eq!(column(&one, 1), column(&direct, 1));

        // formulas: on totals across both models, after the join; of each row, on their model
        let (s, f) = call(
            &app,
            &cookie,
            "POST",
            "/api/query",
            Some(json!({"model": "sales_targets",
            "dimensions": ["sales.region"], "measures": ["sales.revenue", "of_target"], "sort": ["sales.region"],
            "formulas": [{"name": "of_target", "expr": "sales.revenue / targets.target", "format": "percent"}]})),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{f}");
        let names: Vec<&str> = f["columns"].as_array().unwrap().iter().map(|c| c["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["sales.region", "sales.revenue", "of_target"], "the target only feeds the formula");
        for (a, b) in column(&f, 2).iter().zip(column(&direct, 1)) {
            assert!((a.as_f64().unwrap() - b.as_f64().unwrap() / 1000.0).abs() < 1e-6, "{a} vs {b} / 1000");
        }
        let big = json!([{"name": "big", "expr": "sales.amount > 100", "per_row": true}]);
        let (s, by_big) = call(
            &app,
            &cookie,
            "POST",
            "/api/query",
            Some(json!({"model": "sales_targets",
            "dimensions": ["big"], "measures": ["sales.revenue", "targets.target"], "formulas": big})),
        )
        .await;
        // targets are per region, so not by a field of each sale: the same check as for any field
        assert_eq!(s, StatusCode::BAD_REQUEST, "{by_big}");
        let (s, by_big) = call(
            &app,
            &cookie,
            "POST",
            "/api/query",
            Some(json!({"model": "sales_targets",
            "dimensions": ["sales.region", "big"], "measures": ["sales.revenue"], "formulas": big,
            "filters": [{"field": "big", "op": "equals", "values": [true]}]})),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{by_big}");
        let (_, raw) = call(
            &app,
            &cookie,
            "POST",
            "/api/query",
            Some(json!({"model": "sales", "dimensions": ["region"],
            "measures": ["revenue"], "filters": [{"field": "amount", "op": "gt", "values": [100]}]})),
        )
        .await;
        let total = |b: &Value, i: usize| column(b, i).iter().map(|v| v.as_f64().unwrap()).sum::<f64>();
        assert!((total(&by_big, 2) - total(&raw, 1)).abs() < 0.01, "{by_big} vs {raw}");

        // aggregations of a model's column, and totals with a condition, on a blend
        let (s, agg) = call(&app, &cookie, "POST", "/api/query", Some(json!({"model": "sales_targets",
            "dimensions": ["sales.region"], "measures": ["sum(sales.amount)", "big_share", "big_n"], "sort": ["sales.region"],
            "formulas": [{"name": "big_share", "expr": "sum_if(sales.amount, sales.amount > 100) / targets.target"},
                         {"name": "big_n", "expr": "count_if(sales.amount > 100)"}]}))).await;
        assert_eq!(s, StatusCode::OK, "{agg}");
        for (a, b) in column(&agg, 1).iter().zip(column(&direct, 1)) {
            assert!((a.as_f64().unwrap() - b.as_f64().unwrap()).abs() < 0.01, "sum(sales.amount) {a} vs revenue {b}");
        }
        let (_, big) = call(
            &app,
            &cookie,
            "POST",
            "/api/query",
            Some(json!({"model": "sales", "dimensions": ["region"],
            "measures": ["revenue", "transactions"], "sort": ["region"],
            "filters": [{"field": "amount", "op": "gt", "values": [100]}]})),
        )
        .await;
        for (i, r) in agg["rows"].as_array().unwrap().iter().enumerate() {
            let want = &big["rows"][i];
            assert!((r[2].as_f64().unwrap() - want[1].as_f64().unwrap() / 1000.0).abs() < 1e-6, "{r} vs {want}");
            assert_eq!(r[3].as_f64(), want[2].as_f64(), "{r} vs {want}");
        }

        // a shared formula on the blend: for everyone, used by name
        let shared = json!({"name": "per_target", "label": "Of target", "expr": "sales.revenue / targets.target"});
        let (s, b) = call(&app, &cookie, "PUT", "/api/formulas/sales_targets/per_target", Some(shared)).await;
        assert_eq!(s, StatusCode::OK, "{b}");
        let (_, models) = call(&app, &cookie, "GET", "/api/models", None).await;
        let listed = models.to_string();
        assert!(listed.contains("per_target") && listed.contains("Of target"), "listed with the blend's fields");
        let (s, by_name) = call(
            &app,
            &cookie,
            "POST",
            "/api/query",
            Some(json!({"model": "sales_targets",
            "dimensions": ["sales.region"], "measures": ["per_target"], "sort": ["sales.region"]})),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{by_name}");
        for (a, b) in column(&by_name, 1).iter().zip(column(&direct, 1)) {
            assert!((a.as_f64().unwrap() - b.as_f64().unwrap() / 1000.0).abs() < 1e-6, "{a} vs {b} / 1000");
        }
        // one that can't be added up after the join isn't saved
        let spread = json!({"name": "spread", "expr": "stddev(sales.amount)"});
        let (s, b) = call(&app, &cookie, "PUT", "/api/formulas/sales_targets/spread", Some(spread)).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{b}");
        let (s, _) = call(&app, &cookie, "DELETE", "/api/formulas/sales_targets/per_target", None).await;
        assert_eq!(s, StatusCode::OK);

        // the editor checks them as they're typed, as a query on the blend would
        let check = |expr: &str, per_row: bool| json!({"model": "sales_targets", "expr": expr, "per_row": per_row});
        for (expr, per_row, want) in [
            ("sales.revenue / targets.target", false, json!({"ok": true})),
            ("sales.amount > 100", true, json!({"ok": true, "type": "boolean"})),
            ("concat(sales.region, '-', sales.store)", true, json!({"ok": true, "type": "string"})),
            ("sum_if(sales.amount, sales.amount > 1) / targets.target", false, json!({"ok": true})),
        ] {
            let (s, b) = call(&app, &cookie, "POST", "/api/formulas/check", Some(check(expr, per_row))).await;
            assert_eq!((s, &b), (StatusCode::OK, &want), "{expr}");
        }
        for (expr, per_row, why) in [
            ("sales.revenue / targets.nope", false, "nope"),
            ("stddev(sales.amount)", false, "added up again"),
            ("sales.revenue + 'x'", false, "number"),
            ("concat(sales.region, targets.region)", true, "one model's fields"),
        ] {
            let (s, b) = call(&app, &cookie, "POST", "/api/formulas/check", Some(check(expr, per_row))).await;
            assert_eq!(s, StatusCode::OK, "{b}");
            assert_eq!(b["ok"], json!(false), "{expr}: {b}");
            let error = b["error"].as_str().unwrap();
            assert!(error.contains(why) && !error.contains("formula__"), "{expr}: {b}");
        }

        // targets are per region: by store they'd be counted once per store
        let (s, body) = call(
            &app,
            &cookie,
            "POST",
            "/api/query",
            Some(json!({"model": "sales_targets",
            "dimensions": ["sales.store"], "measures": ["targets.target"]})),
        )
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
        assert!(body["detail"].as_str().unwrap().contains("more than once"), "{body}");

        // SQL preview (Explore): every part and the join, without running anything
        let (s, preview) = call(
            &app,
            &cookie,
            "POST",
            "/api/query/sql",
            Some(json!({"model": "sales_targets",
            "dimensions": ["sales.region"], "measures": ["sales.revenue", "targets.target"]})),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{preview}");
        let sql = preview["sql"].as_str().unwrap();
        assert!(sql.contains("source_1: targets") && sql.contains("joined on ScramDB"), "{sql}");

        // the blend is listed like a model
        let (_, models) = call(&app, &cookie, "GET", "/api/models", None).await;
        let b = models["models"].as_array().unwrap().iter().find(|m| m["name"] == json!("sales_targets")).unwrap();
        assert_eq!(b["connection_type"], json!("blend"));
        assert!(b["dimensions"].as_array().unwrap().iter().any(|d| d["name"] == json!("sales.store")));
        call(&app, &cookie, "DELETE", &path, None).await;
    }
}
