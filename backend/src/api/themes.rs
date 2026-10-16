//! Organisations' own themes (Org admins): made in the theme editor, or from CSS design tokens;
//! exported as CSS tokens for the company's own site.
//!
//! They live in the organisation's own ScramDB (one settings document: themes and the default
//! light and dark theme) and join everyone's theme list in that organisation. Saves take
//! ?dry_run=1 and If-Match like other documents agents edit.

use axum::Json;
use axum::extract::{Path, Query as UrlQuery, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};

use super::Body;
use super::dashboards::WriteParams;
use crate::auth::{Auth, session_or_token};
use crate::error::{ApiError, ApiResult};
use crate::state::Shared;
use crate::themes::{OrgThemes, Theme, from_css, to_css};

fn version(t: &OrgThemes) -> String {
    crate::store::sha256_hex(&serde_json::to_string(t).unwrap_or_default())[..16].to_string()
}

/// Everyone's theme list: the project's, plus the organisation's own when signed in (the sign-in
/// page gets the project's), and the default light and dark theme.
pub async fn list(
    State(state): State<Shared>,
    jar: axum_extra::extract::CookieJar,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    let mut project = state.project.current();
    if let Some(session) = session_or_token(&state, &jar, &headers).await? {
        let (access, _) = state.access.get(&state, &project).await;
        let org_header = headers.get(crate::auth::ORG_HEADER).and_then(|v| v.to_str().ok()).map(str::to_lowercase);
        let scope = crate::auth::scope_for(&access, &session, org_header.as_deref());
        if scope.role.is_some()
            && let Ok(t) = state.org_themes(scope.org.as_deref().unwrap_or_default()).await
        {
            project = std::sync::Arc::new(project.with_org_themes(&t));
        }
    }
    Ok(Json(json!({"themes": project.themes, "default_light": project.theme_defaults.0,
                   "default_dark": project.theme_defaults.1})))
}

/// Org admins: the organisation's themes, its defaults, the version (to save), and the themes
/// it can start from.
pub async fn get_org(State(state): State<Shared>, Auth(caller): Auth) -> ApiResult<Json<Value>> {
    caller.require("manage_members")?;
    let t = state.orgs.store(caller.org()).await?.org_themes(caller.org()).await?;
    let available = &state.project.current().themes;
    Ok(Json(json!({"themes": t.themes, "default_light": t.default_light, "default_dark": t.default_dark,
                   "version": version(&t), "available": available})))
}

pub async fn put_org(
    State(state): State<Shared>,
    Auth(caller): Auth,
    UrlQuery(p): UrlQuery<WriteParams>,
    headers: HeaderMap,
    Body(mut t): Body<OrgThemes>,
) -> ApiResult<Json<Value>> {
    caller.require("manage_members")?;
    let store = state.orgs.store(caller.org()).await?;
    if let Some(expected) = headers.get(header::IF_MATCH).and_then(|v| v.to_str().ok()) {
        let current = version(&store.org_themes(caller.org()).await?);
        if expected.trim().trim_matches('"') != current {
            let detail = "the themes changed since you read them: read them again, then save";
            return Err(ApiError::Body(StatusCode::CONFLICT, json!({"detail": detail, "version": current})));
        }
    }
    t.validate(&state.project.current().themes).map_err(ApiError::bad_request)?;
    if p.dry_run {
        return Ok(Json(json!({"ok": true, "dry_run": true})));
    }
    store.put_org_themes(caller.org(), &t, Some(caller.email())).await?;
    state.themes.forget(caller.org());
    state.usage.record(crate::usage::Event {
        detail: Some(json!({"themes": t.themes.len()})),
        ..caller.event("themes_changed")
    });
    Ok(Json(json!({"ok": true, "version": version(&t)})))
}

/// A theme as CSS design tokens and helper classes, for the company's site (any theme the caller
/// can see).
pub async fn css(Auth(caller): Auth, Path(id): Path<String>) -> ApiResult<Response> {
    caller.require("view")?;
    let t = caller
        .project
        .themes
        .iter()
        .find(|t| t.id == id)
        .ok_or_else(|| ApiError::not_found(format!("no theme {id:?}")))?;
    let disposition = format!("attachment; filename=\"magnum-bi-{}.css\"", t.id);
    Ok((
        [(header::CONTENT_TYPE, "text/css; charset=utf-8".to_string()), (header::CONTENT_DISPOSITION, disposition)],
        to_css(t),
    )
        .into_response())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportBody {
    css: String,
    /// the theme whose values fill in tokens the CSS doesn't give (default: the default light theme)
    #[serde(default)]
    base: Option<String>,
}

/// A theme from CSS design tokens (not saved: the editor shows it, the admin saves it).
pub async fn import(Auth(caller): Auth, Body(b): Body<ImportBody>) -> ApiResult<Json<Theme>> {
    caller.require("manage_members")?;
    if b.css.len() > 64 * 1024 {
        return Err(ApiError::bad_request("that CSS is too long (64 KB at most)"));
    }
    let base_id = b.base.unwrap_or_else(|| caller.project.theme_defaults.0.clone());
    let base = caller
        .project
        .themes
        .iter()
        .find(|t| t.id == base_id)
        .ok_or_else(|| ApiError::bad_request(format!("no theme {base_id:?}")))?;
    Ok(Json(from_css(&b.css, base).map_err(ApiError::bad_request)?))
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use serde_json::{Value, json};

    use crate::api::tests::{OWNER, app};

    #[tokio::test]
    async fn organisations_brand_their_magnum_bi() {
        let Some((app, state, cookie)) = app().await else { return };
        let org = format!("brand{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
        state.orgs.pin(&org, state.store.clone());
        let access =
            json!({"system_owners": [OWNER], "organisations": {org.clone(): {"name": "Brand test", "models": ["*"]}}});
        state.access.pin(crate::access::AccessConfig::from_document(access).unwrap());
        let send = |method: &'static str, path: String, body: Option<Value>, if_match: Option<String>| {
            let (app, cookie, org) = (app.clone(), cookie.clone(), org.clone());
            async move {
                let mut req = axum::http::Request::builder()
                    .method(method)
                    .uri(path)
                    .header("cookie", cookie)
                    .header("x-magnum-org", org)
                    .header("content-type", "application/json");
                if let Some(v) = if_match {
                    req = req.header("if-match", v);
                }
                let body = body.map_or(axum::body::Body::empty(), |b| axum::body::Body::from(b.to_string()));
                let resp = tower::ServiceExt::oneshot(app, req.body(body).unwrap()).await.unwrap();
                let (status, ctype) =
                    (resp.status(), resp.headers().get("content-type").map(|v| v.to_str().unwrap().to_string()));
                let bytes = http_body_util::BodyExt::collect(resp.into_body()).await.unwrap().to_bytes();
                let text = String::from_utf8_lossy(&bytes).to_string();
                (status, serde_json::from_str::<Value>(&text).unwrap_or(Value::String(text)), ctype)
            }
        };

        let (s, got, _) = send("GET", "/api/org-themes".into(), None, None).await;
        assert_eq!(s, StatusCode::OK, "{got}");
        assert_eq!(got["themes"], json!([]));
        assert!(got["available"].as_array().unwrap().iter().any(|t| t["id"] == json!("aloha")));
        let version = got["version"].as_str().unwrap().to_string();

        // a theme in the company's colours, made the organisation's default (light)
        let mut acme: Value =
            got["available"].as_array().unwrap().iter().find(|t| t["id"] == json!("slate")).unwrap().clone();
        acme["id"] = json!("acme");
        acme["label"] = json!("ACME");
        acme["colors"]["accent"] = json!("#0055a5");
        acme.as_object_mut().unwrap().remove("builtin");
        let doc = json!({"themes": [acme], "default_light": "acme"});
        let (s, body, _) =
            send("PUT", "/api/org-themes?dry_run=1".into(), Some(doc.clone()), Some(version.clone())).await;
        assert_eq!((s, &body["dry_run"]), (StatusCode::OK, &json!(true)), "{body}");
        let (s, body, _) = send("PUT", "/api/org-themes".into(), Some(doc.clone()), Some(version.clone())).await;
        assert_eq!(s, StatusCode::OK, "{body}");
        let (s, body, _) = send("PUT", "/api/org-themes".into(), Some(doc), Some(version)).await;
        assert_eq!(s, StatusCode::CONFLICT, "stale: {body}");

        // everyone in the organisation gets it
        let (_, themes, _) = send("GET", "/api/themes".into(), None, None).await;
        assert_eq!(themes["default_light"], json!("acme"), "{themes}");
        assert_eq!(themes["default_dark"], json!("aloha-night"));
        assert!(themes["themes"].as_array().unwrap().iter().any(|t| t["id"] == json!("acme")));

        // CSS tokens out, edited by the design team, back in
        let (s, css, ctype) = send("GET", "/api/themes/acme/css".into(), None, None).await;
        assert_eq!(s, StatusCode::OK);
        assert!(ctype.unwrap().starts_with("text/css"));
        let css = css.as_str().unwrap().to_string();
        assert!(css.contains("--mbi-accent: #0055a5;"), "{css}");
        let edited = css.replace("#0055a5", "#ff8a00");
        let (s, imported, _) =
            send("POST", "/api/themes/import".into(), Some(json!({"css": edited, "base": "acme"})), None).await;
        assert_eq!(s, StatusCode::OK, "{imported}");
        assert_eq!(imported["colors"]["accent"], json!("#ff8a00"));
        let (s, _, _) = send("POST", "/api/themes/import".into(), Some(json!({"css": "nothing"})), None).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);

        // ids of built-in themes are taken
        let (_, cur, _) = send("GET", "/api/org-themes".into(), None, None).await;
        let mut taken = cur["themes"][0].clone();
        taken["id"] = json!("aloha");
        let (s, body, _) = send("PUT", "/api/org-themes".into(), Some(json!({"themes": [taken]})), None).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");

        let c = state.store.pool.get().await.unwrap();
        c.execute("DELETE FROM theme_settings WHERE org = $1", &[&org]).await.unwrap();
    }
}
