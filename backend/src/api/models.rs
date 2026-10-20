//! Models, free-form queries (Explore) and period labels.

use axum::Json;
use axum::extract::{Path, Query as UrlQuery, State};
use serde::Deserialize;
use serde_json::{Value, json};

use std::collections::{BTreeMap, BTreeSet};

use super::{Body, available_models, compile_for, connection_for, execute, model_of};
use crate::auth::{Auth, Caller};
use crate::error::{ApiError, ApiResult};
use crate::period::{now_rounded, resolve_period};
use crate::schema::{Model, Period, Query, is_temporal};
use crate::state::{AppState, Shared};

/// The organisation's connections: name -> type ("scramdb", "bigquery", ...).
async fn connection_types(state: &AppState, caller: &Caller) -> ApiResult<BTreeMap<String, &'static str>> {
    let store = state.orgs.store(caller.org()).await?;
    Ok(state.connectors.list(&store, caller.org()).await?.into_iter().map(|c| (c.name, c.config.kind())).collect())
}

/// `connection_type`: null when this organisation has no connection of that name yet.
fn model_json(caller: &crate::auth::Caller, m: &Model, types: &BTreeMap<String, &'static str>) -> Value {
    let blend = caller.project.blends.get(&m.name);
    let conn = if blend.is_some() { Some("blend") } else { types.get(&m.connection).copied() };
    let dims: Vec<Value> = m
        .dimensions
        .iter()
        .filter(|d| !d.hidden)
        .map(|d| {
            // Σ aggregations of columns aren't available on blends (yet)
            let aggregation =
                d.aggregation.map(|a| a.as_str()).or((d.kind == "number").then_some("sum")).filter(|_| blend.is_none());
            json!({"name": d.name, "label": d.label, "description": d.description, "type": d.kind,
                   "hidden": d.hidden, "aggregation": aggregation, "formula": d.formula,
                   "shared": caller.project.formulas.contains(&(m.name.clone(), d.name.clone()))})
        })
        .collect();
    let measures: Vec<Value> = m
        .measures
        .iter()
        .filter(|x| !x.hidden)
        .map(|x| {
            json!({"name": x.name, "label": x.label, "description": x.description, "type": x.kind, "format": x.format,
                   "higher_is_better": x.higher_is_better, "hidden": x.hidden, "formula": x.formula,
                   // one of the organisation's shared formulas (its admins edit it in the app)
                   "shared": caller.project.formulas.contains(&(m.name.clone(), x.name.clone()))})
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
        "blend": blend.map(|b| json!({"base": b.base, "joins": b.joins})),
        // the organisation's own (made in the app; Org admins edit it there)
        "own": caller.project.org_models.contains(&m.name),
    })
}

pub async fn list(State(state): State<Shared>, Auth(caller): Auth) -> ApiResult<Json<Value>> {
    caller.require("view")?;
    let types = connection_types(&state, &caller).await?;
    let models: Vec<Value> = available_models(&caller).values().map(|m| model_json(&caller, m, &types)).collect();
    // configuration problems: builders only
    let errors = if caller.can("edit") { json!(caller.project.errors) } else { json!({}) };
    Ok(Json(json!({"models": models, "errors": errors})))
}

pub async fn get_model(
    State(state): State<Shared>,
    Auth(caller): Auth,
    Path(name): Path<String>,
) -> ApiResult<Json<Value>> {
    caller.require("view")?;
    let types = connection_types(&state, &caller).await?;
    Ok(Json(model_json(&caller, model_of(&caller, &name)?, &types)))
}

/// Free-form query (Explore, tile editor): power users and system owners only.
pub async fn query(State(state): State<Shared>, Auth(caller): Auth, Body(q): Body<Query>) -> ApiResult<Json<Value>> {
    caller.require("explore")?;
    Ok(Json(execute(&state, &caller, &q, super::Source { kind: "explore", dashboard: None }).await?))
}

pub async fn compile_only(
    State(state): State<Shared>,
    Auth(caller): Auth,
    Body(q): Body<Query>,
) -> ApiResult<Json<Value>> {
    caller.require("explore")?;
    caller.require("view_sql")?;
    let m = model_of(&caller, &q.model)?;
    if let Some(b) = caller.project.blends.get(&q.model) {
        return Ok(Json(json!({"sql": super::blends::preview(&state, &caller, b, &q).await?})));
    }
    let (dialect, _) = connection_for(&state, &caller, m).await?;
    Ok(Json(json!({"sql": compile_for(&caller, &q, dialect)?.sql})))
}

#[derive(Deserialize)]
pub struct ModelParam {
    pub model: String,
}

/// Resolve a period spec to concrete windows (labels in the UI); runs no query.
pub async fn period(
    Auth(caller): Auth,
    UrlQuery(p): UrlQuery<ModelParam>,
    Body(period): Body<Period>,
) -> ApiResult<Json<Value>> {
    Ok(Json(period_for(&caller, &p.model, &period)?))
}

/// A period spec's concrete windows, for this caller (signed in, or a guest).
pub fn period_for(caller: &Caller, model: &str, period: &Period) -> ApiResult<Value> {
    caller.require("view")?;
    let m = model_of(caller, model)?;
    let d = m.dimension(&period.field).filter(|d| is_temporal(&d.kind)).ok_or_else(|| {
        ApiError::bad_request(format!("period field {:?} must be a date/time dimension", period.field))
    })?;
    let r = resolve_period(&period.current, period.compare.as_ref(), &d.kind, now_rounded())
        .map_err(ApiError::bad_request)?;
    Ok(r.as_json(&d.kind))
}

/// What the UI needs to know about some models (fields, labels, drill paths), as this caller
/// may see them: a guest link's dashboard brings its own.
pub async fn infos_for(state: &AppState, caller: &Caller, names: &BTreeSet<String>) -> ApiResult<Vec<Value>> {
    let types = connection_types(state, caller).await?;
    Ok(available_models(caller)
        .values()
        .filter(|m| names.contains(&m.name))
        .map(|m| model_json(caller, m, &types))
        .collect())
}
