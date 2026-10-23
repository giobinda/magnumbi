//! Members (Org admins), passwords for members, and the platform (system owners).

use std::collections::BTreeMap;

use axum::Json;
use axum::extract::{Path, Query as UrlQuery, State};
use axum::http::{HeaderMap, StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Body, available_names};
use crate::access::{AccessConfig, Members, OidcSettings, Platform, Role, check_org_sign_in, is_system_owner};
use crate::auth::policy::{SignInPolicy, effective};
use crate::auth::{Auth, Caller, offered, password};
use crate::error::{ApiError, ApiResult};
use crate::orgdb::OrgDbError;
use crate::project::{DASHBOARD_ID, parse_dashboard};
use crate::security::public_base;
use crate::state::{AppState, Shared, store_access};
use crate::usage::Event;

fn current_org(caller: &Caller) -> ApiResult<String> {
    caller.scope.org.clone().ok_or_else(|| ApiError::bad_request("choose an organisation first"))
}

fn role_list(up_to: Role) -> Vec<Value> {
    crate::access::ROLES
        .iter()
        .filter(|r| **r <= up_to)
        .map(|r| json!({"id": r.as_str(), "label": r.label(), "permissions": r.permissions()}))
        .collect()
}

async fn members_response(
    state: &Shared,
    caller: &Caller,
    cfg: &AccessConfig,
    errors: &BTreeMap<String, String>,
) -> ApiResult<Value> {
    let oid = current_org(caller)?;
    let org = &cfg.platform.organisations[&oid];
    let members = cfg.members_of(&oid);
    let accounts: BTreeMap<String, Value> =
        state.store.accounts().await?.into_iter().filter(|(email, _)| members.users.contains_key(email)).collect();
    // never signed in: invited
    let last_login: BTreeMap<&String, &Value> =
        accounts.iter().filter(|(_, a)| !a["last_login"].is_null()).map(|(e, a)| (e, &a["last_login"])).collect();
    Ok(json!({
        "org": {"id": oid, "name": org.name, "domains": org.domains, "filters": org.filters, "models": available_names(caller)},
        "version": members_version(&members),
        "members": without_secrets(&members),
        "last_login": last_login,
        "accounts": accounts,
        "roles": role_list(Role::OrgAdmin),
        "password_login": state.config.password_login,
        "errors": errors.iter().filter(|(k, _)| k.starts_with(&format!("organisations/{oid}/"))).collect::<BTreeMap<_, _>>(),
    }))
}

/// The organisation's members, roles and personal filters, plus what the system owner fixed for
/// the organisation (read-only here).
pub async fn get_members(State(state): State<Shared>, Auth(caller): Auth) -> ApiResult<Json<Value>> {
    caller.require("manage_members")?;
    Ok(Json(members_response(&state, &caller, &caller.access, &caller.access_errors).await?))
}

/// The members settings' version: send it back as If-Match to save only over what you read.
fn members_version(m: &Members) -> String {
    crate::store::sha256_hex(&serde_json::to_string(m).unwrap_or_default())[..16].to_string()
}

pub async fn put_members(
    State(state): State<Shared>,
    Auth(caller): Auth,
    UrlQuery(p): UrlQuery<super::dashboards::WriteParams>,
    headers: HeaderMap,
    Body(members): Body<Members>,
) -> ApiResult<Json<Value>> {
    caller.require("manage_members")?;
    let oid = current_org(&caller)?;
    let cfg = &caller.access;
    let current = members_version(&cfg.members_of(&oid));
    if let Some(expected) = headers.get(axum::http::header::IF_MATCH).and_then(|v| v.to_str().ok())
        && expected.trim().trim_matches('"') != current
    {
        return Err(ApiError::Body(
            StatusCode::CONFLICT,
            json!({"detail": "the members changed since you read them: read them again, then save", "version": current}),
        ));
    }
    let available = available_names(&caller);
    let rules = members
        .default_filters
        .iter()
        .map(|r| ("default", r))
        .chain(members.users.iter().flat_map(|(who, ua)| ua.filters.iter().map(move |r| (who.as_str(), r))));
    for (who, r) in rules {
        if !available.contains(&r.model) {
            return Err(ApiError::bad_request(format!("{who}: unknown model {:?}", r.model)));
        }
    }
    for who in members.users.keys() {
        if is_system_owner(cfg, who) {
            return Err(ApiError::bad_request(format!("{who} is a system owner and can't be an organisation member")));
        }
        if cfg.platform.organisations.keys().any(|o| *o != oid && cfg.members_of(o).users.contains_key(who)) {
            return Err(ApiError::Status(
                StatusCode::CONFLICT,
                format!("{who} already belongs to another organisation"),
            ));
        }
    }
    if caller.scope.role == Some(Role::OrgAdmin)
        && members.users.get(caller.email()).is_none_or(|ua| ua.role != Role::OrgAdmin)
    {
        return Err(ApiError::bad_request("you would remove your own Org admin role; ask another Org admin"));
    }
    // sign-in settings have their own page (and the provider's secret is never sent out): kept
    let mut members = members;
    let existing = cfg.members_of(&oid);
    members.sign_in = existing.sign_in;
    members.oidc = existing.oidc;
    let mut next = (**cfg).clone();
    next.members.insert(oid.clone(), members);
    if p.dry_run {
        let mut out = members_response(&state, &caller, &next, &caller.access_errors).await?;
        out["dry_run"] = json!(true);
        return Ok(Json(out));
    }
    store_access(&state, &next, Some(&oid), Some(caller.email())).await?;
    state.usage.record(caller.event("members_changed"));
    let (fresh, errors) = state.access.get(&state, &caller.project).await;
    Ok(Json(members_response(&state, &caller, &fresh, &errors).await?))
}

/// Members settings as shown: the provider's sealed secret left out.
fn without_secrets(m: &Members) -> Members {
    let mut m = m.clone();
    if let Some(o) = &mut m.oidc {
        o.client_secret_sealed = None;
    }
    m
}

// --------------------------------------------------------------------------- sign-in settings

async fn sign_in_response(
    state: &AppState,
    caller: &Caller,
    cfg: &AccessConfig,
    headers: &HeaderMap,
) -> ApiResult<Value> {
    let oid = current_org(caller)?;
    let offered = offered(state);
    // what the platform allows this organisation (its own choice can only narrow it)
    let above = effective(
        &[Some(&cfg.platform.sign_in), cfg.platform.organisations.get(&oid).and_then(|o| o.sign_in.as_ref())],
        &offered,
    );
    let m = cfg.members_of(&oid);
    let oidc = m.oidc.as_ref().map(|o| {
        json!({"label": o.label, "issuer": o.issuer, "client_id": o.client_id, "groups_claim": o.groups_claim,
               "group_roles": o.group_roles, "has_secret": o.client_secret_sealed.is_some()})
    });
    let methods: Vec<&str> = above.methods.iter().map(|m| m.as_str()).collect();
    Ok(json!({
        "platform": {"methods": methods, "require_two_factor": above.require_two_factor},
        "sign_in": m.sign_in.unwrap_or_default(),
        "oidc": oidc,
        "domains": cfg.platform.organisations.get(&oid).map(|o| o.domains.clone()).unwrap_or_default(),
        "callback_url": format!("{}/api/auth/oidc/callback", public_base(&state.config, headers)),
        "secret_key": state.config.secret_key.is_some(),
    }))
}

/// Org admins: how the organisation's people sign in, within what the platform allows.
pub async fn get_sign_in(
    State(state): State<Shared>,
    Auth(caller): Auth,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    caller.require("manage_members")?;
    Ok(Json(sign_in_response(&state, &caller, &caller.access, &headers).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OidcInput {
    label: String,
    issuer: String,
    client_id: String,
    /// omitted or empty: keep the stored one
    #[serde(default)]
    client_secret: Option<String>,
    #[serde(default)]
    groups_claim: Option<String>,
    #[serde(default)]
    group_roles: BTreeMap<String, Role>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignInBody {
    #[serde(default)]
    sign_in: Option<SignInPolicy>,
    #[serde(default)]
    oidc: Option<OidcInput>,
}

pub async fn put_sign_in(
    State(state): State<Shared>,
    Auth(caller): Auth,
    UrlQuery(p): UrlQuery<super::dashboards::WriteParams>,
    headers: HeaderMap,
    Body(body): Body<SignInBody>,
) -> ApiResult<Json<Value>> {
    caller.require("manage_members")?;
    let oid = current_org(&caller)?;
    let cfg = &caller.access;
    let existing = cfg.members_of(&oid);
    let oidc = match body.oidc {
        None => None,
        Some(o) => {
            let issuer = o.issuer.trim().trim_end_matches('/').to_string();
            // the client secret never goes to another provider (or as another client) on its own
            let given = o.client_secret.as_deref().is_some_and(|s| !s.trim().is_empty());
            if !given
                && existing.oidc.as_ref().is_some_and(|e| {
                    e.client_secret_sealed.is_some() && (e.issuer != issuer || e.client_id != o.client_id.trim())
                })
            {
                return Err(ApiError::bad_request("the provider changed: give its client secret again"));
            }
            // reachable, and a real provider: its discovery document must check out
            state
                .oidc
                .provider(&issuer, state.connectors.net_guard())
                .await
                .map_err(|e| ApiError::bad_request(format!("single sign-on: {}", e.0)))?;
            let sealed = match o.client_secret.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                Some(secret) => {
                    let key = state.config.secret_key.as_ref().ok_or_else(|| {
                        ApiError::bad_request("a client secret needs MAGNUM_SECRET_KEY on the server")
                    })?;
                    if secret.len() > 1024 {
                        return Err(ApiError::bad_request("client secret too long"));
                    }
                    Some(key.seal(secret, &format!("{oid}/oidc"))?)
                }
                None => existing.oidc.as_ref().and_then(|e| e.client_secret_sealed.clone()),
            };
            Some(OidcSettings {
                label: o.label.trim().to_string(),
                issuer,
                client_id: o.client_id.trim().to_string(),
                client_secret_sealed: sealed,
                groups_claim: o.groups_claim.filter(|g| !g.trim().is_empty()).unwrap_or_else(|| "groups".into()),
                group_roles: o.group_roles,
            })
        }
    };
    let candidate = Members { sign_in: body.sign_in, oidc, ..existing };
    check_org_sign_in(cfg, &oid, &candidate, &offered(&state), state.config.secret_key.is_some())
        .map_err(ApiError::bad_request)?;
    let mut next = (**cfg).clone();
    next.members.insert(oid.clone(), candidate);
    if p.dry_run {
        let mut out = sign_in_response(&state, &caller, &next, &headers).await?;
        out["dry_run"] = json!(true);
        return Ok(Json(out));
    }
    store_access(&state, &next, Some(&oid), Some(caller.email())).await?;
    state.usage.record(caller.event("sign_in_changed"));
    Ok(Json(sign_in_response(&state, &caller, &next, &headers).await?))
}

// --------------------------------------------------------------------------- the NYC demo

fn thirteen() -> u32 {
    13
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DemoBody {
    /// an organisation, or "" for the platform workspace
    #[serde(default)]
    org: String,
    #[serde(default = "thirteen")]
    months: u32,
}

/// System owners: how the demo setup is going (or went) on this server.
pub async fn demo_status(State(state): State<Shared>, Auth(caller): Auth) -> ApiResult<Json<Value>> {
    caller.require("manage_orgs")?;
    Ok(Json(json!(*state.demo.lock().expect("demo lock"))))
}

/// System owners: set up the NYC demo for an organisation (or the platform workspace), in the
/// background (crate::demo). Follow it with GET.
pub async fn start_demo(
    State(state): State<Shared>,
    Auth(caller): Auth,
    Body(b): Body<DemoBody>,
) -> ApiResult<Json<Value>> {
    caller.require("manage_orgs")?;
    if !(1..=120).contains(&b.months) {
        return Err(ApiError::bad_request("months: between 1 and 120"));
    }
    if !b.org.is_empty() && !caller.access.platform.organisations.contains_key(&b.org) {
        return Err(ApiError::bad_request(format!("no organisation {:?}", b.org)));
    }
    // its database must be there first (dashboards and connections go in it)
    state.orgs.store(&b.org).await?;
    {
        let mut st = state.demo.lock().expect("demo lock");
        if st.running {
            return Err(ApiError::Status(StatusCode::CONFLICT, "a demo setup is already running".into()));
        }
        *st = crate::demo::Status { running: true, org: b.org.clone(), step: "Starting".into(), ..Default::default() };
    }
    state
        .usage
        .record(Event { detail: Some(json!({"org": b.org, "months": b.months})), ..caller.event("demo_started") });
    let (task_state, by) = (state.clone(), caller.email().to_string());
    tokio::spawn(async move {
        let progress = task_state.demo.clone();
        let today = chrono::Utc::now().date_naive();
        let _ =
            crate::demo::run(&task_state, &b.org, b.months, &by, today, &crate::demo::Sources::default(), &progress)
                .await;
    });
    Ok(Json(json!(*state.demo.lock().expect("demo lock"))))
}

#[derive(Deserialize)]
pub struct ResetBody {
    email: String,
}

/// A one-time password for a member, shown once to the admin, to be changed at first sign-in.
/// The account is then bound to this organisation (see auth::restrict_password_accounts).
pub async fn reset_password(
    State(state): State<Shared>,
    Auth(caller): Auth,
    Body(body): Body<ResetBody>,
) -> ApiResult<Json<Value>> {
    caller.require("manage_members")?;
    if !state.config.password_login {
        return Err(ApiError::bad_request("password sign-in is switched off here"));
    }
    let oid = current_org(&caller)?;
    let email = body.email.trim().to_lowercase();
    if email == caller.email() {
        return Err(ApiError::bad_request("change your own password from your account menu"));
    }
    if is_system_owner(&caller.access, &email) {
        return Err(ApiError::bad_request("system owners set their own passwords"));
    }
    if !caller.access.members_of(&oid).users.contains_key(&email) {
        return Err(ApiError::bad_request(format!("add {email} as a member first (and save)")));
    }
    let one_time = password::generate();
    let p = one_time.clone();
    let hash = tokio::task::spawn_blocking(move || password::hash(&p)).await.map_err(anyhow::Error::from)??;
    let owner_org = (caller.scope.role != Some(Role::SystemOwner)).then_some(oid.as_str());
    state.store.set_password(&email, None, &hash, owner_org, true).await?;
    tracing::info!(org = oid, "one-time password issued");
    state.usage.record(Event { detail: Some(json!({"for": email})), ..caller.event("password_issued") });
    Ok(Json(json!({"email": email, "password": one_time})))
}

async fn platform_response(
    state: &AppState,
    caller: &Caller,
    cfg: &AccessConfig,
    errors: &BTreeMap<String, String>,
) -> ApiResult<Value> {
    let errors: BTreeMap<&String, &String> =
        errors.iter().filter(|(k, _)| k.starts_with("access") || k.starts_with("organisations/")).collect();
    let models: Vec<Value> = caller
        .project
        .models
        .iter()
        .map(|(n, m)| json!({"name": n, "label": m.label.clone().unwrap_or_else(|| n.clone())}))
        .collect();
    Ok(json!({
        "platform": cfg.platform,
        "members": cfg.platform.organisations.keys().map(|o| (o.clone(), cfg.members_of(o).users.len())).collect::<BTreeMap<_, _>>(),
        "models": models,
        "errors": errors,
        // where each organisation's own ScramDB is (redacted)
        "databases": state.store.org_databases().await?,
        "secret_key": state.config.secret_key.is_some(),
    }))
}

pub async fn get_platform(State(state): State<Shared>, Auth(caller): Auth) -> ApiResult<Json<Value>> {
    caller.require("manage_orgs")?;
    Ok(Json(platform_response(&state, &caller, &caller.access, &caller.access_errors).await?))
}

/// System owners: the platform part. Removing an organisation also removes its members.
pub async fn put_platform(
    State(state): State<Shared>,
    Auth(caller): Auth,
    Body(platform): Body<Platform>,
) -> ApiResult<Json<Value>> {
    caller.require("manage_orgs")?;
    // re-normalise (lowercase, dedupe) what the browser sent, then check it
    let platform: Platform = serde_json::from_value(serde_json::to_value(&platform).map_err(anyhow::Error::from)?)
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    platform.validate().map_err(ApiError::bad_request)?;
    let two_factor = platform.sign_in.require_two_factor
        || platform.organisations.values().any(|o| o.sign_in.as_ref().is_some_and(|p| p.require_two_factor));
    if two_factor && state.config.secret_key.is_none() {
        return Err(ApiError::bad_request("authenticator codes need MAGNUM_SECRET_KEY on the server"));
    }
    if !platform.system_owners.iter().any(|o| o == caller.email()) {
        return Err(ApiError::bad_request(
            "you would remove yourself from the system owners; ask another system owner",
        ));
    }
    let cfg = &caller.access;
    let removed: Vec<String> =
        cfg.platform.organisations.keys().filter(|o| !platform.organisations.contains_key(*o)).cloned().collect();
    let next = AccessConfig {
        members: cfg
            .members
            .iter()
            .filter(|(o, _)| platform.organisations.contains_key(*o))
            .map(|(o, m)| (o.clone(), m.clone()))
            .collect(),
        platform,
        broken: false,
    };
    store_access(&state, &next, Some("platform"), Some(caller.email())).await?;
    if !removed.is_empty() {
        let deletes: Vec<(String, Option<Value>)> = removed.iter().map(|o| (format!("members:{o}"), None)).collect();
        state.store.put_settings(&deletes, Some(caller.email())).await?;
        for o in &removed {
            state.orgs.forget(o).await?; // its instance keeps its data; Magnum BI stops using it
        }
    }
    state.usage.record(Event { detail: Some(json!({"removed": removed})), ..caller.event("platform_changed") });
    let (fresh, errors) = state.access.get(&state, &caller.project).await;
    Ok(Json(platform_response(&state, &caller, &fresh, &errors).await?))
}

#[derive(Deserialize)]
pub struct PublishBody {
    #[serde(default)]
    orgs: Vec<String>,
    #[serde(default, rename = "as")]
    target: Option<String>,
    #[serde(default)]
    overwrite: bool,
}

/// Copy a platform template into organisations (each copy is then the organisation's own).
pub async fn publish(
    State(state): State<Shared>,
    Auth(caller): Auth,
    Path(id): Path<String>,
    Body(body): Body<PublishBody>,
) -> ApiResult<Json<Value>> {
    caller.require("manage_orgs")?;
    let text = state
        .store
        .read_dashboard("", &id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("template {id:?} not found")))?;
    let d = parse_dashboard(&text).map_err(ApiError::bad_request)?;
    let target = body.target.filter(|t| !t.is_empty()).unwrap_or(id);
    if !DASHBOARD_ID.is_match(&target) {
        return Err(ApiError::bad_request(format!("invalid dashboard id {target:?}")));
    }
    let mut results = BTreeMap::new();
    for oid in body.orgs {
        let Some(org) = caller.access.platform.organisations.get(&oid) else {
            results.insert(oid, "unknown organisation".to_string());
            continue;
        };
        let all = org.models.iter().any(|m| m == "*");
        let mut missing: Vec<&str> = d
            .tiles
            .iter()
            .filter_map(|t| t.query.as_ref().map(|q| q.model.as_str()))
            .filter(|m| !(all && caller.project.models.contains_key(*m)) && !org.models.iter().any(|x| x == m))
            .collect();
        missing.sort_unstable();
        missing.dedup();
        let dest = match state.orgs.store(&oid).await {
            Ok(s) => s,
            Err(e @ (OrgDbError::NotConfigured(_) | OrgDbError::Unavailable(_))) => {
                results.insert(oid, format!("not published: {e}"));
                continue;
            }
            Err(e) => return Err(e.into()),
        };
        let result = if !missing.is_empty() {
            format!("not published: uses models the organisation doesn't have ({})", missing.join(", "))
        } else if !body.overwrite && dest.read_dashboard(&oid, &target).await?.is_some() {
            "not published: a dashboard with this id exists (overwrite to replace it)".into()
        } else {
            dest.write_dashboard(&oid, &target, &text, Some(caller.email())).await?;
            state.usage.record(Event {
                dashboard_id: Some(target.clone()),
                detail: Some(json!({"to_org": oid})),
                ..caller.event("template_published")
            });
            "published".into()
        };
        results.insert(oid, result);
    }
    Ok(Json(json!({"results": results})))
}

#[derive(Deserialize)]
pub struct DatabaseBody {
    dsn: String,
}

/// System owners: point an organisation at its own ScramDB instance. The connection is tested and
/// migrated first; the organisation's existing dashboards move there.
pub async fn set_org_database(
    State(state): State<Shared>,
    Auth(caller): Auth,
    Path(org): Path<String>,
    Body(body): Body<DatabaseBody>,
) -> ApiResult<Json<Value>> {
    caller.require("manage_orgs")?;
    if !caller.access.platform.organisations.contains_key(&org) {
        return Err(ApiError::not_found(format!("unknown organisation {org:?} (save the organisation first)")));
    }
    if body.dsn.trim().is_empty() || body.dsn.len() > 2048 {
        return Err(ApiError::bad_request(
            "enter the ScramDB connection string, e.g. postgresql://user:password@host:5432/scramdb",
        ));
    }
    let (display, version, moved) = state.orgs.register(&org, &body.dsn, Some(caller.email())).await?;
    state.usage.record(Event {
        detail: Some(json!({"for_org": org, "database": display, "dashboards_moved": moved})),
        ..caller.event("org_database_set")
    });
    Ok(Json(json!({"org": org, "display": display, "version": version, "dashboards_moved": moved})))
}

#[derive(Deserialize)]
pub struct DaysParam {
    #[serde(default = "thirty")]
    days: i64,
}

fn thirty() -> i64 {
    30
}

/// Usage of the caller's organisation (Org admins; system owners inside one), or of the whole
/// platform (system owners in the platform workspace).
pub async fn usage_summary(
    State(state): State<Shared>,
    Auth(caller): Auth,
    UrlQuery(p): UrlQuery<DaysParam>,
) -> ApiResult<Json<Value>> {
    caller.require("manage_members")?;
    if !(1..=400).contains(&p.days) {
        return Err(ApiError::bad_request("days: between 1 and 400"));
    }
    let org = match (&caller.scope.org, caller.scope.role) {
        (Some(o), _) => Some(o.as_str()),
        (None, Some(Role::SystemOwner)) => None,
        _ => return Err(ApiError::forbidden("choose an organisation first")),
    };
    let mut out = crate::usage::summary(&state.store, org, p.days).await?;
    out["scope"] = json!(org.unwrap_or("platform"));
    Ok(Json(out))
}

/// The organisation's query cache: per model, how long results may be reused and what's cached.
pub async fn cache_stats(State(state): State<Shared>, Auth(caller): Auth) -> ApiResult<Json<Value>> {
    caller.require("manage_members")?;
    let org = current_org(&caller)?;
    let ttl: BTreeMap<String, u64> =
        super::available_models(&caller).into_iter().map(|(n, m)| (n, m.cache.unwrap_or(0))).collect();
    let cached = crate::cache::stats(&state.orgs.store(&org).await?, &org).await?;
    Ok(Json(json!({"ttl_seconds": ttl, "cached": cached})))
}

#[derive(Deserialize)]
pub struct ModelParam {
    model: Option<String>,
}

/// Drop the organisation's cached results (all, or one model's), e.g. after loading new data.
pub async fn clear_cache(
    State(state): State<Shared>,
    Auth(caller): Auth,
    UrlQuery(p): UrlQuery<ModelParam>,
) -> ApiResult<Json<Value>> {
    caller.require("manage_members")?;
    let org = current_org(&caller)?;
    let n = crate::cache::clear(&state.orgs.store(&org).await?, &org, p.model.as_deref()).await?;
    state.usage.record(Event {
        model: p.model.clone(),
        detail: Some(json!({"cleared": n})),
        ..caller.event("cache_cleared")
    });
    Ok(Json(json!({"cleared": n})))
}

#[cfg(test)]
mod agent_tests {
    use axum::http::StatusCode;
    use serde_json::{Value, json};

    use crate::access::AccessConfig;
    use crate::api::tests::{OWNER, app};

    async fn send(
        app: &axum::Router,
        cookie: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
        if_match: Option<&str>,
    ) -> (StatusCode, Value) {
        let mut req = axum::http::Request::builder()
            .method(method)
            .uri(path)
            .header("cookie", cookie)
            .header("x-magnum-org", "polm")
            .header("content-type", "application/json");
        if let Some(v) = if_match {
            req = req.header("if-match", v);
        }
        let body = body.map_or(axum::body::Body::empty(), |b| axum::body::Body::from(b.to_string()));
        let resp = tower::ServiceExt::oneshot(app.clone(), req.body(body).unwrap()).await.unwrap();
        let status = resp.status();
        let bytes = http_body_util::BodyExt::collect(resp.into_body()).await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    #[tokio::test]
    async fn demo_setup_is_for_system_owners_and_checked_first() {
        let Some((app, _, cookie)) = app().await else { return };
        let (s, st) = crate::api::tests::call(&app, &cookie, "GET", "/api/platform/demo", None).await;
        assert_eq!(s, StatusCode::OK, "{st}");
        assert_eq!(st["running"], json!(false));
        for (body, why) in [
            (json!({"org": "no-such-org", "months": 13}), "no-such-org"),
            (json!({"org": "", "months": 0}), "months"),
            (json!({"org": "", "months": 500}), "months"),
        ] {
            let (s, r) = crate::api::tests::call(&app, &cookie, "POST", "/api/platform/demo", Some(body)).await;
            assert_eq!(s, StatusCode::BAD_REQUEST, "{r}");
            assert!(r["detail"].as_str().unwrap().contains(why), "{r}");
        }
    }

    #[tokio::test]
    async fn member_changes_check_what_was_read() {
        let Some((app, state, cookie)) = app().await else { return };
        state.access.pin(
            AccessConfig::from_document(json!({"system_owners": [OWNER], "organisations": {"polm": {"name": "Pol",
                "domains": ["polm.example"], "models": ["*"], "members": {"users": {"ana@polm.example": "user"}}}}}))
            .unwrap(),
        );
        let (s, got) = send(&app, &cookie, "GET", "/api/members", None, None).await;
        assert_eq!(s, StatusCode::OK, "{got}");
        let version = got["version"].as_str().unwrap().to_string();
        let next =
            json!({"default_role": "viewer", "users": {"ana@polm.example": "power_user", "bo@polm.example": "viewer"}});

        // a dry run: checked, not saved
        let (s, body) = send(&app, &cookie, "PUT", "/api/members?dry_run=1", Some(next.clone()), Some(&version)).await;
        assert_eq!(s, StatusCode::OK, "{body}");
        assert_eq!(body["dry_run"], json!(true));
        assert!(state.store.setting("members:polm").await.unwrap().is_none(), "nothing written");
        // a stale version: refused, nothing written
        let (s, body) = send(&app, &cookie, "PUT", "/api/members", Some(next), Some("0123456789abcdef")).await;
        assert_eq!(s, StatusCode::CONFLICT, "{body}");
        assert_eq!(body["version"], json!(version));
        assert!(state.store.setting("members:polm").await.unwrap().is_none());
    }
}
