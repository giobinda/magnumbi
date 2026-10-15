//! Dashboards: list, read, save (JSON or YAML), delete, history; running saved tiles.

use axum::Json;
use axum::extract::{Path, Query as UrlQuery, State};
use axum::http::header;
use axum::response::IntoResponse;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Body, available_names, execute, model_of};
use crate::auth::{Auth, Caller};
use crate::error::{ApiError, ApiResult};
use crate::project::{DASHBOARD_ID, dump_dashboard, parse_dashboard};
use crate::schema::{Dashboard, Query};
use crate::state::{AppState, Shared};
use crate::tile_query::{TileRequest, TileRequestError, build};

fn check_id(id: &str) -> ApiResult<()> {
    if DASHBOARD_ID.is_match(id) { Ok(()) } else { Err(ApiError::bad_request(format!("invalid dashboard id {id:?}"))) }
}

pub async fn load(state: &AppState, caller: &Caller, id: &str) -> ApiResult<(Dashboard, String)> {
    check_id(id)?;
    let text = state
        .store
        .read_dashboard(caller.org(), id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("dashboard {id:?} not found")))?;
    let d = parse_dashboard(&text).map_err(ApiError::bad_request)?;
    Ok((d, text))
}

fn response(caller: &Caller, id: &str, d: &Dashboard, text: &str) -> Value {
    let builder = caller.can("edit");
    json!({
        "id": id,
        "dashboard": d,
        "yaml": builder.then_some(text),
        "warnings": if builder { caller.project.validate_dashboard(d, &available_names(caller)) } else { vec![] },
    })
}

pub async fn list(State(state): State<Shared>, Auth(caller): Auth) -> ApiResult<Json<Value>> {
    caller.require("view")?;
    let mut out = vec![];
    for id in state.store.dashboard_ids(caller.org()).await? {
        let text = state.store.read_dashboard(caller.org(), &id).await?.unwrap_or_default();
        out.push(match parse_dashboard(&text) {
            Ok(d) => json!({"id": id, "title": d.title, "description": d.description, "tiles": d.tiles.len()}),
            Err(e) => json!({"id": id, "title": id, "error": e}),
        });
    }
    Ok(Json(json!({"dashboards": out})))
}

pub async fn get_dashboard(
    State(state): State<Shared>,
    Auth(caller): Auth,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    caller.require("view")?;
    let (d, text) = load(&state, &caller, &id).await?;
    Ok(Json(response(&caller, &id, &d, &text)))
}

pub async fn save(
    State(state): State<Shared>,
    Auth(caller): Auth,
    Path(id): Path<String>,
    Body(d): Body<Dashboard>,
) -> ApiResult<Json<Value>> {
    caller.require("edit")?;
    check_id(&id)?;
    d.validate().map_err(ApiError::bad_request)?;
    let text = dump_dashboard(&d);
    state.store.write_dashboard(caller.org(), &id, &text, Some(caller.email())).await?;
    Ok(Json(response(&caller, &id, &d, &text)))
}

pub async fn save_yaml(
    State(state): State<Shared>,
    Auth(caller): Auth,
    Path(id): Path<String>,
    text: String,
) -> ApiResult<Json<Value>> {
    caller.require("edit")?;
    check_id(&id)?;
    let d = parse_dashboard(&text).map_err(ApiError::bad_request)?;
    let text = if text.ends_with('\n') { text } else { text + "\n" };
    state.store.write_dashboard(caller.org(), &id, &text, Some(caller.email())).await?;
    Ok(Json(response(&caller, &id, &d, &text)))
}

pub async fn export_yaml(
    State(state): State<Shared>,
    Auth(caller): Auth,
    Path(id): Path<String>,
) -> ApiResult<impl IntoResponse> {
    caller.require("edit")?;
    let (_, text) = load(&state, &caller, &id).await?;
    Ok(([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], text))
}

pub async fn delete(State(state): State<Shared>, Auth(caller): Auth, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    caller.require("edit")?;
    check_id(&id)?;
    if !state.store.remove_dashboard(caller.org(), &id, Some(caller.email())).await? {
        return Err(ApiError::not_found(format!("dashboard {id:?} not found")));
    }
    Ok(Json(json!({"ok": true})))
}

pub async fn history(
    State(state): State<Shared>,
    Auth(caller): Auth,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    caller.require("edit")?;
    check_id(&id)?;
    Ok(Json(json!({"history": state.store.dashboard_history(caller.org(), &id, 50).await?})))
}

/// Run a saved tile with the viewer's filter values, period window and drill clicks.
pub async fn run_tile(
    State(state): State<Shared>,
    Auth(caller): Auth,
    Path((id, tile_id)): Path<(String, String)>,
    Body(req): Body<TileRequest>,
) -> ApiResult<Json<Value>> {
    caller.require("view")?;
    let (d, _) = load(&state, &caller, &id).await?;
    let tile = d
        .tiles
        .iter()
        .find(|t| t.id == tile_id)
        .ok_or_else(|| ApiError::not_found(format!("tile {tile_id:?} not found")))?;
    let model = model_of(&caller, &tile.query.model)?;
    let q = build(&d, tile, model, &req, caller.can("drill")).map_err(|e| match e {
        TileRequestError::NoDrill => ApiError::forbidden("your role can't drill down"),
        TileRequestError::Invalid(m) => ApiError::bad_request(m),
    })?;
    Ok(Json(execute(&state, &caller, &q).await?))
}

#[derive(Deserialize)]
pub struct KeyParam {
    key: String,
}

/// Distinct values for one of the dashboard's own filters (the filter bar's pick list).
pub async fn filter_values(
    State(state): State<Shared>,
    Auth(caller): Auth,
    Path(id): Path<String>,
    UrlQuery(p): UrlQuery<KeyParam>,
) -> ApiResult<Json<Value>> {
    caller.require("filter")?;
    let (d, _) = load(&state, &caller, &id).await?;
    let f = d
        .filters
        .iter()
        .find(|f| format!("{}.{}", f.model, f.field) == p.key)
        .ok_or_else(|| ApiError::not_found(format!("{:?} is not a filter of this dashboard", p.key)))?;
    let q = Query {
        model: f.model.clone(),
        dimensions: vec![f.field.clone()],
        measures: vec![],
        filters: vec![],
        sort: vec![f.field.clone()],
        limit: 500,
        period: None,
        formulas: vec![],
        top: None,
    };
    Ok(Json(execute(&state, &caller, &q).await?))
}
