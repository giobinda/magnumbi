//! Dashboards: list, read, save (JSON or YAML), delete, history; running saved tiles.
//!
//! An organisation's dashboards live in its own ScramDB; the platform workspace's (templates)
//! in the primary database (orgdb.rs decides).

use axum::Json;
use axum::extract::{Path, Query as UrlQuery, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::IntoResponse;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Body, Source, available_names, execute, model_of};
use crate::auth::{Auth, Caller};
use crate::error::{ApiError, ApiResult};
use crate::project::{DASHBOARD_ID, dump_dashboard, parse_dashboard};
use crate::schema::{Dashboard, Query, Tile};
use crate::state::{AppState, Shared};
use crate::store::Store;
use crate::tile_query::{TileRequest, TileRequestError, build};

fn check_id(id: &str) -> ApiResult<()> {
    if DASHBOARD_ID.is_match(id) { Ok(()) } else { Err(ApiError::bad_request(format!("invalid dashboard id {id:?}"))) }
}

async fn store(state: &AppState, caller: &Caller) -> ApiResult<Store> {
    Ok(state.orgs.store(caller.org()).await?)
}

pub async fn load(state: &AppState, caller: &Caller, id: &str) -> ApiResult<(Dashboard, String)> {
    check_id(id)?;
    let text = store(state, caller)
        .await?
        .read_dashboard(caller.org(), id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("dashboard {id:?} not found")))?;
    let d = parse_dashboard(&text).map_err(ApiError::bad_request)?;
    Ok((d, text))
}

async fn response(state: &AppState, caller: &Caller, id: &str, d: &Dashboard, text: &str) -> ApiResult<Value> {
    let builder = caller.can("edit");
    let warnings = if builder {
        let dialects = super::dialects(state, caller).await?;
        caller.project.validate_dashboard(d, &available_names(caller), &|c| dialects.get(c).copied())
    } else {
        vec![]
    };
    Ok(
        json!({"id": id, "dashboard": d, "yaml": builder.then_some(text), "warnings": warnings, "version": version(text)}),
    )
}

/// A dashboard's version: a hash of its YAML (send it back as If-Match to update only that).
pub fn version(text: &str) -> String {
    crate::store::sha256_hex(text)[..16].to_string()
}

#[derive(Deserialize, Default)]
pub struct WriteParams {
    /// check only: validate and report warnings, save nothing (?dry_run=1 or =true)
    #[serde(default, deserialize_with = "flag")]
    pub dry_run: bool,
}

pub(crate) fn flag<'de, D: serde::Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    let v = String::deserialize(d)?;
    Ok(matches!(v.as_str(), "1" | "true" | "yes"))
}

/// `If-Match: <version>` (or `none`: must not exist yet): refuse a save over a dashboard that
/// changed since it was read (an agent and a person editing at once).
async fn check_version(store: &Store, caller: &Caller, id: &str, headers: &HeaderMap) -> ApiResult<()> {
    let Some(expected) = headers.get(header::IF_MATCH).and_then(|v| v.to_str().ok()) else { return Ok(()) };
    let expected = expected.trim().trim_matches('"');
    let current = store.read_dashboard(caller.org(), id).await?.map(|t| version(&t));
    if current.as_deref().unwrap_or("none") == expected {
        return Ok(());
    }
    let detail = match &current {
        Some(_) => format!("dashboard {id:?} changed since you read it: read it again, then save"),
        None => format!("dashboard {id:?} doesn't exist (any more)"),
    };
    Err(ApiError::Body(
        StatusCode::CONFLICT,
        json!({"detail": detail, "version": current.unwrap_or_else(|| "none".into())}),
    ))
}

fn track(state: &AppState, caller: &Caller, kind: &'static str, id: &str) {
    state.usage.record(crate::usage::Event { dashboard_id: Some(id.into()), ..caller.event(kind) });
}

pub async fn list(State(state): State<Shared>, Auth(caller): Auth) -> ApiResult<Json<Value>> {
    caller.require("view")?;
    let store = store(&state, &caller).await?;
    let mut out = vec![];
    for (id, text) in store.all_dashboards(caller.org()).await? {
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
    track(&state, &caller, "dashboard_view", &id);
    Ok(Json(response(&state, &caller, &id, &d, &text).await?))
}

pub async fn save(
    State(state): State<Shared>,
    Auth(caller): Auth,
    Path(id): Path<String>,
    UrlQuery(p): UrlQuery<WriteParams>,
    headers: HeaderMap,
    Body(d): Body<Dashboard>,
) -> ApiResult<Json<Value>> {
    d.validate().map_err(ApiError::bad_request)?;
    let text = dump_dashboard(&d);
    write(&state, &caller, &id, d, text, p.dry_run, &headers).await
}

pub async fn save_yaml(
    State(state): State<Shared>,
    Auth(caller): Auth,
    Path(id): Path<String>,
    UrlQuery(p): UrlQuery<WriteParams>,
    headers: HeaderMap,
    text: String,
) -> ApiResult<Json<Value>> {
    let d = parse_dashboard(&text).map_err(ApiError::bad_request)?;
    let text = if text.ends_with('\n') { text } else { text + "\n" };
    write(&state, &caller, &id, d, text, p.dry_run, &headers).await
}

async fn write(
    state: &AppState,
    caller: &Caller,
    id: &str,
    d: Dashboard,
    text: String,
    dry_run: bool,
    headers: &HeaderMap,
) -> ApiResult<Json<Value>> {
    caller.require("edit")?;
    check_id(id)?;
    let store = store(state, caller).await?;
    check_version(&store, caller, id, headers).await?;
    if dry_run {
        let mut out = response(state, caller, id, &d, &text).await?;
        out["dry_run"] = json!(true);
        return Ok(Json(out));
    }
    store.write_dashboard(caller.org(), id, &text, Some(caller.email())).await?;
    track(state, caller, "dashboard_saved", id);
    Ok(Json(response(state, caller, id, &d, &text).await?))
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

pub async fn delete(
    State(state): State<Shared>,
    Auth(caller): Auth,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    caller.require("edit")?;
    check_id(&id)?;
    let store = store(&state, &caller).await?;
    check_version(&store, &caller, &id, &headers).await?;
    if !store.remove_dashboard(caller.org(), &id, Some(caller.email())).await? {
        return Err(ApiError::not_found(format!("dashboard {id:?} not found")));
    }
    track(&state, &caller, "dashboard_deleted", &id);
    Ok(Json(json!({"ok": true})))
}

pub async fn history(
    State(state): State<Shared>,
    Auth(caller): Auth,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    caller.require("edit")?;
    check_id(&id)?;
    Ok(Json(json!({"history": store(&state, &caller).await?.dashboard_history(caller.org(), &id, 50).await?})))
}

/// Run a saved tile with the viewer's filter values, period window and drill clicks.
pub async fn run_tile(
    State(state): State<Shared>,
    Auth(caller): Auth,
    Path((id, tile_id)): Path<(String, String)>,
    Body(req): Body<TileRequest>,
) -> ApiResult<Json<Value>> {
    Ok(Json(tile_for(&state, &caller, &id, &tile_id, &req).await?))
}

/// A tile's query: a text tile has none to run.
pub(super) fn query_of(tile: &Tile) -> ApiResult<&Query> {
    tile.query.as_ref().ok_or_else(|| ApiError::bad_request(format!("tile {:?} is text: nothing to run", tile.id)))
}

/// A saved tile's data for this caller (signed in, or a guest link's guest).
pub async fn tile_for(
    state: &AppState,
    caller: &Caller,
    id: &str,
    tile_id: &str,
    req: &TileRequest,
) -> ApiResult<Value> {
    run_saved(state, caller, id, tile_id, req, "tile").await
}

/// Run a saved tile; `kind` says why in the usage log (tile, export).
pub async fn run_saved(
    state: &AppState,
    caller: &Caller,
    id: &str,
    tile_id: &str,
    req: &TileRequest,
    kind: &'static str,
) -> ApiResult<Value> {
    caller.require("view")?;
    let (d, _) = load(state, caller, id).await?;
    let tile = d
        .tiles
        .iter()
        .find(|t| t.id == tile_id)
        .ok_or_else(|| ApiError::not_found(format!("tile {tile_id:?} not found")))?;
    let model = model_of(caller, &query_of(tile)?.model)?;
    let q = build(&d, tile, model, req, caller.can("drill")).map_err(|e| match e {
        TileRequestError::NoDrill => ApiError::forbidden("your role can't drill down"),
        TileRequestError::Invalid(m) => ApiError::bad_request(m),
    })?;
    execute(state, caller, &q, Source { kind, dashboard: Some(id) }).await
}

#[derive(Deserialize)]
pub struct KeyParam {
    pub key: String,
}

/// Distinct values for one of the dashboard's own filters (the filter bar's pick list).
pub async fn filter_values(
    State(state): State<Shared>,
    Auth(caller): Auth,
    Path(id): Path<String>,
    UrlQuery(p): UrlQuery<KeyParam>,
) -> ApiResult<Json<Value>> {
    Ok(Json(filter_values_for(&state, &caller, &id, &p.key).await?))
}

/// The pick list of one of a dashboard's filters, for this caller (signed in, or a guest).
pub async fn filter_values_for(state: &AppState, caller: &Caller, id: &str, key: &str) -> ApiResult<Value> {
    caller.require("filter")?;
    let (d, _) = load(state, caller, id).await?;
    let f = d
        .filters
        .iter()
        .find(|f| format!("{}.{}", f.model, f.field) == key)
        .ok_or_else(|| ApiError::not_found(format!("{key:?} is not a filter of this dashboard")))?;
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
    execute(state, caller, &q, Source { kind: "filter_values", dashboard: Some(id) }).await
}

#[cfg(test)]
mod agent_tests {
    use axum::http::StatusCode;
    use serde_json::{Value, json};

    use crate::api::tests::{app, call};

    async fn put_yaml(
        app: &axum::Router,
        cookie: &str,
        path: &str,
        yaml: &str,
        if_match: Option<&str>,
    ) -> (StatusCode, Value) {
        let mut req = axum::http::Request::builder()
            .method("PUT")
            .uri(path)
            .header("cookie", cookie)
            .header("content-type", "text/plain");
        if let Some(v) = if_match {
            req = req.header("if-match", v);
        }
        let resp = tower::ServiceExt::oneshot(app.clone(), req.body(axum::body::Body::from(yaml.to_string())).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let bytes = http_body_util::BodyExt::collect(resp.into_body()).await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    #[tokio::test]
    async fn dry_runs_and_version_checks() {
        let Some((app, _, cookie)) = app().await else { return };
        let id = format!("agent_{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
        let path = format!("/api/dashboards/{id}/yaml");
        let v1 = "title: Agent test\ntiles:\n  - {id: n, type: kpi, query: {model: sales, measures: [transactions]}}\n";

        // a dry run checks without saving
        let (s, body) = put_yaml(&app, &cookie, &format!("{path}?dry_run=1"), v1, None).await;
        assert_eq!(s, StatusCode::OK, "{body}");
        assert_eq!(body["dry_run"], json!(true));
        assert!(body["warnings"].is_array());
        assert_eq!(call(&app, &cookie, "GET", &format!("/api/dashboards/{id}"), None).await.0, StatusCode::NOT_FOUND);
        let (s, body) = put_yaml(&app, &cookie, &format!("{path}?dry_run=1"), "title: [unclosed", None).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");

        // creating: "none" means it mustn't exist yet
        let (s, body) = put_yaml(&app, &cookie, &path, v1, Some("none")).await;
        assert_eq!(s, StatusCode::OK, "{body}");
        let version = body["version"].as_str().unwrap().to_string();
        let (s, body) = put_yaml(&app, &cookie, &path, v1, Some("none")).await;
        assert_eq!(s, StatusCode::CONFLICT, "exists now: {body}");

        // updating what you read; refused when someone changed it since
        let (_, got) = call(&app, &cookie, "GET", &format!("/api/dashboards/{id}"), None).await;
        assert_eq!(got["version"].as_str(), Some(version.as_str()));
        let v2 = v1.replace("Agent test", "Agent test 2");
        let (s, body) = put_yaml(&app, &cookie, &path, &v2, Some(&version)).await;
        assert_eq!(s, StatusCode::OK, "{body}");
        assert_ne!(body["version"].as_str(), Some(version.as_str()));
        let (s, body) = put_yaml(&app, &cookie, &path, &v1.replace("Agent test", "stale"), Some(&version)).await;
        assert_eq!(s, StatusCode::CONFLICT, "{body}");
        assert!(body["detail"].as_str().unwrap().contains("changed"), "{body}");
        let (_, now) = call(&app, &cookie, "GET", &format!("/api/dashboards/{id}"), None).await;
        assert_eq!(now["dashboard"]["title"], json!("Agent test 2"), "the stale write didn't land");
        assert_eq!(body["version"], now["version"], "the conflict says what's current");

        // without If-Match: as before (last write wins)
        assert_eq!(put_yaml(&app, &cookie, &path, v1, None).await.0, StatusCode::OK);
        call(&app, &cookie, "DELETE", &format!("/api/dashboards/{id}"), None).await;

        // blends: a dry run too
        let blend = json!({"name": id, "base": "sales", "joins": [{"model": "weather", "on": ["region = borough"]}]});
        let (s, body) = call(&app, &cookie, "PUT", &format!("/api/blends/{id}?dry_run=1"), Some(blend)).await;
        assert_eq!(s, StatusCode::OK, "{body}");
        assert_eq!(body["dry_run"], json!(true));
        let (_, list) = call(&app, &cookie, "GET", "/api/blends", None).await;
        assert!(!list.to_string().contains(&id), "not saved");
    }

    #[tokio::test]
    async fn text_tiles_and_tabs() {
        let Some((app, _, cookie)) = app().await else { return };
        let id = format!("text_{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
        let yaml = "title: Notes\ntabs: [Overview, Detail]\ntiles:\n  - {id: note, type: text, text: '## Read me', tab: Detail}\n  - {id: n, type: kpi, query: {model: sales, measures: [transactions]}}\n";
        let (s, body) = put_yaml(&app, &cookie, &format!("/api/dashboards/{id}/yaml"), yaml, None).await;
        assert_eq!(s, StatusCode::OK, "{body}");
        let (_, got) = call(&app, &cookie, "GET", &format!("/api/dashboards/{id}"), None).await;
        assert_eq!(got["dashboard"]["tabs"], json!(["Overview", "Detail"]));
        assert_eq!(got["dashboard"]["tiles"][0]["text"], json!("## Read me"));
        assert_eq!(got["dashboard"]["tiles"][0]["tab"], json!("Detail"));
        assert!(got["dashboard"]["tiles"][0].get("query").is_none(), "{got}");
        let (s, body) =
            call(&app, &cookie, "POST", &format!("/api/dashboards/{id}/tiles/note/query"), Some(json!({}))).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
        assert!(body["detail"].as_str().unwrap().contains("text"), "{body}");
        call(&app, &cookie, "DELETE", &format!("/api/dashboards/{id}"), None).await;
    }
}
