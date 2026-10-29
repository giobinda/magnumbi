//! Shared formulas: calculations Org admins add to a model (`margin = (revenue - cost) / revenue`),
//! for everyone in the organisation, as measures of that model. Formulas carry no SQL (see
//! formula.rs), and they're saved per organisation in its own database.

use axum::Json;
use axum::extract::{Path, Query as UrlQuery, State};
use serde::Deserialize;
use serde_json::{Value, json};

use super::Body;
use crate::auth::{Auth, Caller};
use crate::error::{ApiError, ApiResult};
use crate::schema::Formula;
use crate::state::Shared;

/// Shared with everyone in the organisation, so its admins decide (policies may refine this later).
fn require_admin(caller: &Caller) -> ApiResult<()> {
    caller.require("manage_members")
}

/// The organisation's shared formulas, as saved.
pub async fn list(State(state): State<Shared>, Auth(caller): Auth) -> ApiResult<Json<Value>> {
    require_admin(&caller)?;
    let formulas = state.orgs.store(caller.org()).await?.formulas(caller.org()).await?;
    let out: Vec<Value> = formulas
        .into_iter()
        .map(|(model, f, at, by)| {
            let problem = caller.project.errors.get(&format!("formula {model}.{}", f.name));
            json!({"model": model, "formula": f, "updated_at": at, "updated_by": by, "problem": problem})
        })
        .collect();
    Ok(Json(json!({"formulas": out})))
}

/// Save a shared formula (`?dry_run=1`: check only).
pub async fn save(
    State(state): State<Shared>,
    Auth(caller): Auth,
    Path((model, name)): Path<(String, String)>,
    UrlQuery(p): UrlQuery<super::dashboards::WriteParams>,
    Body(f): Body<Formula>,
) -> ApiResult<Json<Value>> {
    require_admin(&caller)?;
    if f.name != name {
        return Err(ApiError::bad_request("the formula's name must match the address it's saved to"));
    }
    super::model_of(&caller, &model)?;
    caller.project.formula_fits(&model, &f).map_err(ApiError::bad_request)?;
    if p.dry_run {
        return Ok(Json(json!({"ok": true, "dry_run": true})));
    }
    state.orgs.store(caller.org()).await?.put_formula(caller.org(), &model, &f, Some(caller.email())).await?;
    state.formulas.forget(caller.org());
    state.usage.record(crate::usage::Event { model: Some(model), ..caller.event("formula_saved") });
    Ok(Json(json!({"ok": true})))
}

pub async fn delete(
    State(state): State<Shared>,
    Auth(caller): Auth,
    Path((model, name)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    require_admin(&caller)?;
    if !state.orgs.store(caller.org()).await?.delete_formula(caller.org(), &model, &name).await? {
        return Err(ApiError::not_found(format!("no shared formula {name:?} on {model:?}")));
    }
    state.formulas.forget(caller.org());
    state.usage.record(crate::usage::Event { model: Some(model), ..caller.event("formula_deleted") });
    Ok(Json(json!({"ok": true})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Check {
    model: String,
    expr: String,
    #[serde(default)]
    per_row: bool,
    /// the chart's other formulas, which this one may use
    #[serde(default)]
    formulas: Vec<Formula>,
}

/// Checks a formula as it's typed (Explore's editor): fine, or what's wrong and where.
pub async fn check(Auth(caller): Auth, Body(c): Body<Check>) -> ApiResult<Json<Value>> {
    caller.require("explore")?;
    let mut model = super::model_of(&caller, &c.model)?.clone();
    if let Some(b) = caller.project.blends.get(&c.model)
        && (c.per_row || !crate::formula::is_on_results(&c.expr).unwrap_or(false))
    {
        return Ok(Json(check_on_blend(&caller, b, &c)));
    }
    if c.formulas.len() > crate::schema::MAX_FORMULAS {
        return Err(ApiError::bad_request("too many formulas"));
    }
    for other in &c.formulas {
        other.check().map_err(ApiError::bad_request)?;
        if model.dimension(&other.name).is_some() || model.measure(&other.name).is_some() {
            continue; // being edited, or a name the model has: the model's wins
        }
        if other.per_row {
            model.dimensions.push(other.as_dimension());
        } else if crate::formula::is_on_results(&other.expr).unwrap_or(true) {
            continue; // a calculation on results isn't a measure to use
        } else {
            model.measures.push(other.as_measure());
        }
    }
    let f = Formula {
        name: "formula__".into(),
        label: None,
        expr: c.expr,
        format: None,
        description: None,
        per_row: c.per_row,
    };
    if f.per_row {
        model.dimensions.push(f.as_dimension());
    } else {
        model.measures.push(f.as_measure());
    }
    // a calculation on the chart's results, or one using the chart's others (`revenue / so_far`)
    let calcs = crate::results::calculations(&c.formulas).unwrap_or_default();
    let uses_calcs = crate::formula::references(&f.expr).is_ok_and(|r| r.iter().any(|r| calcs.contains(r)));
    if !f.per_row && (uses_calcs || crate::formula::is_on_results(&f.expr).unwrap_or(false)) {
        let model = crate::compiler::with_row_formulas(&model, crate::compiler::Dialect::Postgres)?;
        return Ok(Json(match crate::compiler::result_formula_error(&model, &f.expr, &calcs) {
            None => json!({"ok": true}),
            Some(e) => json!({"ok": false, "error": e.message, "at": e.at}),
        }));
    }
    Ok(Json(match crate::compiler::formula_error(&model, &f.name) {
        // a field of each row: what it gives (number, string or boolean, as a dimension's type)
        None if f.per_row => {
            let model = crate::compiler::with_row_formulas(&model, crate::compiler::Dialect::Postgres)?;
            json!({"ok": true, "type": model.dimension(&f.name).map(|d| d.kind.clone())})
        }
        None => json!({"ok": true}),
        Some(e) => json!({"ok": false, "error": e.message, "at": e.at}),
    }))
}

/// A formula on a blend, checked as a query on the blend would use it (blend.rs): planned, every
/// model's part and the join compiled, nothing run. Fine (and, per row, what it gives), or why not.
fn check_on_blend(caller: &crate::auth::Caller, b: &crate::blend::Blend, c: &Check) -> Value {
    const NAME: &str = "formula__";
    let f = Formula {
        name: NAME.into(),
        label: None,
        expr: c.expr.clone(),
        format: None,
        description: None,
        per_row: c.per_row,
    };
    let others: Vec<Formula> =
        c.formulas.iter().filter(|o| !crate::formula::is_on_results(&o.expr).unwrap_or(true)).cloned().collect();
    match crate::blend::check_formula(b, &caller.project.models, &others, &f) {
        Ok(t) if c.per_row => json!({"ok": true, "type": t}),
        Ok(_) => json!({"ok": true}),
        Err(e) => {
            // "formula__: what's wrong (at character 4)": the message, and where
            let message = e.0.strip_prefix(&format!("{NAME}: ")).unwrap_or(&e.0);
            let (message, at) = match message.rsplit_once(" (at character ") {
                Some((m, n)) => (m, n.trim_end_matches(')').parse::<usize>().map_or(0, |n| n.saturating_sub(1))),
                None => (message, 0),
            };
            json!({"ok": false, "error": message, "at": at})
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use serde_json::json;

    use crate::api::share::tests::{clean_up, org_with, send};

    const DASHBOARD: &str = "title: Formulas test\ntiles: []\n";

    #[tokio::test]
    async fn org_admins_share_formulas_and_everyone_uses_them() {
        let Some((app, state, cookie, org)) = org_with(DASHBOARD).await else { return };
        let c = Some(cookie.as_str());
        let name = format!("per_sale_{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
        let path = format!("/api/formulas/sales/{name}");
        let f = json!({"name": name, "label": "Per sale", "expr": "revenue / transactions", "format": "currency"});

        // Org admins only
        let (s, _, _) = send(&app, c, &org, "PUT", &path, Some(f.clone()), Some("power_user")).await;
        assert_eq!(s, StatusCode::FORBIDDEN);
        let (s, _, _) = send(&app, c, &org, "GET", "/api/formulas", None, Some("power_user")).await;
        assert_eq!(s, StatusCode::FORBIDDEN);

        // what's refused, with the reason
        for (edit, says) in [
            (json!({"expr": "revnue / 2"}), "did you mean revenue"),
            (json!({"expr": "revenue +"}), "ends too early"),
            (json!({"name": "revenue"}), "already has a field"),
            (json!({"format": "bytes"}), "format"),
        ] {
            let mut bad = f.clone();
            for (k, v) in edit.as_object().unwrap() {
                bad[k] = v.clone();
            }
            let at = format!("/api/formulas/sales/{}", bad["name"].as_str().unwrap());
            let (s, b, _) = send(&app, c, &org, "PUT", &at, Some(bad), None).await;
            assert_eq!(s, StatusCode::BAD_REQUEST, "{edit} -> {b}");
            assert!(b["detail"].as_str().unwrap().contains(says), "{edit} -> {b}");
        }
        let (s, _, _) = send(&app, c, &org, "PUT", &format!("/api/formulas/nope/{name}"), Some(f.clone()), None).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);

        // a dry run checks only; saving shares it with everyone, as a measure of the model
        let (s, b, _) = send(&app, c, &org, "PUT", &format!("{path}?dry_run=1"), Some(f.clone()), None).await;
        assert_eq!((s, &b["dry_run"]), (StatusCode::OK, &json!(true)), "{b}");
        let (s, b, _) = send(&app, c, &org, "PUT", &path, Some(f.clone()), None).await;
        assert_eq!(s, StatusCode::OK, "{b}");
        let (_, list, _) = send(&app, c, &org, "GET", "/api/formulas", None, None).await;
        assert_eq!(list["formulas"][0]["formula"]["expr"], json!("revenue / transactions"), "{list}");
        let (_, models, _) = send(&app, c, &org, "GET", "/api/models", None, Some("viewer")).await;
        let sales = models["models"].as_array().unwrap().iter().find(|m| m["name"] == "sales").unwrap();
        let m = sales["measures"].as_array().unwrap().iter().find(|m| m["name"] == json!(name)).unwrap();
        assert_eq!(
            (&m["type"], &m["formula"], &m["shared"]),
            (&json!("formula"), &json!("revenue / transactions"), &json!(true))
        );
        let q = json!({"model": "sales", "dimensions": ["region"], "measures": [name]});
        let (s, r, _) = send(&app, c, &org, "POST", "/api/query", Some(q.clone()), Some("power_user")).await;
        assert_eq!(s, StatusCode::OK, "{r}");
        assert_eq!(r["rows"].as_array().unwrap().len(), 3, "{r}");
        // per row: a field everyone can group by
        let line = format!("{name}_line");
        let f2 =
            json!({"name": line, "label": "Big sale", "expr": "if(amount > 200, 'big', 'small')", "per_row": true});
        let (s, b, _) = send(&app, c, &org, "PUT", &format!("/api/formulas/sales/{line}"), Some(f2), None).await;
        assert_eq!(s, StatusCode::OK, "{b}");
        let (_, models, _) = send(&app, c, &org, "GET", "/api/models", None, Some("viewer")).await;
        let sales = models["models"].as_array().unwrap().iter().find(|m| m["name"] == "sales").unwrap();
        let d = sales["dimensions"].as_array().unwrap().iter().find(|d| d["name"] == json!(line)).unwrap();
        assert_eq!(
            (&d["type"], &d["formula"], &d["shared"]),
            (&json!("string"), &json!("if(amount > 200, 'big', 'small')"), &json!(true))
        );
        let q2 = json!({"model": "sales", "dimensions": [line], "measures": ["transactions", "sum(qty)"]});
        let (s, r, _) = send(&app, c, &org, "POST", "/api/query", Some(q2), Some("power_user")).await;
        assert_eq!(s, StatusCode::OK, "{r}");
        let groups: Vec<&str> = r["rows"].as_array().unwrap().iter().map(|x| x[0].as_str().unwrap()).collect();
        assert!(groups.contains(&"big") && groups.contains(&"small"), "{r}");
        let (s, _, _) = send(&app, c, &org, "DELETE", &format!("/api/formulas/sales/{line}"), None, None).await;
        assert_eq!(s, StatusCode::OK);
        // edited in place
        let (s, b, _) =
            send(&app, c, &org, "PUT", &path, Some(json!({"name": name, "expr": "revenue / units"})), None).await;
        assert_eq!(s, StatusCode::OK, "{b}");

        // another organisation doesn't see it
        let other = format!("other{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
        state.orgs.pin(&other, state.store.clone());
        let (_, models, _) = send(&app, c, &other, "GET", "/api/models", None, None).await;
        assert!(!models.to_string().contains(&name));

        let (s, _, _) = send(&app, c, &org, "DELETE", &path, None, None).await;
        assert_eq!(s, StatusCode::OK);
        let (s, _, _) = send(&app, c, &org, "POST", "/api/query", Some(q), None).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        clean_up(&state, &org).await;
    }

    #[tokio::test]
    async fn formulas_run_on_the_warehouse() {
        let Some((app, state, cookie, org)) = org_with(DASHBOARD).await else { return };
        let c = Some(cookie.as_str());
        let q = json!({"model": "sales", "dimensions": ["region"], "measures": ["revenue", "transactions"],
            "formulas": [{"name": "per_sale", "expr": "round(revenue / transactions, 2)", "format": "currency"},
                         {"name": "stores", "expr": "if(count_distinct(store) > 0, count_distinct(store), null)"}]});
        let (s, body, _) = send(&app, c, &org, "POST", "/api/query", Some(q), None).await;
        assert_eq!(s, StatusCode::OK, "{body}");
        let num = |v: &serde_json::Value| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())).unwrap();
        for row in body["rows"].as_array().unwrap() {
            // region, revenue, transactions, per_sale, stores
            assert!((num(&row[3]) - num(&row[1]) / num(&row[2])).abs() < 0.006, "{row}");
            assert!(num(&row[4]) >= 1.0, "{row}");
        }
        // with the previous period alongside
        let q = json!({"model": "sales", "measures": ["per_sale"],
            "period": {"field": "sold_at", "current": {"last_n_days": 90}, "compare": "previous_period"},
            "formulas": [{"name": "per_sale", "expr": "revenue / transactions"}]});
        let (s, body, _) = send(&app, c, &org, "POST", "/api/query", Some(q), None).await;
        assert_eq!(s, StatusCode::OK, "{body}");
        let row = &body["rows"][0]; // per_sale, its previous value, the change
        assert!(num(&row[0]) > 0.0 && num(&row[1]) > 0.0 && row[2].is_number(), "{body}");
        // per row: grouped by a computed number, every sale is counted once
        let total = json!({"model": "sales", "measures": ["transactions"]});
        let (_, t, _) = send(&app, c, &org, "POST", "/api/query", Some(total), None).await;
        let q = json!({"model": "sales", "dimensions": ["band"], "measures": ["transactions", "sum(line)"],
            "formulas": [{"name": "line", "expr": "amount * qty", "per_row": true},
                         {"name": "band", "expr": "round(line / 100) * 100", "per_row": true}]});
        let (s, body, _) = send(&app, c, &org, "POST", "/api/query", Some(q), None).await;
        assert_eq!(s, StatusCode::OK, "{body}");
        let rows = body["rows"].as_array().unwrap();
        let counted: f64 = rows.iter().map(|r| num(&r[1])).sum();
        assert_eq!(counted, num(&t["rows"][0][0]), "{body}");
        let mut bands: Vec<i64> = rows.iter().map(|r| num(&r[0]) as i64).collect();
        let n = bands.len();
        bands.sort_unstable();
        bands.dedup();
        assert!(n > 1 && bands.len() == n && bands.iter().all(|b| b % 100 == 0), "{body}");
        clean_up(&state, &org).await;
    }

    #[tokio::test]
    async fn calculations_on_results_run_on_scramdb() {
        let Some((app, state, cookie, org)) = org_with(DASHBOARD).await else { return };
        let c = Some(cookie.as_str());
        let num = |v: &serde_json::Value| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok()));
        let q = json!({"model": "sales", "dimensions": ["sold_at:month"], "measures": ["revenue", "running", "share", "before", "place"],
            "sort": ["sold_at:month"],
            "formulas": [{"name": "running", "expr": "running_sum(revenue)"},
                         {"name": "share", "expr": "percent_of_total(revenue)"},
                         {"name": "before", "expr": "revenue - previous(revenue)"},
                         {"name": "place", "expr": "rank(revenue)"},
                         {"name": "units3", "label": "Units, 3 months", "expr": "moving_avg(sum(qty), 3)"}]});
        let (s, body, _) = send(&app, c, &org, "POST", "/api/query", Some(q), None).await;
        assert_eq!(s, StatusCode::OK, "{body}");
        let names: Vec<&str> =
            body["columns"].as_array().unwrap().iter().map(|c| c["name"].as_str().unwrap()).collect();
        // sum(qty) only feeds a calculation: not shown
        assert_eq!(names, ["sold_at__month", "revenue", "running", "share", "before", "place", "units3"]);
        assert_eq!(body["columns"][3]["format"], json!("percent"));
        let rows = body["rows"].as_array().unwrap();
        assert!(rows.len() > 3, "{body}");
        let revenue: Vec<f64> = rows.iter().map(|r| num(&r[1]).unwrap()).collect();
        let total: f64 = revenue.iter().sum();
        let mut running = 0.0;
        for (i, r) in rows.iter().enumerate() {
            running += revenue[i];
            let close = |a: f64, b: f64| (a - b).abs() <= 0.01 * b.abs().max(1.0);
            assert!(close(num(&r[2]).unwrap(), running), "running total, row {i}: {r}");
            assert!(close(num(&r[3]).unwrap(), revenue[i] / total), "share, row {i}: {r}");
            match i {
                0 => assert!(r[4].is_null(), "nothing before the first month: {r}"),
                _ => assert!(close(num(&r[4]).unwrap(), revenue[i] - revenue[i - 1]), "change, row {i}: {r}"),
            }
            let place = 1 + revenue.iter().filter(|&&x| x > revenue[i]).count();
            assert_eq!(num(&r[5]).unwrap() as usize, place, "rank, row {i}: {r}");
            assert!(num(&r[6]).unwrap() > 0.0, "{r}");
        }
        // sorted by a calculation; along the months, separately for each region
        let q = json!({"model": "sales", "dimensions": ["region", "sold_at:month"], "measures": ["transactions", "running"],
            "sort": ["-running"], "formulas": [{"name": "running", "expr": "running_sum(transactions)"}]});
        let (s, body, _) = send(&app, c, &org, "POST", "/api/query", Some(q), None).await;
        assert_eq!(s, StatusCode::OK, "{body}");
        let rows = body["rows"].as_array().unwrap();
        let tops: Vec<f64> = rows.iter().map(|r| num(&r[3]).unwrap()).collect();
        assert!(tops.windows(2).all(|w| w[0] >= w[1]), "sorted by the running total: {body}");
        let all: f64 = rows.iter().map(|r| num(&r[2]).unwrap()).sum();
        let per_region: f64 = ["EMEA", "APAC", "AMER"]
            .iter()
            .map(|g| rows.iter().filter(|r| r[0] == json!(g)).map(|r| num(&r[3]).unwrap()).fold(0.0, f64::max))
            .sum();
        assert_eq!(per_region, all, "each region's running total ends at its own total");
        // a mistake is the person's, with a reason
        let q = json!({"model": "sales", "measures": ["revenue"], "formulas": [{"name": "x", "expr": "rank(region)"}]});
        let (s, body, _) = send(&app, c, &org, "POST", "/api/query", Some(q), None).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
        clean_up(&state, &org).await;
    }

    /// What Explore's "Calculate…" menu writes (frontend/src/formula.ts, quickCalcs), on a model's
    /// measure, on a column's total and on the chart's own formula: each runs, and is right.
    #[tokio::test]
    async fn the_quick_calculations_run() {
        let Some((app, state, cookie, org)) = org_with(DASHBOARD).await else { return };
        let c = Some(cookie.as_str());
        let num = |v: &serde_json::Value| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok()));
        let calcs = |x: &str, n: &str| {
            vec![
                json!({"name": format!("{n}_share"), "expr": format!("percent_of_total({x})"), "format": "percent"}),
                json!({"name": format!("{n}_change"), "expr": format!("difference({x})")}),
                json!({"name": format!("{n}_pct"), "expr": format!("percent_change({x})"), "format": "percent"}),
                json!({"name": format!("{n}_running"), "expr": format!("running_sum({x})")}),
                json!({"name": format!("{n}_avg3"), "expr": format!("moving_avg({x}, 3)")}),
                json!({"name": format!("{n}_rank"), "expr": format!("rank({x})")}),
            ]
        };
        let mut formulas = vec![json!({"name": "per_unit", "expr": "revenue / sum(qty)"})];
        formulas.extend(calcs("revenue", "r"));
        formulas.extend(calcs("sum(qty)", "q"));
        formulas.extend(calcs("per_unit", "p"));
        // divided by another measure: a column's total, and the chart's own formula
        formulas.push(json!({"name": "r_per_q", "expr": "revenue / sum(qty)"}));
        formulas.push(json!({"name": "r_per_p", "expr": "revenue / per_unit"}));
        let names: Vec<String> = formulas.iter().map(|f| f["name"].as_str().unwrap().to_string()).collect();
        let mut measures = vec!["revenue".to_string(), "sum(qty)".to_string()];
        measures.extend(names.iter().cloned());
        let q = json!({"model": "sales", "dimensions": ["sold_at:month"], "measures": measures,
            "sort": ["sold_at:month"], "formulas": formulas});
        let (s, body, _) = send(&app, c, &org, "POST", "/api/query", Some(q), None).await;
        assert_eq!(s, StatusCode::OK, "{body}");
        let at =
            |name: &str| body["columns"].as_array().unwrap().iter().position(|c| c["name"] == json!(name)).unwrap();
        let rows = body["rows"].as_array().unwrap();
        assert!(rows.len() > 3, "{body}");
        let close = |a: f64, b: f64| (a - b).abs() <= 0.01 * b.abs().max(1.0);
        for (base, n) in [("revenue", "r"), ("sum__qty", "q"), ("per_unit", "p")] {
            let x: Vec<f64> = rows.iter().map(|r| num(&r[at(base)]).unwrap()).collect();
            let total: f64 = x.iter().sum();
            for (i, r) in rows.iter().enumerate() {
                let v = |c: &str| num(&r[at(&format!("{n}_{c}"))]);
                assert!(close(v("share").unwrap(), x[i] / total), "{n} share, row {i}: {r}");
                assert!(close(v("running").unwrap(), x[..=i].iter().sum()), "{n} running, row {i}: {r}");
                let last3 = &x[i.saturating_sub(2)..=i];
                assert!(
                    close(v("avg3").unwrap(), last3.iter().sum::<f64>() / last3.len() as f64),
                    "{n} avg3, row {i}: {r}"
                );
                let place = 1 + x.iter().filter(|&&y| y > x[i]).count();
                assert_eq!(v("rank").unwrap() as usize, place, "{n} rank, row {i}: {r}");
                if i == 0 {
                    assert!(v("change").is_none() && v("pct").is_none(), "nothing before the first month: {r}");
                } else {
                    assert!(close(v("change").unwrap(), x[i] - x[i - 1]), "{n} change, row {i}: {r}");
                    assert!(close(v("pct").unwrap(), (x[i] - x[i - 1]) / x[i - 1]), "{n} % change, row {i}: {r}");
                }
            }
        }
        for r in rows {
            let (rev, qty, per) =
                (num(&r[at("revenue")]).unwrap(), num(&r[at("sum__qty")]).unwrap(), num(&r[at("per_unit")]).unwrap());
            assert!(close(num(&r[at("r_per_q")]).unwrap(), rev / qty), "{r}");
            assert!(close(num(&r[at("r_per_p")]).unwrap(), rev / per), "{r}");
        }
        assert_eq!(body["columns"][at("r_pct")]["format"], json!("percent"));
        clean_up(&state, &org).await;
    }

    /// Dates in fields of each row (formula.rs's date functions), on ScramDB: they group, take a
    /// grain, filter, and the editor says what they give.
    #[tokio::test]
    async fn dates_in_fields_of_each_row() {
        let Some((app, state, cookie, org)) = org_with(DASHBOARD).await else { return };
        let c = Some(cookie.as_str());
        let num = |v: &serde_json::Value| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())).unwrap();
        let run = |q: serde_json::Value| {
            let (app, org) = (&app, &org);
            async move {
                let (s, body, _) = send(app, c, org, "POST", "/api/query", Some(q), None).await;
                assert_eq!(s, StatusCode::OK, "{body}");
                body["rows"].as_array().unwrap().clone()
            }
        };
        // year and month of each sale: the same counts as grouping by the month
        let by_month = run(json!({"model": "sales", "dimensions": ["sold_at:month"], "measures": ["transactions"],
            "sort": ["sold_at:month"]}))
        .await;
        let parts =
            run(json!({"model": "sales", "dimensions": ["y", "m"], "measures": ["transactions"], "sort": ["y", "m"],
            "formulas": [{"name": "y", "expr": "year(sold_at)", "per_row": true},
                         {"name": "m", "expr": "month(sold_at)", "per_row": true}]}))
            .await;
        assert_eq!(by_month.len(), parts.len());
        for (a, b) in by_month.iter().zip(&parts) {
            let month = a[0].as_str().unwrap();
            assert_eq!(format!("{}-{:02}", num(&b[0]), num(&b[1]) as u32), month[..7], "{a} {b}");
            assert_eq!(num(&a[1]), num(&b[2]), "{a} {b}");
        }
        // a date moved a year on is a date: it takes a grain
        let years = run(json!({"model": "sales", "dimensions": ["sold_at:year"], "measures": ["transactions"],
            "sort": ["sold_at:year"]}))
        .await;
        let later = run(json!({"model": "sales", "dimensions": ["later:year"], "measures": ["transactions"],
            "sort": ["later:year"], "formulas": [{"name": "later", "expr": "date_add(sold_at, 1, 'year')", "per_row": true}]}))
        .await;
        assert_eq!(years.len(), later.len());
        for (a, b) in years.iter().zip(&later) {
            let year = |v: &serde_json::Value| v.as_str().unwrap()[..4].parse::<i32>().unwrap();
            assert_eq!((year(&a[0]) + 1, num(&a[1])), (year(&b[0]), num(&b[1])), "{a} {b}");
        }
        // weekdays from 1 (Monday) to 7, and filtered on
        let weekdays = [json!({"name": "wd", "expr": "weekday(sold_at)", "per_row": true})];
        let all =
            run(json!({"model": "sales", "dimensions": ["wd"], "measures": ["transactions"], "formulas": weekdays}))
                .await;
        assert!(all.iter().all(|r| (1.0..=7.0).contains(&num(&r[0]))), "{all:?}");
        let weekend = run(json!({"model": "sales", "measures": ["transactions"], "formulas": weekdays,
            "filters": [{"field": "wd", "op": "gte", "values": [6]}]}))
        .await;
        let counted: f64 = all.iter().filter(|r| num(&r[0]) >= 6.0).map(|r| num(&r[1])).sum();
        assert_eq!(num(&weekend[0][0]), counted);
        // a condition on dates: nothing was sold in the future
        let future = run(json!({"model": "sales", "dimensions": ["ahead"], "measures": ["transactions"],
            "formulas": [{"name": "ahead", "expr": "days_between(today(), sold_at) > 0", "per_row": true}]}))
        .await;
        assert!(future.iter().all(|r| r[0] == json!(false)), "{future:?}");
        // the editor says what each gives
        for (expr, given) in [("date_trunc(sold_at, 'week')", "time"), ("year(sold_at)", "number"), ("today()", "date")]
        {
            let check = json!({"model": "sales", "expr": expr, "per_row": true});
            let (s, b, _) = send(&app, c, &org, "POST", "/api/formulas/check", Some(check), None).await;
            assert_eq!((s, &b), (StatusCode::OK, &json!({"ok": true, "type": given})), "{expr}");
        }
        clean_up(&state, &org).await;
    }

    /// Text and choices in fields of each row, on ScramDB: case groups, in and contains filter.
    #[tokio::test]
    async fn text_and_choices_in_fields_of_each_row() {
        let Some((app, state, cookie, org)) = org_with(DASHBOARD).await else { return };
        let c = Some(cookie.as_str());
        let num = |v: &serde_json::Value| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())).unwrap();
        let run = |q: serde_json::Value| {
            let (app, org) = (&app, &org);
            async move {
                let (s, body, _) = send(app, c, org, "POST", "/api/query", Some(q), None).await;
                assert_eq!(s, StatusCode::OK, "{body}");
                body["rows"].as_array().unwrap().clone()
            }
        };
        let count = |rows: &[serde_json::Value], key: &str| {
            rows.iter().filter(|r| r[0] == json!(key)).map(|r| num(&r[1])).sum::<f64>()
        };
        let regions = run(json!({"model": "sales", "dimensions": ["region"], "measures": ["transactions"]})).await;
        let areas = run(json!({"model": "sales", "dimensions": ["area"], "measures": ["transactions"],
            "formulas": [{"name": "area", "expr": "case(region, 'EMEA', 'Europe', 'APAC', 'Asia', 'Other')", "per_row": true}]}))
        .await;
        assert_eq!(count(&areas, "Europe"), count(&regions, "EMEA"));
        assert_eq!(count(&areas, "Asia"), count(&regions, "APAC"));
        assert_eq!(count(&areas, "Other"), count(&regions, "AMER"));
        let two = run(json!({"model": "sales", "measures": ["transactions"],
            "formulas": [{"name": "two", "expr": "in(region, 'EMEA', 'APAC')", "per_row": true}],
            "filters": [{"field": "two", "op": "equals", "values": [true]}]}))
        .await;
        assert_eq!(num(&two[0][0]), count(&regions, "EMEA") + count(&regions, "APAC"));
        // contains ignores capitals: the stores whose name has the first store's first letters
        let stores = run(json!({"model": "sales", "dimensions": ["store"], "measures": ["transactions"]})).await;
        let bit: String = stores[0][0].as_str().unwrap().chars().take(3).collect::<String>().to_uppercase();
        let want: f64 =
            stores.iter().filter(|r| r[0].as_str().unwrap().to_uppercase().contains(&bit)).map(|r| num(&r[1])).sum();
        let found = run(json!({"model": "sales", "measures": ["transactions"],
            "formulas": [{"name": "has", "expr": format!("contains(store, '{bit}')"), "per_row": true}],
            "filters": [{"field": "has", "op": "equals", "values": [true]}]}))
        .await;
        assert_eq!(num(&found[0][0]), want, "{bit}");
        clean_up(&state, &org).await;
    }

    /// Totals with a condition, on ScramDB: the same as filtering, period included.
    #[tokio::test]
    async fn totals_with_a_condition() {
        let Some((app, state, cookie, org)) = org_with(DASHBOARD).await else { return };
        let c = Some(cookie.as_str());
        let num = |v: &serde_json::Value| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())).unwrap();
        let run = |q: serde_json::Value| {
            let (app, org) = (&app, &org);
            async move {
                let (s, body, _) = send(app, c, org, "POST", "/api/query", Some(q), None).await;
                assert_eq!(s, StatusCode::OK, "{body}");
                body["rows"].as_array().unwrap().clone()
            }
        };
        let close = |a: f64, b: f64| (a - b).abs() <= 1e-6 * b.abs().max(1.0);
        let emea = [json!({"field": "region", "op": "equals", "values": ["EMEA"]})];
        let filtered =
            run(json!({"model": "sales", "measures": ["revenue", "transactions", "avg_ticket"], "filters": emea}))
                .await;
        let ifs = run(json!({"model": "sales", "measures": ["s", "n", "a"], "formulas": [
            {"name": "s", "expr": "sum_if(amount, region = 'EMEA')"},
            {"name": "n", "expr": "count_if(region = 'EMEA')"},
            {"name": "a", "expr": "avg_if(amount, region = 'EMEA')"}]}))
        .await;
        for i in 0..3 {
            assert!(close(num(&ifs[0][i]), num(&filtered[0][i])), "{ifs:?} {filtered:?}");
        }
        // with the previous period: the same as the filtered measure's, both periods
        let period = json!({"field": "sold_at", "current": {"last_n_days": 90}, "compare": "previous_period"});
        let by_filter =
            run(json!({"model": "sales", "measures": ["revenue"], "filters": emea, "period": period})).await;
        let by_if = run(json!({"model": "sales", "measures": ["s"], "period": period,
            "formulas": [{"name": "s", "expr": "sum_if(amount, region = 'EMEA')"}]}))
        .await;
        assert!(close(num(&by_if[0][0]), num(&by_filter[0][0])), "{by_if:?} {by_filter:?}");
        assert!(close(num(&by_if[0][1]), num(&by_filter[0][1])), "previous: {by_if:?} {by_filter:?}");
        // the rest of them, the same as the filtered aggregations
        let filtered =
            run(json!({"model": "sales", "measures": ["min(amount)", "max(amount)", "count_distinct(store)"],
            "filters": emea}))
            .await;
        let ifs = run(json!({"model": "sales", "measures": ["lo", "hi", "stores"], "formulas": [
            {"name": "lo", "expr": "min_if(amount, region = 'EMEA')"},
            {"name": "hi", "expr": "max_if(amount, region = 'EMEA')"},
            {"name": "stores", "expr": "count_distinct_if(store, region = 'EMEA')"}]}))
        .await;
        for i in 0..3 {
            assert!(close(num(&ifs[0][i]), num(&filtered[0][i])), "{ifs:?} {filtered:?}");
        }
        // the spread of a sample: the variance as written out by hand, and its square root
        let spread = run(json!({"model": "sales", "measures": ["sd", "var", "by_hand"], "formulas": [
            {"name": "sd", "expr": "stddev(amount)"}, {"name": "var", "expr": "variance(amount)"},
            {"name": "by_hand", "expr": "(sum_if(amount * amount, true) - power(sum_if(amount, true), 2) / count_if(true)) / (count_if(true) - 1)"}]}))
        .await;
        let (sd, var, by_hand) = (num(&spread[0][0]), num(&spread[0][1]), num(&spread[0][2]));
        assert!(close(var, by_hand) && close(sd * sd, var), "{spread:?}");
        clean_up(&state, &org).await;
    }

    /// More calculations on results, filtered on and ranked by, on ScramDB.
    #[tokio::test]
    async fn more_calculations_on_results_filtered_and_ranked() {
        let Some((app, state, cookie, org)) = org_with(DASHBOARD).await else { return };
        let c = Some(cookie.as_str());
        let num = |v: &serde_json::Value| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok()));
        let run = |q: serde_json::Value| {
            let (app, org) = (&app, &org);
            async move {
                let (s, body, _) = send(app, c, org, "POST", "/api/query", Some(q), None).await;
                assert_eq!(s, StatusCode::OK, "{body}");
                body
            }
        };
        let calcs = json!([{"name": "after", "expr": "next(revenue)"}, {"name": "change", "expr": "difference(revenue)"},
            {"name": "pct", "expr": "percent_change(revenue)"}, {"name": "avg_so_far", "expr": "running_avg(revenue)"},
            {"name": "place", "expr": "dense_rank(revenue)"}]);
        let body = run(json!({"model": "sales", "dimensions": ["sold_at:month"], "sort": ["sold_at:month"],
            "measures": ["revenue", "after", "change", "pct", "avg_so_far", "place"], "formulas": calcs}))
        .await;
        assert_eq!(body["columns"][3]["format"], json!("number"));
        assert_eq!(body["columns"][4]["format"], json!("percent"));
        let rows = body["rows"].as_array().unwrap();
        let x: Vec<f64> = rows.iter().map(|r| num(&r[1]).unwrap()).collect();
        let close = |a: f64, b: f64| (a - b).abs() <= 1e-6 * b.abs().max(1.0);
        let mut distinct = x.clone();
        distinct.sort_by(|a, b| b.total_cmp(a));
        distinct.dedup();
        for (i, r) in rows.iter().enumerate() {
            match x.get(i + 1) {
                Some(&n) => assert!(close(num(&r[2]).unwrap(), n), "next, row {i}: {r}"),
                None => assert!(r[2].is_null(), "nothing after the last: {r}"),
            }
            if i == 0 {
                assert!(r[3].is_null() && r[4].is_null(), "{r}");
            } else {
                assert!(close(num(&r[3]).unwrap(), x[i] - x[i - 1]), "difference, row {i}: {r}");
                assert!(close(num(&r[4]).unwrap(), (x[i] - x[i - 1]) / x[i - 1]), "% change, row {i}: {r}");
            }
            assert!(close(num(&r[5]).unwrap(), x[..=i].iter().sum::<f64>() / (i + 1) as f64), "running avg: {r}");
            let place = 1 + distinct.iter().position(|&d| d == x[i]).unwrap();
            assert_eq!(num(&r[6]).unwrap() as usize, place, "dense rank, row {i}: {r}");
        }
        // filtered on a calculation: the months above the average share
        let share = json!([{"name": "share", "expr": "percent_of_total(revenue)"}]);
        let all = run(
            json!({"model": "sales", "dimensions": ["region"], "measures": ["revenue", "share"], "formulas": share}),
        )
        .await;
        let all = all["rows"].as_array().unwrap().clone();
        let cut = 1.0 / all.len() as f64;
        let above = run(
            json!({"model": "sales", "dimensions": ["region"], "measures": ["revenue", "share"], "formulas": share,
            "filters": [{"field": "share", "op": "gt", "values": [cut]}]}),
        )
        .await;
        let want: Vec<&serde_json::Value> = all.iter().filter(|r| num(&r[2]).unwrap() > cut).map(|r| &r[0]).collect();
        let got: Vec<&serde_json::Value> = above["rows"].as_array().unwrap().iter().map(|r| &r[0]).collect();
        assert!(!want.is_empty() && want.len() < all.len(), "{all:?}");
        assert_eq!(got, want);
        // top 1 by a calculation: the region with the largest share
        let top = run(json!({"model": "sales", "dimensions": ["region"], "measures": ["share"], "formulas": share,
            "top": {"dimension": "region", "n": 1, "by": "share"}}))
        .await;
        let best = all.iter().max_by(|a, b| num(&a[2]).unwrap().total_cmp(&num(&b[2]).unwrap())).unwrap();
        assert_eq!(top["rows"].as_array().unwrap().iter().map(|r| &r[0]).collect::<Vec<_>>(), [&best[0]]);
        // along and within what they're told: ranked across the whole chart, shares within each region
        let told = json!([{"name": "overall", "expr": "rank(revenue, within())"},
            {"name": "of_region", "expr": "percent_of_total(revenue, within(region))"}]);
        let body = run(json!({"model": "sales", "dimensions": ["region", "sold_at:quarter"],
            "measures": ["revenue", "overall", "of_region"], "formulas": told}))
        .await;
        let rows = body["rows"].as_array().unwrap();
        let x: Vec<f64> = rows.iter().map(|r| num(&r[2]).unwrap()).collect();
        for (i, r) in rows.iter().enumerate() {
            let place = 1 + x.iter().filter(|&&y| y > x[i]).count();
            assert_eq!(num(&r[3]).unwrap() as usize, place, "rank over the whole chart, row {i}: {r}");
        }
        for region in ["EMEA", "APAC", "AMER"] {
            let total: f64 = rows.iter().filter(|r| r[0] == json!(region)).map(|r| num(&r[4]).unwrap()).sum();
            assert!(close(total, 1.0), "{region}'s shares add up to {total}");
        }
        clean_up(&state, &org).await;
    }

    /// Round 2 on ScramDB: first, last, ntile; calculations using one another; dates as text,
    /// fiscal years.
    #[tokio::test]
    async fn calculations_using_one_another_and_dates_as_text() {
        let Some((app, state, cookie, org)) = org_with(DASHBOARD).await else { return };
        let c = Some(cookie.as_str());
        let num = |v: &serde_json::Value| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())).unwrap();
        let run = |q: serde_json::Value| {
            let (app, org) = (&app, &org);
            async move {
                let (s, body, _) = send(app, c, org, "POST", "/api/query", Some(q), None).await;
                assert_eq!(s, StatusCode::OK, "{body}");
                body["rows"].as_array().unwrap().clone()
            }
        };
        let rows = run(json!({"model": "sales", "dimensions": ["sold_at:month"], "sort": ["sold_at:month"],
            "measures": ["revenue", "opening", "closing", "half", "place", "of_so_far"],
            "formulas": [{"name": "opening", "expr": "first(revenue)"}, {"name": "closing", "expr": "last(revenue)"},
                {"name": "half", "expr": "ntile(revenue, 2)"}, {"name": "so_far", "expr": "running_sum(revenue)"},
                {"name": "place", "expr": "rank(so_far)"}, {"name": "of_so_far", "expr": "revenue / so_far"}]}))
        .await;
        let x: Vec<f64> = rows.iter().map(|r| num(&r[1])).collect();
        let close = |a: f64, b: f64| (a - b).abs() <= 1e-6 * b.abs().max(1.0);
        let n = x.len();
        assert!(n > 3, "{rows:?}");
        let mut sorted = x.clone();
        sorted.sort_by(|a, b| b.total_cmp(a));
        for (i, r) in rows.iter().enumerate() {
            assert!(close(num(&r[2]), x[0]) && close(num(&r[3]), x[n - 1]), "first and last, row {i}: {r}");
            let rank = 1 + sorted.iter().position(|&y| y == x[i]).unwrap();
            assert_eq!(num(&r[4]) as usize, if rank <= n.div_ceil(2) { 1 } else { 2 }, "ntile, row {i}: {r}");
            // so_far (a column too, as every formula), then its rank (the latest month first: it
            // only grows) and the month's share of it
            let so_far: f64 = x[..=i].iter().sum();
            assert!(close(num(&r[5]), so_far), "running total, row {i}: {r}");
            assert_eq!(num(&r[6]) as usize, n - i, "rank of the running total, row {i}: {r}");
            assert!(close(num(&r[7]), x[i] / so_far), "share of the running total, row {i}: {r}");
        }
        // dates as text and fiscal years, as fields of each row
        let fy = run(json!({"model": "sales", "dimensions": ["label", "fy"], "measures": ["transactions"],
            "sort": ["label"], "formulas": [
                {"name": "label", "expr": "format_date(sold_at, 'YYYY-MM')", "per_row": true},
                {"name": "fy", "expr": "fiscal_year(sold_at, 4)", "per_row": true}]}))
        .await;
        for r in &fy {
            let label = r[0].as_str().unwrap();
            let (year, month): (i64, i64) = (label[..4].parse().unwrap(), label[5..7].parse().unwrap());
            assert_eq!(num(&r[1]) as i64, if month >= 4 { year + 1 } else { year }, "{r}");
        }
        // the editor: a formula using another calculation is one too
        let others = json!([{"name": "so_far", "expr": "running_sum(revenue)"}]);
        let check = json!({"model": "sales", "expr": "revenue / so_far", "formulas": others});
        let (s, b, _) = send(&app, c, &org, "POST", "/api/formulas/check", Some(check), None).await;
        assert_eq!((s, &b), (StatusCode::OK, &json!({"ok": true})));
        clean_up(&state, &org).await;
    }

    #[tokio::test]
    async fn the_editor_checks_formulas_as_they_are_typed() {
        let Some((app, state, cookie, org)) = org_with(DASHBOARD).await else { return };
        let c = Some(cookie.as_str());
        let check = |expr: &str| json!({"model": "sales", "expr": expr});
        let (s, b, _) =
            send(&app, c, &org, "POST", "/api/formulas/check", Some(check("sum(qty) / units")), Some("power_user"))
                .await;
        assert_eq!((s, &b), (StatusCode::OK, &json!({"ok": true})));
        let (_, b, _) = send(&app, c, &org, "POST", "/api/formulas/check", Some(check("1 + revnue")), None).await;
        assert_eq!((&b["ok"], &b["at"]), (&json!(false), &json!(4)), "{b}");
        assert!(b["error"].as_str().unwrap().contains("did you mean revenue"), "{b}");
        // per row: columns, not totals
        let row = |expr: &str| json!({"model": "sales", "expr": expr, "per_row": true});
        let (_, b, _) = send(&app, c, &org, "POST", "/api/formulas/check", Some(row("amount * qty")), None).await;
        assert_eq!(b, json!({"ok": true, "type": "number"}));
        let (_, b, _) = send(&app, c, &org, "POST", "/api/formulas/check", Some(row("lower(region)")), None).await;
        assert_eq!(b, json!({"ok": true, "type": "string"}));
        // with the chart's other formulas
        let others = json!([{"name": "line", "expr": "amount * qty", "per_row": true}]);
        let with = json!({"model": "sales", "expr": "sum(line) / transactions", "formulas": others});
        let (_, b, _) = send(&app, c, &org, "POST", "/api/formulas/check", Some(with), None).await;
        assert_eq!(b, json!({"ok": true}));
        let (_, b, _) = send(&app, c, &org, "POST", "/api/formulas/check", Some(row("revenue / 2")), None).await;
        assert!(b["error"].as_str().unwrap().contains("one row at a time"), "{b}");
        // calculations on a chart's results
        let (_, b, _) =
            send(&app, c, &org, "POST", "/api/formulas/check", Some(check("revenue / running_sum(sum(qty))")), None)
                .await;
        assert_eq!(b, json!({"ok": true}));
        let (_, b, _) = send(&app, c, &org, "POST", "/api/formulas/check", Some(check("rank(revnue)")), None).await;
        assert!(b["error"].as_str().unwrap().contains("did you mean revenue"), "{b}");
        let (_, b, _) = send(&app, c, &org, "POST", "/api/formulas/check", Some(row("previous(amount)")), None).await;
        assert!(b["error"].as_str().unwrap().contains("chart's results"), "{b}");
        // viewers don't explore
        let (s, _, _) = send(&app, c, &org, "POST", "/api/formulas/check", Some(check("1")), Some("viewer")).await;
        assert_eq!(s, StatusCode::FORBIDDEN);
        clean_up(&state, &org).await;
    }
}
