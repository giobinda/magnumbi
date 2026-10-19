//! Guest links: a dashboard for anyone with the link and its password, until it expires.
//!
//! Power users and up make them, for colleagues or people outside: guests use the dashboard as
//! its maker could, with a role up to the maker's (Viewer by default) and the link's own
//! filters on top of the maker's restrictions. A link lasts at most 14 days. Its password
//! unlocks it for a few hours in that browser (a sealed cookie scoped to the link); wrong
//! passwords lock it like sign-ins do.

use std::collections::BTreeSet;
use std::net::SocketAddr;

use axum::extract::{ConnectInfo, Path, Query as UrlQuery, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use axum_extra::extract::CookieJar;
use axum_extra::extract::cookie::{Cookie, SameSite};
use chrono::{Duration, Utc};
use serde::Deserialize;
use serde_json::{Value, json};

use super::Body;
use crate::access::{Role, RowRule};
use crate::auth::{Auth, Caller, LinkAccess, link_caller, password};
use crate::error::{ApiError, ApiResult};
use crate::schema::Period;
use crate::state::{AppState, Shared};
use crate::store::{GuestLink, random_token, sha256_hex};
use crate::tile_query::TileRequest;

/// The longest a guest link may last.
pub const MAX_HOURS: i64 = 14 * 24;
/// How long a right password keeps the link open in that browser (at most until it expires).
const UNLOCKED_HOURS: i64 = 12;
const COOKIE: &str = "magnum_guest";
/// What guests may do on a dashboard, whatever their role: never share, edit or explore.
const GUEST_PERMISSIONS: [&str; 5] = ["view", "filter", "drill", "view_sql", "download"];

fn gone() -> ApiError {
    ApiError::not_found("this link doesn't work: it expired, was removed, or never existed")
}

fn sealed_for(org: &str) -> String {
    format!("{org}/guest")
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewLink {
    expires_in_hours: i64,
    /// empty: one is made up (and shown once)
    #[serde(default)]
    password: String,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    filters: Vec<RowRule>,
}

/// Power users and up, in an organisation (the platform workspace's dashboards are templates).
fn require_maker(caller: &Caller) -> ApiResult<()> {
    caller.require("edit")?;
    if caller.org().is_empty() {
        return Err(ApiError::bad_request(
            "guest links are for an organisation's dashboards (the platform workspace's are templates)",
        ));
    }
    Ok(())
}

/// The roles a guest can be given: Viewer to Power user, and never above the maker.
fn guest_role(caller: &Caller, asked: Option<&str>) -> ApiResult<Role> {
    let role = match asked {
        None => Role::Viewer,
        Some(r) => Role::parse(r).ok_or_else(|| ApiError::bad_request(format!("unknown role {r:?}")))?,
    };
    if role > Role::PowerUser {
        return Err(ApiError::bad_request("guests can be Viewers, Users or Power users at most"));
    }
    if caller.role.is_none_or(|mine| role > mine) {
        return Err(ApiError::bad_request("guests can't have a higher role than yours"));
    }
    Ok(role)
}

/// The link's filters: on the organisation's models, on their dimensions, with values.
fn check_filters(caller: &Caller, filters: &[RowRule]) -> ApiResult<()> {
    for f in filters {
        // a blend has no restrictions of its own (each part runs with its model's): a limit on it
        // would narrow nothing
        if let Some(b) = caller.project.blends.get(&f.model) {
            let models: Vec<&str> =
                std::iter::once(b.base.as_str()).chain(b.joins.iter().map(|j| j.model.as_str())).collect();
            return Err(ApiError::bad_request(format!(
                "{} is a blend: limit one of its models instead ({})",
                f.model,
                models.join(", ")
            )));
        }
        let m = super::model_of(caller, &f.model)?;
        if m.dimension(&f.dimension).is_none() {
            return Err(ApiError::bad_request(format!("{} has no dimension {:?}", f.model, f.dimension)));
        }
        if f.values.is_empty() {
            return Err(ApiError::bad_request(format!("pick at least one value for {}.{}", f.model, f.dimension)));
        }
    }
    Ok(())
}

fn base_url(state: &AppState, headers: &HeaderMap) -> String {
    state.config.public_url.clone().unwrap_or_else(|| {
        let host = headers.get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or("localhost");
        format!("http://{host}")
    })
}

fn describe(state: &AppState, org: &str, l: &GuestLink, base: &str) -> ApiResult<Value> {
    let key = state.config.secret_key.as_ref().ok_or_else(|| ApiError::Internal(anyhow::anyhow!("no secret key")))?;
    let url = format!("{base}/g/{org}/{}", key.open(&l.token_sealed, &sealed_for(org))?);
    Ok(json!({
        "id": l.id(), "dashboard": l.dashboard_id, "url": url, "qr_svg": crate::auth::totp::qr_svg(&url),
        "role": l.role, "filters": l.filters, "expires_at": l.expires_at.to_rfc3339(),
        "created_by": l.created_by, "created_at": l.created_at.to_rfc3339(),
    }))
}

/// Make a guest link to a dashboard.
pub async fn create(
    State(state): State<Shared>,
    Auth(caller): Auth,
    Path(id): Path<String>,
    headers: HeaderMap,
    Body(n): Body<NewLink>,
) -> ApiResult<Json<Value>> {
    require_maker(&caller)?;
    if !(1..=MAX_HOURS).contains(&n.expires_in_hours) {
        return Err(ApiError::bad_request("a guest link lasts from 1 hour to 14 days (expires_in_hours: 1 to 336)"));
    }
    let role = guest_role(&caller, n.role.as_deref())?;
    check_filters(&caller, &n.filters)?;
    super::dashboards::load(&state, &caller, &id).await?; // it exists, and they can see it
    let made_up = n.password.is_empty();
    let pw = if made_up { password::generate() } else { n.password };
    password::check_policy(&pw, caller.email()).map_err(ApiError::bad_request)?;
    let key = state
        .config
        .secret_key
        .as_ref()
        .ok_or_else(|| ApiError::bad_request("guest links need MAGNUM_SECRET_KEY (to keep their addresses)"))?;
    let org = caller.org();
    let token = random_token(32);
    let to_hash = pw.clone();
    let link = GuestLink {
        token_hash: sha256_hex(&token),
        token_sealed: key.seal(&token, &sealed_for(org))?,
        dashboard_id: id.clone(),
        password_hash: super::auth::hashing(&state, move || password::hash(&to_hash)).await??,
        role: role.as_str().to_string(),
        filters: n.filters,
        expires_at: Utc::now() + Duration::hours(n.expires_in_hours),
        created_by: caller.email().to_string(),
        created_at: Utc::now(),
    };
    state.orgs.store(org).await?.put_guest_link(org, &link).await?;
    state.usage.record(crate::usage::Event {
        dashboard_id: Some(id),
        detail: Some(
            json!({"link": link.id(), "role": link.role, "hours": n.expires_in_hours, "filters": link.filters}),
        ),
        ..caller.event("guest_link_created")
    });
    let mut out = describe(&state, org, &link, &base_url(&state, &headers))?;
    if made_up {
        out["password"] = json!(pw); // shown this once
    }
    Ok(Json(out))
}

/// A dashboard's guest links that haven't expired.
pub async fn list(
    State(state): State<Shared>,
    Auth(caller): Auth,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    require_maker(&caller)?;
    let org = caller.org();
    let base = base_url(&state, &headers);
    let links = state.orgs.store(org).await?.guest_links(org, Some(&id)).await?;
    let links = links.iter().map(|l| describe(&state, org, l, &base)).collect::<ApiResult<Vec<_>>>()?;
    Ok(Json(json!({"links": links, "max_hours": MAX_HOURS})))
}

/// Remove a guest link: its maker, or an Org admin. It stops working at once, even for guests
/// who already entered the password.
pub async fn revoke(State(state): State<Shared>, Auth(caller): Auth, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    require_maker(&caller)?;
    let org = caller.org();
    let store = state.orgs.store(org).await?;
    let link = store
        .guest_links(org, None)
        .await?
        .into_iter()
        .find(|l| l.id() == id)
        .ok_or_else(|| ApiError::not_found(format!("no guest link {id:?}")))?;
    if link.created_by != caller.email() && !caller.can("manage_members") {
        return Err(ApiError::forbidden("only its maker or an Org admin can remove a guest link"));
    }
    store.delete_guest_link(org, &link.token_hash).await?;
    state.usage.record(crate::usage::Event {
        dashboard_id: Some(link.dashboard_id.clone()),
        detail: Some(json!({"link": link.id()})),
        ..caller.event("guest_link_revoked")
    });
    Ok(Json(json!({"ok": true})))
}

/// The guest link at this address (it exists, hasn't expired, its organisation is known).
async fn find(state: &AppState, org: &str, token: &str) -> ApiResult<GuestLink> {
    let (access, _) = state.access.get(state, &state.project.current()).await;
    if org.is_empty() || !access.platform.organisations.contains_key(org) {
        return Err(gone());
    }
    state.orgs.store(org).await?.guest_link(org, &sha256_hex(token)).await?.ok_or_else(gone)
}

/// A guest who entered the link's password (its cookie), acting as the link's maker allows.
async fn guest(state: &AppState, jar: &CookieJar, org: &str, token: &str) -> ApiResult<(GuestLink, Caller)> {
    let link = find(state, org, token).await?;
    let key = state.config.secret_key.as_ref().ok_or_else(gone)?;
    let unlocked = jar
        .get(COOKIE)
        .and_then(|c| key.open(c.value(), &sealed_for(org)).ok())
        .and_then(|v| v.split_once('|').map(|(h, exp)| (h.to_string(), exp.parse::<i64>().unwrap_or(0))))
        .is_some_and(|(hash, exp)| hash == link.token_hash && exp > Utc::now().timestamp());
    if !unlocked {
        let body = json!({"detail": "this link needs its password", "password": true});
        return Err(ApiError::Body(StatusCode::UNAUTHORIZED, body));
    }
    let access = LinkAccess {
        min: Role::PowerUser,
        cap: Role::parse(&link.role).unwrap_or(Role::Viewer),
        filters: &link.filters,
        name: format!("guest link {}", link.id()),
    };
    let caller = link_caller(state, org, &link.created_by, access).await?.ok_or_else(gone)?;
    Ok((link, caller))
}

/// Guests' answers: never kept by browsers or proxies, never indexed.
fn private(body: Value) -> Response {
    let mut r = Json(body).into_response();
    r.headers_mut().insert("x-robots-tag", HeaderValue::from_static("noindex"));
    r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    r
}

#[derive(Deserialize)]
pub struct Unlock {
    password: String,
}

/// The link's password: the link opens in this browser for a few hours.
pub async fn unlock(
    State(state): State<Shared>,
    Path((org, token)): Path<(String, String)>,
    jar: CookieJar,
    headers: HeaderMap,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    Body(body): Body<Unlock>,
) -> ApiResult<Response> {
    let link = find(&state, &org, &token).await?;
    let ip = super::auth::client_ip(&state, &headers, peer);
    let (link_key, address_key) = (format!("guest:{org}:{}", link.token_hash), format!("ip:{ip}"));
    for limiter_key in [(&state.limiter, &link_key), (&state.ip_limiter, &address_key)] {
        if let Some(wait) = limiter_key.0.locked(limiter_key.1) {
            let detail = format!("too many wrong passwords; try again in {} minutes", wait.div_ceil(60));
            return Err(ApiError::Status(StatusCode::TOO_MANY_REQUESTS, detail));
        }
    }
    let (pw, stored) = (body.password, link.password_hash.clone());
    let event = |kind: &'static str, ok: bool| crate::usage::Event {
        org: org.clone(),
        dashboard_id: Some(link.dashboard_id.clone()),
        via: Some(format!("guest link {}", link.id())),
        ok,
        detail: Some(json!({"ip": ip})),
        ..crate::usage::Event::new(kind)
    };
    if pw.len() > 4 * password::MAX_LENGTH
        || !super::auth::hashing(&state, move || password::verify(&pw, Some(&stored))).await?
    {
        state.limiter.failed(&link_key);
        state.ip_limiter.failed(&address_key);
        state.usage.record(event("guest_password_failed", false));
        return Err(ApiError::Status(StatusCode::UNAUTHORIZED, "wrong password".into()));
    }
    state.limiter.succeeded(&link_key);
    state.usage.record(event("guest_unlocked", true));
    let key = state.config.secret_key.as_ref().ok_or_else(gone)?;
    let until = link.expires_at.min(Utc::now() + Duration::hours(UNLOCKED_HOURS));
    let value = key.seal(&format!("{}|{}", link.token_hash, until.timestamp()), &sealed_for(&org))?;
    let cookie = Cookie::build((COOKIE, value))
        .path(format!("/api/guest/{org}/{token}"))
        .http_only(true)
        .same_site(SameSite::Strict)
        .secure(state.config.is_https())
        .max_age(cookie::time::Duration::seconds((until - Utc::now()).num_seconds().max(0)))
        .build();
    Ok((jar.add(cookie), private(json!({"ok": true, "until": until.to_rfc3339()}))).into_response())
}

/// The dashboard, the models it uses (as the guest may see them), and what the guest is
/// limited to.
pub async fn dashboard(
    State(state): State<Shared>,
    Path((org, token)): Path<(String, String)>,
    jar: CookieJar,
) -> ApiResult<Response> {
    let (link, caller) = guest(&state, &jar, &org, &token).await?;
    let (d, _) = super::dashboards::load(&state, &caller, &link.dashboard_id).await?;
    // the dashboard's models, and a blend's own models (the link's limits are on those)
    let mut names: BTreeSet<String> = d
        .tiles
        .iter()
        .filter_map(|t| t.query.as_ref().map(|q| q.model.clone()))
        .chain(d.filters.iter().map(|f| f.model.clone()))
        .collect();
    for n in names.clone() {
        if let Some(b) = caller.project.blends.get(&n) {
            names.extend(std::iter::once(b.base.clone()).chain(b.joins.iter().map(|j| j.model.clone())));
        }
    }
    let models = super::models::infos_for(&state, &caller, &names).await?;
    let theme_id = d.theme.clone().unwrap_or_else(|| caller.project.theme_defaults.0.clone());
    let theme = caller.project.themes.iter().find(|t| t.id == theme_id);
    let role = caller.role.unwrap_or(Role::Viewer);
    let permissions: Vec<&str> = GUEST_PERMISSIONS.into_iter().filter(|p| role.can(p)).collect();
    Ok(private(json!({
        "id": link.dashboard_id, "dashboard": d, "models": models, "theme": theme,
        "role": role.as_str(), "permissions": permissions, "filters": link.filters,
        "expires_at": link.expires_at.to_rfc3339(), "shared_by": link.created_by,
    })))
}

pub async fn tile(
    State(state): State<Shared>,
    Path((org, token, tile)): Path<(String, String, String)>,
    jar: CookieJar,
    Body(req): Body<TileRequest>,
) -> ApiResult<Response> {
    let (link, caller) = guest(&state, &jar, &org, &token).await?;
    Ok(private(super::dashboards::tile_for(&state, &caller, &link.dashboard_id, &tile, &req).await?))
}

/// Downloads, for guests whose role has `download` (users and up).
pub async fn tile_export(
    State(state): State<Shared>,
    Path((org, token, tile)): Path<(String, String, String)>,
    jar: CookieJar,
    Body(req): Body<TileRequest>,
) -> ApiResult<Response> {
    let (link, caller) = guest(&state, &jar, &org, &token).await?;
    let mut r = super::exports::tile_csv(&state, &caller, &link.dashboard_id, &tile, &req).await?;
    r.headers_mut().insert("x-robots-tag", HeaderValue::from_static("noindex"));
    Ok(r)
}

pub async fn export(
    State(state): State<Shared>,
    Path((org, token)): Path<(String, String)>,
    jar: CookieJar,
    Body(req): Body<TileRequest>,
) -> ApiResult<Response> {
    let (link, caller) = guest(&state, &jar, &org, &token).await?;
    let mut r = super::exports::dashboard_zip(&state, &caller, &link.dashboard_id, &req).await?;
    r.headers_mut().insert("x-robots-tag", HeaderValue::from_static("noindex"));
    Ok(r)
}

pub async fn filter_values(
    State(state): State<Shared>,
    Path((org, token)): Path<(String, String)>,
    jar: CookieJar,
    UrlQuery(p): UrlQuery<super::dashboards::KeyParam>,
) -> ApiResult<Response> {
    let (link, caller) = guest(&state, &jar, &org, &token).await?;
    Ok(private(super::dashboards::filter_values_for(&state, &caller, &link.dashboard_id, &p.key).await?))
}

pub async fn period(
    State(state): State<Shared>,
    Path((org, token)): Path<(String, String)>,
    jar: CookieJar,
    UrlQuery(p): UrlQuery<super::models::ModelParam>,
    Body(period): Body<Period>,
) -> ApiResult<Response> {
    let (_, caller) = guest(&state, &jar, &org, &token).await?;
    Ok(private(super::models::period_for(&caller, &p.model, &period)?))
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, StatusCode};
    use serde_json::{Value, json};

    use crate::api::exports::tests::{raw, unzip};
    use crate::api::share::tests::{clean_up, org_with, send};

    const DASHBOARD: &str = "title: Guest sales\nfilters:\n  - {model: sales, field: region}\ntiles:\n  - id: regions\n    type: bar\n    drill: location\n    query: {model: sales, dimensions: [region], measures: [transactions]}\n    layout: {x: 0, y: 0, w: 6, h: 4}\n";
    const PASSWORD: &str = "Kona-Coffee-1984";

    /// The guest cookie a response set, as a request header ("magnum_guest=...").
    fn cookie_of(headers: &HeaderMap) -> String {
        let set = headers["set-cookie"].to_str().unwrap();
        set.split(';').next().unwrap().to_string()
    }

    fn guest_api(url: &Value) -> String {
        let url = url.as_str().unwrap();
        url[url.find("/g/").unwrap()..].replacen("/g/", "/api/guest/", 1)
    }

    async fn new_link(app: &axum::Router, cookie: &str, org: &str, body: Value) -> (StatusCode, Value) {
        let (s, b, _) =
            send(app, Some(cookie), org, "POST", "/api/dashboards/shared/guest-links", Some(body), None).await;
        (s, b)
    }

    /// A guest's request with the guest cookie (and nothing else).
    async fn as_guest(
        app: &axum::Router,
        cookie: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut req = axum::http::Request::builder().method(method).uri(path).header("cookie", cookie);
        let body = match body {
            Some(b) => {
                req = req.header("content-type", "application/json");
                axum::body::Body::from(b.to_string())
            }
            None => axum::body::Body::empty(),
        };
        let resp = tower::ServiceExt::oneshot(app.clone(), req.body(body).unwrap()).await.unwrap();
        let status = resp.status();
        let bytes = http_body_util::BodyExt::collect(resp.into_body()).await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    #[tokio::test]
    async fn power_users_share_with_guests_who_know_the_password() {
        let Some((app, state, cookie, org)) = org_with(DASHBOARD).await else { return };
        let emea_apac = json!([{"model": "sales", "dimension": "region", "values": ["EMEA", "APAC"]}]);

        // power users and up; up to 14 days; a real password; a role up to the maker's
        let (s, _, _) = send(
            &app,
            Some(&cookie),
            &org,
            "POST",
            "/api/dashboards/shared/guest-links",
            Some(json!({"expires_in_hours": 24, "password": PASSWORD})),
            Some("user"),
        )
        .await;
        assert_eq!(s, StatusCode::FORBIDDEN);
        for bad in [
            json!({"expires_in_hours": 15 * 24, "password": PASSWORD}),
            json!({"expires_in_hours": 0, "password": PASSWORD}),
            json!({"expires_in_hours": 24, "password": "short"}),
            json!({"expires_in_hours": 24, "password": PASSWORD, "role": "org_admin"}),
            json!({"expires_in_hours": 24, "password": PASSWORD, "filters": [{"model": "sales", "dimension": "nope", "values": ["x"]}]}),
            // a blend has no restrictions of its own: a limit on it would do nothing
            json!({"expires_in_hours": 24, "password": PASSWORD, "filters": [{"model": "taxi_weather", "dimension": "taxi.pickup_borough", "values": ["Manhattan"]}]}),
        ] {
            let (s, b) = new_link(&app, &cookie, &org, bad.clone()).await;
            assert_eq!(s, StatusCode::BAD_REQUEST, "{bad} -> {b}");
        }
        let (_, b) = new_link(&app, &cookie, &org, json!({"expires_in_hours": 15 * 24, "password": PASSWORD})).await;
        assert!(b["detail"].as_str().unwrap().contains("14 days"), "{b}");

        let (s, link) =
            new_link(&app, &cookie, &org, json!({"expires_in_hours": 24, "password": PASSWORD, "filters": emea_apac}))
                .await;
        assert_eq!(s, StatusCode::OK, "{link}");
        assert_eq!(link["role"], json!("viewer")); // the default
        assert!(link.get("password").is_none(), "a password chosen by its maker isn't sent back: {link}");
        let api = guest_api(&link["url"]);

        // the link alone isn't enough
        let (s, b, _) = send(&app, None, "", "GET", &api, None, None).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        assert_eq!(b["password"], json!(true));
        let (s, _, _) =
            send(&app, None, "", "POST", &format!("{api}/unlock"), Some(json!({"password": "Wrong-Password-1"})), None)
                .await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        let (s, _, headers) =
            send(&app, None, "", "POST", &format!("{api}/unlock"), Some(json!({"password": PASSWORD})), None).await;
        assert_eq!(s, StatusCode::OK);
        let set = headers["set-cookie"].to_str().unwrap();
        assert!(set.contains(&format!("Path={api}")) && set.contains("HttpOnly"), "{set}");
        let guest = cookie_of(&headers);

        // the dashboard, the models it needs, and what the guest is limited to
        let (s, d) = as_guest(&app, &guest, "GET", &api, None).await;
        assert_eq!(s, StatusCode::OK, "{d}");
        assert_eq!(d["dashboard"]["title"], json!("Guest sales"));
        assert_eq!(d["models"][0]["name"], json!("sales"));
        assert_eq!(d["role"], json!("viewer"));
        assert_eq!(d["permissions"], json!(["view", "filter"]));
        let (s, _, _) = raw(&app, &guest, "", &format!("{api}/export"), json!({}), None).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "a viewer only looks");
        assert_eq!(d["filters"][0]["values"], json!(["EMEA", "APAC"]));

        // the link's filters narrow every query; guests filter further, but a viewer can't drill
        let tile = format!("{api}/tiles/regions/query");
        let (s, r) = as_guest(&app, &guest, "POST", &tile, Some(json!({}))).await;
        assert_eq!(s, StatusCode::OK, "{r}");
        let mut regions: Vec<String> =
            r["rows"].as_array().unwrap().iter().map(|x| x[0].as_str().unwrap().to_string()).collect();
        regions.sort();
        assert_eq!(regions, ["APAC", "EMEA"]);
        assert!(r["sql"].is_null());
        let (_, r) = as_guest(&app, &guest, "POST", &tile, Some(json!({"filters": {"sales.region": ["EMEA"]}}))).await;
        assert_eq!(r["rows"].as_array().unwrap().len(), 1, "{r}");
        let (s, _) =
            as_guest(&app, &guest, "POST", &tile, Some(json!({"drill": [{"field": "region", "value": "EMEA"}]}))).await;
        assert_eq!(s, StatusCode::FORBIDDEN);
        let (s, v) = as_guest(&app, &guest, "GET", &format!("{api}/filters/values?key=sales.region"), None).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(v["rows"].as_array().unwrap().len(), 2, "{v}");
        let period = json!({"field": "sold_at", "current": {"last_n_days": 30}});
        let (s, p) = as_guest(&app, &guest, "POST", &format!("{api}/period?model=sales"), Some(period)).await;
        assert_eq!(s, StatusCode::OK, "{p}");

        // a guest cookie opens its own link only
        let (_, other) = new_link(&app, &cookie, &org, json!({"expires_in_hours": 1, "password": PASSWORD})).await;
        let (s, _) = as_guest(&app, &guest, "GET", &guest_api(&other["url"]), None).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);

        // the maker's colleagues see the links; removing one ends it, cookie or not
        let (_, list, _) =
            send(&app, Some(&cookie), &org, "GET", "/api/dashboards/shared/guest-links", None, None).await;
        assert_eq!(list["links"].as_array().unwrap().len(), 2, "{list}");
        let id = link["id"].as_str().unwrap();
        let (s, _, _) = send(&app, Some(&cookie), &org, "DELETE", &format!("/api/guest-links/{id}"), None, None).await;
        assert_eq!(s, StatusCode::OK);
        let (s, _) = as_guest(&app, &guest, "GET", &api, None).await;
        assert_eq!(s, StatusCode::NOT_FOUND);
        clean_up(&state, &org).await;
    }

    #[tokio::test]
    async fn guests_get_the_role_they_were_given_and_a_generated_password() {
        let Some((app, state, cookie, org)) = org_with(DASHBOARD).await else { return };
        let (s, link) =
            new_link(&app, &cookie, &org, json!({"expires_in_hours": 1, "password": "", "role": "user"})).await;
        assert_eq!(s, StatusCode::OK, "{link}");
        let password = link["password"].as_str().unwrap().to_string(); // made up: shown once
        let api = guest_api(&link["url"]);
        let (s, _, headers) =
            send(&app, None, "", "POST", &format!("{api}/unlock"), Some(json!({"password": password})), None).await;
        assert_eq!(s, StatusCode::OK);
        let guest = cookie_of(&headers);
        let (_, d) = as_guest(&app, &guest, "GET", &api, None).await;
        assert_eq!(d["permissions"], json!(["view", "filter", "drill", "download"]));
        // a user may download: the link's limits and the guest's drill still apply
        let drill = json!({"drill": [{"field": "region", "value": "EMEA"}]});
        let (s, _, csv) = raw(&app, &guest, "", &format!("{api}/tiles/regions/export"), drill, None).await;
        assert_eq!(s, StatusCode::OK);
        assert!(String::from_utf8(csv).unwrap().starts_with("\u{feff}Store,"), "drilled down to stores");
        let (s, _, zip) = raw(&app, &guest, "", &format!("{api}/export"), json!({}), None).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(unzip(zip).len(), 1);
        let (s, r) = as_guest(
            &app,
            &guest,
            "POST",
            &format!("{api}/tiles/regions/query"),
            Some(json!({"drill": [{"field": "region", "value": "EMEA"}]})),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{r}");
        clean_up(&state, &org).await;
    }

    #[tokio::test]
    async fn guest_links_expire_and_lock_after_wrong_passwords() {
        let Some((app, state, cookie, org)) = org_with(DASHBOARD).await else { return };
        let (_, link) = new_link(&app, &cookie, &org, json!({"expires_in_hours": 1, "password": PASSWORD})).await;
        let api = guest_api(&link["url"]);
        for _ in 0..5 {
            let (s, _, _) = send(
                &app,
                None,
                "",
                "POST",
                &format!("{api}/unlock"),
                Some(json!({"password": "Not-It-At-All"})),
                None,
            )
            .await;
            assert_eq!(s, StatusCode::UNAUTHORIZED);
        }
        let (s, _, _) =
            send(&app, None, "", "POST", &format!("{api}/unlock"), Some(json!({"password": PASSWORD})), None).await;
        assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);

        // past its time, a link is gone
        let c = state.store.pool.get().await.unwrap();
        c.execute("UPDATE guest_links SET expires_at = now() - INTERVAL '1 minute' WHERE org = $1", &[&org])
            .await
            .unwrap();
        let (s, _, _) = send(&app, None, "", "GET", &api, None, None).await;
        assert_eq!(s, StatusCode::NOT_FOUND);
        let (_, list, _) =
            send(&app, Some(&cookie), &org, "GET", "/api/dashboards/shared/guest-links", None, None).await;
        assert_eq!(list["links"], json!([]));
        clean_up(&state, &org).await;
    }
}
