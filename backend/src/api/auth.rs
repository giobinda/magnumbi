//! Sign-in endpoints: who am I, password sign-in, Google sign-in, sign-out, change password.

use axum::Json;
use std::net::SocketAddr;

use axum::Extension;
use axum::extract::{ConnectInfo, Path, Query as UrlQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum_extra::extract::CookieJar;
use axum_extra::extract::cookie::{Cookie, SameSite};
use chrono::Duration;
use serde::Deserialize;
use serde_json::{Value, json};

use super::Body;
use crate::access::{
    ROLES, Role, describe_restrictions, effective_role, is_system_owner, oidc_for, org_policy, sign_in_org,
    sign_in_policy,
};
use crate::auth::oidc::Oidc;
use crate::auth::password::{self, check_policy};
use crate::auth::policy::Method;
use crate::auth::totp;
use crate::auth::{
    COOKIE, ORG_HEADER, VIEW_AS_HEADER, offered, refuse_session, removal_cookie, scope_for, session_cookie, session_of,
    session_or_token,
};
use crate::error::{ApiError, ApiResult};
use crate::security::{is_https, public_base};
use crate::state::{AppState, Shared};
use crate::store::{OAuthState, SessionInfo, random_token};
use webauthn_rs::Webauthn;
use webauthn_rs::prelude::{
    Passkey, PasskeyAuthentication, PasskeyRegistration, PublicKeyCredential, RegisterPublicKeyCredential,
};

const OAUTH_COOKIE: &str = "magnum_oauth_state";

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok()).map(str::trim).filter(|v| !v.is_empty())
}

/// Where to go after sign-in: only a path on this site (never //host, javascript: or a URL).
fn safe_next(n: Option<&str>) -> String {
    match n {
        Some(n) if n.starts_with('/') && !n.starts_with("//") && !n.contains('\\') => n.to_string(),
        _ => "/".into(),
    }
}

fn login_failed(message: &str) -> Response {
    let q: String = url::form_urlencoded::byte_serialize(message.as_bytes()).collect();
    Redirect::to(&format!("/login?error={q}")).into_response()
}

async fn start_session(state: &AppState, jar: CookieJar, email: &str, method: &str) -> ApiResult<CookieJar> {
    start_session_as(state, jar, email, method, None).await
}

/// A session, with a role from the organisation's provider groups ("<org>:<role>").
async fn start_session_as(
    state: &AppState,
    jar: CookieJar,
    email: &str,
    method: &str,
    granted_role: Option<&str>,
) -> ApiResult<CookieJar> {
    let ttl = Duration::hours(state.config.session_hours);
    let token = state.store.create_session_as(email, method, ttl, granted_role).await?;
    state.store.record_login(email).await?;
    tracing::info!(method, "signed in");
    let project = state.project.current();
    let (access, _) = state.access.get(state, &project).await;
    let scope = crate::access::scope_of(&access, email, None);
    state.usage.record(crate::usage::Event {
        org: scope.org.unwrap_or_default(),
        user_email: Some(email.to_lowercase()),
        role: scope.role.map(|r| r.as_str().to_string()),
        detail: Some(json!({"method": method})),
        ..crate::usage::Event::new("sign_in")
    });
    Ok(jar.add(session_cookie(state, token)))
}

pub async fn me(State(state): State<Shared>, jar: CookieJar, headers: HeaderMap) -> ApiResult<Json<Value>> {
    let session = session_or_token(&state, &jar, &headers).await?;
    let methods = json!({"password": state.config.password_login, "google": state.google.is_some()});
    let signed_out = || Json(json!({"required": true, "authenticated": false, "user": null, "methods": methods}));
    let Some(session) = session else { return Ok(signed_out()) };
    let project = state.project.current();
    let (access, _) = state.access.get(&state, &project).await;
    let two_factor_setup = match refuse_session(&state, &access, &session, &jar).await {
        None => false,
        Some(r) if r.status() == StatusCode::UNAUTHORIZED => return Ok(signed_out()),
        Some(_) => true,
    };
    let mut out = json!({
        "required": true,
        "authenticated": true,
        "user": {"email": session.user.email, "name": session.user.name},
        "methods": methods,
        "method": session.method,
        "has_password": session.user.password_hash.is_some(),
        "two_factor": session.user.two_factor,
        "must_change_password": session.user.must_change_password,
    });
    if session.user.must_change_password {
        return Ok(Json(out)); // nothing else until a new password is chosen
    }
    if two_factor_setup {
        out["two_factor_setup"] = json!(true);
        return Ok(Json(out)); // nothing else until an authenticator is set up
    }
    let org_choice = header(&headers, ORG_HEADER).map(str::to_lowercase);
    let scope = scope_for(&access, &session, org_choice.as_deref());
    let o = out.as_object_mut().expect("object");
    o.insert("member".into(), json!(scope.role.is_some()));
    o.insert(
        "org".into(),
        match &scope.org {
            Some(id) => json!({"id": id, "name": access.platform.organisations.get(id).map(|o| o.name.clone())}),
            None => Value::Null,
        },
    );
    if let Some(actual) = scope.role {
        let role = effective_role(Some(actual), header(&headers, VIEW_AS_HEADER)).unwrap_or(actual);
        let view_as: Vec<&str> = ROLES.iter().filter(|r| **r <= actual).map(|r| r.as_str()).collect();
        o.insert("role".into(), json!(role.as_str()));
        o.insert("role_label".into(), json!(role.label()));
        o.insert("actual_role".into(), json!(actual.as_str()));
        o.insert("permissions".into(), json!(role.permissions()));
        o.insert("view_as".into(), json!(view_as));
        o.insert("restrictions".into(), json!(describe_restrictions(&scope, &project)));
        if actual == Role::SystemOwner {
            // the organisation switcher
            let orgs: Vec<Value> =
                access.platform.organisations.iter().map(|(id, o)| json!({"id": id, "name": o.name})).collect();
            o.insert("orgs".into(), json!(orgs));
        }
    }
    Ok(Json(out))
}

/// May `email` sign in with `method`? (After proving who they are: the answer reveals their
/// organisation's rules.)
async fn allowed(state: &AppState, email: &str, method: Method) -> Result<(), String> {
    let project = state.project.current();
    let (access, _) = state.access.get(state, &project).await;
    let policy = sign_in_policy(&access, email, &offered(state));
    if policy.allows(method) {
        return Ok(());
    }
    let ways: Vec<&str> = policy.methods.iter().map(|m| m.label()).collect();
    let detail = if ways.is_empty() {
        "this account can't sign in here at the moment; ask your administrator".to_string()
    } else {
        format!("this account signs in with {}", ways.join(" or "))
    };
    Err(detail)
}

#[derive(Deserialize)]
pub struct MethodsBody {
    email: String,
}

/// The ways `email` can sign in, for the sign-in page's second step. Someone unknown gets the
/// platform's (as does anyone the platform alone decides for).
pub async fn methods(State(state): State<Shared>, Body(body): Body<MethodsBody>) -> ApiResult<Json<Value>> {
    let email = body.email.trim().to_lowercase();
    let project = state.project.current();
    let (access, _) = state.access.get(&state, &project).await;
    let policy = sign_in_policy(&access, &email, &offered(&state));
    let provider = oidc_for(&access, &email);
    let usable: Vec<&str> = policy.usable(provider.is_some()).iter().map(|m| m.as_str()).collect();
    let oidc = provider
        .filter(|_| policy.allows(Method::Oidc))
        .map(|(oid, o)| json!({"label": o.label, "url": format!("/api/auth/oidc/{oid}/login")}));
    Ok(Json(json!({"methods": usable, "oidc": oidc})))
}

const OIDC_COOKIE: &str = "magnum_oidc_state";

/// An organisation's provider, step 1: off to it (PKCE, state and nonce, like Google).
pub async fn oidc_login(
    State(state): State<Shared>,
    jar: CookieJar,
    headers: HeaderMap,
    Path(org): Path<String>,
    UrlQuery(p): UrlQuery<NextParam>,
) -> ApiResult<Response> {
    let project = state.project.current();
    let (access, _) = state.access.get(&state, &project).await;
    let Some(settings) = access.members.get(&org).and_then(|m| m.oidc.clone()) else {
        return Ok(login_failed("this organisation has no single sign-on set up"));
    };
    if !org_policy(&access, &org, &offered(&state)).allows(Method::Oidc) {
        return Ok(login_failed("single sign-on isn't allowed for this organisation"));
    }
    let provider = match state.oidc.provider(&settings.issuer, state.connectors.net_guard()).await {
        Ok(p) => p,
        Err(e) => return Ok(login_failed(&e.0)),
    };
    let (st, nonce, verifier) = (random_token(24), random_token(24), random_token(48));
    let redirect_uri = format!("{}/api/auth/oidc/callback", public_base(&state.config, &headers));
    let next = safe_next(p.next.as_deref());
    state.store.put_oauth_state(&st, &verifier, &nonce, &redirect_uri, &next, Some(&org)).await?;
    let cookie = Cookie::build((OIDC_COOKIE, st.clone()))
        .path("/api/auth/oidc")
        .http_only(true)
        .same_site(SameSite::Lax) // must survive the top-level redirect back from the provider
        .secure(is_https(&state.config, &headers))
        .max_age(cookie::time::Duration::minutes(10))
        .build();
    let url = Oidc::authorize_url(&provider, &settings.client_id, &redirect_uri, &st, &nonce, &verifier);
    Ok((jar.add(cookie), Redirect::to(&url)).into_response())
}

/// Step 2: verify who this is, check they belong to the organisation, start a session (with a
/// role from their provider groups).
pub async fn oidc_callback(
    State(state): State<Shared>,
    jar: CookieJar,
    UrlQuery(p): UrlQuery<CallbackParams>,
) -> ApiResult<Response> {
    let jar_without = jar.clone().remove(Cookie::build((OIDC_COOKIE, "")).path("/api/auth/oidc").build());
    if let Some(e) = p.error {
        return Ok(login_failed(if e == "access_denied" { "sign-in was cancelled" } else { &e }));
    }
    let (Some(code), Some(st)) = (p.code, p.state) else { return Ok(login_failed("sign-in was cancelled")) };
    if jar.get(OIDC_COOKIE).map(|c| c.value()) != Some(st.as_str()) {
        return Ok(login_failed(
            "your sign-in session was lost (cookies blocked, or Magnum BI is reachable under more than one address); please try again",
        ));
    }
    let Some(OAuthState { verifier, nonce, redirect_uri, next, provider: Some(org) }) =
        state.store.take_oauth_state(&st).await?
    else {
        return Ok(login_failed("this sign-in link expired or was already used; please try again"));
    };
    let project = state.project.current();
    let (access, _) = state.access.get(&state, &project).await;
    let (Some(settings), Some(organisation)) =
        (access.members.get(&org).and_then(|m| m.oidc.clone()), access.platform.organisations.get(&org))
    else {
        return Ok(login_failed("this organisation has no single sign-on set up"));
    };
    let secret = match (&settings.client_secret_sealed, &state.config.secret_key) {
        (Some(sealed), Some(key)) => Some(key.open(sealed, &format!("{org}/oidc"))?),
        _ => None,
    };
    let provider = match state.oidc.provider(&settings.issuer, state.connectors.net_guard()).await {
        Ok(p) => p,
        Err(e) => return Ok(login_failed(&e.0)),
    };
    let finished = state
        .oidc
        .finish(
            &provider,
            &settings.client_id,
            secret.as_deref(),
            &code,
            &redirect_uri,
            &verifier,
            &nonce,
            &settings.groups_claim,
        )
        .await;
    let identity = match finished {
        Ok(i) => i,
        Err(e) => return Ok(login_failed(&e.0)),
    };
    // the provider vouches for its organisation's own domains only, and never for a platform account
    let domain = identity.email.rsplit_once('@').map(|(_, d)| d).unwrap_or_default();
    if !organisation.domains.iter().any(|d| d == domain) || is_system_owner(&access, &identity.email) {
        let domains: Vec<String> = organisation.domains.iter().map(|d| format!("@{d}")).collect();
        return Ok(login_failed(&format!(
            "{} can't sign in here: this provider is for {} addresses",
            identity.email,
            domains.join(", ")
        )));
    }
    if sign_in_org(&access, &identity.email).as_deref() != Some(org.as_str()) {
        return Ok(login_failed(&format!("{} belongs to another organisation", identity.email)));
    }
    if let Err(e) = allowed(&state, &identity.email, Method::Oidc).await {
        return Ok(login_failed(&e));
    }
    let user = state.store.upsert_sso_user(&identity.email, &identity.name).await?;
    if user.disabled {
        return Ok(login_failed("this account is disabled"));
    }
    let granted = identity
        .groups
        .iter()
        .filter_map(|g| settings.group_roles.get(g))
        .filter(|r| **r <= Role::OrgAdmin)
        .max()
        .map(|r| format!("{org}:{}", r.as_str()));
    let jar = start_session_as(&state, jar_without, &user.email, "oidc", granted.as_deref()).await?;
    Ok((jar, Redirect::to(&next)).into_response())
}

#[derive(Deserialize)]
pub struct LoginBody {
    email: String,
    password: String,
}

/// The client's address: the TCP peer, or (behind a proxy) the configured header's first entry.
pub(super) fn client_ip(
    state: &AppState,
    headers: &HeaderMap,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
) -> String {
    let forwarded = state.config.client_ip_header.as_deref().and_then(|h| header(headers, h));
    match forwarded.and_then(|v| v.split(',').next()).map(str::trim).filter(|v| !v.is_empty()) {
        Some(ip) => ip.chars().take(64).collect(),
        None => peer.map_or_else(|| "unknown".into(), |Extension(ConnectInfo(a))| a.ip().to_string()),
    }
}

/// Run slow password hashing off the async threads, a few at a time (sign-in floods can't take
/// every CPU).
pub(super) async fn hashing<T: Send + 'static>(
    state: &AppState,
    f: impl FnOnce() -> T + Send + 'static,
) -> ApiResult<T> {
    let busy = || ApiError::Status(StatusCode::SERVICE_UNAVAILABLE, "busy signing people in; try again shortly".into());
    let _permit = tokio::time::timeout(Duration::seconds(5).to_std().expect("5 s"), state.hashing.acquire())
        .await
        .map_err(|_| busy())?
        .map_err(|_| busy())?;
    Ok(tokio::task::spawn_blocking(f).await.map_err(anyhow::Error::from)?)
}

fn sign_in_failed(state: &AppState, email: &str, ip: &str, reason: &str) {
    tracing::warn!(reason, ip, "sign-in failed");
    state.usage.record(crate::usage::Event {
        user_email: Some(email.chars().take(254).collect()),
        ok: false,
        detail: Some(json!({"reason": reason, "ip": ip})),
        ..crate::usage::Event::new("sign_in_failed")
    });
}

pub async fn login(
    State(state): State<Shared>,
    jar: CookieJar,
    headers: HeaderMap,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    Body(body): Body<LoginBody>,
) -> ApiResult<Response> {
    if !state.config.password_login {
        return Err(ApiError::not_found("password sign-in is switched off here"));
    }
    let wrong = || ApiError::Status(StatusCode::UNAUTHORIZED, "wrong email or password".into());
    let email = body.email.trim().to_lowercase();
    if email.len() > 320 || body.password.len() > 4 * password::MAX_LENGTH {
        return Err(wrong());
    }
    let ip = client_ip(&state, &headers, peer);
    let address_key = format!("ip:{ip}");
    for (limiter, key, reason) in
        [(&state.ip_limiter, &address_key, "address_locked"), (&state.limiter, &email, "account_locked")]
    {
        if let Some(wait) = limiter.locked(key) {
            sign_in_failed(&state, &email, &ip, reason);
            let detail = format!("too many failed attempts; try again in {} minutes", wait.div_ceil(60));
            return Err(ApiError::Status(StatusCode::TOO_MANY_REQUESTS, detail));
        }
    }
    let user = state.store.user(&email).await?;
    let stored = user.as_ref().filter(|u| !u.disabled).and_then(|u| u.password_hash.clone());
    let pw = body.password;
    if !hashing(&state, move || password::verify(&pw, stored.as_deref())).await? {
        state.limiter.failed(&email);
        state.ip_limiter.failed(&address_key);
        sign_in_failed(&state, &email, &ip, "wrong_password");
        return Err(wrong());
    }
    state.limiter.succeeded(&email);
    let user = user.expect("verified against a stored hash");
    allowed(&state, &user.email, Method::Password).await.map_err(ApiError::forbidden)?;
    if user.two_factor {
        // the password was right: now the code, against a short-lived ticket
        let ticket = state.store.create_login_challenge(&user.email).await?;
        return Ok(Json(json!({"ok": true, "two_factor": true, "ticket": ticket})).into_response());
    }
    let jar = start_session(&state, jar, &user.email, "password").await?;
    Ok((jar, Json(json!({"ok": true, "must_change_password": user.must_change_password}))).into_response())
}

#[derive(Deserialize)]
pub struct SecondStepBody {
    ticket: String,
    code: String,
}

/// Sign-in, step 2: the authenticator code (or a recovery code) for the ticket from step 1.
pub async fn login_two_factor(
    State(state): State<Shared>,
    jar: CookieJar,
    headers: HeaderMap,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    Body(body): Body<SecondStepBody>,
) -> ApiResult<Response> {
    let wrong = || ApiError::Status(StatusCode::UNAUTHORIZED, "that code isn't right (or the sign-in expired)".into());
    let Some(email) = state.store.login_challenge(&body.ticket).await? else { return Err(wrong()) };
    let ip = client_ip(&state, &headers, peer);
    if let Some(wait) = state.limiter.locked(&email) {
        sign_in_failed(&state, &email, &ip, "account_locked");
        let detail = format!("too many failed attempts; try again in {} minutes", wait.div_ceil(60));
        return Err(ApiError::Status(StatusCode::TOO_MANY_REQUESTS, detail));
    }
    if !second_factor(&state, &email, &body.code).await? {
        state.limiter.failed(&email);
        sign_in_failed(&state, &email, &ip, "wrong_code");
        return Err(wrong());
    }
    state.limiter.succeeded(&email);
    state.store.end_login_challenge(&body.ticket).await?;
    allowed(&state, &email, Method::Password).await.map_err(ApiError::forbidden)?;
    let jar = start_session(&state, jar, &email, "password+totp").await?;
    Ok((jar, Json(json!({"ok": true}))).into_response())
}

/// Is `code` a fresh authenticator code, or an unused recovery code? (Uses it up.)
async fn second_factor(state: &AppState, email: &str, code: &str) -> ApiResult<bool> {
    let tf = state.store.two_factor(email).await?;
    let (Some(sealed), Some(key)) = (&tf.secret_sealed, &state.config.secret_key) else { return Ok(false) };
    let secret = key.open(sealed, &totp_context(email))?;
    let secret = secret_bytes(&secret)?;
    if let Some(step) = totp::verify(&secret, code, chrono::Utc::now().timestamp(), tf.last_step) {
        return Ok(state.store.use_two_factor_step(email, step).await?);
    }
    Ok(state.store.use_recovery_code(email, &totp::recovery_hash(code)).await?)
}

fn totp_context(email: &str) -> String {
    format!("{}/totp", email.to_lowercase())
}

/// Secrets are sealed as hex.
fn secret_bytes(hex_text: &str) -> ApiResult<Vec<u8>> {
    (0..hex_text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(hex_text.get(i..i + 2).unwrap_or("x"), 16))
        .collect::<Result<_, _>>()
        .map_err(|_| ApiError::Internal(anyhow::anyhow!("unreadable authenticator secret")))
}

/// The signed-in person (also while a new password or an authenticator must be set up).
async fn signed_in(state: &AppState, jar: &CookieJar) -> ApiResult<SessionInfo> {
    session_of(state, jar).await?.ok_or_else(ApiError::sign_in)
}

/// Set up an authenticator, step 1: a new secret, as a QR code (confirmed by `enable`).
pub async fn two_factor_setup(State(state): State<Shared>, jar: CookieJar) -> ApiResult<Json<Value>> {
    let session = signed_in(&state, &jar).await?;
    let Some(key) = &state.config.secret_key else {
        return Err(ApiError::Status(
            StatusCode::SERVICE_UNAVAILABLE,
            "authenticator codes need MAGNUM_SECRET_KEY on the server".into(),
        ));
    };
    if session.user.two_factor {
        return Err(ApiError::bad_request("an authenticator is already set up; switch it off first to replace it"));
    }
    let email = &session.user.email;
    let secret = totp::new_secret();
    let hex_text: String = secret.iter().map(|b| format!("{b:02x}")).collect();
    state.store.set_two_factor_pending(email, Some(&key.seal(&hex_text, &totp_context(email))?)).await?;
    let uri = totp::uri(&secret, email);
    Ok(Json(json!({"secret": totp::base32(&secret), "qr_svg": totp::qr_svg(&uri), "uri": uri})))
}

#[derive(Deserialize)]
pub struct CodeBody {
    code: String,
}

/// Step 2: the app's first code confirms it; recovery codes are shown once. Other sessions end;
/// this one continues with the second factor.
pub async fn two_factor_enable(
    State(state): State<Shared>,
    jar: CookieJar,
    Body(body): Body<CodeBody>,
) -> ApiResult<Response> {
    let session = signed_in(&state, &jar).await?;
    let email = session.user.email.clone();
    let tf = state.store.two_factor(&email).await?;
    let (Some(pending), Some(key)) = (&tf.pending_sealed, &state.config.secret_key) else {
        return Err(ApiError::bad_request("start setting up an authenticator first"));
    };
    let secret = secret_bytes(&key.open(pending, &totp_context(&email))?)?;
    let Some(step) = totp::verify(&secret, &body.code, chrono::Utc::now().timestamp(), None) else {
        return Err(ApiError::bad_request("that code isn't right: check the app's clock and try the current code"));
    };
    let (codes, hashes) = totp::recovery_codes();
    state.store.enable_two_factor(&email, pending, step, &hashes).await?;
    let method =
        if Method::of_session(&session.method) == Some(Method::Password) { "password+totp" } else { &session.method };
    let jar = start_session(&state, jar, &email, method).await?;
    state
        .usage
        .record(crate::usage::Event { user_email: Some(email), ..crate::usage::Event::new("two_factor_enabled") });
    Ok((jar, Json(json!({"ok": true, "recovery_codes": codes}))).into_response())
}

#[derive(Deserialize)]
pub struct DisableBody {
    password: String,
}

/// Switch the authenticator off (with the password), unless the organisation requires one.
pub async fn two_factor_disable(
    State(state): State<Shared>,
    jar: CookieJar,
    Body(body): Body<DisableBody>,
) -> ApiResult<Json<Value>> {
    let session = signed_in(&state, &jar).await?;
    let email = session.user.email.clone();
    let project = state.project.current();
    let (access, _) = state.access.get(&state, &project).await;
    if sign_in_policy(&access, &email, &offered(&state)).require_two_factor {
        return Err(ApiError::forbidden("your organisation requires an authenticator code"));
    }
    let stored = session.user.password_hash.clone();
    let pw = body.password;
    if !hashing(&state, move || password::verify(&pw, stored.as_deref())).await? {
        state.limiter.failed(&email);
        return Err(ApiError::bad_request("your password isn't right"));
    }
    state.store.disable_two_factor(&email).await?;
    state
        .usage
        .record(crate::usage::Event { user_email: Some(email), ..crate::usage::Event::new("two_factor_disabled") });
    Ok(Json(json!({"ok": true})))
}

// --------------------------------------------------------------------------- passkeys

fn relying_party(state: &AppState) -> ApiResult<&Webauthn> {
    state
        .passkeys
        .as_ref()
        .ok_or_else(|| ApiError::Status(StatusCode::SERVICE_UNAVAILABLE, "passkeys aren't available here".into()))
}

fn stored_passkeys(rows: &[(String, String, String, String, Option<String>)]) -> Vec<(String, Passkey)> {
    rows.iter().filter_map(|(id, _, json, _, _)| serde_json::from_str(json).ok().map(|p| (id.clone(), p))).collect()
}

/// Add a passkey to your account, step 1: the options for the browser.
pub async fn passkey_register_start(State(state): State<Shared>, jar: CookieJar) -> ApiResult<Json<Value>> {
    let session = signed_in(&state, &jar).await?;
    let rp = relying_party(&state)?;
    let email = session.user.email.clone();
    allowed(&state, &email, Method::Passkey).await.map_err(ApiError::forbidden)?;
    let id = uuid::Uuid::parse_str(&state.store.webauthn_id(&email).await?).map_err(anyhow::Error::from)?;
    let existing: Vec<_> =
        stored_passkeys(&state.store.passkeys(&email).await?).iter().map(|(_, p)| p.cred_id().clone()).collect();
    let (options, registration) = rp
        .start_passkey_registration(id, &email, &session.user.name, Some(existing))
        .map_err(|e| ApiError::bad_request(format!("can't start adding a passkey: {e}")))?;
    let ticket = state
        .store
        .put_webauthn_state(&email, "register", &serde_json::to_string(&registration).map_err(anyhow::Error::from)?)
        .await?;
    Ok(Json(json!({"ticket": ticket, "options": options})))
}

#[derive(Deserialize)]
pub struct PasskeyRegisterBody {
    ticket: String,
    #[serde(default)]
    name: String,
    credential: RegisterPublicKeyCredential,
}

/// Step 2: the browser's answer is checked and the passkey kept.
pub async fn passkey_register_finish(
    State(state): State<Shared>,
    jar: CookieJar,
    Body(body): Body<PasskeyRegisterBody>,
) -> ApiResult<Json<Value>> {
    let session = signed_in(&state, &jar).await?;
    let rp = relying_party(&state)?;
    let email = session.user.email.clone();
    let Some((owner, registration)) = state.store.take_webauthn_state(&body.ticket, "register").await? else {
        return Err(ApiError::bad_request("this passkey setup expired; please start again"));
    };
    if owner != email {
        return Err(ApiError::bad_request("this passkey setup belongs to another account"));
    }
    let registration: PasskeyRegistration = serde_json::from_str(&registration).map_err(anyhow::Error::from)?;
    let passkey = rp
        .finish_passkey_registration(&body.credential, &registration)
        .map_err(|e| ApiError::bad_request(format!("the passkey couldn't be added: {e}")))?;
    let id =
        serde_json::to_value(passkey.cred_id()).map_err(anyhow::Error::from)?.as_str().unwrap_or_default().to_string();
    let name: String = body.name.trim().chars().take(60).collect();
    let name = if name.is_empty() { "Passkey".to_string() } else { name };
    state.store.add_passkey(&email, &id, &name, &serde_json::to_string(&passkey).map_err(anyhow::Error::from)?).await?;
    state.usage.record(crate::usage::Event { user_email: Some(email), ..crate::usage::Event::new("passkey_added") });
    Ok(Json(json!({"ok": true, "id": id})))
}

/// Your passkeys (names and dates; never the keys).
pub async fn passkeys(State(state): State<Shared>, jar: CookieJar) -> ApiResult<Json<Value>> {
    let session = signed_in(&state, &jar).await?;
    let list: Vec<Value> = state
        .store
        .passkeys(&session.user.email)
        .await?
        .into_iter()
        .map(
            |(id, name, _, created, used)| json!({"id": id, "name": name, "created_at": created, "last_used_at": used}),
        )
        .collect();
    Ok(Json(json!({"passkeys": list, "available": state.passkeys.is_some()})))
}

pub async fn delete_passkey(
    State(state): State<Shared>,
    jar: CookieJar,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let session = signed_in(&state, &jar).await?;
    if !state.store.delete_passkey(&session.user.email, &id).await? {
        return Err(ApiError::not_found("no such passkey"));
    }
    Ok(Json(json!({"ok": true})))
}

/// Sign in with a passkey, step 1: options for the account's passkeys.
pub async fn passkey_login_start(State(state): State<Shared>, Body(body): Body<MethodsBody>) -> ApiResult<Json<Value>> {
    let rp = relying_party(&state)?;
    let email = body.email.trim().to_lowercase();
    allowed(&state, &email, Method::Passkey).await.map_err(ApiError::forbidden)?;
    let keys: Vec<Passkey> =
        stored_passkeys(&state.store.passkeys(&email).await?).into_iter().map(|(_, p)| p).collect();
    if keys.is_empty() {
        return Err(ApiError::bad_request("no passkey is set up for this account"));
    }
    let (options, authentication) =
        rp.start_passkey_authentication(&keys).map_err(|e| ApiError::bad_request(format!("can't start: {e}")))?;
    let ticket = state
        .store
        .put_webauthn_state(&email, "login", &serde_json::to_string(&authentication).map_err(anyhow::Error::from)?)
        .await?;
    Ok(Json(json!({"ticket": ticket, "options": options})))
}

#[derive(Deserialize)]
pub struct PasskeyLoginBody {
    ticket: String,
    credential: PublicKeyCredential,
}

/// Step 2: the device's signature, checked; a session.
pub async fn passkey_login_finish(
    State(state): State<Shared>,
    jar: CookieJar,
    headers: HeaderMap,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    Body(body): Body<PasskeyLoginBody>,
) -> ApiResult<Response> {
    let rp = relying_party(&state)?;
    let refused = || ApiError::Status(StatusCode::UNAUTHORIZED, "the passkey wasn't accepted; please try again".into());
    let Some((email, authentication)) = state.store.take_webauthn_state(&body.ticket, "login").await? else {
        return Err(refused());
    };
    let ip = client_ip(&state, &headers, peer);
    let authentication: PasskeyAuthentication = serde_json::from_str(&authentication).map_err(anyhow::Error::from)?;
    let result = match rp.finish_passkey_authentication(&body.credential, &authentication) {
        Ok(r) => r,
        Err(e) => {
            state.limiter.failed(&email);
            sign_in_failed(&state, &email, &ip, &format!("passkey: {e}"));
            return Err(refused());
        }
    };
    let rows = state.store.passkeys(&email).await?;
    if let Some((id, mut passkey)) = stored_passkeys(&rows).into_iter().find(|(_, p)| p.cred_id() == result.cred_id()) {
        passkey.update_credential(&result);
        state.store.used_passkey(&id, &serde_json::to_string(&passkey).map_err(anyhow::Error::from)?).await?;
    }
    let user = state.store.user(&email).await?.filter(|u| !u.disabled).ok_or_else(refused)?;
    allowed(&state, &user.email, Method::Passkey).await.map_err(ApiError::forbidden)?;
    state.limiter.succeeded(&email);
    let jar = start_session(&state, jar, &user.email, "passkey").await?;
    Ok((jar, Json(json!({"ok": true}))).into_response())
}

pub async fn logout(State(state): State<Shared>, jar: CookieJar) -> ApiResult<Response> {
    if let Some(c) = jar.get(COOKIE) {
        state.store.delete_session(c.value()).await?;
    }
    Ok((jar.remove(removal_cookie(COOKIE)), Json(json!({"ok": true}))).into_response())
}

#[derive(Deserialize)]
pub struct PasswordBody {
    #[serde(default)]
    current: Option<String>,
    new: String,
}

/// Set or change your own password. Allowed while a new password is required (that's the point).
pub async fn change_password(
    State(state): State<Shared>,
    jar: CookieJar,
    Body(body): Body<PasswordBody>,
) -> ApiResult<Response> {
    if !state.config.password_login {
        return Err(ApiError::not_found("password sign-in is switched off here"));
    }
    let Some(SessionInfo { user, method, .. }) = session_of(&state, &jar).await? else {
        return Err(ApiError::sign_in());
    };
    if let Some(wait) = state.limiter.locked(&user.email) {
        return Err(ApiError::Status(
            StatusCode::TOO_MANY_REQUESTS,
            format!("too many failed attempts; try again in {} minutes", wait.div_ceil(60)),
        ));
    }
    // a password, once set, is needed to change it (a Google sign-in may set the first one)
    if let Some(stored) = user.password_hash.clone() {
        let current = body.current.clone().unwrap_or_default();
        let ok = hashing(&state, move || password::verify(&current, Some(&stored))).await?;
        if !ok {
            state.limiter.failed(&user.email);
            return Err(ApiError::bad_request("your current password isn't right"));
        }
    }
    check_policy(&body.new, &user.email).map_err(ApiError::bad_request)?;
    if body.current.as_deref() == Some(body.new.as_str()) {
        return Err(ApiError::bad_request("choose a password different from the current one"));
    }
    let new = body.new;
    let hash = hashing(&state, move || password::hash(&new)).await??;
    // keeps the organisation binding: an admin-created account stays inside its organisation
    state.store.set_password(&user.email, None, &hash, user.owner_org.as_deref(), false).await?;
    // every session ended, this one too: start a fresh one
    let jar = start_session(&state, jar, &user.email, &method).await?;
    Ok((jar, Json(json!({"ok": true}))).into_response())
}

#[derive(Deserialize)]
pub struct NextParam {
    next: Option<String>,
}

/// Google sign-in, step 1: off to Google. The state is also kept in a short-lived cookie, so the
/// callback only completes in the browser that started it.
pub async fn google_login(
    State(state): State<Shared>,
    jar: CookieJar,
    headers: HeaderMap,
    UrlQuery(p): UrlQuery<NextParam>,
) -> ApiResult<Response> {
    let Some(google) = &state.google else { return Ok(login_failed("Google sign-in isn't configured here")) };
    let (st, nonce, verifier) = (random_token(24), random_token(24), random_token(48));
    let redirect_uri = format!("{}/api/auth/google/callback", public_base(&state.config, &headers));
    state.store.put_oauth_state(&st, &verifier, &nonce, &redirect_uri, &safe_next(p.next.as_deref()), None).await?;
    let cookie = Cookie::build((OAUTH_COOKIE, st.clone()))
        .path("/api/auth/google")
        .http_only(true)
        .same_site(SameSite::Lax) // must survive the top-level redirect back from Google
        .secure(is_https(&state.config, &headers))
        .max_age(cookie::time::Duration::minutes(10))
        .build();
    let url = google.authorize_url(&redirect_uri, &st, &nonce, &verifier);
    Ok((jar.add(cookie), Redirect::to(&url)).into_response())
}

#[derive(Deserialize)]
pub struct CallbackParams {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

/// Step 2: verify who this is, bind the email to the Google account, start a session.
pub async fn google_callback(
    State(state): State<Shared>,
    jar: CookieJar,
    UrlQuery(p): UrlQuery<CallbackParams>,
) -> ApiResult<Response> {
    let Some(google) = &state.google else { return Ok(login_failed("Google sign-in isn't configured here")) };
    let jar_without = jar.clone().remove(Cookie::build((OAUTH_COOKIE, "")).path("/api/auth/google").build());
    if let Some(e) = p.error {
        return Ok(login_failed(if e == "access_denied" { "Google sign-in was cancelled" } else { &e }));
    }
    let (Some(code), Some(st)) = (p.code, p.state) else { return Ok(login_failed("sign-in was cancelled")) };
    let lost = "your sign-in session was lost (cookies blocked, or Magnum BI is reachable under more than one address: set MAGNUM_PUBLIC_URL); please try again";
    if jar.get(OAUTH_COOKIE).map(|c| c.value()) != Some(st.as_str()) {
        return Ok(login_failed(lost));
    }
    let Some(OAuthState { verifier, nonce, redirect_uri, next, provider: None }) =
        state.store.take_oauth_state(&st).await?
    else {
        return Ok(login_failed("this sign-in link expired or was already used; please try again"));
    };
    let g = match google.finish(&code, &redirect_uri, &verifier, &nonce).await {
        Ok(g) => g,
        Err(e) => return Ok(login_failed(&e.0)),
    };
    let user = state.store.upsert_google_user(&g.email, &g.name, &g.sub).await?;
    if user.google_sub.as_deref() != Some(g.sub.as_str()) {
        return Ok(login_failed("this email is already linked to a different Google account"));
    }
    if user.disabled {
        return Ok(login_failed("this account is disabled"));
    }
    if let Err(e) = allowed(&state, &user.email, Method::Google).await {
        return Ok(login_failed(&e));
    }
    let jar = start_session(&state, jar_without, &user.email, "google").await?;
    Ok((jar, Redirect::to(&next)).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::access::AccessConfig;
    use crate::api::tests::{OWNER, app, call};

    /// An organisation `pol` (domain pol.example) with these sign-in rules, held in memory.
    fn settings(platform: Value, for_org: Value, org: Value) -> AccessConfig {
        AccessConfig::from_document(json!({
            "system_owners": [OWNER],
            "sign_in": platform,
            "organisations": {"pol": {"name": "Pol", "domains": ["pol.example"], "models": ["*"], "sign_in": for_org,
                                      "members": {"default_role": "user", "sign_in": org}}},
        }))
        .unwrap()
    }

    async fn person(state: &AppState) -> String {
        let email = format!("p{}@pol.example", &uuid::Uuid::new_v4().simple().to_string()[..10]);
        let hash = password::hash("a long enough passphrase").unwrap();
        state.store.set_password(&email, None, &hash, None, false).await.unwrap();
        email
    }

    /// A request that only needs a valid session (the test organisation has no database): 404
    /// (unknown model) once past the session checks.
    async fn probe(app: &axum::Router, cookie: &str) -> (StatusCode, Value) {
        let body = json!({"field": "d", "current": {"last_n_days": 7}});
        call(app, cookie, "POST", "/api/period?model=nope", Some(body)).await
    }

    async fn sign_in(app: &axum::Router, email: &str) -> (StatusCode, Value, Option<String>) {
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/api/auth/login")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(json!({"email": email, "password": "a long enough passphrase"}).to_string()))
            .unwrap();
        let resp = tower::ServiceExt::oneshot(app.clone(), req).await.unwrap();
        let status = resp.status();
        let cookie = resp
            .headers()
            .get_all("set-cookie")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .find(|v| v.starts_with("magnum_session="))
            .map(|v| v.split(';').next().unwrap().to_string());
        let bytes = http_body_util::BodyExt::collect(resp.into_body()).await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null), cookie)
    }

    fn base32_decode(s: &str) -> Vec<u8> {
        let (mut out, mut buffer, mut bits) = (vec![], 0u32, 0u32);
        for c in s.chars() {
            buffer = (buffer << 5) | "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567".find(c).unwrap() as u32;
            bits += 5;
            if bits >= 8 {
                bits -= 8;
                out.push((buffer >> bits) as u8);
            }
        }
        out
    }

    async fn post(app: &axum::Router, cookie: &str, path: &str, body: Value) -> (StatusCode, Value, Option<String>) {
        let req = axum::http::Request::builder()
            .method("POST")
            .uri(path)
            .header("cookie", cookie)
            .header("content-type", "application/json")
            .body(axum::body::Body::from(body.to_string()))
            .unwrap();
        let resp = tower::ServiceExt::oneshot(app.clone(), req).await.unwrap();
        let status = resp.status();
        let cookie = resp
            .headers()
            .get_all("set-cookie")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .find(|v| v.starts_with("magnum_session=") && !v.starts_with("magnum_session=;"))
            .map(|v| v.split(';').next().unwrap().to_string());
        let bytes = http_body_util::BodyExt::collect(resp.into_body()).await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null), cookie)
    }

    #[tokio::test]
    async fn authenticator_codes() {
        use crate::auth::totp;
        let Some((app, state, _)) = app().await else { return };
        let email = person(&state).await;
        state.access.pin(settings(Value::Null, Value::Null, Value::Null));
        let (_, _, cookie) = sign_in(&app, &email).await;
        let cookie = cookie.unwrap();

        // set up: a secret for the app, confirmed by its first code
        let (s, setup, _) = post(&app, &cookie, "/api/auth/two-factor/setup", json!({})).await;
        assert_eq!(s, StatusCode::OK, "{setup}");
        assert!(setup["uri"].as_str().unwrap().starts_with("otpauth://totp/"));
        assert!(setup["qr_svg"].as_str().unwrap().contains("<svg"));
        let secret = base32_decode(setup["secret"].as_str().unwrap());
        let now = chrono::Utc::now().timestamp();
        let (s, body, _) = post(&app, &cookie, "/api/auth/two-factor/enable", json!({"code": "000000x"})).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
        let first = totp::code(&secret, totp::step_at(now));
        let (s, enabled, cookie) = post(&app, &cookie, "/api/auth/two-factor/enable", json!({"code": first})).await;
        assert_eq!(s, StatusCode::OK, "{enabled}");
        let recovery: Vec<String> =
            enabled["recovery_codes"].as_array().unwrap().iter().map(|c| c.as_str().unwrap().to_string()).collect();
        assert_eq!(recovery.len(), totp::RECOVERY_CODES);
        let cookie = cookie.expect("a fresh session, now with the second factor");
        let (_, me) = call(&app, &cookie, "GET", "/api/auth/me", None).await;
        assert_eq!(me["two_factor"], json!(true), "{me}");

        // signing in now takes a code: the password alone gives a ticket, not a session
        let (s, step1, cookie1) = sign_in(&app, &email).await;
        assert_eq!(s, StatusCode::OK, "{step1}");
        assert!(cookie1.is_none());
        assert_eq!(step1["two_factor"], json!(true));
        let ticket = step1["ticket"].as_str().unwrap().to_string();
        let second = |code: &str| json!({"ticket": ticket, "code": code});
        let (s, _, _) = post(&app, "", "/api/auth/login/two-factor", second(&first)).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "a code is good once");
        let (s, body, signed_in) = post(&app, "", "/api/auth/login/two-factor", second(&recovery[0])).await;
        assert_eq!(s, StatusCode::OK, "{body}");
        let signed_in = signed_in.unwrap();
        assert_eq!(probe(&app, &signed_in).await.0, StatusCode::BAD_REQUEST, "signed in");
        let (s, _, _) = post(&app, "", "/api/auth/login/two-factor", second(&recovery[1])).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "a ticket is good once");

        // a recovery code works once; a ticket stops after a few wrong codes
        let (_, step1, _) = sign_in(&app, &email).await;
        let ticket = step1["ticket"].as_str().unwrap().to_string();
        let (s, _, _) =
            post(&app, "", "/api/auth/login/two-factor", json!({"ticket": ticket, "code": recovery[0]})).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "used already");
        for _ in 0..5 {
            post(&app, "", "/api/auth/login/two-factor", json!({"ticket": ticket, "code": "123456"})).await;
        }
        let (s, _, _) =
            post(&app, "", "/api/auth/login/two-factor", json!({"ticket": ticket, "code": recovery[2]})).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "too many attempts on this ticket");
        // wrong codes count like wrong passwords: the account is locked for a while
        assert_eq!(sign_in(&app, &email).await.0, StatusCode::TOO_MANY_REQUESTS);
        state.limiter.succeeded(&email); // (the lock running out)

        // required by the organisation: can't be switched off; otherwise with the password
        state.access.pin(settings(Value::Null, json!({"require_two_factor": true}), Value::Null));
        assert_eq!(probe(&app, &signed_in).await.0, StatusCode::BAD_REQUEST, "already has the second factor");
        let off = json!({"password": "a long enough passphrase"});
        let (s, body, _) = post(&app, &signed_in, "/api/auth/two-factor/disable", off.clone()).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{body}");
        state.access.pin(settings(Value::Null, Value::Null, Value::Null));
        let (s, _, _) = post(&app, &signed_in, "/api/auth/two-factor/disable", json!({"password": "wrong"})).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        let (s, body, _) = post(&app, &signed_in, "/api/auth/two-factor/disable", off).await;
        assert_eq!(s, StatusCode::OK, "{body}");
        let (s, body, cookie) = sign_in(&app, &email).await;
        assert!(s == StatusCode::OK && cookie.is_some(), "{body}");
    }

    /// A stand-in identity provider: discovery, token endpoint (checks the client secret and
    /// PKCE), signing keys. `identity` is what its next ID token says (email, groups, nonce).
    async fn mock_provider() -> (String, Arc<std::sync::Mutex<Value>>) {
        use axum::routing::{get, post};
        let identity = Arc::new(std::sync::Mutex::new(json!({})));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (b1, b2, id) = (base.clone(), base.clone(), identity.clone());
        let app = axum::Router::new()
            .route(
                "/.well-known/openid-configuration",
                get(move || async move {
                    axum::Json(json!({"issuer": b1, "authorization_endpoint": format!("{b1}/authorize"),
                        "token_endpoint": format!("{b1}/token"), "jwks_uri": format!("{b1}/jwks")}))
                }),
            )
            .route(
                "/jwks",
                get(|| async {
                    let jwks: Value =
                        serde_json::from_str(include_str!("../../tests/fixtures/test-sa-key.jwks.json")).unwrap();
                    axum::Json(jwks)
                }),
            )
            .route(
                "/token",
                post(move |axum::Form(f): axum::Form<std::collections::HashMap<String, String>>| {
                    let (base, id) = (b2.clone(), id.clone());
                    async move {
                        let good = f.get("client_secret").map(String::as_str) == Some("s3cret")
                            && f.get("client_id").map(String::as_str) == Some("magnum")
                            && f.get("code").map(String::as_str) == Some("the-code")
                            && f.get("code_verifier").is_some_and(|v| v.len() >= 43);
                        if !good {
                            return (StatusCode::BAD_REQUEST, axum::Json(json!({"error": "invalid_grant"})));
                        }
                        let mut claims = id.lock().unwrap().clone();
                        let now = chrono::Utc::now().timestamp();
                        claims["iss"] = json!(base);
                        claims["aud"] = json!("magnum");
                        claims["sub"] = json!("user-1");
                        claims["iat"] = json!(now);
                        claims["exp"] = json!(now + 300);
                        let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
                        header.kid = Some("test-1".into());
                        let key = jsonwebtoken::EncodingKey::from_rsa_pem(include_bytes!(
                            "../../tests/fixtures/test-sa-key.pem"
                        ))
                        .unwrap();
                        let token = jsonwebtoken::encode(&header, &claims, &key).unwrap();
                        (StatusCode::OK, axum::Json(json!({"id_token": token, "token_type": "Bearer"})))
                    }
                }),
            );
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (base, identity)
    }

    async fn get_raw(app: &axum::Router, cookie: &str, path: &str) -> (StatusCode, String, Vec<String>) {
        let req =
            axum::http::Request::builder().uri(path).header("cookie", cookie).body(axum::body::Body::empty()).unwrap();
        let resp = tower::ServiceExt::oneshot(app.clone(), req).await.unwrap();
        let location = resp.headers().get("location").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
        let cookies = resp
            .headers()
            .get_all("set-cookie")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .map(|v| v.split(';').next().unwrap().to_string())
            .filter(|v| !v.ends_with('='))
            .collect();
        (resp.status(), location, cookies)
    }

    #[tokio::test]
    async fn organisations_sign_in_with_their_own_provider() {
        let Some((app, state, _)) = app().await else { return };
        let (issuer, identity) = mock_provider().await;
        let sealed = state.config.secret_key.as_ref().unwrap().seal("s3cret", "pol/oidc").unwrap();
        let provider = json!({"label": "Pol (Entra ID)", "issuer": issuer, "client_id": "magnum",
                              "client_secret_sealed": sealed, "group_roles": {"BI-Analysts": "power_user"}});
        let with = |org_policy: Value| {
            let mut c = settings(Value::Null, Value::Null, org_policy);
            c.members.get_mut("pol").unwrap().oidc = Some(serde_json::from_value(provider.clone()).unwrap());
            c
        };
        state.access.pin(with(Value::Null));
        let email = format!("ana{}@pol.example", &uuid::Uuid::new_v4().simple().to_string()[..8]);

        // the sign-in page learns the organisation's provider from the email
        let (_, m) = call(&app, "", "POST", "/api/auth/methods", Some(json!({"email": email}))).await;
        assert!(m["methods"].as_array().unwrap().contains(&json!("oidc")), "{m}");
        assert_eq!(m["oidc"]["label"], json!("Pol (Entra ID)"));
        let start = m["oidc"]["url"].as_str().unwrap().to_string();
        assert_eq!(start, "/api/auth/oidc/pol/login");

        // off to the provider, and back with a code
        let run = |claims: Value, next: &'static str| {
            let (app, identity, start, issuer) = (app.clone(), identity.clone(), start.clone(), issuer.clone());
            async move {
                let (s, location, cookies) = get_raw(&app, "", &format!("{start}?next={next}")).await;
                assert_eq!(s, StatusCode::SEE_OTHER, "{location}");
                assert!(location.starts_with(&format!("{issuer}/authorize?")), "{location}");
                let url = url::Url::parse(&location).unwrap();
                let q: std::collections::HashMap<String, String> = url.query_pairs().into_owned().collect();
                assert_eq!(q["code_challenge_method"], "S256");
                let mut claims = claims;
                if claims.get("nonce").is_none() {
                    claims["nonce"] = json!(q["nonce"]);
                }
                *identity.lock().unwrap() = claims;
                let cookie = cookies.join("; ");
                get_raw(&app, &cookie, &format!("/api/auth/oidc/callback?code=the-code&state={}", q["state"])).await
            }
        };
        let (s, location, cookies) =
            run(json!({"email": email, "name": "Ana", "groups": ["Staff", "BI-Analysts"]}), "/dashboards/x").await;
        assert_eq!((s, location.as_str()), (StatusCode::SEE_OTHER, "/dashboards/x"));
        let session = cookies.iter().find(|c| c.starts_with("magnum_session=")).cloned().expect("signed in");
        let (_, me) = call(&app, &session, "GET", "/api/auth/me", None).await;
        assert_eq!(
            (me["user"]["email"].as_str(), me["role"].as_str()),
            (Some(email.as_str()), Some("power_user")),
            "{me}"
        );
        assert_eq!(me["method"], json!("oidc"));

        // only the organisation's own domains, never a platform account, never a forged answer
        for (claims, why) in [
            (json!({"email": "mallory@else.example"}), "pol.example"),
            (json!({"email": OWNER}), "pol.example"),
            (json!({"email": email, "nonce": "forged"}), "browser"),
            (json!({"email": email, "email_verified": false}), "verified"),
        ] {
            let (s, location, cookies) = run(claims.clone(), "/").await;
            assert_eq!(s, StatusCode::SEE_OTHER);
            assert!(location.starts_with("/login?error="), "{claims}: {location}");
            let msg = url::form_urlencoded::parse(location.split_once('?').unwrap().1.as_bytes())
                .find(|(k, _)| k == "error")
                .map(|(_, v)| v.into_owned())
                .unwrap();
            assert!(msg.contains(why), "{claims}: {msg}");
            assert!(!cookies.iter().any(|c| c.starts_with("magnum_session=")), "{claims}");
        }

        // the organisation (or the platform) doesn't allow single sign-on: no way in through it
        state.access.pin(with(json!({"methods": ["password"]})));
        let (_, m) = call(&app, "", "POST", "/api/auth/methods", Some(json!({"email": email}))).await;
        assert!(m["oidc"].is_null(), "{m}");
        let (s, location, _) = get_raw(&app, "", &start).await;
        assert_eq!(s, StatusCode::SEE_OTHER);
        assert!(location.starts_with("/login?error="), "{location}");
        let (s, _) = call(&app, &session, "GET", "/api/auth/me", None).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(probe(&app, &session).await.0, StatusCode::UNAUTHORIZED, "existing single sign-on sessions end");
    }

    #[tokio::test]
    async fn org_admins_set_up_sign_in() {
        let Some((app, state, owner)) = app().await else { return };
        let (issuer, _) = mock_provider().await;
        state.access.pin(settings(json!({"methods": ["password", "oidc", "passkey"]}), Value::Null, Value::Null));
        let req = |method: &str, body: Option<Value>| {
            let mut r = axum::http::Request::builder()
                .method(method)
                .uri("/api/members/sign-in")
                .header("cookie", owner.clone())
                .header("x-magnum-org", "pol");
            if body.is_some() {
                r = r.header("content-type", "application/json");
            }
            r.body(body.map_or(axum::body::Body::empty(), |b| axum::body::Body::from(b.to_string()))).unwrap()
        };
        let send = |r: axum::http::Request<axum::body::Body>| {
            let app = app.clone();
            async move {
                let resp = tower::ServiceExt::oneshot(app, r).await.unwrap();
                let status = resp.status();
                let bytes = http_body_util::BodyExt::collect(resp.into_body()).await.unwrap().to_bytes();
                (status, serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null))
            }
        };

        let (s, got) = send(req("GET", None)).await;
        assert_eq!(s, StatusCode::OK, "{got}");
        assert_eq!(got["platform"]["methods"], json!(["password", "oidc", "passkey"]), "allowed here: {got}");
        assert!(got["callback_url"].as_str().unwrap().ends_with("/api/auth/oidc/callback"));

        let provider = json!({"label": "Pol (Keycloak)", "issuer": issuer, "client_id": "magnum", "client_secret": "s3cret",
                              "group_roles": {"BI-Admins": "org_admin"}});
        let (s, body) = send(req("PUT", Some(json!({"sign_in": {"methods": ["oidc"]}, "oidc": provider})))).await;
        assert_eq!(s, StatusCode::OK, "{body}");
        assert_eq!(body["oidc"]["has_secret"], json!(true), "{body}");
        assert!(
            !body.to_string().contains("s3cret") && !body.to_string().contains("client_secret_sealed"),
            "write-only"
        );
        let saved = state.store.setting("members:pol").await.unwrap().unwrap();
        let sealed = saved["oidc"]["client_secret_sealed"].as_str().unwrap();
        assert_eq!(state.config.secret_key.as_ref().unwrap().open(sealed, "pol/oidc").unwrap(), "s3cret");

        // refused: an unreachable provider; nobody left a way in
        let (s, body) = send(req(
            "PUT",
            Some(json!({"sign_in": {"methods": ["oidc"]}, "oidc": {"label": "x", "issuer": "http://127.0.0.1:1", "client_id": "c"}})),
        ))
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
        let (s, body) = send(req("PUT", Some(json!({"sign_in": {"methods": ["oidc"]}, "oidc": null})))).await;
        assert!(s == StatusCode::BAD_REQUEST && body["detail"].as_str().unwrap().contains("nobody"), "{body}");

        // saving the member list keeps the sign-in settings
        state.access.pin({
            let mut c = settings(json!({"methods": ["password", "oidc", "passkey"]}), Value::Null, Value::Null);
            c.members.insert("pol".into(), serde_json::from_value(saved.clone()).unwrap());
            c
        });
        // the client secret never goes to another provider on its own: a new issuer needs it again
        let moved = json!({"label": "x", "issuer": "https://login.elsewhere.example", "client_id": "magnum"});
        let (s, body) = send(req("PUT", Some(json!({"sign_in": {"methods": ["oidc"]}, "oidc": moved})))).await;
        assert!(s == StatusCode::BAD_REQUEST && body["detail"].as_str().unwrap().contains("again"), "{body}");
        let members = json!({"default_role": "viewer", "users": {}});
        let put = axum::http::Request::builder()
            .method("PUT")
            .uri("/api/members")
            .header("cookie", owner.clone())
            .header("x-magnum-org", "pol")
            .header("content-type", "application/json");
        let (s, body) = send(put.body(axum::body::Body::from(members.to_string())).unwrap()).await;
        assert_eq!(s, StatusCode::OK, "{body}");
        assert!(
            body["members"].get("oidc").is_none() || body["members"]["oidc"]["client_secret_sealed"].is_null(),
            "{body}"
        );
        let after = state.store.setting("members:pol").await.unwrap().unwrap();
        assert_eq!(after["oidc"], saved["oidc"], "kept");
        assert_eq!(after["sign_in"], saved["sign_in"], "kept");
        state.store.put_settings(&[("members:pol".into(), None)], None).await.unwrap();
    }

    #[tokio::test]
    async fn passkeys() {
        use webauthn_authenticator_rs::WebauthnAuthenticator;
        use webauthn_authenticator_rs::softpasskey::SoftPasskey;
        use webauthn_rs::prelude::{CreationChallengeResponse, RequestChallengeResponse};
        let Some((app, state, _)) = app().await else { return };
        let email = person(&state).await;
        state.access.pin(settings(Value::Null, Value::Null, Value::Null));
        let (_, _, cookie) = sign_in(&app, &email).await;
        let cookie = cookie.unwrap();
        // no MAGNUM_PUBLIC_URL: the development origins
        let origin = url::Url::parse("http://localhost:8000").unwrap();
        let mut device = WebauthnAuthenticator::new(SoftPasskey::new(true));

        // add a passkey to the account
        let (s, start, _) = post(&app, &cookie, "/api/auth/passkeys/register/start", json!({})).await;
        assert_eq!(s, StatusCode::OK, "{start}");
        let options: CreationChallengeResponse = serde_json::from_value(start["options"].clone()).unwrap();
        let credential = device.do_registration(origin.clone(), options).unwrap();
        let finish = json!({"ticket": start["ticket"], "name": "Laptop", "credential": credential});
        let (s, body, _) = post(&app, &cookie, "/api/auth/passkeys/register/finish", finish).await;
        assert_eq!(s, StatusCode::OK, "{body}");
        let (_, list) = call(&app, &cookie, "GET", "/api/auth/passkeys", None).await;
        assert_eq!(list["passkeys"][0]["name"], json!("Laptop"), "{list}");
        let id = list["passkeys"][0]["id"].as_str().unwrap().to_string();
        let (_, m) = call(&app, "", "POST", "/api/auth/methods", Some(json!({"email": email}))).await;
        assert!(m["methods"].as_array().unwrap().contains(&json!("passkey")), "{m}");

        // sign in with it
        let login = |device: &mut WebauthnAuthenticator<SoftPasskey>, origin: &url::Url, start: &Value| {
            let options: RequestChallengeResponse = serde_json::from_value(start["options"].clone()).unwrap();
            json!({"ticket": start["ticket"], "credential": device.do_authentication(origin.clone(), options).unwrap()})
        };
        let (s, start, _) = post(&app, "", "/api/auth/passkeys/login/start", json!({"email": email})).await;
        assert_eq!(s, StatusCode::OK, "{start}");
        let answer = login(&mut device, &origin, &start);
        let (s, body, session) = post(&app, "", "/api/auth/passkeys/login/finish", answer.clone()).await;
        assert_eq!(s, StatusCode::OK, "{body}");
        let (_, me) = call(&app, &session.unwrap(), "GET", "/api/auth/me", None).await;
        assert_eq!(me["method"], json!("passkey"));
        let (s, _, _) = post(&app, "", "/api/auth/passkeys/login/finish", answer).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "a ticket is good once");

        // an answer made on a page the server doesn't trust doesn't count (browsers already refuse
        // other domains; this is the same domain, another origin)
        let (_, start, _) = post(&app, "", "/api/auth/passkeys/login/start", json!({"email": email})).await;
        let phished = login(&mut device, &url::Url::parse("https://localhost:9999").unwrap(), &start);
        let (s, _, session) = post(&app, "", "/api/auth/passkeys/login/finish", phished).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        assert!(session.is_none());

        // the organisation switches passkeys off
        state.access.pin(settings(Value::Null, Value::Null, json!({"methods": ["password"]})));
        let (s, body, _) = post(&app, "", "/api/auth/passkeys/login/start", json!({"email": email})).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{body}");
        let (s, _, _) = post(&app, &cookie, "/api/auth/passkeys/register/start", json!({})).await;
        assert_eq!(s, StatusCode::FORBIDDEN);

        // removed: no way in with it
        state.access.pin(settings(Value::Null, Value::Null, Value::Null));
        let (s, _) = call(&app, &cookie, "DELETE", &format!("/api/auth/passkeys/{id}"), None).await;
        assert_eq!(s, StatusCode::OK);
        let (s, body, _) = post(&app, "", "/api/auth/passkeys/login/start", json!({"email": email})).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
    }

    #[tokio::test]
    async fn sign_in_follows_the_policies() {
        let Some((app, state, _)) = app().await else { return };
        let email = person(&state).await;
        state.access.pin(settings(json!({"methods": ["password", "oidc"]}), Value::Null, Value::Null));

        // which ways in, by email (before any password)
        let (s, m) = call(&app, "", "POST", "/api/auth/methods", Some(json!({"email": email}))).await;
        assert_eq!(s, StatusCode::OK, "{m}");
        assert_eq!(m["methods"], json!(["password"]), "single sign-on isn't set up yet: {m}");

        let (s, body, cookie) = sign_in(&app, &email).await;
        assert_eq!(s, StatusCode::OK, "{body}");
        let cookie = cookie.unwrap();
        assert_eq!(probe(&app, &cookie).await.0, StatusCode::BAD_REQUEST, "signed in (unknown model)");

        // the organisation now requires an authenticator code: only setting one up is allowed
        state.access.pin(settings(
            json!({"methods": ["password", "oidc"]}),
            json!({"require_two_factor": true}),
            Value::Null,
        ));
        let (s, body) = probe(&app, &cookie).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{body}");
        assert_eq!(body["two_factor_setup"], json!(true), "{body}");
        let (_, me) = call(&app, &cookie, "GET", "/api/auth/me", None).await;
        assert_eq!(me["two_factor_setup"], json!(true), "{me}");

        // the organisation switches passwords off: signed-in sessions end, password sign-in stops
        state.access.pin(settings(json!({"methods": ["password", "oidc"]}), Value::Null, json!({"methods": ["oidc"]})));
        let (s, body) = probe(&app, &cookie).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "{body}");
        assert!(body["detail"].as_str().unwrap().contains("password"), "{body}");
        let (_, me) = call(&app, &cookie, "GET", "/api/auth/me", None).await;
        assert_eq!(me["authenticated"], json!(false));
        let (s, body, cookie) = sign_in(&app, &email).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{body}");
        assert!(cookie.is_none());
        assert!(body["detail"].as_str().unwrap().contains("single sign-on"), "{body}");

        // an organisation's rules can't loosen the platform's
        state.access.pin(settings(
            json!({"methods": ["oidc", "passkey"]}),
            Value::Null,
            json!({"methods": ["password"]}),
        ));
        assert_eq!(sign_in(&app, &email).await.0, StatusCode::FORBIDDEN);
        // someone unknown: the platform's rules (nothing about organisations is revealed)
        let (_, m) = call(&app, "", "POST", "/api/auth/methods", Some(json!({"email": "x@else.example"}))).await;
        assert!(m["methods"].as_array().unwrap().iter().all(|x| x != "password"), "{m}");
    }

    #[test]
    fn next_urls_stay_on_site() {
        assert_eq!(safe_next(Some("/dashboards/x?y=1")), "/dashboards/x?y=1");
        for bad in ["//evil.example", "https://evil.example", "javascript:alert(1)", "/\\evil.example"] {
            assert_eq!(safe_next(Some(bad)), "/");
        }
        assert_eq!(safe_next(None), "/");
    }
}
