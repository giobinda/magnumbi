//! API tokens: agents and scripts acting for the person who made them, in one organisation,
//! with at most that person's role, read-only or read-write, for a limited time.
//!
//! Made, listed and revoked by people signed in (a token can't make tokens). Org admins see and
//! can revoke every token of their organisation. Tokens are a sign-in method in the policies
//! (auth::policy), so the platform or an organisation can switch them off.

use axum::Json;
use axum::extract::{Path, Query as UrlQuery, State};
use chrono::{Duration, Utc};
use serde::Deserialize;
use serde_json::{Value, json};

use super::Body;
use crate::access::{Role, sign_in_policy};
use crate::auth::policy::Method;
use crate::auth::{Auth, Caller, offered};
use crate::error::{ApiError, ApiResult};
use crate::state::Shared;
use crate::store::random_token;

pub const DEFAULT_DAYS: i64 = 90;
pub const MAX_DAYS: i64 = 365;

fn people_only(caller: &Caller) -> ApiResult<()> {
    if caller.session.token.is_some() {
        return Err(ApiError::forbidden("API tokens are managed by people signed in, not by tokens"));
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewToken {
    name: String,
    /// "read" (reading and queries) or "write"
    scope: String,
    /// at most your own role (default: yours)
    #[serde(default)]
    role: Option<Role>,
    #[serde(default)]
    expires_in_days: Option<i64>,
}

/// A new token, shown once.
pub async fn create(
    State(state): State<Shared>,
    Auth(caller): Auth,
    Body(t): Body<NewToken>,
) -> ApiResult<Json<Value>> {
    people_only(&caller)?;
    caller.require("view")?;
    let actual = caller.scope.role.ok_or_else(|| ApiError::forbidden("you're not a member of any organisation"))?;
    if !sign_in_policy(&caller.access, caller.email(), &offered(&state)).allows(Method::Token) {
        return Err(ApiError::forbidden("API tokens aren't allowed for your organisation"));
    }
    let name: String = t.name.trim().chars().take(60).collect();
    if name.is_empty() {
        return Err(ApiError::bad_request("give the token a name (what will use it?)"));
    }
    let write = match t.scope.as_str() {
        "read" => false,
        "write" => true,
        _ => return Err(ApiError::bad_request("scope: read or write")),
    };
    let role = t.role.unwrap_or(actual);
    if role > actual {
        return Err(ApiError::forbidden("a token can't have a higher role than yours"));
    }
    let days = t.expires_in_days.unwrap_or(DEFAULT_DAYS);
    if !(1..=MAX_DAYS).contains(&days) {
        return Err(ApiError::bad_request(format!("expires_in_days: between 1 and {MAX_DAYS}")));
    }
    let expires = Utc::now() + Duration::days(days);
    let token = format!("mbi_{}", random_token(32));
    let id =
        state.store.create_token(&token, &name, caller.email(), caller.org(), role.as_str(), write, expires).await?;
    state.usage.record(crate::usage::Event {
        detail: Some(json!({"token": name, "role": role.as_str(), "write": write})),
        ..caller.event("token_created")
    });
    Ok(Json(json!({"id": id, "token": token, "name": name, "role": role.as_str(),
                    "scope": t.scope, "expires_at": expires.to_rfc3339()})))
}

#[derive(Deserialize)]
pub struct ListParams {
    #[serde(default)]
    all: bool,
}

/// Your tokens here; with ?all=true, Org admins see everyone's in the organisation.
pub async fn list(
    State(state): State<Shared>,
    Auth(caller): Auth,
    UrlQuery(p): UrlQuery<ListParams>,
) -> ApiResult<Json<Value>> {
    people_only(&caller)?;
    caller.require("view")?;
    let everyone = p.all && caller.can("manage_members");
    let tokens = state.store.tokens(caller.org(), (!everyone).then_some(caller.email())).await?;
    Ok(Json(json!({"tokens": tokens, "default_days": DEFAULT_DAYS, "max_days": MAX_DAYS})))
}

/// Revoke one of your tokens (Org admins: any of the organisation's).
pub async fn revoke(State(state): State<Shared>, Auth(caller): Auth, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    people_only(&caller)?;
    caller.require("view")?;
    let only = (!caller.can("manage_members")).then_some(caller.email());
    if !state.store.revoke_token(caller.org(), &id, only).await? {
        return Err(ApiError::not_found("no such token"));
    }
    state.usage.record(crate::usage::Event { detail: Some(json!({"token_id": id})), ..caller.event("token_revoked") });
    Ok(Json(json!({"ok": true})))
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use serde_json::{Value, json};

    use crate::api::tests::{app, call};

    async fn bearer(
        app: &axum::Router,
        token: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut req =
            axum::http::Request::builder().method(method).uri(path).header("authorization", format!("Bearer {token}"));
        if body.is_some() {
            req = req.header("content-type", "application/json");
        }
        let req = req.body(body.map_or(axum::body::Body::empty(), |b| axum::body::Body::from(b.to_string()))).unwrap();
        let resp = tower::ServiceExt::oneshot(app.clone(), req).await.unwrap();
        let status = resp.status();
        let bytes = http_body_util::BodyExt::collect(resp.into_body()).await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    #[tokio::test]
    async fn tokens_act_for_their_maker_within_limits() {
        let Some((app, state, cookie)) = app().await else { return };
        // made by someone signed in (here the system owner, in the platform workspace)
        let (s, made) =
            call(&app, &cookie, "POST", "/api/tokens", Some(json!({"name": "my agent", "scope": "write"}))).await;
        assert_eq!(s, StatusCode::OK, "{made}");
        let token = made["token"].as_str().unwrap().to_string();
        assert!(token.starts_with("mbi_") && token.len() > 40, "{token}");
        assert!(made["expires_at"].is_string(), "every token expires");

        // the agent: who am I, and work
        let (s, me) = bearer(&app, &token, "GET", "/api/auth/me", None).await;
        assert_eq!(s, StatusCode::OK, "{me}");
        assert_eq!((me["method"].as_str(), me["role"].as_str()), (Some("token"), Some("system_owner")), "{me}");
        assert_eq!(bearer(&app, &token, "GET", "/api/models", None).await.0, StatusCode::OK);
        // listed for its maker (never the secret again)
        let (_, list) = call(&app, &cookie, "GET", "/api/tokens", None).await;
        let mine = list["tokens"].as_array().unwrap().iter().find(|t| t["id"] == made["id"]).cloned().unwrap();
        assert_eq!(mine["name"], json!("my agent"));
        assert!(mine["last_used_at"].is_string(), "{mine}");
        assert!(!list.to_string().contains(&token));

        // tokens can't make tokens, nor manage how anyone signs in
        let (s, _) = bearer(&app, &token, "POST", "/api/tokens", Some(json!({"name": "more", "scope": "write"}))).await;
        assert_eq!(s, StatusCode::FORBIDDEN);
        for path in ["/api/auth/two-factor/setup", "/api/auth/passkeys/register/start"] {
            assert_eq!(bearer(&app, &token, "POST", path, Some(json!({}))).await.0, StatusCode::UNAUTHORIZED, "{path}");
        }

        // read-only: reading and querying, no changes
        let (_, ro) = call(
            &app,
            &cookie,
            "POST",
            "/api/tokens",
            Some(json!({"name": "reader", "scope": "read", "role": "viewer"})),
        )
        .await;
        let ro = ro["token"].as_str().unwrap().to_string();
        assert_eq!(bearer(&app, &ro, "GET", "/api/dashboards", None).await.0, StatusCode::OK);
        let (s, body) = bearer(&app, &ro, "PUT", "/api/dashboards/x/yaml", Some(json!("title: x"))).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{body}");
        assert!(body["detail"].as_str().unwrap().contains("read-only"), "{body}");
        // and its role: viewers don't explore
        let (s, _) =
            bearer(&app, &ro, "POST", "/api/query", Some(json!({"model": "sales", "measures": ["revenue"]}))).await;
        assert_eq!(s, StatusCode::FORBIDDEN);
        // never above its maker's role
        let (s, _) =
            call(&app, &cookie, "POST", "/api/tokens", Some(json!({"name": "x", "scope": "read", "role": "nonsense"})))
                .await;
        assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);

        // revoked, expired, or no longer allowed: refused
        let (s, _) =
            call(&app, &cookie, "DELETE", &format!("/api/tokens/{}", made["id"].as_str().unwrap()), None).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(bearer(&app, &token, "GET", "/api/models", None).await.0, StatusCode::UNAUTHORIZED);
        let c = state.store.pool.get().await.unwrap();
        c.execute("UPDATE api_tokens SET expires_at = now() - INTERVAL '1 hour' WHERE name = 'reader'", &[])
            .await
            .unwrap();
        assert_eq!(bearer(&app, &ro, "GET", "/api/models", None).await.0, StatusCode::UNAUTHORIZED);
        let (_, ok) =
            call(&app, &cookie, "POST", "/api/tokens", Some(json!({"name": "policy", "scope": "read"}))).await;
        let ok = ok["token"].as_str().unwrap().to_string();
        let mut cfg = (*state.access.get(&state, &state.project.current()).await.0).clone();
        cfg.platform.sign_in = serde_json::from_value(json!({"methods": ["password"]})).unwrap();
        state.access.pin(cfg);
        assert_eq!(bearer(&app, &ok, "GET", "/api/models", None).await.0, StatusCode::UNAUTHORIZED);
        assert_eq!(bearer(&app, "mbi_nonsense", "GET", "/api/models", None).await.0, StatusCode::UNAUTHORIZED);
        // the test's own tokens, gone
        for t in [&token, &ro, &ok] {
            let hash = crate::store::sha256_hex(t);
            c.execute("DELETE FROM api_tokens WHERE token_hash = $1", &[&hash]).await.unwrap();
        }
    }
}
