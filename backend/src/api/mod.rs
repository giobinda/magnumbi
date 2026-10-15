//! The HTTP API (/api/...) and the built frontend (everything else).

mod admin;
mod auth;
mod connections;
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
use crate::cache;
use crate::compiler::{Compiled, Dialect, compile_query, expand};
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
        .route("/api/platform/organisations/{org}/database", axum::routing::put(admin::set_org_database))
        .route("/api/usage", get(admin::usage_summary))
        .route("/api/connections", get(connections::list))
        .route("/api/connections/{name}", axum::routing::put(connections::save).delete(connections::delete))
        .route("/api/connections/{name}/test", post(connections::test))
        .route("/api/cache", get(admin::cache_stats).delete(admin::clear_cache))
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
/// conditions (compile_query never drops those, unlike ordinary filters).
pub fn compile_for(caller: &Caller, q: &Query, dialect: Dialect) -> ApiResult<Compiled> {
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

/// Compile and run a query: from the organisation's cache when the model allows (`cache:`),
/// else on the model's connection (then cached in the background). Every query is a usage event.
pub async fn execute(state: &AppState, caller: &Caller, q: &Query, source: Source<'_>) -> ApiResult<Value> {
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
    let response = |rows: Vec<Vec<Value>>, stats: Value| {
        json!({
            "columns": compiled.columns,
            "rows": rows,
            "sql": caller.can("view_sql").then_some(&compiled.sql),
            "stats": stats,
            "period": compiled.period,
        })
    };

    // the cache lives in the organisation's own ScramDB (none for the platform workspace)
    let ttl = m.cache.unwrap_or(0);
    let cache_store = match (ttl > 0, caller.org()) {
        (true, org) if !org.is_empty() => state.orgs.store(org).await.ok(),
        _ => None,
    };
    let key = cache::key(caller.org(), &m.connection, &compiled.sql);
    if let Some(store) = &cache_store
        && !(caller.refresh && caller.can("edit"))
    {
        match cache::get(store, &key).await {
            Ok(Some(hit)) => {
                state.usage.record(crate::usage::Event {
                    row_count: Some(hit.rows.len() as i64),
                    duration_ms: Some(0),
                    cache_hit: Some(true),
                    ..event()
                });
                let stats = json!({"duration_ms": 0, "cache_hit": true, "cached_at": hit.created_at.to_rfc3339(),
                                       "cache_ttl_s": ttl});
                return Ok(response(hit.rows, stats));
            }
            Ok(None) => {}
            Err(e) => tracing::warn!(org = caller.org(), "query cache unavailable: {e:#}"),
        }
    }

    let names: Vec<String> = compiled.columns.iter().map(|c| c.name.clone()).collect();
    let result = match connector.run(&compiled.sql, &names).await {
        Ok(r) => r,
        Err(e) => {
            // database errors can quote SQL, tables and values: details for builders only
            let reference = crate::store::random_token(4);
            tracing::warn!(reference, model = q.model, "query failed: {e}");
            state.usage.record(crate::usage::Event { ok: false, detail: Some(json!({"ref": reference})), ..event() });
            let detail = match (&e, caller.can("view_sql")) {
                (ConnectorError::Unavailable(m), _) => format!("{m} (ref {reference})"),
                (ConnectorError::Query(m), true) => format!("{m} (ref {reference})"),
                _ => format!("the query failed (ref {reference}); a system owner can look it up in the logs"),
            };
            return Err(ApiError::Status(StatusCode::BAD_GATEWAY, detail));
        }
    };
    state.usage.record(crate::usage::Event {
        row_count: Some(result.rows.len() as i64),
        duration_ms: Some(result.duration_ms as i64),
        cache_hit: Some(false),
        ..event()
    });
    if let Some(store) = cache_store {
        // fill the cache without making this request wait
        let (org, model, connection) = (caller.org().to_string(), q.model.clone(), m.connection.clone());
        let (rows, types) = (result.rows.clone(), result.types.clone());
        tokio::spawn(async move {
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
    let mut stats = json!({"duration_ms": result.duration_ms, "cache_hit": false});
    if ttl > 0 {
        stats["cache_ttl_s"] = json!(ttl);
    }
    Ok(response(result.rows, stats))
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
        let url = std::env::var("MAGNUM_TEST_DATABASE_URL").ok()?;
        let key = crate::crypto::SecretKey::generate();
        let dir = std::env::temp_dir().join(format!("magnum-api-{}", uuid::Uuid::new_v4().simple()));
        let vars = [
            ("MAGNUM_DATABASE_URL", url.clone()),
            ("MAGNUM_SECRET_KEY", key),
            ("MAGNUM_SYSTEM_OWNERS", OWNER.to_string()),
            ("MAGNUM_PROJECT_DIR", concat!(env!("CARGO_MANIFEST_DIR"), "/../project").to_string()),
            ("MAGNUM_DUCKDB_DIR", dir.display().to_string()),
            ("MAGNUM_DUCKDB_INSTALL_EXTENSIONS", "0".to_string()),
        ];
        let config =
            crate::config::Config::from_lookup(|k| vars.iter().find(|(n, _)| *n == k).map(|(_, v)| v.clone())).unwrap();
        let pool = crate::db::make_pool(&url, 8).unwrap();
        crate::db::migrate(&pool, crate::db::PRIMARY).await.unwrap();
        let store = Store::new(pool);
        if store.user(OWNER).await.unwrap().is_none() {
            let hash = crate::auth::password::hash("test-only-password").unwrap();
            store.set_password(OWNER, Some("Owner"), &hash, None, false).await.unwrap();
        }
        let session = store.create_session(OWNER, "password", chrono::Duration::hours(1)).await.unwrap();
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
