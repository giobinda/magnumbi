//! Org admins' own models (org_models.rs): from a connection's table to a model, in the app.
//!
//! A model makes a table visible to everyone in the organisation, and restrictions are set per
//! model (and follow the same table into new models): so only Org admins make them.

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query as UrlQuery, State};
use serde::Deserialize;
use serde_json::{Value, json};

use super::Body;
use crate::auth::{Auth, Caller};
use crate::compiler::Dialect;
use crate::connectors::{Connector, ConnectorError};
use crate::error::{ApiError, ApiResult};
use crate::materialize::ColType;
use crate::schema::{Model, check_name};
use crate::state::{AppState, Shared};

fn require_org_admin(caller: &Caller) -> ApiResult<()> {
    caller.require("manage_members")?;
    if caller.org().is_empty() {
        return Err(ApiError::bad_request("the platform workspace's models are the project's files (project/models)"));
    }
    Ok(())
}

async fn connector(state: &AppState, caller: &Caller, name: &str) -> ApiResult<(Dialect, Arc<dyn Connector>)> {
    let store = state.orgs.store(caller.org()).await?;
    Ok(state.connectors.get(&store, caller.org(), name).await?)
}

fn conn_error(what: &str, e: ConnectorError) -> ApiError {
    match e {
        ConnectorError::Unavailable(m) => ApiError::Status(axum::http::StatusCode::SERVICE_UNAVAILABLE, m),
        ConnectorError::Query(m) => ApiError::bad_request(format!("{what}: {m}")),
    }
}

/// A connection's tables (and views), as models name them.
pub async fn tables(
    State(state): State<Shared>,
    Auth(caller): Auth,
    Path(name): Path<String>,
) -> ApiResult<Json<Value>> {
    require_org_admin(&caller)?;
    let (_, c) = connector(&state, &caller, &name).await?;
    let tables = c.tables().await.map_err(|e| conn_error(&format!("connection {name}"), e))?;
    Ok(Json(json!({"tables": tables})))
}

#[derive(Deserialize)]
pub struct NameParam {
    name: String,
}

/// A model proposed for a table, from its columns (to adjust, then save).
pub async fn proposal(
    State(state): State<Shared>,
    Auth(caller): Auth,
    Path((connection, table)): Path<(String, String)>,
    UrlQuery(p): UrlQuery<NameParam>,
) -> ApiResult<Json<Value>> {
    require_org_admin(&caller)?;
    check_name(&p.name).map_err(ApiError::bad_request)?;
    let (dialect, c) = connector(&state, &caller, &connection).await?;
    let columns = c.columns(&table).await.map_err(|e| conn_error(&format!("table {table}"), e))?;
    let (model, skipped) = crate::org_models::propose(&p.name, &connection, &table, dialect, &columns);
    Ok(Json(json!({"model": model, "skipped": skipped})))
}

/// The organisation's own models, as saved.
pub async fn list(State(state): State<Shared>, Auth(caller): Auth) -> ApiResult<Json<Value>> {
    require_org_admin(&caller)?;
    let models = state.orgs.store(caller.org()).await?.org_models(caller.org()).await?;
    let out: Vec<Value> =
        models.into_iter().map(|(m, at, by)| json!({"model": m, "updated_at": at, "updated_by": by})).collect();
    Ok(Json(json!({"models": out})))
}

/// Save one of the organisation's models (`?dry_run=1`: check only).
pub async fn save(
    State(state): State<Shared>,
    Auth(caller): Auth,
    Path(name): Path<String>,
    UrlQuery(p): UrlQuery<super::dashboards::WriteParams>,
    Body(m): Body<Model>,
) -> ApiResult<Json<Value>> {
    require_org_admin(&caller)?;
    if m.name != name {
        return Err(ApiError::bad_request("the model's name must match the address it's saved to"));
    }
    if state.project.current().models.contains_key(&name) {
        return Err(ApiError::bad_request(format!("{name:?} is the name of one of the platform's models or blends")));
    }
    if state.org_blends(caller.org()).await?.iter().any(|b| b.name == name) {
        return Err(ApiError::bad_request(format!("{name:?} is the name of one of your blends")));
    }
    let mut m = m;
    crate::org_models::check(&m).map_err(ApiError::bad_request)?;
    // every field is one of the table's columns (typos caught now, not on a dashboard)
    let (dialect, c) = connector(&state, &caller, &m.connection).await?;
    crate::org_models::quote_columns(&mut m, dialect);
    let table = m.table.clone().unwrap_or_default();
    let columns = c.columns(&table).await.map_err(|e| conn_error(&format!("table {table}"), e))?;
    check_columns(&m, &columns)?;
    if p.dry_run {
        return Ok(Json(json!({"ok": true, "dry_run": true})));
    }
    let store = state.orgs.store(caller.org()).await?;
    store.put_org_model(caller.org(), &m, Some(caller.email())).await?;
    // a new model belongs to its maker
    if !store.governance(caller.org()).await?.iter().any(|(model, ..)| *model == name) {
        let g = crate::governance::Governance { owner: Some(caller.email().to_string()), ..Default::default() };
        store.put_governance(caller.org(), &name, &g, Some(caller.email())).await?;
        state.governance.forget(caller.org());
    }
    state.models.forget(caller.org());
    state.blends.forget(caller.org());
    state.usage.record(crate::usage::Event { model: Some(name), ..caller.event("model_saved") });
    Ok(Json(json!({"ok": true})))
}

/// Every dimension, and every measure's column, is a column of the table.
fn check_columns(m: &Model, columns: &[(String, ColType)]) -> ApiResult<()> {
    let have: BTreeSet<String> = columns.iter().map(|(c, _)| c.to_lowercase()).collect();
    let table = m.table.as_deref().unwrap_or_default();
    for d in &m.dimensions {
        if !have.contains(&d.name.to_lowercase()) {
            return Err(ApiError::bad_request(format!("dimension {:?}: {table} has no column {:?}", d.name, d.name)));
        }
    }
    for x in &m.measures {
        let Some(sql) = &x.sql else { continue };
        let col = sql.trim().trim_matches(|c| c == '"' || c == '`');
        if !have.contains(&col.to_lowercase()) {
            return Err(ApiError::bad_request(format!("measure {:?}: {table} has no column {col:?}", x.name)));
        }
    }
    Ok(())
}

pub async fn delete(
    State(state): State<Shared>,
    Auth(caller): Auth,
    Path(name): Path<String>,
) -> ApiResult<Json<Value>> {
    require_org_admin(&caller)?;
    let store = state.orgs.store(caller.org()).await?;
    if !store.delete_org_model(caller.org(), &name).await? {
        return Err(ApiError::not_found(format!("no model of yours named {name:?}")));
    }
    store.delete_governance(caller.org(), &name).await?;
    state.governance.forget(caller.org());
    state.models.forget(caller.org());
    state.blends.forget(caller.org());
    state.usage.record(crate::usage::Event { model: Some(name), ..caller.event("model_deleted") });
    Ok(Json(json!({"ok": true})))
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use serde_json::json;

    use crate::api::share::tests::{clean_up, org_with, send};

    const DASHBOARD: &str = "title: Models test\ntiles: []\n";

    #[tokio::test]
    async fn org_admins_make_models_from_their_tables() {
        let Some((app, state, cookie, org)) = org_with(DASHBOARD).await else { return };
        let c = Some(cookie.as_str());
        let name = format!("my_sales_{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);

        // the connection's tables, and a model proposed for one
        let (s, t, _) = send(&app, c, &org, "GET", "/api/connections/warehouse/tables", None, None).await;
        assert_eq!(s, StatusCode::OK, "{t}");
        assert!(t["tables"].as_array().unwrap().iter().any(|x| x == "sample_sales"), "{t}");
        let proposal = format!("/api/connections/warehouse/tables/sample_sales/model?name={name}");
        let (s, p, _) = send(&app, c, &org, "GET", &proposal, None, None).await;
        assert_eq!(s, StatusCode::OK, "{p}");
        let model = p["model"].clone();
        assert_eq!(model["table"], json!("sample_sales"));
        assert!(model["dimensions"].as_array().unwrap().iter().any(|d| d["name"] == "region"), "{model}");
        assert_eq!(model["measures"][0]["type"], json!("count"));

        // Org admins only, in an organisation
        let (s, _, _) = send(&app, c, &org, "GET", &proposal, None, Some("power_user")).await;
        assert_eq!(s, StatusCode::FORBIDDEN);
        let path = format!("/api/org-models/{name}");
        let (s, _, _) = send(&app, c, &org, "PUT", &path, Some(model.clone()), Some("power_user")).await;
        assert_eq!(s, StatusCode::FORBIDDEN);
        let (s, _, _) = send(&app, c, "", "PUT", &path, Some(model.clone()), None).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);

        // what's refused: a taken name, SQL, fields that aren't the table's columns, unknown tables
        let mut taken = model.clone();
        taken["name"] = json!("sales");
        let (s, b, _) = send(&app, c, &org, "PUT", "/api/org-models/sales", Some(taken), None).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{b}");
        for (edit, says) in [
            (json!({"dimensions": [{"name": "region", "sql": "current_user"}]}), "no SQL"),
            (json!({"dimensions": [{"name": "nope"}]}), "nope"),
            (json!({"measures": [{"name": "total", "type": "sum", "sql": "missing"}]}), "missing"),
            (json!({"table": "no_such_table"}), "no_such_table"),
        ] {
            let mut bad = model.clone();
            for (k, v) in edit.as_object().unwrap() {
                bad[k] = v.clone();
            }
            let (s, b, _) = send(&app, c, &org, "PUT", &path, Some(bad), None).await;
            assert_eq!(s, StatusCode::BAD_REQUEST, "{edit} -> {b}");
            assert!(b["detail"].as_str().unwrap().contains(says), "{edit} -> {b}");
        }

        // a dry run checks; saving makes it the organisation's, everywhere
        let (s, b, _) = send(&app, c, &org, "PUT", &format!("{path}?dry_run=1"), Some(model.clone()), None).await;
        assert_eq!((s, &b["dry_run"]), (StatusCode::OK, &json!(true)), "{b}");
        let (_, list, _) = send(&app, c, &org, "GET", "/api/org-models", None, None).await;
        assert_eq!(list["models"], json!([]));
        let (s, b, _) = send(&app, c, &org, "PUT", &path, Some(model.clone()), None).await;
        assert_eq!(s, StatusCode::OK, "{b}");
        let (_, list, _) = send(&app, c, &org, "GET", "/api/org-models", None, None).await;
        assert_eq!(list["models"][0]["model"]["name"], json!(name));
        let (_, models, _) = send(&app, c, &org, "GET", "/api/models", None, None).await;
        let mine = models["models"].as_array().unwrap().iter().find(|m| m["name"] == json!(name)).cloned().unwrap();
        assert_eq!(mine["own"], json!(true));
        let q = json!({"model": name, "dimensions": ["region"], "measures": ["count"]});
        let (s, r, _) = send(&app, c, &org, "POST", "/api/query", Some(q.clone()), None).await;
        assert_eq!(s, StatusCode::OK, "{r}");
        assert_eq!(r["rows"].as_array().unwrap().len(), 3, "{r}");
        // the organisation's blends can join its own models
        let blend = json!({"name": format!("{name}_blend"), "base": "sales", "joins": [{"model": name, "on": ["region = region"]}]});
        let (s, b, _) = send(&app, c, &org, "PUT", &format!("/api/blends/{name}_blend"), Some(blend), None).await;
        assert_eq!(s, StatusCode::OK, "{b}");

        let (s, _, _) = send(&app, c, &org, "DELETE", &path, None, None).await;
        assert_eq!(s, StatusCode::OK);
        let (s, _, _) = send(&app, c, &org, "POST", "/api/query", Some(q), None).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        clean_up(&state, &org).await;
    }
}
