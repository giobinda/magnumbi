//! Public links: anyone with the link sees a dashboard, without signing in.
//!
//! Visitors see the dashboard as saved (its default filters and period; no filtering or
//! drilling, which would each be new queries), as the link's maker would see it as a Viewer.
//! Each tile's result is kept in the organisation's ScramDB for the link's cache period, so the
//! data warehouse answers at most once per tile per period however many people visit, and
//! visitors arriving together wait for that one query. If those results can't be read, the link
//! fails (503) rather than ask the warehouse. A link without a cache sends every visit to the
//! warehouse: making one needs its risks accepted.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Duration, SubsecRound, Utc};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Body, Source, execute, model_of};
use crate::access::Role;
use crate::auth::{Auth, Caller, LinkAccess, link_caller};
use crate::config::Config;
use crate::error::{ApiError, ApiResult};
use crate::schema::Dashboard;
use crate::state::{AppState, Shared};
use crate::store::{ShareLink, random_token, sha256_hex};
use crate::tile_query::{TileRequest, TileRequestError, build};

/// The cache periods a link can have, in seconds (5 minutes to a day). 0, no cache, only with
/// its risks accepted.
pub const CACHE_PERIODS: [i64; 5] = [300, 900, 3600, 6 * 3600, 24 * 3600];

const UNCACHED_RISK: &str = "without a cache, every visit to a public link runs the dashboard's queries on your data \
    warehouse: anyone with the link could overload it, or run up its bill. To make one anyway, accept that risk \
    (accept_uncached_risk: true)";

/// Public links are allowed on this server (MAGNUM_PUBLIC_LINKS, on by default).
pub fn allowed(config: &Config) -> ApiResult<()> {
    if config.public_links {
        Ok(())
    } else {
        Err(ApiError::forbidden("public links are turned off on this Magnum BI (MAGNUM_PUBLIC_LINKS=0)"))
    }
}

/// The same answer for a link that never existed, was removed, or whose maker left: nothing to
/// learn by guessing.
fn gone() -> ApiError {
    ApiError::not_found("this link doesn't work: it was removed, or never existed")
}

pub(super) fn sealed_for(org: &str) -> String {
    format!("{org}/share")
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewLink {
    cache_seconds: i64,
    #[serde(default)]
    accept_uncached_risk: bool,
}

/// Org admins only: a link makes data public. Not in the platform workspace, whose dashboards
/// are templates.
fn require_sharer(caller: &Caller) -> ApiResult<()> {
    caller.require("manage_members")?;
    if caller.org().is_empty() {
        return Err(ApiError::bad_request(
            "public links are for an organisation's dashboards (the platform workspace's are templates)",
        ));
    }
    Ok(())
}

/// The address people open: MAGNUM_PUBLIC_URL, else the host this request came to.
fn base_url(state: &AppState, headers: &HeaderMap) -> String {
    state.config.public_url.clone().unwrap_or_else(|| {
        let host = headers.get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or("localhost");
        format!("http://{host}")
    })
}

fn describe(state: &AppState, org: &str, l: &ShareLink, base: &str) -> ApiResult<Value> {
    let key = state.config.secret_key.as_ref().ok_or_else(|| ApiError::Internal(anyhow::anyhow!("no secret key")))?;
    let token = key.open(&l.token_sealed, &sealed_for(org))?;
    let url = format!("{base}/s/{org}/{token}");
    Ok(json!({
        "id": l.id(), "dashboard": l.dashboard_id, "url": url, "qr_svg": crate::auth::totp::qr_svg(&url),
        "cache_seconds": l.cache_seconds, "created_by": l.created_by, "created_at": l.created_at.to_rfc3339(),
    }))
}

/// Make a public link to a dashboard.
pub async fn create(
    State(state): State<Shared>,
    Auth(caller): Auth,
    Path(id): Path<String>,
    headers: HeaderMap,
    Body(n): Body<NewLink>,
) -> ApiResult<Json<Value>> {
    allowed(&state.config)?;
    require_sharer(&caller)?;
    match n.cache_seconds {
        0 if !n.accept_uncached_risk => return Err(ApiError::bad_request(UNCACHED_RISK)),
        0 => {}
        s if !CACHE_PERIODS.contains(&s) => {
            return Err(ApiError::bad_request(format!("the cache period must be one of {CACHE_PERIODS:?} seconds")));
        }
        _ => {}
    }
    super::dashboards::load(&state, &caller, &id).await?; // it exists, and they can see it
    let key = state
        .config
        .secret_key
        .as_ref()
        .ok_or_else(|| ApiError::bad_request("public links need MAGNUM_SECRET_KEY (to keep their addresses)"))?;
    let org = caller.org();
    let token = random_token(32);
    let link = ShareLink {
        token_hash: sha256_hex(&token),
        token_sealed: key.seal(&token, &sealed_for(org))?,
        dashboard_id: id.clone(),
        cache_seconds: n.cache_seconds,
        created_by: caller.email().to_string(),
        created_at: Utc::now(),
    };
    let store = state.orgs.store(org).await?;
    store.put_share_link(org, &link).await?;
    state.usage.record(crate::usage::Event {
        dashboard_id: Some(id),
        detail: Some(json!({"cache_seconds": n.cache_seconds, "link": link.id()})),
        ..caller.event("share_link_created")
    });
    Ok(Json(describe(&state, org, &link, &base_url(&state, &headers))?))
}

/// A dashboard's public links.
pub async fn list(
    State(state): State<Shared>,
    Auth(caller): Auth,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    require_sharer(&caller)?;
    let org = caller.org();
    let base = base_url(&state, &headers);
    let links = state.orgs.store(org).await?.share_links(org, Some(&id)).await?;
    let links = links.iter().map(|l| describe(&state, org, l, &base)).collect::<ApiResult<Vec<_>>>()?;
    Ok(Json(json!({"links": links, "allowed": state.config.public_links, "cache_periods": CACHE_PERIODS})))
}

/// Remove a public link (by its id): its address stops working at once.
pub async fn revoke(State(state): State<Shared>, Auth(caller): Auth, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    require_sharer(&caller)?;
    let org = caller.org();
    let store = state.orgs.store(org).await?;
    let link = store
        .share_links(org, None)
        .await?
        .into_iter()
        .find(|l| l.id() == id)
        .ok_or_else(|| ApiError::not_found(format!("no public link {id:?}")))?;
    store.delete_share_link(org, &link.token_hash).await?;
    state.usage.record(crate::usage::Event {
        dashboard_id: Some(link.dashboard_id.clone()),
        detail: Some(json!({"link": link.id()})),
        ..caller.event("share_link_revoked")
    });
    Ok(Json(json!({"ok": true})))
}

/// A link's organisation, link, visitor (its maker, as a Viewer) and dashboard (and version).
async fn open(state: &AppState, org: &str, token: &str) -> ApiResult<(ShareLink, Caller, Dashboard, String)> {
    if !state.config.public_links {
        return Err(gone());
    }
    let (access, _) = state.access.get(state, &state.project.current()).await;
    if org.is_empty() || !access.platform.organisations.contains_key(org) {
        return Err(gone());
    }
    let store = state.orgs.store(org).await?;
    let link = store.share_link(org, &sha256_hex(token)).await?.ok_or_else(gone)?;
    let access =
        LinkAccess { min: Role::Viewer, cap: Role::Viewer, filters: &[], name: format!("public link {}", link.id()) };
    let caller = link_caller(state, org, &link.created_by, access).await?.ok_or_else(gone)?;
    let (d, text) = match super::dashboards::load(state, &caller, &link.dashboard_id).await {
        Err(ApiError::Status(code, _)) if code == axum::http::StatusCode::NOT_FOUND => return Err(gone()),
        other => other?,
    };
    Ok((link, caller, d, super::dashboards::version(&text)))
}

/// Not for search engines; cached by browsers and proxies for as long as the result stays.
fn public_response(body: Value, cache_control: String) -> Response {
    let mut r = Json(body).into_response();
    let h = r.headers_mut();
    h.insert("x-robots-tag", HeaderValue::from_static("noindex"));
    if let Ok(v) = HeaderValue::from_str(&cache_control) {
        h.insert(header::CACHE_CONTROL, v);
    }
    r
}

/// The dashboard behind a public link: its layout (not its queries), and its theme.
pub async fn dashboard(State(state): State<Shared>, Path((org, token)): Path<(String, String)>) -> ApiResult<Response> {
    let (link, caller, d, _) = open(&state, &org, &token).await?;
    let theme_id = d.theme.clone().unwrap_or_else(|| caller.project.theme_defaults.0.clone());
    let theme = caller.project.themes.iter().find(|t| t.id == theme_id);
    let tiles: Vec<Value> = d
        .tiles
        .iter()
        .map(|t| {
            json!({"id": t.id, "title": t.title, "type": t.kind, "layout": t.layout, "options": t.options,
                   "text": t.text, "tab": t.tab})
        })
        .collect();
    let body = json!({"title": d.title, "description": d.description, "tabs": d.tabs, "tiles": tiles, "theme": theme,
                      "cache_seconds": link.cache_seconds});
    Ok(public_response(body, "no-cache".into()))
}

/// One tile's data behind a public link: from what the link last showed while it's fresh, else
/// from the data warehouse (once, however many visitors are waiting).
pub async fn tile(
    State(state): State<Shared>,
    Path((org, token, tile_id)): Path<(String, String, String)>,
) -> ApiResult<Response> {
    let (link, caller, d, version) = open(&state, &org, &token).await?;
    let tile =
        d.tiles.iter().find(|t| t.id == tile_id).ok_or_else(|| ApiError::not_found(format!("no tile {tile_id:?}")))?;
    let store = state.orgs.store(&org).await?;
    let (hash, ttl) = (&link.token_hash, link.cache_seconds);
    let kept = || async { if ttl > 0 { store.shared_result(&org, hash, &tile_id, &version).await } else { Ok(None) } };
    if let Some(hit) = kept().await? {
        return Ok(answer(hit, true));
    }
    let _turn = state.flights.lock(&format!("{org}/{hash}/{tile_id}")).await;
    if let Some(hit) = kept().await? {
        return Ok(answer(hit, true)); // a visitor ahead of this one asked
    }
    let model = model_of(&caller, &super::dashboards::query_of(tile)?.model)?;
    let q = build(&d, tile, model, &TileRequest::default(), false).map_err(|e| match e {
        TileRequestError::NoDrill => ApiError::forbidden("no drilling on a public link"),
        TileRequestError::Invalid(m) => ApiError::bad_request(m),
    })?;
    let result = execute(&state, &caller, &q, Source { kind: "public", dashboard: Some(&link.dashboard_id) }).await?;
    let now = Utc::now().trunc_subsecs(6); // as the database keeps it, so every visitor sees the same time
    let expires = now + Duration::seconds(ttl);
    let text = result.to_string();
    if ttl > 0
        && let Err(e) = store.put_shared_result(&org, hash, &tile_id, &version, &text, (now, expires)).await
    {
        tracing::warn!(org, link = link.id(), "public link's result not kept: {e:#}");
    }
    Ok(answer((text, now, if ttl > 0 { expires } else { now }), false))
}

fn answer((text, as_of, expires): (String, DateTime<Utc>, DateTime<Utc>), from_cache: bool) -> Response {
    let mut body: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    body["as_of"] = json!(as_of.to_rfc3339());
    body["from_cache"] = json!(from_cache);
    let left = (expires - Utc::now()).num_seconds();
    let cache_control = if left > 0 {
        body["expires_at"] = json!(expires.to_rfc3339());
        format!("public, max-age={left}")
    } else {
        "no-store".to_string()
    };
    public_response(body, cache_control)
}

#[cfg(test)]
pub(crate) mod tests {
    use axum::http::{HeaderMap, StatusCode};
    use serde_json::{Value, json};

    use crate::api::tests::{OWNER, app, scramdb_config};

    const DASHBOARD: &str = "title: Shared sales\ntabs: [Main, Notes]\ntiles:\n  - id: t\n    type: kpi\n    query: {model: sales, measures: [transactions]}\n    layout: {x: 0, y: 0, w: 3, h: 2}\n  - id: regions\n    type: bar\n    query: {model: sales, dimensions: [region], measures: [transactions]}\n    layout: {x: 3, y: 0, w: 6, h: 4}\n  - {id: note, type: text, text: Hello, tab: Notes}\n";

    /// A request, signed in (cookie) or not, in an organisation: (status, JSON, headers).
    pub(crate) async fn send(
        app: &axum::Router,
        cookie: Option<&str>,
        org: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
        view_as: Option<&str>,
    ) -> (StatusCode, Value, HeaderMap) {
        let mut req = axum::http::Request::builder().method(method).uri(path).header("x-magnum-org", org);
        if let Some(c) = cookie {
            req = req.header("cookie", c);
        }
        if let Some(r) = view_as {
            req = req.header("x-magnum-view-as", r);
        }
        let body = match body {
            Some(b) => {
                req = req.header("content-type", "application/json");
                axum::body::Body::from(b.to_string())
            }
            None => axum::body::Body::empty(),
        };
        let resp = tower::ServiceExt::oneshot(app.clone(), req.body(body).unwrap()).await.unwrap();
        let (status, headers) = (resp.status(), resp.headers().clone());
        let bytes = http_body_util::BodyExt::collect(resp.into_body()).await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null), headers)
    }

    /// An organisation with the sample data as its `warehouse` and a dashboard.
    async fn org_with_dashboard() -> Option<(axum::Router, std::sync::Arc<crate::state::AppState>, String, String)> {
        org_with(DASHBOARD).await
    }

    /// An organisation with the sample data as its `warehouse` and the dashboard `shared`.
    pub(crate) async fn org_with(
        dashboard: &str,
    ) -> Option<(axum::Router, std::sync::Arc<crate::state::AppState>, String, String)> {
        let (app, state, cookie) = app().await?;
        let org = format!("share{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
        state.orgs.pin(&org, state.store.clone());
        let access =
            json!({"system_owners": [OWNER], "organisations": {org.clone(): {"name": "Share test", "models": ["*"]}}});
        state.access.pin(crate::access::AccessConfig::from_document(access).unwrap());
        let c = Some(cookie.as_str());
        let (s, b, _) =
            send(&app, c, &org, "PUT", "/api/connections/warehouse", Some(json!({"config": scramdb_config()})), None)
                .await;
        assert_eq!(s, StatusCode::OK, "{b}");
        let req = axum::http::Request::builder()
            .method("PUT")
            .uri("/api/dashboards/shared/yaml")
            .header("cookie", &cookie)
            .header("x-magnum-org", &org)
            .header("content-type", "text/plain")
            .body(axum::body::Body::from(dashboard.to_string()))
            .unwrap();
        let resp = tower::ServiceExt::oneshot(app.clone(), req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        Some((app, state, cookie, org))
    }

    pub(crate) async fn clean_up(state: &crate::state::AppState, org: &str) {
        let c = state.store.pool.get().await.unwrap();
        for table in [
            "shared_results",
            "share_links",
            "guest_links",
            "org_models",
            "formulas",
            "dashboards",
            "dashboard_versions",
            "connections",
            "mail_servers",
            "webhooks",
            "schedules",
        ] {
            c.execute(&format!("DELETE FROM {table} WHERE org = $1"), &[&org]).await.unwrap();
        }
    }

    fn path_of(url: &Value) -> String {
        let url = url.as_str().unwrap();
        url[url.find("/s/").unwrap()..].to_string()
    }

    #[tokio::test]
    async fn org_admins_share_dashboards_publicly_from_the_cache() {
        let Some((app, state, cookie, org)) = org_with_dashboard().await else { return };
        let c = Some(cookie.as_str());
        let links = "/api/dashboards/shared/share-links";

        // only Org admins make links
        let (s, _, _) =
            send(&app, c, &org, "POST", links, Some(json!({"cache_seconds": 3600})), Some("power_user")).await;
        assert_eq!(s, StatusCode::FORBIDDEN);
        let (s, link, _) = send(&app, c, &org, "POST", links, Some(json!({"cache_seconds": 3600})), None).await;
        assert_eq!(s, StatusCode::OK, "{link}");
        assert_eq!(link["cache_seconds"], json!(3600));
        assert!(link["qr_svg"].as_str().unwrap().contains("<svg"), "{link}");
        let page = path_of(&link["url"]); // /s/<org>/<token>
        assert!(page.starts_with(&format!("/s/{org}/")), "{page}");
        let api = page.replacen("/s/", "/api/public/", 1);

        // anyone with the link: the dashboard's layout, not its queries
        let (s, d, headers) = send(&app, None, "", "GET", &api, None, None).await;
        assert_eq!(s, StatusCode::OK, "{d}");
        assert_eq!(d["title"], json!("Shared sales"));
        assert_eq!(d["tiles"][0]["id"], json!("t"));
        assert!(d["tiles"][0].get("query").is_none(), "{d}");
        assert_eq!(d["tabs"], json!(["Main", "Notes"]));
        assert_eq!((&d["tiles"][2]["text"], &d["tiles"][2]["tab"]), (&json!("Hello"), &json!("Notes")));
        assert_eq!(d["cache_seconds"], json!(3600));
        assert_eq!(headers["x-robots-tag"], "noindex");

        // the first visit asks the data warehouse; the next ones get that answer until it expires
        let tile = format!("{api}/tiles/t");
        let (s, first, headers) = send(&app, None, "", "GET", &tile, None, None).await;
        assert_eq!(s, StatusCode::OK, "{first}");
        assert_eq!(first["rows"].as_array().unwrap().len(), 1, "{first}");
        assert_eq!(first["from_cache"], json!(false));
        assert!(first["sql"].is_null(), "{first}");
        let cache_control = headers["cache-control"].to_str().unwrap();
        assert!(cache_control.starts_with("public, max-age="), "{cache_control}");
        let (_, again, _) = send(&app, None, "", "GET", &tile, None, None).await;
        assert_eq!(again["from_cache"], json!(true));
        assert_eq!(again["as_of"], first["as_of"]);
        assert_eq!(again["rows"], first["rows"]);

        // Org admins see the links and remove them; a removed link shows nothing
        let (_, list, _) = send(&app, c, &org, "GET", links, None, None).await;
        assert_eq!(list["links"].as_array().unwrap().len(), 1, "{list}");
        assert_eq!(list["links"][0]["url"], link["url"]);
        let id = link["id"].as_str().unwrap();
        let (s, _, _) = send(&app, c, &org, "DELETE", &format!("/api/share-links/{id}"), None, None).await;
        assert_eq!(s, StatusCode::OK);
        let (s, _, _) = send(&app, None, "", "GET", &tile, None, None).await;
        assert_eq!(s, StatusCode::NOT_FOUND);
        let (s, _, _) = send(&app, None, "", "GET", &api, None, None).await;
        assert_eq!(s, StatusCode::NOT_FOUND);
        clean_up(&state, &org).await;
    }

    #[tokio::test]
    async fn turning_the_cache_off_needs_the_risks_accepted() {
        let Some((app, state, cookie, org)) = org_with_dashboard().await else { return };
        let c = Some(cookie.as_str());
        let links = "/api/dashboards/shared/share-links";
        let (s, b, _) = send(&app, c, &org, "POST", links, Some(json!({"cache_seconds": 0})), None).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert!(b["detail"].as_str().unwrap().contains("every visit"), "{b}");
        // odd periods are refused too
        let (s, _, _) = send(&app, c, &org, "POST", links, Some(json!({"cache_seconds": 5})), None).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);

        let body = json!({"cache_seconds": 0, "accept_uncached_risk": true});
        let (s, link, _) = send(&app, c, &org, "POST", links, Some(body), None).await;
        assert_eq!(s, StatusCode::OK, "{link}");
        assert_eq!(link["cache_seconds"], json!(0));
        assert_eq!(link["created_by"], json!(OWNER));
        let tile = format!("{}/tiles/t", path_of(&link["url"]).replacen("/s/", "/api/public/", 1));
        for _ in 0..2 {
            let (s, r, headers) = send(&app, None, "", "GET", &tile, None, None).await;
            assert_eq!(s, StatusCode::OK, "{r}");
            assert_eq!(r["from_cache"], json!(false));
            assert_eq!(headers["cache-control"], "no-store");
        }
        clean_up(&state, &org).await;
    }

    #[tokio::test]
    async fn links_only_work_for_organisations_and_their_members() {
        let Some((app, state, cookie, org)) = org_with_dashboard().await else { return };
        let c = Some(cookie.as_str());
        // the platform workspace's dashboards are templates: not shared publicly
        let (s, _, _) =
            send(&app, c, "", "POST", "/api/dashboards/shared/share-links", Some(json!({"cache_seconds": 3600})), None)
                .await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        // a made-up link, or one in another organisation: nothing
        let (s, _, _) = send(&app, None, "", "GET", &format!("/api/public/{org}/not-a-token"), None, None).await;
        assert_eq!(s, StatusCode::NOT_FOUND);

        let (_, link, _) = send(
            &app,
            c,
            &org,
            "POST",
            "/api/dashboards/shared/share-links",
            Some(json!({"cache_seconds": 300})),
            None,
        )
        .await;
        let api = path_of(&link["url"]).replacen("/s/", "/api/public/", 1);
        let (s, _, _) = send(&app, None, "", "GET", &api.replacen(&org, "elsewhere", 1), None, None).await;
        assert_eq!(s, StatusCode::NOT_FOUND);
        // its maker no longer in the organisation: the link stops working
        let access = json!({"system_owners": ["someone-else@magnum.example"], "organisations": {org.clone(): {"name": "Share test", "models": ["*"]}}});
        state.access.pin(crate::access::AccessConfig::from_document(access).unwrap());
        let (s, b, _) = send(&app, None, "", "GET", &format!("{api}/tiles/t"), None, None).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{b}");
        clean_up(&state, &org).await;
    }

    #[tokio::test]
    async fn visitors_see_only_what_the_links_maker_may_see() {
        let Some((app, state, _, org)) = org_with_dashboard().await else { return };
        // an Org admin limited to EMEA makes a link
        let maker = format!("emea-{org}@share.example");
        let access = json!({"system_owners": [OWNER], "organisations": {org.clone(): {"name": "Share test", "models": ["*"],
            "members": {"users": {maker.clone(): {"role": "org_admin",
                "filters": [{"model": "sales", "dimension": "region", "values": ["EMEA"]}]}}}}}});
        state.access.pin(crate::access::AccessConfig::from_document(access).unwrap());
        let hash = crate::auth::password::hash("test-only-password").unwrap();
        state.store.set_password(&maker, None, &hash, None, false).await.unwrap();
        let token = crate::store::random_token(32);
        let key = state.config.secret_key.as_ref().unwrap();
        let link = crate::store::ShareLink {
            token_hash: crate::store::sha256_hex(&token),
            token_sealed: key.seal(&token, &format!("{org}/share")).unwrap(),
            dashboard_id: "shared".into(),
            cache_seconds: 300,
            created_by: maker.clone(),
            created_at: chrono::Utc::now(),
        };
        state.store.put_share_link(&org, &link).await.unwrap();

        let (s, r, _) =
            send(&app, None, "", "GET", &format!("/api/public/{org}/{token}/tiles/regions"), None, None).await;
        assert_eq!(s, StatusCode::OK, "{r}");
        let regions: Vec<&Value> = r["rows"].as_array().unwrap().iter().map(|row| &row[0]).collect();
        assert_eq!(regions, [&json!("EMEA")], "{r}");
        clean_up(&state, &org).await;
        state.store.pool.get().await.unwrap().execute("DELETE FROM users WHERE email = $1", &[&maker]).await.unwrap();
    }

    #[test]
    fn the_platform_can_forbid_public_links() {
        let config = |v: &str| {
            let v = v.to_string();
            crate::config::Config::from_lookup(move |k| (k == "MAGNUM_PUBLIC_LINKS").then(|| v.clone())).unwrap()
        };
        assert!(super::allowed(&config("1")).is_ok());
        let e = super::allowed(&config("0")).unwrap_err();
        assert!(format!("{e:?}").contains("turned off"), "{e:?}");
    }

    #[tokio::test]
    async fn visitors_arriving_together_share_one_query() {
        let flights = std::sync::Arc::new(crate::state::Flights::default());
        let runs = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let done = std::sync::Arc::new(tokio::sync::Mutex::new(None::<u32>));
        let visits = (0..20).map(|_| {
            let (flights, runs, done) = (flights.clone(), runs.clone(), done.clone());
            tokio::spawn(async move {
                let _turn = flights.lock("acme/link/tile").await;
                let mut cached = done.lock().await;
                if cached.is_none() {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await; // the query
                    *cached = Some(runs.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1);
                }
            })
        });
        for v in visits.collect::<Vec<_>>() {
            v.await.unwrap();
        }
        assert_eq!(runs.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}
