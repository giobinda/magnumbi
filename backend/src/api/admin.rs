//! Members (Org admins), passwords for members, and the platform (system owners).

use std::collections::BTreeMap;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Body, available_names};
use crate::access::{AccessConfig, Members, Platform, Role, is_system_owner};
use crate::auth::{Auth, Caller, password};
use crate::error::{ApiError, ApiResult};
use crate::project::{DASHBOARD_ID, parse_dashboard};
use crate::state::{Shared, store_access};

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
        "members": members,
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

pub async fn put_members(
    State(state): State<Shared>,
    Auth(caller): Auth,
    Body(members): Body<Members>,
) -> ApiResult<Json<Value>> {
    caller.require("manage_members")?;
    let oid = current_org(&caller)?;
    let cfg = &caller.access;
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
    let mut next = (**cfg).clone();
    next.members.insert(oid.clone(), members);
    store_access(&state, &next, Some(&oid), Some(caller.email())).await?;
    let (fresh, errors) = state.access.get(&state, &caller.project).await;
    Ok(Json(members_response(&state, &caller, &fresh, &errors).await?))
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
    Ok(Json(json!({"email": email, "password": one_time})))
}

fn platform_response(caller: &Caller, cfg: &AccessConfig, errors: &BTreeMap<String, String>) -> Value {
    let errors: BTreeMap<&String, &String> =
        errors.iter().filter(|(k, _)| k.starts_with("access") || k.starts_with("organisations/")).collect();
    let models: Vec<Value> = caller
        .project
        .models
        .iter()
        .map(|(n, m)| json!({"name": n, "label": m.label.clone().unwrap_or_else(|| n.clone())}))
        .collect();
    json!({
        "platform": cfg.platform,
        "members": cfg.platform.organisations.keys().map(|o| (o.clone(), cfg.members_of(o).users.len())).collect::<BTreeMap<_, _>>(),
        "models": models,
        "errors": errors,
    })
}

pub async fn get_platform(Auth(caller): Auth) -> ApiResult<Json<Value>> {
    caller.require("manage_orgs")?;
    Ok(Json(platform_response(&caller, &caller.access, &caller.access_errors)))
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
    }
    let (fresh, errors) = state.access.get(&state, &caller.project).await;
    Ok(Json(platform_response(&caller, &fresh, &errors)))
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
            .map(|t| t.query.model.as_str())
            .filter(|m| !(all && caller.project.models.contains_key(*m)) && !org.models.iter().any(|x| x == m))
            .collect();
        missing.sort_unstable();
        missing.dedup();
        let result = if !missing.is_empty() {
            format!("not published: uses models the organisation doesn't have ({})", missing.join(", "))
        } else if !body.overwrite && state.store.read_dashboard(&oid, &target).await?.is_some() {
            "not published: a dashboard with this id exists (overwrite to replace it)".into()
        } else {
            state.store.write_dashboard(&oid, &target, &text, Some(caller.email())).await?;
            "published".into()
        };
        results.insert(oid, result);
    }
    Ok(Json(json!({"results": results})))
}
