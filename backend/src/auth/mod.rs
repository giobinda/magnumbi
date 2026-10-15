//! Sign-in and sessions.
//!
//! Two ways to sign in, both ending in the same server-side session:
//! - **password** (auth::password): email + password, Argon2id hashes in ScramDB;
//! - **Google** (auth::google): OpenID Connect, for a Google-verified email.
//!
//! The session is an opaque random token in an HttpOnly cookie; ScramDB keeps only its SHA-256.
//! Who someone *is* comes from here; what they may do and see comes from organisations, roles
//! and row filters (access.rs).

pub mod google;
pub mod password;

use std::sync::Arc;

use axum::extract::{FromRequestParts, Request, State};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum_extra::extract::CookieJar;
use axum_extra::extract::cookie::{Cookie, SameSite};
use serde_json::json;

use crate::access::{AccessConfig, Role, Scope, effective_role, scope_of};
use crate::error::ApiError;
use crate::project::ProjectState;
use crate::state::{AppState, Shared};
use crate::store::SessionInfo;

pub const COOKIE: &str = "magnum_session";
pub const ORG_HEADER: &str = "x-magnum-org";
pub const VIEW_AS_HEADER: &str = "x-magnum-view-as";
/// "1": bypass the query cache (builders only), e.g. after loading new data
pub const REFRESH_HEADER: &str = "x-magnum-refresh";

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
    match (&session.method[..], &session.user.owner_org) {
        ("password", Some(owner)) if scope.role == Some(Role::SystemOwner) || scope.org.as_deref() != Some(owner) => {
            Scope::none()
        }
        _ => scope,
    }
}

pub fn scope_for(cfg: &AccessConfig, session: &SessionInfo, org_header: Option<&str>) -> Scope {
    restrict_password_accounts(scope_of(cfg, &session.user.email, org_header), session)
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
    let session = match session_of(&state, &jar).await {
        Ok(Some(s)) => s,
        Ok(None) => return ApiError::sign_in().into_response(),
        Err(e) => return e.into_response(),
    };
    if session.user.must_change_password {
        let body = json!({"detail": "choose a new password first", "change_password": true});
        return (StatusCode::FORBIDDEN, axum::Json(body)).into_response();
    }
    let (mut parts, body) = request.into_parts();
    let project = state.project.current();
    let (access, access_errors) = state.access.get(&state, &project).await;
    let org_choice = header(&parts, ORG_HEADER).map(str::to_lowercase);
    let scope = scope_for(&access, &session, org_choice.as_deref());
    let role = effective_role(scope.role, header(&parts, VIEW_AS_HEADER));
    let refresh = header(&parts, REFRESH_HEADER) == Some("1");
    // the organisation's own blends join the project's models for this request
    let mut project = project;
    if scope.role.is_some() {
        let org = scope.org.clone().unwrap_or_default();
        match state.blends.get(&state.orgs, &org).await {
            Ok(b) if !b.is_empty() => project = Arc::new(project.with_org_blends(&b)),
            Ok(_) => {}
            Err(e) => tracing::warn!(org, "organisation's blends unavailable: {e:#}"),
        }
    }
    let caller = Caller { session, scope, role, access, access_errors, project, refresh };
    parts.extensions.insert(Arc::new(caller));
    next.run(Request::from_parts(parts, body)).await
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
            },
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
