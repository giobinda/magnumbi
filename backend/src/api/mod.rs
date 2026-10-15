//! The HTTP API (/api/...) and the built frontend (everything else).

mod admin;
mod auth;
mod dashboards;
mod models;

use std::collections::{BTreeMap, BTreeSet};

use axum::Router;
use axum::extract::{FromRequest, Request, State};
use axum::http::StatusCode;
use axum::middleware::from_fn_with_state;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tower::ServiceExt;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

use crate::access::{model_available, row_filters_for};
use crate::auth::{Caller, require_session};
use crate::compiler::{Compiled, compile_query, expand};
use crate::connectors::ConnectorError;
use crate::error::{ApiError, ApiResult};
use crate::period::now_rounded;
use crate::schema::{Model, Query};
use crate::security::{MAX_BODY_BYTES, hardening};
use crate::state::{AppState, Shared};

pub fn router(state: Shared) -> Router {
    let open = Router::new()
        .route("/api/health", get(|| async { axum::Json(json!({"ok": true})) }))
        .route("/api/themes", get(themes))
        .route("/api/auth/me", get(auth::me))
        .route("/api/auth/login", post(auth::login))
        .route("/api/auth/logout", post(auth::logout))
        .route("/api/auth/password", post(auth::change_password))
        .route("/api/auth/google/login", get(auth::google_login))
        .route("/api/auth/google/callback", get(auth::google_callback));
    let protected = Router::new()
        .route("/api/models", get(models::list))
        .route("/api/models/{name}", get(models::get_model))
        .route("/api/query", post(models::query))
        .route("/api/query/sql", post(models::compile_only))
        .route("/api/period", post(models::period))
        .route("/api/dashboards", get(dashboards::list))
        .route("/api/dashboards/{id}", get(dashboards::get_dashboard).put(dashboards::save).delete(dashboards::delete))
        .route("/api/dashboards/{id}/yaml", get(dashboards::export_yaml).put(dashboards::save_yaml))
        .route("/api/dashboards/{id}/history", get(dashboards::history))
        .route("/api/dashboards/{id}/tiles/{tile}/query", post(dashboards::run_tile))
        .route("/api/dashboards/{id}/filters/values", get(dashboards::filter_values))
        .route("/api/members", get(admin::get_members).put(admin::put_members))
        .route("/api/members/password", post(admin::reset_password))
        .route("/api/platform", get(admin::get_platform).put(admin::put_platform))
        .route("/api/platform/dashboards/{id}/publish", post(admin::publish))
        .route_layer(from_fn_with_state(state.clone(), require_session));
    Router::new()
        .merge(open)
        .merge(protected)
        .fallback(frontend)
        .layer(axum::extract::DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(from_fn_with_state(state.clone(), hardening))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// The built UI (frontend/dist) with client-side routes falling back to index.html; unknown
/// /api paths are a JSON 404.
async fn frontend(State(state): State<Shared>, request: Request) -> Response {
    if request.uri().path().starts_with("/api/") {
        return ApiError::not_found("no such endpoint").into_response();
    }
    let dist = &state.config.frontend_dist;
    if !dist.join("index.html").exists() {
        return (
            StatusCode::NOT_FOUND,
            "the UI isn't built: run `make build`, or use the Vite dev server (`make dev-frontend`)",
        )
            .into_response();
    }
    let serve = ServeDir::new(dist).fallback(ServeFile::new(dist.join("index.html")));
    match serve.oneshot(request).await {
        Ok(r) => r.into_response(),
        Err(e) => ApiError::Internal(anyhow::anyhow!("serving the UI: {e}")).into_response(),
    }
}

async fn themes(State(state): State<Shared>) -> axum::Json<Value> {
    let project = state.project.current();
    axum::Json(json!({"themes": project.themes, "default_light": crate::themes::DEFAULT_LIGHT,
                      "default_dark": crate::themes::DEFAULT_DARK}))
}

/// JSON bodies with API-style errors ({"detail": ...}) instead of plain-text rejections.
pub struct Body<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequest<S> for Body<T> {
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let bytes = axum::body::Bytes::from_request(req, state)
            .await
            .map_err(|e| ApiError::Status(StatusCode::PAYLOAD_TOO_LARGE, e.body_text()))?;
        serde_json::from_slice(&bytes)
            .map(Body)
            .map_err(|e| ApiError::Status(StatusCode::UNPROCESSABLE_ENTITY, format!("invalid request: {e}")))
    }
}

// --------------------------------------------------------------------------- shared helpers

/// The models of the caller's organisation; others don't exist as far as it can tell.
pub fn available_models(caller: &Caller) -> BTreeMap<String, &Model> {
    caller
        .project
        .models
        .iter()
        .filter(|(n, _)| model_available(&caller.scope, &caller.project, n))
        .map(|(n, m)| (n.clone(), m))
        .collect()
}

pub fn available_names(caller: &Caller) -> BTreeSet<String> {
    available_models(caller).into_keys().collect()
}

pub fn model_of<'a>(caller: &'a Caller, name: &str) -> ApiResult<&'a Model> {
    if !model_available(&caller.scope, &caller.project, name) {
        return Err(ApiError::bad_request(format!("unknown model {name:?}")));
    }
    Ok(&caller.project.models[name])
}

/// The only way to SQL: with the caller's data restrictions for this model as mandatory
/// conditions (compile_query never drops those, unlike ordinary filters).
pub fn compile_for(caller: &Caller, q: &Query) -> ApiResult<Compiled> {
    let m = model_of(caller, &q.model)?;
    let mandatory = row_filters_for(&caller.scope, &caller.project, &q.model)?;
    let (m2, q2) = expand(m, q)?;
    Ok(compile_query(&m2, &q2, caller.project.dialect(m), now_rounded(), &mandatory)?)
}

pub async fn execute(state: &AppState, caller: &Caller, q: &Query) -> ApiResult<Value> {
    let m = model_of(caller, &q.model)?;
    let compiled = compile_for(caller, q)?;
    let conn = caller
        .project
        .connections
        .get(&m.connection)
        .ok_or_else(|| ApiError::bad_request(format!("unknown connection {:?}", m.connection)))?;
    let connector = state.connectors.get(&m.connection, conn)?;
    let names: Vec<String> = compiled.columns.iter().map(|c| c.name.clone()).collect();
    let result = match connector.run(&compiled.sql, &names).await {
        Ok(r) => r,
        Err(e) => {
            // database errors can quote SQL, tables and values: details for builders only
            let reference = crate::store::random_token(4);
            tracing::warn!(reference, model = q.model, "query failed: {e}");
            let detail = match (&e, caller.can("view_sql")) {
                (ConnectorError::Unavailable(m), _) => format!("{m} (ref {reference})"),
                (ConnectorError::Query(m), true) => format!("{m} (ref {reference})"),
                _ => format!("the query failed (ref {reference}); a system owner can look it up in the logs"),
            };
            return Err(ApiError::Status(StatusCode::BAD_GATEWAY, detail));
        }
    };
    Ok(json!({
        "columns": compiled.columns,
        "rows": result.rows,
        "sql": caller.can("view_sql").then_some(compiled.sql),
        "stats": {"duration_ms": result.duration_ms},
        "period": compiled.period,
    }))
}
