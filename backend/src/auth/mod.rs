//! Sign-in and sessions.
//!
//! Ways to sign in, all ending in the same server-side session:
//! - **password** (auth::password): email + password, Argon2id hashes in ScramDB;
//! - **Google** (auth::google): OpenID Connect, for a Google-verified email;
//! - **an organisation's own provider** (auth::oidc): OpenID Connect, for its own domains;
//! - **passkeys** (auth::passkeys): WebAuthn.
//!
//! Passwords can need a second factor: an authenticator code (auth::totp).
//!
//! Which of them someone may use is a policy (auth::policy): the platform's, tightened for an
//! organisation by the platform and by its own admins. It's checked at sign-in and again on
//! every request, so a stricter policy ends sessions that no longer meet it.
//!
//! The session is an opaque random token in an HttpOnly cookie; ScramDB keeps only its SHA-256.
//! Who someone *is* comes from here; what they may do and see comes from organisations, roles
//! and row filters (access.rs).

pub mod google;
pub mod oidc;
pub mod passkeys;
pub mod password;
pub mod policy;
pub mod totp;

use std::sync::Arc;

use axum::extract::{FromRequestParts, Request, State};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum_extra::extract::CookieJar;
use axum_extra::extract::cookie::{Cookie, SameSite};
use serde_json::json;

use std::collections::BTreeSet;

use crate::access::{AccessConfig, Role, Scope, effective_role, scope_of, sign_in_policy};
use crate::error::ApiError;
use crate::project::ProjectState;
use crate::state::{AppState, Shared};
use crate::store::SessionInfo;
use policy::{Method, SessionCheck};

pub const COOKIE: &str = "magnum_session";
pub const ORG_HEADER: &str = "x-magnum-org";
pub const VIEW_AS_HEADER: &str = "x-magnum-view-as";
/// "1": bypass the query cache (builders only), e.g. after loading new data
pub const REFRESH_HEADER: &str = "x-magnum-refresh";

/// The sign-in mechanisms this server offers.
pub fn offered(state: &AppState) -> BTreeSet<Method> {
    let mut m = BTreeSet::new();
    if state.config.password_login {
        m.insert(Method::Password);
    }
    if state.google.is_some() {
        m.insert(Method::Google);
    }
    if state.config.secret_key.is_some() {
        m.insert(Method::Oidc); // providers' client secrets are sealed with it
    }
    if state.passkeys.is_some() {
        m.insert(Method::Passkey);
    }
    m.insert(Method::Token);
    m
}

/// A response refusing this session under the policy now in force, if it must be refused.
pub async fn refuse_session(
    state: &AppState,
    access: &AccessConfig,
    session: &SessionInfo,
    jar: &CookieJar,
) -> Option<Response> {
    let policy = sign_in_policy(access, &session.user.email, &offered(state));
    match policy.check_session(&session.method, session.user.two_factor) {
        SessionCheck::Ok => None,
        SessionCheck::SignInAgain(detail) => {
            if let Some(c) = jar.get(COOKIE) {
                let _ = state.store.delete_session(c.value()).await;
            }
            Some((StatusCode::UNAUTHORIZED, axum::Json(json!({"detail": detail, "login": true}))).into_response())
        }
        SessionCheck::SetUpTwoFactor => {
            let body = json!({"detail": "your organisation requires an authenticator code: set one up first",
                              "two_factor_setup": true});
            Some((StatusCode::FORBIDDEN, axum::Json(body)).into_response())
        }
    }
}

pub fn session_cookie(state: &AppState, token: String) -> Cookie<'static> {
    let same_site = match state.config.cookie_samesite.as_str() {
        "strict" => SameSite::Strict,
        "none" => SameSite::None,
        _ => SameSite::Lax,
    };
    Cookie::build((COOKIE, token))
        .path("/")
        .http_only(true)
        .same_site(same_site)
        .secure(state.config.is_https() || same_site == SameSite::None)
        .max_age(time_duration(state.config.session_hours * 3600))
        .build()
}

pub fn removal_cookie(name: &'static str) -> Cookie<'static> {
    Cookie::build((name, "")).path("/").build()
}

fn time_duration(seconds: i64) -> cookie::time::Duration {
    cookie::time::Duration::seconds(seconds)
}

/// The signed-in person behind a request, if any.
/// The browser session, or an API token (`Authorization: Bearer mbi_...`) for agents and scripts.
pub async fn session_or_token(
    state: &AppState,
    jar: &CookieJar,
    headers: &axum::http::HeaderMap,
) -> Result<Option<SessionInfo>, ApiError> {
    let bearer = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    match bearer {
        Some(token) => Ok(state.store.token_session(token.trim()).await?),
        None => session_of(state, jar).await,
    }
}

/// Requests a read-only token may make: reading, and queries (which read through POST).
pub fn read_only_request(method: &axum::http::Method, path: &str, query: Option<&str>) -> bool {
    if matches!(*method, axum::http::Method::GET | axum::http::Method::HEAD) {
        return true;
    }
    if query.is_some_and(|q| q.split('&').any(|p| p == "dry_run=1" || p == "dry_run=true")) {
        return true; // checks without saving
    }
    // MCP: its tools are read-only or not one by one (api/mcp.rs)
    *method == axum::http::Method::POST
        && (matches!(path, "/api/query" | "/api/query/sql" | "/api/period" | "/api/mcp")
            || (path.starts_with("/api/dashboards/") && (path.ends_with("/query") || path.contains("/filters/values"))))
}

pub async fn session_of(state: &AppState, jar: &CookieJar) -> Result<Option<SessionInfo>, ApiError> {
    match jar.get(COOKIE) {
        Some(c) => Ok(state.store.session(c.value()).await?),
        None => Ok(None),
    }
}

/// An account whose password was set by an organisation's admin may only ever act inside that
/// organisation: never as a system owner, never in another organisation. (Emails aren't
/// verified on that path, so the admin could have created an account for someone else's email.)
pub fn restrict_password_accounts(scope: Scope, session: &SessionInfo) -> Scope {
    let password = Method::of_session(&session.method) == Some(Method::Password);
    match &session.user.owner_org {
        Some(owner) if password && (scope.role == Some(Role::SystemOwner) || scope.org.as_deref() != Some(owner)) => {
            Scope::none()
        }
        _ => scope,
    }
}

pub fn scope_for(cfg: &AccessConfig, session: &SessionInfo, org_header: Option<&str>) -> Scope {
    if let Some(t) = &session.token {
        // a token works in its own organisation only, with at most the role it was made with
        let org = (!t.org.is_empty()).then_some(t.org.as_str());
        let mut scope = scope_of(cfg, &session.user.email, org);
        if scope.org.as_deref().unwrap_or_default() != t.org
            || session.user.owner_org.as_ref().is_some_and(|o| *o != t.org)
        {
            return Scope::none();
        }
        let cap = Role::parse(&t.role).unwrap_or(Role::Viewer);
        scope.role = scope.role.map(|r| r.min(cap));
        return scope;
    }
    let mut scope = restrict_password_accounts(scope_of(cfg, &session.user.email, org_header), session);
    // a role from the organisation's provider groups: for people who join by domain (a listed
    // member keeps the role set here), in that organisation only, never above Org admin
    if let (Some(granted), Some(oid)) = (&session.granted_role, &scope.org)
        && let Some((org, role)) = granted.split_once(':')
        && org == oid
        && let Some(role) = Role::parse(role).filter(|r| *r <= Role::OrgAdmin)
        && scope.role.is_some_and(|r| r < Role::SystemOwner)
        && !cfg.members_of(oid).users.contains_key(&session.user.email.to_lowercase())
    {
        scope.role = Some(role);
    }
    scope
}

fn header<'a>(parts: &'a Parts, name: &str) -> Option<&'a str> {
    parts.headers.get(name).and_then(|v| v.to_str().ok()).map(str::trim).filter(|v| !v.is_empty())
}

/// A signed-in request: who, in which organisation, as which role.
pub struct Caller {
    pub session: SessionInfo,
    pub scope: Scope,
    /// the role in effect ("view as" can only lower it)
    pub role: Option<Role>,
    pub access: Arc<AccessConfig>,
    pub access_errors: std::collections::BTreeMap<String, String>,
    pub project: Arc<ProjectState>,
    /// asked to bypass the query cache
    pub refresh: bool,
}

impl Caller {
    pub fn email(&self) -> &str {
        &self.session.user.email
    }

    /// The dashboard space of this request: its organisation, or "" (platform workspace).
    pub fn org(&self) -> &str {
        self.scope.org.as_deref().unwrap_or("")
    }

    /// A usage event about this person, in this organisation.
    pub fn event(&self, kind: &'static str) -> crate::usage::Event {
        crate::usage::Event {
            org: self.org().to_string(),
            user_email: Some(self.email().to_string()),
            role: self.scope.role.map(|r| r.as_str().to_string()),
            via: self.session.token.as_ref().map(|t| format!("token:{}", t.name)),
            ..crate::usage::Event::new(kind)
        }
    }

    pub fn can(&self, perm: &str) -> bool {
        self.role.is_some_and(|r| r.can(perm))
    }

    pub fn require(&self, perm: &str) -> Result<(), ApiError> {
        if self.scope.broken {
            return Err(ApiError::forbidden("the access settings are broken; a system owner must fix them"));
        }
        match self.role {
            None => Err(ApiError::forbidden(
                "you're not a member of any organisation yet: ask your company's Magnum BI admin",
            )),
            Some(r) if !r.can(perm) => {
                Err(ApiError::forbidden(format!("your role ({}) can't do this ({perm})", r.label())))
            }
            Some(_) => Ok(()),
        }
    }
}

/// Middleware on every protected route: a valid session, a settled password, then the scope.
pub async fn require_session(State(state): State<Shared>, jar: CookieJar, request: Request, next: Next) -> Response {
    let session = match session_or_token(&state, &jar, request.headers()).await {
        Ok(Some(s)) => s,
        Ok(None) => return ApiError::sign_in().into_response(),
        Err(e) => return e.into_response(),
    };
    if session.user.must_change_password {
        let body = json!({"detail": "choose a new password first", "change_password": true});
        return (StatusCode::FORBIDDEN, axum::Json(body)).into_response();
    }
    if session.token.as_ref().is_some_and(|t| !t.write)
        && !read_only_request(request.method(), request.uri().path(), request.uri().query())
    {
        return ApiError::forbidden("this API token is read-only").into_response();
    }
    let (mut parts, body) = request.into_parts();
    let project = state.project.current();
    let (access, access_errors) = state.access.get(&state, &project).await;
    if let Some(refused) = refuse_session(&state, &access, &session, &jar).await {
        return refused;
    }
    let org_choice = header(&parts, ORG_HEADER).map(str::to_lowercase);
    let scope = scope_for(&access, &session, org_choice.as_deref());
    let role = effective_role(scope.role, header(&parts, VIEW_AS_HEADER));
    let refresh = header(&parts, REFRESH_HEADER) == Some("1");
    let project = project_for(&state, &scope, project).await;
    let caller = Caller { session, scope, role, access, access_errors, project, refresh };
    parts.extensions.insert(Arc::new(caller));
    next.run(Request::from_parts(parts, body)).await
}

/// The project as one scope sees it: with its organisation's own blends and themes.
async fn project_for(state: &AppState, scope: &Scope, mut project: Arc<ProjectState>) -> Arc<ProjectState> {
    if scope.role.is_none() {
        return project;
    }
    let org = scope.org.clone().unwrap_or_default();
    match state.org_blends(&org).await {
        Ok(b) if !b.is_empty() => project = Arc::new(project.with_org_blends(&b)),
        Ok(_) => {}
        Err(e) => tracing::warn!(org, "organisation's blends unavailable: {e:#}"),
    }
    match state.org_themes(&org).await {
        Ok(t) if !t.themes.is_empty() || t.default_light.is_some() || t.default_dark.is_some() => {
            project = Arc::new(project.with_org_themes(&t))
        }
        Ok(_) => {}
        Err(e) => tracing::warn!(org, "organisation's themes unavailable: {e:#}"),
    }
    project
}

/// Who a public link's visitors are: the person who made the link, in its organisation, as a
/// Viewer at most. Their data restrictions apply, so a link never shows more than they may see.
/// None once they can't see that organisation any more (removed, disabled): the link stops working.
pub async fn link_caller(state: &AppState, org: &str, email: &str) -> Result<Option<Caller>, ApiError> {
    let Some(user) = state.store.user(email).await? else { return Ok(None) };
    if user.disabled || user.owner_org.as_deref().is_some_and(|o| o != org) {
        return Ok(None);
    }
    let project = state.project.current();
    let (access, access_errors) = state.access.get(state, &project).await;
    let scope = scope_of(&access, email, Some(org));
    if scope.org.as_deref() != Some(org) || scope.role.is_none() || scope.broken {
        return Ok(None);
    }
    let project = project_for(state, &scope, project).await;
    let session = SessionInfo { method: "share".into(), user, granted_role: None, token: None };
    Ok(Some(Caller { session, scope, role: Some(Role::Viewer), access, access_errors, project, refresh: false }))
}

pub struct Auth(pub Arc<Caller>);

impl<S: Send + Sync> FromRequestParts<S> for Auth {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts.extensions.get::<Arc<Caller>>().cloned().map(Auth).ok_or_else(ApiError::sign_in)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::User;

    fn session(method: &str, owner_org: Option<&str>) -> SessionInfo {
        SessionInfo {
            method: method.into(),
            user: User {
                email: "boss@acme.example".into(),
                name: "Boss".into(),
                password_hash: None,
                google_sub: None,
                owner_org: owner_org.map(String::from),
                must_change_password: false,
                disabled: false,
                two_factor: false,
            },
            granted_role: None,
            token: None,
        }
    }

    #[test]
    fn org_set_passwords_stay_in_their_organisation() {
        let cfg = AccessConfig::from_document(json!({
            "system_owners": ["boss@acme.example"],
            "organisations": {"acme": {"name": "ACME", "members": {"users": {}}}, "globex": {"name": "Globex"}}
        }))
        .unwrap();
        // a system owner whose password an Org admin of acme set: no system owner rights with it
        assert_eq!(scope_for(&cfg, &session("password", Some("acme")), None).role, None);
        assert_eq!(scope_for(&cfg, &session("password", Some("acme")), Some("acme")).role, None);
        // the same person signed in with Google, or with a password they set themselves: fine
        assert_eq!(scope_for(&cfg, &session("google", Some("acme")), None).role, Some(Role::SystemOwner));
        assert_eq!(scope_for(&cfg, &session("password", None), None).role, Some(Role::SystemOwner));

        let cfg = AccessConfig::from_document(json!({
            "organisations": {"acme": {"name": "ACME", "members": {"users": {"boss@acme.example": "org_admin"}}}, "globex": {"name": "Globex"}}
        }))
        .unwrap();
        assert_eq!(scope_for(&cfg, &session("password", Some("acme")), None).role, Some(Role::OrgAdmin));
        assert_eq!(scope_for(&cfg, &session("password", Some("globex")), None).role, None);
    }
}
