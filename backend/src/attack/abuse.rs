//! Abuse that isn't about one request: guessing passwords, requests from other sites, crowds of
//! visitors on a public link, and sessions or tokens used after they ended.

use axum::http::StatusCode;
use serde_json::json;

use super::{As, Who, World};

fn anyone() -> As {
    As { who: Who::Anonymous, org: String::new(), view_as: None }
}

#[tokio::test]
async fn guessing_passwords_locks_the_account_and_says_nothing_about_who_exists() {
    let Some(w) = World::new().await else { return };
    let login = |email: &str, password: &str| json!({"email": email, "password": password});
    // wrong passwords for someone who exists, and for someone who doesn't: the same answer
    let real = w.send(&anyone(), "POST", "/api/auth/login", Some(login(&w.viewer_a, "Wrong-Password-1"))).await;
    let ghost =
        w.send(&anyone(), "POST", "/api/auth/login", Some(login("ghost@attack.example", "Wrong-Password-1"))).await;
    assert_eq!((real.status, &real.body), (ghost.status, &ghost.body), "telling accounts apart");
    for _ in 0..5 {
        w.send(&anyone(), "POST", "/api/auth/login", Some(login(&w.viewer_a, "Wrong-Password-1"))).await;
    }
    // now even the right password waits
    let r = w.send(&anyone(), "POST", "/api/auth/login", Some(login(&w.viewer_a, "attack-suite-password"))).await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS, "{}", r.body);
    // another account isn't locked with it
    let r = w.send(&anyone(), "POST", "/api/auth/login", Some(login(&w.user_a, "attack-suite-password"))).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    w.clean_up().await;
}

#[tokio::test]
async fn requests_from_other_sites_are_refused() {
    let Some(w) = World::new().await else { return };
    let admin = As { who: Who::Person(w.admin_a.clone()), org: w.a.clone(), view_as: None };
    let body = json!({"name": "from another site", "scope": "write"}).to_string();
    for headers in [
        vec![("origin", "https://evil.example")],
        vec![("origin", "null")],
        vec![("sec-fetch-site", "cross-site")],
        vec![("sec-fetch-site", "same-site")],
    ] {
        let r = w.raw_with(&admin, "POST", "/api/tokens", &headers, Some("application/json"), body.clone()).await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{headers:?}: {}", r.body);
    }
    // a form post (no preflight in a browser) isn't taken as JSON
    let r = w.raw_with(&admin, "POST", "/api/tokens", &[], Some("text/plain"), body.clone()).await;
    assert!(r.status.is_client_error(), "text/plain: {} {}", r.status, r.body);
    // and nothing was made
    let tokens = w.send(&admin, "GET", "/api/tokens", None).await.json();
    assert_eq!(tokens["tokens"].as_array().map(|t| t.len()), Some(2), "{tokens}");
    w.clean_up().await;
}

#[tokio::test]
async fn a_crowd_on_a_public_link_asks_the_warehouse_once() {
    let Some(w) = World::new().await else { return };
    let owner = As { who: Who::Person(crate::api::tests::OWNER.into()), org: w.a.clone(), view_as: None };
    let links = format!("/api/dashboards/{}/share-links", w.dashboard);
    let link = w.send(&owner, "POST", &links, Some(json!({"cache_seconds": 300}))).await.json();
    let token = link["url"].as_str().unwrap().rsplit('/').next().unwrap().to_string();
    let tile = format!("/api/public/{}/{token}/tiles/t", w.a);
    let visitor = anyone();
    let visits = (0..30).map(|_| w.send(&visitor, "GET", &tile, None));
    let replies = futures_util::future::join_all(visits).await;
    assert!(replies.iter().all(|r| r.status == StatusCode::OK), "every visitor gets the dashboard");
    let asked = replies.iter().filter(|r| r.json()["from_cache"] == json!(false)).count();
    assert_eq!(asked, 1, "30 visitors at once must make one warehouse query, not {asked}");
    w.clean_up().await;
}

#[tokio::test]
async fn ended_sessions_and_tokens_and_huge_bodies_get_nothing() {
    let Some(w) = World::new().await else { return };
    let viewer = As { who: Who::Person(w.viewer_a.clone()), org: w.a.clone(), view_as: None };
    assert_eq!(w.send(&viewer, "GET", "/api/dashboards", None).await.status, StatusCode::OK);
    assert_eq!(w.send(&viewer, "POST", "/api/auth/logout", None).await.status, StatusCode::OK);
    let r = w.send(&viewer, "GET", "/api/dashboards", None).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED, "a signed-out session still works: {}", r.body);

    let admin = As { who: Who::Person(w.admin_a.clone()), org: w.a.clone(), view_as: None };
    let read = As { who: Who::Token(w.read_token.1.clone()), org: w.a.clone(), view_as: None };
    assert_eq!(w.send(&read, "GET", "/api/dashboards", None).await.status, StatusCode::OK);
    let r = w.send(&admin, "DELETE", &format!("/api/tokens/{}", w.read_token.0), None).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let r = w.send(&read, "GET", "/api/dashboards", None).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED, "a revoked token still works: {}", r.body);

    let huge =
        format!("{{\"model\": \"sales\", \"measures\": [\"transactions\"], \"x\": \"{}\"}}", "a".repeat(3 << 20));
    let r = w.raw(&admin, "POST", "/api/query", Some("application/json"), huge).await;
    assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE, "{}", r.body.chars().take(200).collect::<String>());
    w.clean_up().await;
}
