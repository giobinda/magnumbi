//! HTTP hardening.
//!
//! - Security headers on every response (CSP, nosniff, referrer policy, HSTS on https, no-store
//!   on the API).
//! - CSRF: state-changing requests (POST/PUT/PATCH/DELETE) from a browser must come from Magnum
//!   BI's own origin (or one in MAGNUM_TRUSTED_ORIGINS). Browsers always send Origin or
//!   Sec-Fetch-Site on such requests, so a page on another site (a sibling subdomain included,
//!   which SameSite=Lax does not stop) can't act with a signed-in person's cookie.
//! - Request bodies: 1 MB at most, with a declared length.

use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::config::Config;
use crate::state::Shared;

pub const MAX_BODY_BYTES: usize = 1024 * 1024;

fn unsafe_method(m: &Method) -> bool {
    matches!(*m, Method::POST | Method::PUT | Method::PATCH | Method::DELETE)
}

fn origin_of(url: &str) -> Option<String> {
    let u = url::Url::parse(url).ok()?;
    Some(u.origin().ascii_serialization().to_lowercase())
}

pub fn is_https(config: &Config, headers: &HeaderMap) -> bool {
    config.is_https() || headers.get("x-forwarded-proto").is_some_and(|v| v == "https")
}

/// The address the browser is using: MAGNUM_PUBLIC_URL, else scheme + Host of the request.
pub fn public_base(config: &Config, headers: &HeaderMap) -> String {
    if let Some(u) = &config.public_url {
        return u.clone();
    }
    let host = headers.get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or("localhost");
    format!("{}://{host}", if is_https(config, headers) { "https" } else { "http" })
}

fn own_origins(config: &Config, headers: &HeaderMap) -> Vec<String> {
    let mut out: Vec<String> = config.trusted_origins.clone();
    out.extend(origin_of(&public_base(config, headers)));
    if let Some(u) = config.public_url.as_deref().and_then(origin_of) {
        out.push(u);
    }
    out
}

fn reject(code: StatusCode, detail: &str) -> Response {
    (code, axum::Json(json!({"detail": detail}))).into_response()
}

pub fn csrf_rejection(config: &Config, method: &Method, headers: &HeaderMap) -> Option<Response> {
    if !unsafe_method(method) {
        return None;
    }
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    match origin {
        Some(o) if o != "null" => {
            let o = o.trim_end_matches('/').to_lowercase();
            if own_origins(config, headers).contains(&o) {
                None
            } else {
                Some(reject(StatusCode::FORBIDDEN, "cross-site request refused"))
            }
        }
        _ => {
            // no Origin: non-browser clients. Every current browser sends Sec-Fetch-Site.
            let site = headers.get("sec-fetch-site").and_then(|v| v.to_str().ok());
            if origin == Some("null") || matches!(site, Some("cross-site") | Some("same-site")) {
                Some(reject(StatusCode::FORBIDDEN, "cross-site request refused"))
            } else {
                None
            }
        }
    }
}

fn size_rejection(method: &Method, headers: &HeaderMap) -> Option<Response> {
    if !unsafe_method(method) {
        return None;
    }
    match headers.get(header::CONTENT_LENGTH).and_then(|v| v.to_str().ok()) {
        None if headers.contains_key(header::TRANSFER_ENCODING) => {
            Some(reject(StatusCode::LENGTH_REQUIRED, "send a Content-Length"))
        }
        None => None,
        Some(n) => match n.parse::<usize>() {
            Ok(n) if n <= MAX_BODY_BYTES => None,
            _ => Some(reject(
                StatusCode::PAYLOAD_TOO_LARGE,
                &format!("request body larger than {} KB", MAX_BODY_BYTES / 1024),
            )),
        },
    }
}

pub fn content_security_policy(config: &Config) -> String {
    [
        "default-src 'self'".to_string(),
        "script-src 'self'".into(),
        "style-src 'self' 'unsafe-inline'".into(), // chart libraries set inline styles
        "img-src 'self' data: blob:".into(),
        "font-src 'self' data:".into(),
        "connect-src 'self'".into(),
        "object-src 'none'".into(),
        "base-uri 'none'".into(),
        "form-action 'self' https://accounts.google.com".into(),
        format!("frame-ancestors {}", config.frame_ancestors),
    ]
    .join("; ")
}

pub async fn hardening(State(state): State<Shared>, request: Request, next: Next) -> Response {
    let (method, headers, path) =
        (request.method().clone(), request.headers().clone(), request.uri().path().to_string());
    let mut resp = match size_rejection(&method, &headers).or_else(|| csrf_rejection(&state.config, &method, &headers))
    {
        Some(r) => r,
        None => next.run(request).await,
    };
    let h = resp.headers_mut();
    let mut set = |name: &'static str, value: String| {
        if !h.contains_key(name)
            && let Ok(v) = HeaderValue::from_str(&value)
        {
            h.insert(name, v);
        }
    };
    set("content-security-policy", content_security_policy(&state.config));
    set("x-content-type-options", "nosniff".into());
    set("referrer-policy", "strict-origin-when-cross-origin".into());
    set("permissions-policy", "camera=(), microphone=(), geolocation=()".into());
    set("cross-origin-opener-policy", "same-origin-allow-popups".into()); // Google sign-in from embeds uses a popup
    if is_https(&state.config, &headers) {
        set("strict-transport-security", "max-age=31536000; includeSubDomains".into());
    }
    if path.starts_with("/api/") {
        set("cache-control", "no-store".into()); // data and identities never land in shared caches
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        Config::from_lookup(|_| None).unwrap()
    }

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, HeaderValue::from_str(v).unwrap());
        }
        h
    }

    #[test]
    fn csrf() {
        let c = config();
        let same = headers(&[("host", "localhost:5173"), ("origin", "http://localhost:5173")]);
        assert!(csrf_rejection(&c, &Method::POST, &same).is_none());
        let evil = headers(&[("host", "localhost:5173"), ("origin", "https://evil.example")]);
        assert!(csrf_rejection(&c, &Method::POST, &evil).is_some());
        assert!(csrf_rejection(&c, &Method::GET, &evil).is_none());
        let sibling = headers(&[("host", "bi.acme.example"), ("sec-fetch-site", "same-site")]);
        assert!(csrf_rejection(&c, &Method::DELETE, &sibling).is_some());
        let cli = headers(&[("host", "localhost:8000")]);
        assert!(csrf_rejection(&c, &Method::PUT, &cli).is_none());
    }

    #[test]
    fn sizes() {
        let big = headers(&[("content-length", "2000000")]);
        assert!(size_rejection(&Method::POST, &big).is_some());
        let chunked = headers(&[("transfer-encoding", "chunked")]);
        assert!(size_rejection(&Method::PUT, &chunked).is_some());
        assert!(size_rejection(&Method::POST, &headers(&[("content-length", "12")])).is_none());
    }
}
