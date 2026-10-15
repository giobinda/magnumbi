//! Models, free-form queries (Explore) and period labels.

use axum::Json;
use axum::extract::{Path, Query as UrlQuery, State};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Body, available_models, compile_for, execute, model_of};
use crate::auth::Auth;
use crate::error::{ApiError, ApiResult};
use crate::period::{now_rounded, resolve_period};
use crate::schema::{Model, Period, Query, is_temporal};
use crate::state::Shared;

fn model_json(caller: &crate::auth::Caller, m: &Model) -> Value {
    let conn = caller.project.connections.get(&m.connection).map_or("unknown", |c| c.type_name());
    let dims: Vec<Value> = m
        .dimensions
        .iter()
        .filter(|d| !d.hidden)
        .map(|d| {
            let aggregation = d.aggregation.map(|a| a.as_str()).or((d.kind == "number").then_some("sum"));
            json!({"name": d.name, "label": d.label, "description": d.description, "type": d.kind,
                   "hidden": d.hidden, "aggregation": aggregation})
        })
        .collect();
    let measures: Vec<Value> = m
        .measures
        .iter()
        .filter(|x| !x.hidden)
        .map(|x| {
            json!({"name": x.name, "label": x.label, "description": x.description, "type": x.kind, "format": x.format,
                   "higher_is_better": x.higher_is_better, "hidden": x.hidden})
        })
        .collect();
    json!({
        "name": m.name,
        "label": m.label.clone().unwrap_or_else(|| m.name.clone()),
        "description": m.description,
        "connection": m.connection,
        "connection_type": conn,
        "extends": caller.project.bases.get(&m.name),
        "dimensions": dims,
        "measures": measures,
        "hierarchies": m.hierarchies,
    })
}

pub async fn list(Auth(caller): Auth) -> ApiResult<Json<Value>> {
    caller.require("view")?;
    let models: Vec<Value> = available_models(&caller).values().map(|m| model_json(&caller, m)).collect();
    // configuration problems: builders only
    let errors = if caller.can("edit") { json!(caller.project.errors) } else { json!({}) };
    Ok(Json(json!({"models": models, "errors": errors})))
}

pub async fn get_model(Auth(caller): Auth, Path(name): Path<String>) -> ApiResult<Json<Value>> {
    caller.require("view")?;
    Ok(Json(model_json(&caller, model_of(&caller, &name)?)))
}

/// Free-form query (Explore, tile editor): power users and system owners only.
pub async fn query(State(state): State<Shared>, Auth(caller): Auth, Body(q): Body<Query>) -> ApiResult<Json<Value>> {
    caller.require("explore")?;
    Ok(Json(execute(&state, &caller, &q, super::Source { kind: "explore", dashboard: None }).await?))
}

pub async fn compile_only(Auth(caller): Auth, Body(q): Body<Query>) -> ApiResult<Json<Value>> {
    caller.require("explore")?;
    caller.require("view_sql")?;
    Ok(Json(json!({"sql": compile_for(&caller, &q)?.sql})))
}

#[derive(Deserialize)]
pub struct ModelParam {
    model: String,
}

/// Resolve a period spec to concrete windows (labels in the UI); runs no query.
pub async fn period(
    Auth(caller): Auth,
    UrlQuery(p): UrlQuery<ModelParam>,
    Body(period): Body<Period>,
) -> ApiResult<Json<Value>> {
    caller.require("view")?;
    let m = model_of(&caller, &p.model)?;
    let d = m.dimension(&period.field).filter(|d| is_temporal(&d.kind)).ok_or_else(|| {
        ApiError::bad_request(format!("period field {:?} must be a date/time dimension", period.field))
    })?;
    let r = resolve_period(&period.current, period.compare.as_ref(), &d.kind, now_rounded())
        .map_err(ApiError::bad_request)?;
    Ok(Json(r.as_json(&d.kind)))
}
