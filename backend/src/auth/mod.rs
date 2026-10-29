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
    if state.config.lite && matches!(session.method.as_str(), "local" | "pin") {
        return None; // no sign-in to check (lite.rs)
    }
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
        None => match session_of(state, jar).await? {
            // lite: one person on their own computer, who doesn't sign in (lite.rs), unless it's
            // locked with a PIN: then a session comes from giving it
            None if state.config.lite && state.config.lite_pin.is_none() => {
                Ok(crate::lite::session(&state.store).await?)
            }
            found => Ok(found),
        },
    }
}

/// Requests a read-only token may make: reading, queries and downloads (which read through POST),
/// and dry runs where the handler checks without saving. Any other write is refused, whatever
/// its query string says.
pub fn read_only_request(method: &axum::http::Method, path: &str, query: Option<&str>) -> bool {
    use axum::http::Method;
    if matches!(*method, Method::GET | Method::HEAD) {
        return true;
    }
    let dry_run = query.is_some_and(|q| q.split('&').any(|p| p == "dry_run=1" || p == "dry_run=true"));
    let parts: Vec<&str> = path.trim_start_matches("/api/").split('/').collect();
    if *method == Method::PUT && dry_run {
        // the handlers that honour ?dry_run (dashboards, blends, members, organisation themes)
        return matches!(
            parts.as_slice(),
            ["dashboards", _]
                | ["dashboards", _, "yaml"]
                | ["blends", _]
                | ["members"]
                | ["org-themes"]
                | ["formulas", _, _]
                | ["governance", _]
                | ["governance", _, "docs"]
                | ["glossary", _]
        );
    }
    // a report checked, not made (a plan of files isn't: it may reach the addresses they name)
    if *method == Method::POST && dry_run && matches!(parts.as_slice(), ["dashboards", _, "schedules"]) {
        return true;
    }
    // MCP: its tools are read-only or not one by one (api/mcp.rs)
    *method == Method::POST
        && (matches!(parts.as_slice(), ["query"] | ["query", "sql"] | ["period"] | ["mcp"] | ["formulas", "check"])
            || matches!(
                parts.as_slice(),
                ["dashboards", _, "tiles", _, "query" | "export"] | ["dashboards", _, "export"]
            ))
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
#[derive(Clone)]
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
    /// what an apply (api/code.rs) adds before what it's checking: it counts as there
    pub planned: Arc<Planned>,
}

/// What files being applied will have added by the time a later change is made (api/code.rs),
/// so the plan checks that change as it will be made. Empty for every other request.
#[derive(Clone, Default)]
pub struct Planned {
    pub terms: Vec<String>,
    /// connections: settings, and a new secret (none: the stored one)
    pub connections: std::collections::BTreeMap<String, (crate::connections::ConnectionConfig, Option<String>)>,
    /// dashboards: their YAML
    pub dashboards: std::collections::BTreeMap<String, String>,
    pub mail: Option<crate::delivery::MailServer>,
    pub webhooks: Vec<String>,
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
    // (lite's owner doesn't sign in, so has no password to change and nowhere to change it)
    let lite_owner = state.config.lite && matches!(session.method.as_str(), "local" | "pin");
    if session.user.must_change_password && !lite_owner {
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
    // a request naming an organisation acts in that one or not at all (never quietly elsewhere)
    if let Some(choice) = org_choice.as_deref()
        && scope.org.as_deref() != Some(choice)
    {
        return ApiError::forbidden(format!("you can't work in the organisation {choice:?}")).into_response();
    }
    let role = effective_role(scope.role, header(&parts, VIEW_AS_HEADER));
    let refresh = header(&parts, REFRESH_HEADER) == Some("1");
    let project = project_for(&state, &scope, project).await;
    let caller = Caller { session, scope, role, access, access_errors, project, refresh, planned: Default::default() };
    parts.extensions.insert(Arc::new(caller));
    next.run(Request::from_parts(parts, body)).await
}

/// The project as one scope sees it: with its organisation's own blends and themes.
async fn project_for(state: &AppState, scope: &Scope, mut project: Arc<ProjectState>) -> Arc<ProjectState> {
    if scope.role.is_none() {
        return project;
    }
    let org = scope.org.clone().unwrap_or_default();
    // the models made in the app first, the platform's and its own: its blends may join them
    if let Some(p) = with_app_models(state, &org, &project).await {
        project = Arc::new(p);
    }
    match state.org_formulas(&org).await {
        Ok(f) if !f.is_empty() => project = Arc::new(project.with_formulas(&f)),
        Ok(_) => {}
        Err(e) => tracing::warn!(org, "organisation's formulas unavailable: {e:#}"),
    }
    match state.org_blends(&org).await {
        Ok(b) if !b.is_empty() => project = Arc::new(project.with_org_blends(&b)),
        Ok(_) => {}
        Err(e) => tracing::warn!(org, "organisation's blends unavailable: {e:#}"),
    }
    // the shared formulas on blends, the organisation's own blends in
    match state.org_formulas(&org).await {
        Ok(f) if !f.is_empty() => project = Arc::new(project.with_blend_formulas(&f)),
        Ok(_) => {}
        Err(e) => tracing::warn!(org, "organisation's formulas unavailable: {e:#}"),
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

/// The project plus the models made in the app: the platform's (made in the platform workspace,
/// for every organisation), and the organisation's own, which keep their names in it. When the
/// platform's can't be read, organisations go on with their own (and the YAML models). None: there
/// are none.
pub async fn with_app_models(state: &AppState, org: &str, project: &ProjectState) -> Option<ProjectState> {
    let own = if org.is_empty() {
        Arc::new(vec![])
    } else {
        state.org_models(org).await.unwrap_or_else(|e| {
            tracing::warn!(org, "organisation's models unavailable: {e:#}");
            Arc::new(vec![])
        })
    };
    let platform = state.org_models("").await.unwrap_or_else(|e| {
        tracing::warn!("the platform's models unavailable: {e:#}");
        Arc::new(vec![])
    });
    if own.is_empty() && platform.is_empty() {
        return None;
    }
    let names: Vec<String> = own.iter().map(|m| m.name.clone()).collect();
    Some(project.with_platform_models(&platform, &names).with_org_models(&own))
}

/// Who acts through a link, for someone not signed in here (a public link's visitors, a guest
/// link's guests): the link's maker, in its organisation, with a role capped at `link.cap`.
/// The maker's data restrictions apply, plus the link's own filters (which only narrow), so a
/// link never shows more than its maker may see. None once the maker can't see that organisation
/// any more (removed, disabled) or is below `link.min`: the link stops working.
pub async fn link_caller(
    state: &AppState,
    org: &str,
    maker: &str,
    link: LinkAccess<'_>,
) -> Result<Option<Caller>, ApiError> {
    let Some(user) = state.store.user(maker).await? else { return Ok(None) };
    if user.disabled || user.owner_org.as_deref().is_some_and(|o| o != org) {
        return Ok(None);
    }
    let project = state.project.current();
    let (access, access_errors) = state.access.get(state, &project).await;
    let mut scope = scope_of(&access, maker, Some(org));
    // in that organisation ("": the platform workspace, its system owners)
    if scope.org.as_deref().unwrap_or_default() != org || scope.broken || !scope.role.is_some_and(|r| r >= link.min) {
        return Ok(None);
    }
    scope.personal_filters.extend(link.filters.iter().cloned());
    let role = scope.role.map(|r| r.min(link.cap));
    let project = project_for(state, &scope, project).await;
    // recorded as this link in the usage log
    let token =
        crate::store::TokenInfo { name: link.name, org: org.into(), role: link.cap.as_str().into(), write: false };
    let session = SessionInfo { method: "link".into(), user, granted_role: None, token: Some(token) };
    Ok(Some(Caller {
        session,
        scope,
        role,
        access,
        access_errors,
        project,
        refresh: false,
        planned: Default::default(),
    }))
}

/// What a link lets its users do: see [`link_caller`].
pub struct LinkAccess<'a> {
    /// the lowest role its maker may have now for it to work
    pub min: Role,
    /// the highest role its users get
    pub cap: Role,
    /// rows its users may see, on top of the maker's restrictions
    pub filters: &'a [crate::access::RowRule],
    /// for the usage log: "public link <id>"
    pub name: String,
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

    #[test]
    fn read_only_tokens_read_and_check_only() {
        use axum::http::Method;
        let ok = |m: Method, path: &str, q: Option<&str>| read_only_request(&m, path, q);
        assert!(ok(Method::GET, "/api/dashboards", None));
        assert!(ok(Method::POST, "/api/query", None) && ok(Method::POST, "/api/mcp", None));
        assert!(ok(Method::POST, "/api/dashboards/d/tiles/t/query", None));
        assert!(ok(Method::POST, "/api/dashboards/d/tiles/t/export", None), "downloading is reading");
        assert!(ok(Method::POST, "/api/dashboards/d/export", None));
        // a plan may connect to the addresses its files name, which reading never does
        assert!(!ok(Method::POST, "/api/code", Some("dry_run=1")) && !ok(Method::POST, "/api/code", None));
        // a dry run only where the handler checks without saving
        for path in ["/api/dashboards/d", "/api/dashboards/d/yaml", "/api/blends/b", "/api/members", "/api/org-themes"]
        {
            assert!(ok(Method::PUT, path, Some("dry_run=1")), "{path}");
            assert!(!ok(Method::PUT, path, None), "{path}");
        }
        for (m, path) in [
            (Method::DELETE, "/api/dashboards/d"),
            (Method::DELETE, "/api/share-links/x"),
            (Method::POST, "/api/members/password"),
            (Method::PUT, "/api/delivery/mail"),
            (Method::PUT, "/api/connections/warehouse"),
            (Method::POST, "/api/connections/warehouse/test"),
            (Method::DELETE, "/api/cache"),
            (Method::POST, "/api/dashboards/d/guest-links"),
        ] {
            assert!(!ok(m.clone(), path, Some("dry_run=1")), "{m} {path} with dry_run");
        }
    }
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
