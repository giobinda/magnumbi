//! Every route, who may call it, and that nobody else gets through.

use std::collections::BTreeSet;

use axum::http::StatusCode;
use serde_json::{Value, json};

use super::{As, Who, World};
use crate::access::Role;
use crate::api::tests::{OWNER, scramdb_config};

/// Who may call a route.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Min {
    /// no account needed (sign-in, public and guest links): checked by their own tests
    Open,
    /// a session or token, at this role or above, in the organisation asked for
    Role(Role),
}

use Min::Open;
const VIEWER: Min = Min::Role(Role::Viewer);
const USER: Min = Min::Role(Role::User);
const POWER: Min = Min::Role(Role::PowerUser);
const ADMIN: Min = Min::Role(Role::OrgAdmin);
const OWNER_ONLY: Min = Min::Role(Role::SystemOwner);

/// The access matrix: every route of `api::router`, by method. A route missing here fails
/// `every_route_is_in_the_matrix`, so a new endpoint can't skip these tests.
pub const MATRIX: &[(&str, &str, Min)] = &[
    ("GET", "/api/health", Open),
    ("GET", "/api/themes", Open),
    ("GET", "/api/auth/me", Open),
    ("POST", "/api/auth/methods", Open),
    ("POST", "/api/auth/login", Open),
    ("POST", "/api/auth/login/two-factor", Open),
    ("POST", "/api/auth/two-factor/setup", Open),
    ("POST", "/api/auth/two-factor/enable", Open),
    ("POST", "/api/auth/two-factor/disable", Open),
    ("GET", "/api/auth/passkeys", Open),
    ("DELETE", "/api/auth/passkeys/{id}", Open),
    ("POST", "/api/auth/passkeys/register/start", Open),
    ("POST", "/api/auth/passkeys/register/finish", Open),
    ("POST", "/api/auth/passkeys/login/start", Open),
    ("POST", "/api/auth/passkeys/login/finish", Open),
    ("POST", "/api/auth/logout", Open),
    ("POST", "/api/auth/password", Open),
    ("GET", "/api/auth/google/login", Open),
    ("GET", "/api/auth/google/callback", Open),
    ("GET", "/api/auth/oidc/{org}/login", Open),
    ("GET", "/api/auth/oidc/callback", Open),
    ("GET", "/api/public/{org}/{token}", Open),
    ("GET", "/api/public/{org}/{token}/tiles/{tile}", Open),
    ("GET", "/api/guest/{org}/{token}", Open),
    ("POST", "/api/guest/{org}/{token}/unlock", Open),
    ("POST", "/api/guest/{org}/{token}/tiles/{tile}/query", Open),
    ("POST", "/api/guest/{org}/{token}/tiles/{tile}/export", Open),
    ("POST", "/api/guest/{org}/{token}/export", Open),
    ("GET", "/api/guest/{org}/{token}/filters/values", Open),
    ("POST", "/api/guest/{org}/{token}/period", Open),
    ("POST", "/api/unsubscribe", Open),
    ("GET", "/api/models", VIEWER),
    ("POST", "/api/mcp", VIEWER),
    ("GET", "/api/org-themes", ADMIN),
    ("PUT", "/api/org-themes", ADMIN),
    ("POST", "/api/themes/import", ADMIN),
    ("GET", "/api/themes/{id}/css", VIEWER),
    ("GET", "/api/tokens", VIEWER),
    ("POST", "/api/tokens", VIEWER),
    ("DELETE", "/api/tokens/{id}", VIEWER),
    ("GET", "/api/blends", VIEWER),
    ("PUT", "/api/blends/{name}", POWER),
    ("DELETE", "/api/blends/{name}", POWER),
    ("GET", "/api/models/{name}", VIEWER),
    ("POST", "/api/query", POWER),
    ("POST", "/api/query/sql", POWER),
    ("POST", "/api/period", VIEWER),
    ("GET", "/api/dashboards", VIEWER),
    ("GET", "/api/dashboards/{id}", VIEWER),
    ("PUT", "/api/dashboards/{id}", POWER),
    ("DELETE", "/api/dashboards/{id}", POWER),
    ("GET", "/api/dashboards/{id}/yaml", POWER),
    ("PUT", "/api/dashboards/{id}/yaml", POWER),
    ("GET", "/api/dashboards/{id}/history", POWER),
    ("POST", "/api/dashboards/{id}/tiles/{tile}/query", VIEWER),
    ("POST", "/api/dashboards/{id}/tiles/{tile}/export", USER),
    ("POST", "/api/dashboards/{id}/export", USER),
    ("GET", "/api/delivery", ADMIN),
    ("PUT", "/api/delivery/mail", ADMIN),
    ("DELETE", "/api/delivery/mail", ADMIN),
    ("POST", "/api/delivery/mail/test", ADMIN),
    ("PUT", "/api/delivery/webhooks/{name}", ADMIN),
    ("DELETE", "/api/delivery/webhooks/{name}", ADMIN),
    ("POST", "/api/delivery/webhooks/{name}/test", ADMIN),
    ("GET", "/api/dashboards/{id}/filters/values", VIEWER),
    ("GET", "/api/dashboards/{id}/share-links", ADMIN),
    ("POST", "/api/dashboards/{id}/share-links", ADMIN),
    ("DELETE", "/api/share-links/{id}", ADMIN),
    ("GET", "/api/dashboards/{id}/guest-links", POWER),
    ("POST", "/api/dashboards/{id}/guest-links", POWER),
    ("DELETE", "/api/guest-links/{id}", POWER),
    ("GET", "/api/dashboards/{id}/schedules", USER),
    ("POST", "/api/dashboards/{id}/schedules", USER),
    ("GET", "/api/schedules", ADMIN),
    ("DELETE", "/api/schedules/{id}", USER),
    ("PUT", "/api/schedules/{id}/active", USER),
    ("POST", "/api/schedules/{id}/run", USER),
    ("GET", "/api/members", ADMIN),
    ("PUT", "/api/members", ADMIN),
    ("GET", "/api/members/sign-in", ADMIN),
    ("PUT", "/api/members/sign-in", ADMIN),
    ("POST", "/api/members/password", ADMIN),
    ("GET", "/api/platform", OWNER_ONLY),
    ("PUT", "/api/platform", OWNER_ONLY),
    ("POST", "/api/platform/dashboards/{id}/publish", OWNER_ONLY),
    ("GET", "/api/platform/demo", OWNER_ONLY),
    ("POST", "/api/platform/demo", OWNER_ONLY),
    ("PUT", "/api/platform/organisations/{org}/database", OWNER_ONLY),
    ("GET", "/api/usage", ADMIN),
    ("GET", "/api/connections", ADMIN),
    ("PUT", "/api/connections/{name}", ADMIN),
    ("DELETE", "/api/connections/{name}", ADMIN),
    ("POST", "/api/connections/{name}/test", ADMIN),
    ("GET", "/api/connections/{name}/tables", ADMIN),
    ("GET", "/api/connections/{name}/tables/{table}/model", ADMIN),
    // ownership isn't a role: any member may own a model (the handler decides; see below)
    ("GET", "/api/governance", VIEWER),
    ("PUT", "/api/governance/{model}", VIEWER),
    ("PUT", "/api/governance/{model}/docs", VIEWER),
    ("GET", "/api/glossary", VIEWER),
    ("GET", "/api/catalog/search", VIEWER),
    ("GET", "/api/catalog/{model}/lineage", VIEWER),
    ("PUT", "/api/glossary/{name}", VIEWER),
    ("DELETE", "/api/glossary/{name}", VIEWER),
    ("GET", "/api/formulas", ADMIN),
    ("POST", "/api/formulas/check", POWER),
    ("PUT", "/api/formulas/{model}/{name}", ADMIN),
    ("DELETE", "/api/formulas/{model}/{name}", ADMIN),
    ("GET", "/api/org-models", ADMIN),
    ("PUT", "/api/org-models/{name}", ADMIN),
    ("DELETE", "/api/org-models/{name}", ADMIN),
    ("GET", "/api/setup", VIEWER),
    ("GET", "/api/cache", ADMIN),
    ("DELETE", "/api/cache", ADMIN),
];

/// The routes in api/mod.rs, read from its source: (method, path).
fn routes_in_source() -> BTreeSet<(String, String)> {
    let src = include_str!("../api/mod.rs");
    let src = &src[..src.find("#[cfg(test)]").unwrap_or(src.len())];
    let method = regex::Regex::new(r"\b(get|post|put|delete)\(").unwrap();
    let mut out = BTreeSet::new();
    for chunk in src.split(".route(").skip(1) {
        let path = chunk.split('"').nth(1).unwrap().to_string();
        // the handlers: up to the end of this .route(...) call
        let (mut depth, mut end) = (1, chunk.len());
        for (i, c) in chunk.char_indices() {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = i;
                        break;
                    }
                }
                _ => {}
            }
        }
        for m in method.captures_iter(&chunk[..end]) {
            out.insert((m[1].to_uppercase(), path.clone()));
        }
    }
    out
}

/// A route with the world's ids in it.
pub fn fill(w: &World, path: &str) -> String {
    let name = if path.starts_with("/api/connections/") {
        "warehouse"
    } else if path.starts_with("/api/delivery/webhooks/") {
        "team"
    } else if path.starts_with("/api/org-models/") {
        "attack_model"
    } else if path.starts_with("/api/glossary/") {
        "attack_term"
    } else if path.starts_with("/api/formulas/") {
        "attack_formula"
    } else {
        "attack_blend"
    };
    let id = if path.starts_with("/api/share-links/") {
        w.share_id.clone()
    } else if path.starts_with("/api/guest-links/") {
        w.guest_id.clone()
    } else if path.starts_with("/api/tokens/") {
        w.write_token.0.clone()
    } else if path.starts_with("/api/schedules/") {
        w.schedule_id.clone()
    } else if path.starts_with("/api/themes/") {
        "aloha".into()
    } else {
        w.dashboard.clone()
    };
    let token = if path.starts_with("/api/guest/") { &w.guest_token } else { &w.share_token };
    let mut p = path
        .replace("{id}", &id)
        .replace("{model}", "sales")
        .replace("{name}", name)
        .replace("{tile}", "t")
        .replace("{table}", "sample_sales")
        .replace("{org}", &w.a)
        .replace("{token}", token);
    if p == "/api/period" {
        p.push_str("?model=sales");
    }
    if path.ends_with("/tables/{table}/model") {
        p.push_str("?name=attack_model");
    }
    p
}

/// A body good enough to reach the permission check (refused callers must be refused for their
/// role, not for a malformed request).
pub fn body(w: &World, method: &str, path: &str) -> Option<Value> {
    if method == "GET" || method == "DELETE" {
        return None;
    }
    let query = json!({"model": "sales", "measures": ["transactions"]});
    Some(match path {
        "/api/mcp" => json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
        "/api/org-themes" => json!({"themes": []}),
        "/api/themes/import" => json!({"css": ":root { --accent: #c8102e; }"}),
        "/api/tokens" => json!({"name": "attack", "scope": "read"}),
        "/api/blends/{name}" => {
            json!({"name": "attack_blend", "base": "sales", "joins": [{"model": "weather", "on": ["region = borough"]}]})
        }
        "/api/query" | "/api/query/sql" => query,
        "/api/period" => json!({"field": "sold_at", "current": {"last_n_days": 7}}),
        "/api/dashboards/{id}" => json!({"title": "Overwritten", "tiles": []}),
        "/api/delivery/mail" => {
            json!({"host": "smtp.evil.example", "port": 587, "security": "starttls", "from": "x@evil.example"})
        }
        "/api/delivery/webhooks/{name}" => json!({"kind": "slack", "url": "https://hooks.evil.example/x"}),
        "/api/dashboards/{id}/share-links" => json!({"cache_seconds": 300}),
        "/api/dashboards/{id}/guest-links" => json!({"expires_in_hours": 1, "password": "Evil-Password-1"}),
        "/api/dashboards/{id}/schedules" => json!({"name": "evil", "format": "csv", "recipients": [OWNER],
            "when": {"every": "daily", "at": "08:00", "timezone": "UTC"}}),
        "/api/schedules/{id}/active" => json!({"active": false}),
        "/api/members" => json!({"users": {"evil@evil.example": "org_admin"}}),
        "/api/members/sign-in" => json!({"sign_in": null}),
        "/api/members/password" => json!({"email": w.user_a}),
        "/api/platform" => json!({"system_owners": ["evil@evil.example"]}),
        "/api/platform/dashboards/{id}/publish" => json!({"orgs": [w.b], "as": "stolen"}),
        "/api/platform/demo" => json!({"org": w.a, "months": 1}),
        "/api/platform/organisations/{org}/database" => json!({"dsn": "postgresql://evil@127.0.0.1:1/x"}),
        "/api/connections/{name}" | "/api/connections/{name}/test" => json!({"config": scramdb_config()}),
        "/api/governance/{model}" => json!({"owner": "evil@evil.example", "status": "certified"}),
        "/api/governance/{model}/docs" => json!({"description": "evil"}),
        "/api/glossary/{name}" => json!({"name": "attack_term", "definition": "evil"}),
        "/api/formulas/{model}/{name}" => json!({"name": "attack_formula", "expr": "revenue * 1000"}),
        "/api/formulas/check" => json!({"model": "sales", "expr": "revenue / transactions"}),
        "/api/org-models/{name}" => json!({"name": "attack_model", "connection": "warehouse", "table": "sample_sales",
                                           "dimensions": [{"name": "region"}], "measures": [{"name": "n", "type": "count"}]}),
        _ => json!({}),
    })
}

/// Who is at each role in organisation A.
fn person(w: &World, role: Role) -> String {
    match role {
        Role::Viewer => w.viewer_a.clone(),
        Role::User => w.user_a.clone(),
        Role::PowerUser => w.power_a.clone(),
        Role::OrgAdmin => w.admin_a.clone(),
        Role::SystemOwner => OWNER.into(),
    }
}

async fn send(w: &World, who: &As, method: &str, path: &str) -> super::Reply {
    let p = fill(w, path);
    if path == "/api/dashboards/{id}/yaml" && method == "PUT" {
        return w.yaml(who, &p, "title: Overwritten\n").await;
    }
    w.send(who, method, &p, body(w, method, path)).await
}

fn refused(s: StatusCode) -> bool {
    matches!(s, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN | StatusCode::NOT_FOUND)
}

/// Nothing the world holds was changed or removed by refused requests.
pub async fn assert_intact(w: &World) {
    let owner = As { who: Who::Person(OWNER.into()), org: w.a.clone(), view_as: None };
    let d = w.send(&owner, "GET", &format!("/api/dashboards/{}", w.dashboard), None).await.json();
    assert_eq!(d["dashboard"]["title"], json!("Attack target"), "A's dashboard was changed: {d}");
    let r = w.send(&owner, "GET", &format!("/api/public/{}/{}", w.a, w.share_token), None).await;
    assert_eq!(r.status, StatusCode::OK, "the public link stopped working: {}", r.body);
    let guest = As { who: Who::Cookie(w.guest_cookie.clone()), org: String::new(), view_as: None };
    let r = w.send(&guest, "GET", &format!("/api/guest/{}/{}", w.a, w.guest_token), None).await;
    assert_eq!(r.status, StatusCode::OK, "the guest link stopped working: {}", r.body);
    let admin = As { who: Who::Person(w.admin_a.clone()), org: w.a.clone(), view_as: None };
    let t = w.send(&admin, "GET", "/api/tokens", None).await.json();
    assert_eq!(t["tokens"].as_array().map(|t| t.len()), Some(2), "admin's tokens changed: {t}");
    let d = w.send(&owner, "GET", "/api/delivery", None).await.json();
    assert_eq!(d["mail"]["host"], json!("smtp.attack.example"), "the mail server changed: {d}");
    assert_eq!(d["webhooks"][0]["host"], json!("hooks.attack.example"), "the webhook changed: {d}");
    let c = w.send(&owner, "GET", "/api/connections", None).await.json();
    assert_eq!(c["connections"].as_array().map(|c| c.len()), Some(2), "connections changed: {c}");
    let s = w.send(&owner, "GET", "/api/schedules", None).await.json();
    let report = s["schedules"].as_array().and_then(|s| s.iter().find(|x| x["id"] == json!(w.schedule_id)).cloned());
    assert!(report.as_ref().is_some_and(|r| r["active"] == json!(true)), "the scheduled report changed: {s}");
    let p = w.send(&owner, "GET", "/api/platform", None).await.json();
    assert!(!p.to_string().contains("evil"), "the platform settings changed: {p}");
    let f = w.send(&owner, "GET", "/api/formulas", None).await.json();
    assert_eq!(f["formulas"], json!([]), "A's shared formulas changed: {f}");
    let g = w.send(&owner, "GET", "/api/governance", None).await.json();
    assert!(!g.to_string().contains("evil"), "A's governance changed: {g}");
    let g = w.send(&owner, "GET", "/api/glossary", None).await.json();
    assert!(!g.to_string().contains("evil"), "A's glossary changed: {g}");
    let m = w.send(&owner, "GET", "/api/models/sales", None).await.json();
    assert!(!m.to_string().contains("evil"), "A's docs changed: {m}");
    let m = w.send(&owner, "GET", "/api/members", None).await.json();
    assert!(!m.to_string().contains("evil"), "A's members changed: {m}");
}

#[test]
fn every_route_is_in_the_matrix() {
    let source = routes_in_source();
    let matrix: BTreeSet<(String, String)> = MATRIX.iter().map(|(m, p, _)| (m.to_string(), p.to_string())).collect();
    let missing: Vec<_> = source.difference(&matrix).collect();
    let stale: Vec<_> = matrix.difference(&source).collect();
    assert!(missing.is_empty(), "routes without an entry in attack::routes::MATRIX: {missing:?}");
    assert!(stale.is_empty(), "entries in the matrix for routes that don't exist: {stale:?}");
    assert_eq!(MATRIX.len(), source.len(), "a route is listed twice");
}

#[tokio::test]
async fn nobody_gets_past_their_role() {
    let Some(w) = World::new().await else { return };
    let mut leaks = vec![];
    for &(method, path, min) in MATRIX {
        let Min::Role(min) = min else { continue };
        for role in [Role::Viewer, Role::User, Role::PowerUser, Role::OrgAdmin].into_iter().filter(|r| *r < min) {
            // their own session, and an Org admin "viewing as" that role
            let own = As { who: Who::Person(person(&w, role)), org: w.a.clone(), view_as: None };
            let viewing =
                As { who: Who::Person(w.admin_a.clone()), org: w.a.clone(), view_as: Some(role.as_str().into()) };
            for who in [own, viewing] {
                let r = send(&w, &who, method, path).await;
                if !refused(r.status) {
                    leaks.push(format!("{method} {path} as {role:?} ({who:?}): {} {}", r.status, r.body));
                }
            }
        }
        // asking to view as a higher role never raises yours
        if min > Role::Viewer {
            let up = As { who: Who::Person(w.viewer_a.clone()), org: w.a.clone(), view_as: Some(min.as_str().into()) };
            let r = send(&w, &up, method, path).await;
            if !refused(r.status) {
                leaks.push(format!("{method} {path}: a viewer asking to view as {min:?} got {} {}", r.status, r.body));
            }
        }
    }
    let power = As { who: Who::Person(w.power_a.clone()), org: w.a.clone(), view_as: None };
    // a colleague who doesn't own a model can't change its governance, nor take it
    let g = json!({"owner": w.power_a, "status": "certified"});
    let r = w.send(&power, "PUT", "/api/governance/sales", Some(g)).await;
    if r.status != StatusCode::FORBIDDEN {
        leaks.push(format!("a colleague took the sales model: {} {}", r.status, r.body));
    }
    let r = w.send(&power, "PUT", "/api/governance/sales/docs", Some(json!({"description": "evil"}))).await;
    let t = w
        .send(&power, "PUT", "/api/glossary/attack_term", Some(json!({"name": "attack_term", "definition": "evil"})))
        .await;
    for (what, r) in [("documented", r), ("added a glossary term to", t)] {
        if r.status != StatusCode::FORBIDDEN {
            leaks.push(format!("a colleague {what} the organisation: {} {}", r.status, r.body));
        }
    }
    // a colleague at the right role, but neither its maker nor an Org admin, can't touch a report
    for (method, path) in [("DELETE", ""), ("PUT", "/active"), ("POST", "/run")] {
        let p = format!("/api/schedules/{}{path}", w.schedule_id);
        let r = w.send(&power, method, &p, Some(json!({"active": false}))).await;
        if r.status != StatusCode::FORBIDDEN {
            leaks.push(format!("{method} {p} by a colleague: {} {}", r.status, r.body));
        }
    }
    assert!(leaks.is_empty(), "requests that got past their role:\n{}", leaks.join("\n"));
    assert_intact(&w).await;
    w.clean_up().await;
}

#[tokio::test]
async fn the_right_role_gets_in() {
    let Some(w) = World::new().await else { return };
    let mut shut_out = vec![];
    for &(method, path, min) in MATRIX {
        let Min::Role(min) = min else { continue };
        // reading only: writes are checked by the feature tests
        let reads = method == "GET" || path.ends_with("/query") || path.ends_with("/export") || path == "/api/period";
        if !reads || path.starts_with("/api/platform/demo") {
            continue;
        }
        let who = As { who: Who::Person(person(&w, min)), org: w.a.clone(), view_as: None };
        let r = send(&w, &who, method, path).await;
        if matches!(r.status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) || r.status.is_server_error() {
            shut_out.push(format!("{method} {path} as {min:?}: {} {}", r.status, r.body));
        }
    }
    assert!(shut_out.is_empty(), "the role the matrix allows was refused:\n{}", shut_out.join("\n"));
    w.clean_up().await;
}

#[tokio::test]
async fn other_organisations_and_strangers_get_nothing() {
    let Some(w) = World::new().await else { return };
    let mut leaks = vec![];
    let callers = [
        ("anonymous", As { who: Who::Anonymous, org: w.a.clone(), view_as: None }),
        ("a forged session", As { who: Who::Cookie("magnum_session=forged".into()), org: w.a.clone(), view_as: None }),
        ("a forged token", As { who: Who::Token("mbi_forged".into()), org: w.a.clone(), view_as: None }),
        ("a guest cookie", As { who: Who::Cookie(w.guest_cookie.clone()), org: w.a.clone(), view_as: None }),
        ("B's Org admin in A", As { who: Who::Person(w.admin_b.clone()), org: w.a.clone(), view_as: None }),
        ("B's token in A", As { who: Who::Token(w.token_b.clone()), org: w.a.clone(), view_as: None }),
    ];
    for &(method, path, min) in MATRIX {
        if min == Open {
            continue;
        }
        for (name, who) in &callers {
            let r = send(&w, who, method, path).await;
            if !refused(r.status) {
                leaks.push(format!("{method} {path} as {name}: {} {}", r.status, r.body));
            }
        }
    }
    // naming an organisation you aren't working in never quietly acts somewhere else
    for (name, who) in [
        (
            "a system owner, in an organisation that doesn't exist",
            As { who: Who::Person(OWNER.into()), org: "no-such-org".into(), view_as: None },
        ),
        ("A's Org admin in B", As { who: Who::Person(w.admin_a.clone()), org: w.b.clone(), view_as: None }),
        ("A's token in B", As { who: Who::Token(w.write_token.1.clone()), org: w.b.clone(), view_as: None }),
    ] {
        let r = w.send(&who, "GET", "/api/dashboards", None).await;
        if r.status != StatusCode::FORBIDDEN {
            leaks.push(format!("GET /api/dashboards as {name}: {} {}", r.status, r.body));
        }
    }
    // in their own organisation, with A's ids: B's things, or nothing
    let b = As { who: Who::Person(w.admin_b.clone()), org: w.b.clone(), view_as: None };
    let d = w.send(&b, "GET", &format!("/api/dashboards/{}", w.dashboard), None).await.json();
    assert_eq!(d["dashboard"]["title"], json!("B's private dashboard"), "{d}");
    for (method, path) in [
        ("DELETE", format!("/api/share-links/{}", w.share_id)),
        ("DELETE", format!("/api/guest-links/{}", w.guest_id)),
        ("DELETE", format!("/api/tokens/{}", w.write_token.0)),
        ("GET", format!("/api/public/{}/{}", w.b, w.share_token)),
        ("GET", format!("/api/guest/{}/{}", w.b, w.guest_token)),
    ] {
        let r = w.send(&b, method, &path, None).await;
        if !refused(r.status) && !r.status.is_client_error() {
            leaks.push(format!("{method} {path} as B's Org admin in B: {} {}", r.status, r.body));
        }
    }
    assert!(leaks.is_empty(), "requests that crossed organisations:\n{}", leaks.join("\n"));
    assert_intact(&w).await;
    w.clean_up().await;
}

#[tokio::test]
async fn read_tokens_only_read() {
    let Some(w) = World::new().await else { return };
    let mut writes = vec![];
    let read = As { who: Who::Token(w.read_token.1.clone()), org: w.a.clone(), view_as: None };
    for &(method, path, min) in MATRIX {
        if min == Open || method == "GET" {
            continue;
        }
        let reads = path.ends_with("/query")
            || path.ends_with("/export")
            || matches!(path, "/api/period" | "/api/mcp" | "/api/query/sql" | "/api/formulas/check");
        for dry in ["", "?dry_run=1"] {
            let p = fill(&w, path);
            let p = if dry.is_empty() {
                p
            } else if p.contains('?') {
                format!("{p}&dry_run=1")
            } else {
                format!("{p}{dry}")
            };
            let r = if path == "/api/dashboards/{id}/yaml" {
                w.yaml(&read, &p, "title: Overwritten\n").await
            } else {
                w.send(&read, method, &p, body(&w, method, path)).await
            };
            let changed = r.status.is_success() && !reads && !r.json()["dry_run"].as_bool().unwrap_or(false);
            if changed {
                writes.push(format!("{method} {p} with a read token: {} {}", r.status, r.body));
            }
        }
    }
    // through MCP: every tool that writes, claiming a dry run
    let yaml = "title: Overwritten\n";
    for (tool, args) in [
        ("save_dashboard", json!({"id": w.dashboard, "yaml": yaml})),
        ("delete_dashboard", json!({"id": w.dashboard})),
        (
            "save_blend",
            json!({"blend": {"name": "attack_blend", "base": "sales", "joins": [{"model": "weather", "on": ["region = borough"]}]}}),
        ),
        ("delete_blend", json!({"name": "attack_blend"})),
        ("save_connection", json!({"name": "decoy", "config": scramdb_config()})),
        ("delete_connection", json!({"name": "decoy"})),
        ("save_members", json!({"members": {"users": {"evil@evil.example": "org_admin"}}})),
        (
            "create_report",
            json!({"dashboard": w.dashboard, "report": {"name": "evil", "format": "csv", "recipients": [OWNER],
                   "when": {"every": "daily", "at": "07:00", "timezone": "UTC"}}}),
        ),
        ("pause_report", json!({"id": w.schedule_id})),
        ("resume_report", json!({"id": w.schedule_id})),
        ("send_report_now", json!({"id": w.schedule_id})),
        ("delete_report", json!({"id": w.schedule_id})),
        ("save_formula", json!({"model": "sales", "formula": {"name": "evil", "expr": "revenue * 1000"}})),
        ("delete_formula", json!({"model": "sales", "name": "evil"})),
    ] {
        for dry in [false, true] {
            let mut args = args.clone();
            if dry {
                args["dry_run"] = json!(true);
            }
            let call =
                json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": tool, "arguments": args}});
            let r = w.send(&read, "POST", "/api/mcp", Some(call)).await.json();
            let result = &r["result"];
            let said_dry = result["structuredContent"]["dry_run"].as_bool().unwrap_or(false);
            if result["isError"] == json!(false) && !said_dry {
                writes.push(format!("MCP {tool} (dry_run: {dry}) with a read token: {r}"));
            }
        }
    }
    assert!(writes.is_empty(), "a read-only token changed things:\n{}", writes.join("\n"));
    assert_intact(&w).await;
    w.clean_up().await;
}
