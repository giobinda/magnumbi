//! Sign-in endpoints: who am I, password sign-in, Google sign-in, sign-out, change password.

use axum::Json;
use axum::extract::{Query as UrlQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum_extra::extract::CookieJar;
use axum_extra::extract::cookie::{Cookie, SameSite};
use chrono::Duration;
use serde::Deserialize;
use serde_json::{Value, json};

use super::Body;
use crate::access::{ROLES, Role, describe_restrictions, effective_role};
use crate::auth::password::{self, check_policy};
use crate::auth::{COOKIE, ORG_HEADER, VIEW_AS_HEADER, removal_cookie, scope_for, session_cookie, session_of};
use crate::error::{ApiError, ApiResult};
use crate::security::{is_https, public_base};
use crate::state::{AppState, Shared};
use crate::store::{SessionInfo, random_token};

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
    let ttl = Duration::hours(state.config.session_hours);
    let token = state.store.create_session(email, method, ttl).await?;
    state.store.record_login(email).await?;
    tracing::info!(method, "signed in");
    Ok(jar.add(session_cookie(state, token)))
}

pub async fn me(State(state): State<Shared>, jar: CookieJar, headers: HeaderMap) -> ApiResult<Json<Value>> {
    let session = session_of(&state, &jar).await?;
    let methods = json!({"password": state.config.password_login, "google": state.google.is_some()});
    let Some(session) = session else {
        return Ok(Json(json!({"required": true, "authenticated": false, "user": null, "methods": methods})));
    };
    let mut out = json!({
        "required": true,
        "authenticated": true,
        "user": {"email": session.user.email, "name": session.user.name},
        "methods": methods,
        "method": session.method,
        "has_password": session.user.password_hash.is_some(),
        "must_change_password": session.user.must_change_password,
    });
    if session.user.must_change_password {
        return Ok(Json(out)); // nothing else until a new password is chosen
    }
    let project = state.project.current();
    let (access, _) = state.access.get(&state, &project).await;
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

#[derive(Deserialize)]
pub struct LoginBody {
    email: String,
    password: String,
}

pub async fn login(State(state): State<Shared>, jar: CookieJar, Body(body): Body<LoginBody>) -> ApiResult<Response> {
    if !state.config.password_login {
        return Err(ApiError::not_found("password sign-in is switched off here"));
    }
    let email = body.email.trim().to_lowercase();
    if email.len() > 320 || body.password.len() > 4 * password::MAX_LENGTH {
        return Err(ApiError::Status(StatusCode::UNAUTHORIZED, "wrong email or password".into()));
    }
    if let Some(wait) = state.limiter.locked(&email) {
        let detail = format!("too many failed attempts; try again in {} minutes", wait.div_ceil(60));
        return Err(ApiError::Status(StatusCode::TOO_MANY_REQUESTS, detail));
    }
    let user = state.store.user(&email).await?;
    let stored = user.as_ref().filter(|u| !u.disabled).and_then(|u| u.password_hash.clone());
    let pw = body.password;
    let ok = tokio::task::spawn_blocking(move || password::verify(&pw, stored.as_deref()))
        .await
        .map_err(anyhow::Error::from)?;
    if !ok {
        state.limiter.failed(&email);
        return Err(ApiError::Status(StatusCode::UNAUTHORIZED, "wrong email or password".into()));
    }
    state.limiter.succeeded(&email);
    let user = user.expect("verified against a stored hash");
    let jar = start_session(&state, jar, &user.email, "password").await?;
    Ok((jar, Json(json!({"ok": true, "must_change_password": user.must_change_password}))).into_response())
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
        let ok = tokio::task::spawn_blocking(move || password::verify(&current, Some(&stored)))
            .await
            .map_err(anyhow::Error::from)?;
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
    let hash = tokio::task::spawn_blocking(move || password::hash(&new)).await.map_err(anyhow::Error::from)??;
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
    state.store.put_oauth_state(&st, &verifier, &nonce, &redirect_uri, &safe_next(p.next.as_deref())).await?;
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
    let Some((verifier, nonce, redirect_uri, next)) = state.store.take_oauth_state(&st).await? else {
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
    let jar = start_session(&state, jar_without, &user.email, "google").await?;
    Ok((jar, Redirect::to(&next)).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_urls_stay_on_site() {
        assert_eq!(safe_next(Some("/dashboards/x?y=1")), "/dashboards/x?y=1");
        for bad in ["//evil.example", "https://evil.example", "javascript:alert(1)", "/\\evil.example"] {
            assert_eq!(safe_next(Some(bad)), "/");
        }
        assert_eq!(safe_next(None), "/");
    }
}
