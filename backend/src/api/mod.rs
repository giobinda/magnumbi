//! The HTTP API (/api/...) and the built frontend (everything else).

mod admin;
mod auth;
mod blends;
mod connections;
mod dashboards;
pub(crate) mod delivery;
mod exports;
mod formulas;
pub(crate) mod governance;
mod guest;
mod mcp;
mod models;
mod org_models;
mod schedules;
mod setup;
pub(crate) mod share;
mod themes;
mod tokens;

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
use crate::cache;
use crate::compiler::{Column, Compiled, Dialect, compile_query, expand};
use crate::connectors::ConnectorError;
use crate::error::{ApiError, ApiResult};
use crate::materialize::{self, ColType};
use crate::period::now_rounded;
use crate::schema::{Model, Query};
use crate::security::{MAX_BODY_BYTES, hardening};
use crate::state::{AppState, Shared};
use crate::store::Store;

/// Send the scheduled reports that are due (the server's minute loop).
pub async fn run_scheduled_reports(state: &AppState) {
    schedules::tick(state, chrono::Utc::now()).await;
}

pub fn router(state: Shared) -> Router {
    let open = Router::new()
        .route("/api/health", get(|| async { axum::Json(json!({"ok": true})) }))
        .route("/api/themes", get(themes::list))
        .route("/api/auth/me", get(auth::me))
        .route("/api/auth/methods", post(auth::methods))
        .route("/api/auth/login", post(auth::login))
        .route("/api/auth/login/two-factor", post(auth::login_two_factor))
        .route("/api/auth/two-factor/setup", post(auth::two_factor_setup))
        .route("/api/auth/two-factor/enable", post(auth::two_factor_enable))
        .route("/api/auth/two-factor/disable", post(auth::two_factor_disable))
        .route("/api/auth/passkeys", get(auth::passkeys))
        .route("/api/auth/passkeys/{id}", axum::routing::delete(auth::delete_passkey))
        .route("/api/auth/passkeys/register/start", post(auth::passkey_register_start))
        .route("/api/auth/passkeys/register/finish", post(auth::passkey_register_finish))
        .route("/api/auth/passkeys/login/start", post(auth::passkey_login_start))
        .route("/api/auth/passkeys/login/finish", post(auth::passkey_login_finish))
        .route("/api/auth/logout", post(auth::logout))
        .route("/api/auth/password", post(auth::change_password))
        .route("/api/auth/google/login", get(auth::google_login))
        .route("/api/auth/google/callback", get(auth::google_callback))
        .route("/api/auth/oidc/{org}/login", get(auth::oidc_login))
        .route("/api/auth/oidc/callback", get(auth::oidc_callback))
        .route("/api/public/{org}/{token}", get(share::dashboard))
        .route("/api/public/{org}/{token}/tiles/{tile}", get(share::tile))
        .route("/api/guest/{org}/{token}", get(guest::dashboard))
        .route("/api/guest/{org}/{token}/unlock", post(guest::unlock))
        .route("/api/guest/{org}/{token}/tiles/{tile}/query", post(guest::tile))
        .route("/api/guest/{org}/{token}/tiles/{tile}/export", post(guest::tile_export))
        .route("/api/guest/{org}/{token}/export", post(guest::export))
        .route("/api/guest/{org}/{token}/filters/values", get(guest::filter_values))
        .route("/api/guest/{org}/{token}/period", post(guest::period))
        .route("/api/unsubscribe", post(schedules::unsubscribe));
    let protected = Router::new()
        .route("/api/models", get(models::list))
        .route("/api/mcp", post(mcp::endpoint))
        .route("/api/org-themes", get(themes::get_org).put(themes::put_org))
        .route("/api/themes/import", post(themes::import))
        .route("/api/themes/{id}/css", get(themes::css))
        .route("/api/tokens", get(tokens::list).post(tokens::create))
        .route("/api/tokens/{id}", axum::routing::delete(tokens::revoke))
        .route("/api/blends", get(blends::list))
        .route("/api/blends/{name}", axum::routing::put(blends::save).delete(blends::delete))
        .route("/api/models/{name}", get(models::get_model))
        .route("/api/query", post(models::query))
        .route("/api/query/sql", post(models::compile_only))
        .route("/api/period", post(models::period))
        .route("/api/dashboards", get(dashboards::list))
        .route("/api/dashboards/{id}", get(dashboards::get_dashboard).put(dashboards::save).delete(dashboards::delete))
        .route("/api/dashboards/{id}/yaml", get(dashboards::export_yaml).put(dashboards::save_yaml))
        .route("/api/dashboards/{id}/history", get(dashboards::history))
        .route("/api/dashboards/{id}/tiles/{tile}/query", post(dashboards::run_tile))
        .route("/api/dashboards/{id}/tiles/{tile}/export", post(exports::tile))
        .route("/api/dashboards/{id}/export", post(exports::dashboard))
        .route("/api/delivery", get(delivery::get))
        .route("/api/delivery/mail", axum::routing::put(delivery::put_mail).delete(delivery::delete_mail))
        .route("/api/delivery/mail/test", post(delivery::test_mail))
        .route(
            "/api/delivery/webhooks/{name}",
            axum::routing::put(delivery::put_webhook).delete(delivery::delete_webhook),
        )
        .route("/api/delivery/webhooks/{name}/test", post(delivery::test_webhook))
        .route("/api/dashboards/{id}/filters/values", get(dashboards::filter_values))
        .route("/api/dashboards/{id}/share-links", get(share::list).post(share::create))
        .route("/api/share-links/{id}", axum::routing::delete(share::revoke))
        .route("/api/dashboards/{id}/guest-links", get(guest::list).post(guest::create))
        .route("/api/dashboards/{id}/schedules", get(schedules::list).post(schedules::create))
        .route("/api/schedules", get(schedules::list_all))
        .route("/api/schedules/{id}", axum::routing::delete(schedules::delete))
        .route("/api/schedules/{id}/active", axum::routing::put(schedules::set_active))
        .route("/api/schedules/{id}/run", post(schedules::run_now))
        .route("/api/guest-links/{id}", axum::routing::delete(guest::revoke))
        .route("/api/members", get(admin::get_members).put(admin::put_members))
        .route("/api/members/sign-in", get(admin::get_sign_in).put(admin::put_sign_in))
        .route("/api/members/password", post(admin::reset_password))
        .route("/api/platform", get(admin::get_platform).put(admin::put_platform))
        .route("/api/platform/dashboards/{id}/publish", post(admin::publish))
        .route("/api/platform/demo", get(admin::demo_status).post(admin::start_demo))
        .route("/api/platform/organisations/{org}/database", axum::routing::put(admin::set_org_database))
        .route("/api/usage", get(admin::usage_summary))
        .route("/api/connections", get(connections::list))
        .route("/api/connections/{name}", axum::routing::put(connections::save).delete(connections::delete))
        .route("/api/connections/{name}/test", post(connections::test))
        .route("/api/connections/{name}/tables", get(org_models::tables))
        .route("/api/connections/{name}/tables/{table}/model", get(org_models::proposal))
        .route("/api/governance", get(governance::list))
        .route("/api/governance/{model}", axum::routing::put(governance::save))
        .route("/api/governance/{model}/docs", axum::routing::put(governance::save_docs))
        .route("/api/glossary", get(governance::glossary))
        .route("/api/catalog/search", get(governance::search))
        .route("/api/catalog/{model}/lineage", get(governance::lineage))
        .route("/api/glossary/{name}", axum::routing::put(governance::save_term).delete(governance::delete_term))
        .route("/api/formulas", get(formulas::list))
        .route("/api/formulas/check", post(formulas::check))
        .route("/api/formulas/{model}/{name}", axum::routing::put(formulas::save).delete(formulas::delete))
        .route("/api/org-models", get(org_models::list))
        .route("/api/setup", get(setup::checklist))
        .route("/api/org-models/{name}", axum::routing::put(org_models::save).delete(org_models::delete))
        .route("/api/cache", get(admin::cache_stats).delete(admin::clear_cache))
        .route_layer(from_fn_with_state(state.clone(), require_session));
    Router::new()
        .merge(open)
        .merge(protected)
        .fallback(frontend)
        .layer(axum::extract::DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(from_fn_with_state(state.clone(), hardening))
        .layer(TraceLayer::new_for_http().make_span_with(|r: &axum::http::Request<_>| {
            tracing::debug_span!("request", method = %r.method(), path = %crate::security::logged_path(r.uri().path()))
        }))
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

/// JSON bodies with API-style errors ({"detail": ...}) instead of plain-text rejections.
pub struct Body<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequest<S> for Body<T> {
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        // A browser posts text/plain or a form to another site without asking first (no CORS
        // preflight). Requests on a session cookie come from the app, which always sends JSON:
        // anything else is refused (scripts with a bearer token may be less careful).
        let h = req.headers();
        let json = h
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|ct| ct.split(';').next().unwrap_or_default().trim().to_ascii_lowercase())
            .is_some_and(|ct| ct == "application/json" || ct.ends_with("+json"));
        if !json && h.contains_key(axum::http::header::COOKIE) && !h.contains_key(axum::http::header::AUTHORIZATION) {
            return Err(ApiError::Status(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "send JSON (Content-Type: application/json)".into(),
            ));
        }
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

/// The organisation's connection a model reads from: its SQL dialect and live connector.
pub async fn connection_for(
    state: &AppState,
    caller: &Caller,
    m: &Model,
) -> ApiResult<(Dialect, std::sync::Arc<dyn crate::connectors::Connector>)> {
    let store = state.orgs.store(caller.org()).await?;
    Ok(state.connectors.get(&store, caller.org(), &m.connection).await?)
}

/// The organisation's connections: name -> SQL dialect (for checking dashboards).
pub async fn dialects(state: &AppState, caller: &Caller) -> ApiResult<BTreeMap<String, Dialect>> {
    let store = state.orgs.store(caller.org()).await?;
    Ok(state.connectors.list(&store, caller.org()).await?.into_iter().map(|c| (c.name, c.config.dialect())).collect())
}

/// The only way to SQL: with the caller's data restrictions for this model as mandatory
/// conditions (compile_query never drops those, unlike ordinary filters). Calculations on
/// results are checked, and shown after the SQL (they run on ScramDB: `execute`).
pub fn compile_for(caller: &Caller, q: &Query, dialect: Dialect) -> ApiResult<Compiled> {
    if let Some(split) = crate::results::split(q).map_err(ApiError::bad_request)? {
        let mut c = compile_for(caller, &split.inner, dialect)?;
        let calc_sql = crate::results::sql("results", &c.columns, &split.calcs).map_err(ApiError::bad_request)?;
        c.sql = format!("{}\n\n-- then, on the results:\n{calc_sql}", c.sql);
        return Ok(c);
    }
    let m = model_of(caller, &q.model)?;
    let mandatory = row_filters_for(&caller.scope, &caller.project, &q.model)?;
    let (m2, q2) = expand(m, q, dialect)?;
    Ok(compile_query(&m2, &q2, dialect, now_rounded(), &mandatory)?)
}

/// Where a query comes from, for usage metrics.
pub struct Source<'a> {
    /// explore | tile | filter_values
    pub kind: &'static str,
    pub dashboard: Option<&'a str>,
}

/// A query's result, before it's sent.
pub struct Outcome {
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<Value>>,
    pub sql: String,
    pub stats: Value,
    pub period: Option<Value>,
}

impl Outcome {
    fn json(self, caller: &Caller) -> Value {
        json!({
            "columns": self.columns,
            "rows": self.rows,
            "sql": caller.can("view_sql").then_some(&self.sql),
            "stats": self.stats,
            "period": self.period,
        })
    }
}

/// Compile and run a query (on a model or a blend), then its calculations on results, if any.
/// Every query is a usage event.
pub async fn execute(state: &AppState, caller: &Caller, q: &Query, source: Source<'_>) -> ApiResult<Value> {
    let Some(split) = crate::results::split(q).map_err(ApiError::bad_request)? else {
        return Ok(outcome(state, caller, q, source).await?.json(caller));
    };
    let out = outcome(state, caller, &split.inner, source).await?;
    Ok(on_results(state, caller, out, &split, &q.sort).await?.json(caller))
}

async fn outcome(state: &AppState, caller: &Caller, q: &Query, source: Source<'_>) -> ApiResult<Outcome> {
    if let Some(b) = caller.project.blends.get(&q.model) {
        model_of(caller, &q.model)?; // available to this person?
        return blends::execute(state, caller, b, q, source).await;
    }
    run_model(state, caller, q, source).await
}

/// The calculations on results (results.rs), on the organisation's ScramDB: the result as a work
/// table, the calculations' values read by row, the table dropped.
async fn on_results(
    state: &AppState,
    caller: &Caller,
    out: Outcome,
    split: &crate::results::Split,
    sort: &[String],
) -> ApiResult<Outcome> {
    let started = std::time::Instant::now();
    let db = state.orgs.store(caller.org()).await?;
    let table = materialize::work_table_name();
    let calc_sql = crate::results::sql(&table, &out.columns, &split.calcs).map_err(ApiError::bad_request)?;
    let types = crate::results::column_types(&out.columns);
    let values = async {
        materialize::copy(&db.pool, &table, &types, &out.rows).await?;
        let c = db.pool.get().await?;
        let rows = c.query(&calc_sql, &[]).await?;
        Ok::<_, anyhow::Error>(rows.iter().map(|r| crate::connectors::postgres::row_json(r)[1..].to_vec()).collect())
    }
    .await;
    if let Err(e) = materialize::drop(&db.pool, "public", &table).await {
        tracing::warn!(table, "results table not dropped (a cache sweep will): {e:#}");
    }
    let values: Vec<Vec<Value>> = values?;
    let Outcome { columns, rows, sql, mut stats, period } = out;
    let (columns, rows) = crate::results::finish(columns, rows, values, split, sort);
    stats["calculations_ms"] = json!(started.elapsed().as_millis() as u64);
    let sql = format!("{sql}\n\n-- then, on the results:\n{calc_sql}");
    Ok(Outcome { columns, rows, sql, stats, period })
}

/// The organisation's ScramDB when results may be cached (`ttl` > 0; none for the platform
/// workspace).
async fn cache_store(state: &AppState, caller: &Caller, ttl: u64) -> Option<Store> {
    match (ttl > 0, caller.org()) {
        (true, org) if !org.is_empty() => state.orgs.store(org).await.ok(),
        _ => None,
    }
}

/// A cached result, unless the person (a builder) asked for fresh data.
async fn cached(caller: &Caller, store: &Option<Store>, key: &str) -> Option<cache::Hit> {
    let store = store.as_ref()?;
    if caller.refresh && caller.can("edit") {
        return None;
    }
    match cache::get(store, key).await {
        Ok(hit) => hit,
        Err(e) => {
            tracing::warn!(org = caller.org(), "query cache unavailable: {e:#}");
            None
        }
    }
}

/// Fill the cache without making the request wait.
fn cache_later(
    store: Option<Store>,
    key: String,
    put: (String, String, String, u64),
    result: (Vec<String>, Vec<ColType>, Vec<Vec<Value>>),
) {
    let Some(store) = store else { return };
    let (org, model, connection, ttl) = put;
    tokio::spawn(async move {
        let (names, types, rows) = result;
        let put = cache::Put {
            key: &key,
            org: &org,
            model: &model,
            connection: &connection,
            ttl_seconds: ttl,
            names: &names,
            types: &types,
            rows: &rows,
        };
        if let Err(e) = cache::put(&store, put).await {
            tracing::warn!(org, model, "result not cached: {e:#}");
        }
    });
}

/// A failed query, recorded; database errors can quote SQL, tables and values: details for
/// builders only.
fn query_failed(
    state: &AppState,
    caller: &Caller,
    e: &ConnectorError,
    event: crate::usage::Event,
    what: &str,
) -> ApiError {
    let reference = crate::store::random_token(4);
    tracing::warn!(reference, what, "query failed: {e}");
    state.usage.record(crate::usage::Event { ok: false, detail: Some(json!({"ref": reference})), ..event });
    let detail = match (e, caller.can("view_sql")) {
        (ConnectorError::Unavailable(m), _) => format!("{m} (ref {reference})"),
        (ConnectorError::Query(m), true) => format!("{m} (ref {reference})"),
        _ => format!("the query failed (ref {reference}); a system owner can look it up in the logs"),
    };
    ApiError::Status(StatusCode::BAD_GATEWAY, detail)
}

/// Compile and run a query on a model: from the organisation's cache when the model allows
/// (`cache:`), else on the model's connection (then cached in the background).
pub async fn run_model(state: &AppState, caller: &Caller, q: &Query, source: Source<'_>) -> ApiResult<Outcome> {
    let m = model_of(caller, &q.model)?;
    let (dialect, connector) = connection_for(state, caller, m).await?;
    let compiled = compile_for(caller, q, dialect)?;
    let event = || crate::usage::Event {
        source: Some(source.kind),
        dashboard_id: source.dashboard.map(String::from),
        model: Some(q.model.clone()),
        connection: Some(m.connection.clone()),
        ..caller.event("query")
    };
    let Compiled { sql, columns, period } = compiled;
    let ttl = m.cache.unwrap_or(0);
    let store = cache_store(state, caller, ttl).await;
    let key = cache::key(caller.org(), &m.connection, &sql);
    if let Some(hit) = cached(caller, &store, &key).await {
        state.usage.record(crate::usage::Event {
            row_count: Some(hit.rows.len() as i64),
            duration_ms: Some(0),
            cache_hit: Some(true),
            ..event()
        });
        let stats =
            json!({"duration_ms": 0, "cache_hit": true, "cached_at": hit.created_at.to_rfc3339(), "cache_ttl_s": ttl});
        return Ok(Outcome { columns, rows: hit.rows, sql, stats, period });
    }

    let names: Vec<String> = columns.iter().map(|c| c.name.clone()).collect();
    let result = connector.run(&sql, &names).await.map_err(|e| query_failed(state, caller, &e, event(), &q.model))?;
    state.usage.record(crate::usage::Event {
        row_count: Some(result.rows.len() as i64),
        duration_ms: Some(result.duration_ms as i64),
        cache_hit: Some(false),
        ..event()
    });
    cache_later(
        store,
        key,
        (caller.org().to_string(), q.model.clone(), m.connection.clone(), ttl),
        (names, result.types.clone(), result.rows.clone()),
    );
    let mut stats = json!({"duration_ms": result.duration_ms, "cache_hit": false});
    if ttl > 0 {
        stats["cache_ttl_s"] = json!(ttl);
    }
    Ok(Outcome { columns, rows: result.rows, sql, stats, period })
}

#[cfg(test)]
pub mod tests {
    use std::sync::Arc;

    use axum::body::Body as HttpBody;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use crate::state::AppState;
    use crate::store::Store;

    pub const OWNER: &str = "owner@magnum.example";

    /// The real router and state on a test ScramDB (MAGNUM_TEST_DATABASE_URL, e.g. the dev
    /// primary), with a signed-in system owner. None (test skipped) without that variable.
    pub async fn app() -> Option<(axum::Router, Arc<AppState>, String)> {
        app_with(concat!(env!("CARGO_MANIFEST_DIR"), "/../project")).await
    }

    /// `app` on another project directory.
    pub async fn app_with(project: &str) -> Option<(axum::Router, Arc<AppState>, String)> {
        let url = std::env::var("MAGNUM_TEST_DATABASE_URL").ok()?;
        let key = crate::crypto::SecretKey::generate();
        let dir = std::env::temp_dir().join(format!("magnum-api-{}", uuid::Uuid::new_v4().simple()));
        let vars = [
            ("MAGNUM_DATABASE_URL", url.clone()),
            ("MAGNUM_SECRET_KEY", key),
            ("MAGNUM_SYSTEM_OWNERS", OWNER.to_string()),
            ("MAGNUM_PROJECT_DIR", project.to_string()),
            ("MAGNUM_DUCKDB_DIR", dir.display().to_string()),
            ("MAGNUM_DUCKDB_INSTALL_EXTENSIONS", "0".to_string()),
        ];
        let config =
            crate::config::Config::from_lookup(|k| vars.iter().find(|(n, _)| *n == k).map(|(_, v)| v.clone())).unwrap();
        let pool = crate::db::make_pool(&url, 8).unwrap();
        let store = Store::new(pool);
        // once per test run: on a new database, tests starting together would race to create
        // the owner's account
        static SETUP: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();
        SETUP
            .get_or_init(|| async {
                crate::db::migrate(&store.pool, crate::db::PRIMARY).await.unwrap();
                // tests give organisations this database too (state.orgs.pin): their tables as well
                crate::db::migrate(&store.pool, crate::db::ORG).await.unwrap();
                if store.user(OWNER).await.unwrap().is_none() {
                    let hash = crate::auth::password::hash("test-only-password").unwrap();
                    store.set_password(OWNER, Some("Owner"), &hash, None, false).await.unwrap();
                }
            })
            .await;
        let session = store.create_session_as(OWNER, "password", chrono::Duration::hours(1), None).await.unwrap();
        let state = Arc::new(AppState {
            orgs: crate::orgdb::OrgDbs::new(store.clone(), config.secret_key.clone()),
            usage: crate::usage::Usage::start(store.clone(), 400),
            connectors: crate::connectors::Connectors::new(config.secret_key.clone(), config.duckdb_dir.clone(), false),
            store,
            project: crate::project::Project::new(&config.project_dir),
            access: crate::state::AccessCache::new(),
            google: None,
            limiter: crate::auth::password::LoginLimiter::default(),
            ip_limiter: crate::auth::password::LoginLimiter::new(20),
            hashing: tokio::sync::Semaphore::new(crate::state::hashing_permits()),
            blends: crate::state::OrgBlends::default(),
            themes: Default::default(),
            models: Default::default(),
            formulas: Default::default(),
            governance: Default::default(),
            docs: Default::default(),
            oidc: crate::auth::oidc::Oidc::new(),
            passkeys: crate::auth::passkeys::relying_party(&config),
            demo: Default::default(),
            flights: Default::default(),
            config,
        });
        Some((super::router(state.clone()), state, format!("magnum_session={session}")))
    }

    pub async fn call(
        app: &axum::Router,
        cookie: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut req = Request::builder().method(method).uri(path).header("cookie", cookie);
        if body.is_some() {
            req = req.header("content-type", "application/json");
        }
        let req = req.body(body.map_or(HttpBody::empty(), |b| HttpBody::from(b.to_string()))).unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    /// The test database as a `scramdb` connection.
    pub fn scramdb_config() -> Value {
        let u = url::Url::parse(&std::env::var("MAGNUM_TEST_DATABASE_URL").unwrap()).unwrap();
        json!({"type": "scramdb", "host": u.host_str().unwrap(), "port": u.port().unwrap_or(5432),
               "database": u.path().trim_start_matches('/'), "user": u.username(), "tls": false})
    }

    #[tokio::test]
    async fn connections_are_managed_per_organisation() {
        let Some((app, _, cookie)) = app().await else { return };
        let name = format!("c{}", uuid::Uuid::new_v4().simple());
        let path = format!("/api/connections/{name}");

        // not signed in
        assert_eq!(call(&app, "", "GET", "/api/connections", None).await.0, StatusCode::UNAUTHORIZED);

        // try it out before saving
        let (s, body) =
            call(&app, &cookie, "POST", &format!("{path}/test"), Some(json!({"config": scramdb_config()}))).await;
        assert_eq!(s, StatusCode::OK, "{body}");
        assert!(body["message"].as_str().unwrap().contains("ScramDB"), "{body}");

        let (s, body) =
            call(&app, &cookie, "PUT", &path, Some(json!({"config": scramdb_config(), "secret": "pw"}))).await;
        assert_eq!(s, StatusCode::OK, "{body}");
        let (_, list) = call(&app, &cookie, "GET", "/api/connections", None).await;
        let mine = list["connections"].as_array().unwrap().iter().find(|c| c["name"] == json!(name)).cloned().unwrap();
        assert_eq!((mine["type"].as_str(), mine["has_secret"].as_bool()), (Some("scramdb"), Some(true)));
        assert!(!list.to_string().contains("\"pw\""));

        // invalid settings are refused with a reason
        let bad = json!({"type": "postgres", "host": "x y", "database": "d", "user": "u"});
        let (s, body) = call(&app, &cookie, "PUT", &path, Some(json!({"config": bad}))).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
        assert!(body["detail"].as_str().unwrap().contains("host"), "{body}");

        let (s, _) = call(&app, &cookie, "DELETE", &path, None).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(call(&app, &cookie, "DELETE", &path, None).await.0, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn models_query_through_the_organisations_connection() {
        let Some((app, _, cookie)) = app().await else { return };
        // the sample model reads `warehouse`: without it, a clear error
        call(&app, &cookie, "DELETE", "/api/connections/warehouse", None).await;
        let q = json!({"model": "sales", "dimensions": ["region"], "measures": ["transactions"]});
        let (s, body) = call(&app, &cookie, "POST", "/api/query", Some(q.clone())).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
        assert!(body["detail"].as_str().unwrap().contains("no connection named \"warehouse\""), "{body}");

        call(&app, &cookie, "PUT", "/api/connections/warehouse", Some(json!({"config": scramdb_config()}))).await;
        let (s, body) = call(&app, &cookie, "POST", "/api/query", Some(q)).await;
        assert_eq!(s, StatusCode::OK, "{body}");
        assert_eq!(body["rows"].as_array().unwrap().len(), 3, "{body}");
        let (_, models) = call(&app, &cookie, "GET", "/api/models", None).await;
        assert_eq!(models["models"][0]["connection_type"], json!("scramdb"));
    }

    async fn login_from(app: &axum::Router, ip: &str, email: &str, password: &str) -> StatusCode {
        let mut req = Request::builder()
            .method("POST")
            .uri("/api/auth/login")
            .header("content-type", "application/json")
            .body(HttpBody::from(json!({"email": email, "password": password}).to_string()))
            .unwrap();
        let addr: std::net::SocketAddr = format!("{ip}:40000").parse().unwrap();
        req.extensions_mut().insert(axum::extract::ConnectInfo(addr));
        app.clone().oneshot(req).await.unwrap().status()
    }

    #[tokio::test]
    async fn password_spraying_is_throttled_per_address() {
        let Some((app, _, _)) = app().await else { return };
        let run = uuid::Uuid::new_v4().simple().to_string();
        let ip = format!("203.0.113.{}", u8::from_str_radix(&run[..2], 16).unwrap() % 250 + 1);
        for i in 0..20 {
            let s = login_from(&app, &ip, &format!("u{i}-{run}@spray.example"), "Winter2026!!").await;
            assert_eq!(s, StatusCode::UNAUTHORIZED, "attempt {i}");
        }
        // one more account from the same address: refused without even checking
        assert_eq!(
            login_from(&app, &ip, &format!("u20-{run}@spray.example"), "x").await,
            StatusCode::TOO_MANY_REQUESTS
        );
        // someone else, elsewhere, is unaffected
        assert_eq!(
            login_from(&app, "198.51.100.7", &format!("other-{run}@spray.example"), "x").await,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn failed_sign_ins_are_audited() {
        let Some((app, state, _)) = app().await else { return };
        let email = format!("audit-{}@spray.example", uuid::Uuid::new_v4().simple());
        assert_eq!(login_from(&app, "192.0.2.10", &email, "wrong password").await, StatusCode::UNAUTHORIZED);
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        let c = state.store.pool.get().await.unwrap();
        let row = c
            .query_opt("SELECT detail FROM usage_events WHERE kind = 'sign_in_failed' AND user_email = $1", &[&email])
            .await
            .unwrap()
            .expect("a sign_in_failed event");
        let detail: Value = serde_json::from_str(&row.get::<_, String>(0)).unwrap();
        assert_eq!(detail["reason"], json!("wrong_password"));
        assert_eq!(detail["ip"], json!("192.0.2.10"));
    }
}
