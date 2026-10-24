//! GET/PUT /api/tour: whether to offer a person the first-time tour (once per organisation:
//! roles, and so the tour, differ between them).

use axum::Json;
use axum::extract::State;
use serde::Deserialize;
use serde_json::{Value, json};

use super::Body;
use crate::auth::Auth;
use crate::error::ApiResult;
use crate::state::Shared;

/// `offer`: they haven't taken it, skipped it or said "never" yet.
pub async fn get(State(state): State<Shared>, Auth(caller): Auth) -> ApiResult<Json<Value>> {
    caller.require("view")?;
    let seen = state.orgs.store(caller.org()).await?.tour_seen(caller.org(), caller.email()).await?;
    Ok(Json(json!({"offer": !seen})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Seen {
    seen: bool,
}

/// Taken, skipped or declined (`seen: true`), or to be offered again (`false`).
pub async fn put(State(state): State<Shared>, Auth(caller): Auth, Body(s): Body<Seen>) -> ApiResult<Json<Value>> {
    caller.require("view")?;
    state.orgs.store(caller.org()).await?.put_tour_seen(caller.org(), caller.email(), s.seen).await?;
    Ok(Json(json!({"ok": true})))
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use serde_json::json;

    use crate::api::share::tests::{clean_up, org_with, send};

    #[tokio::test]
    async fn the_tour_is_offered_until_it_is_seen() {
        let Some((app, state, cookie, org)) = org_with("title: Tour test\ntiles: []\n").await else { return };
        let c = Some(cookie.as_str());
        let (s, r, _) = send(&app, c, &org, "GET", "/api/tour", None, Some("viewer")).await;
        assert_eq!((s, &r["offer"]), (StatusCode::OK, &json!(true)), "{r}");
        let (s, _, _) = send(&app, c, &org, "PUT", "/api/tour", Some(json!({"seen": true})), Some("viewer")).await;
        assert_eq!(s, StatusCode::OK);
        let (_, r, _) = send(&app, c, &org, "GET", "/api/tour", None, None).await;
        assert_eq!(r["offer"], json!(false), "{r}");
        // per organisation: the platform workspace still offers it
        let (_, r, _) = send(&app, c, "", "GET", "/api/tour", None, None).await;
        assert_eq!(r["offer"], json!(true), "{r}");
        // and again on request
        send(&app, c, &org, "PUT", "/api/tour", Some(json!({"seen": false})), None).await;
        let (_, r, _) = send(&app, c, &org, "GET", "/api/tour", None, None).await;
        assert_eq!(r["offer"], json!(true), "{r}");
        let (s, _, _) = send(&app, c, &org, "PUT", "/api/tour", Some(json!({"seen": "yes"})), None).await;
        assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
        clean_up(&state, &org).await;
    }
}
