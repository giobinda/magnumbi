//! The organisation's data connections (Org admins and system owners). Secrets are write-only:
//! sent when set or changed, never returned.

use axum::Json;
use axum::extract::{Path, Query as UrlQuery, State};
use serde::Deserialize;
use serde_json::{Value, json};

use super::Body;
use super::dashboards::WriteParams;
use crate::auth::{Auth, Caller};
use crate::connections::ConnectionConfig;
use crate::error::{ApiError, ApiResult};
use crate::state::{AppState, Shared};
use crate::store::Store;
use crate::usage::Event;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectionBody {
    config: ConnectionConfig,
    /// omitted or empty: keep the stored secret
    #[serde(default)]
    secret: Option<String>,
}

async fn store(state: &AppState, caller: &Caller) -> ApiResult<Store> {
    caller.require("manage_members")?;
    Ok(state.orgs.store(caller.org()).await?)
}

pub async fn list(State(state): State<Shared>, Auth(caller): Auth) -> ApiResult<Json<Value>> {
    let store = store(&state, &caller).await?;
    let list: Vec<Value> = state.connectors.list(&store, caller.org()).await?.iter().map(|c| c.public()).collect();
    Ok(Json(json!({"connections": list, "secret_key": state.config.secret_key.is_some()})))
}

/// Check settings work, without saving them.
pub async fn test(
    State(state): State<Shared>,
    Auth(caller): Auth,
    Path(name): Path<String>,
    Body(b): Body<ConnectionBody>,
) -> ApiResult<Json<Value>> {
    let store = store(&state, &caller).await?;
    let message = state.connectors.try_out(&store, caller.org(), &name, &b.config, b.secret.as_deref()).await?;
    Ok(Json(json!({"ok": true, "message": message})))
}

/// Save a connection (`?dry_run=1`: check only, nothing saved or connected to).
pub async fn save(
    State(state): State<Shared>,
    Auth(caller): Auth,
    Path(name): Path<String>,
    UrlQuery(p): UrlQuery<WriteParams>,
    Body(b): Body<ConnectionBody>,
) -> ApiResult<Json<Value>> {
    let store = store(&state, &caller).await?;
    let kind = b.config.kind();
    if p.dry_run {
        state.connectors.check(&store, caller.org(), &name, &b.config, b.secret.as_deref()).await?;
        return Ok(Json(json!({"ok": true, "dry_run": true})));
    }
    let by = Some(caller.email());
    state.connectors.save(&store, caller.org(), &name, b.config, b.secret.as_deref(), by).await?;
    state.usage.record(Event {
        connection: Some(name.clone()),
        detail: Some(json!({"type": kind})),
        ..caller.event("connection_saved")
    });
    Ok(Json(json!({"ok": true})))
}

pub async fn delete(
    State(state): State<Shared>,
    Auth(caller): Auth,
    Path(name): Path<String>,
) -> ApiResult<Json<Value>> {
    let store = store(&state, &caller).await?;
    if !state.connectors.delete(&store, caller.org(), &name).await? {
        return Err(ApiError::not_found(format!("no connection named {name:?}")));
    }
    state.usage.record(Event { connection: Some(name), ..caller.event("connection_deleted") });
    Ok(Json(json!({"ok": true})))
}
