//! No planted secret comes back: not in responses, headers or errors, for anyone, and not in
//! the server's log.

use std::sync::{Mutex, OnceLock};

use serde_json::json;

use super::routes::{MATRIX, Min, fill};
use super::{As, Who, World};
use crate::api::tests::OWNER;

/// Everything the server logs during the tests (the suite's own subscriber, at debug level).
fn log() -> &'static Mutex<Vec<u8>> {
    static LOG: OnceLock<&'static Mutex<Vec<u8>>> = OnceLock::new();
    LOG.get_or_init(|| {
        let buffer: &'static Mutex<Vec<u8>> = Box::leak(Box::new(Mutex::new(Vec::new())));
        let writer = move || Capture(buffer);
        let subscriber = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::new("magnum_bi=debug,tower_http=debug"))
            .with_writer(writer)
            .with_ansi(false)
            .finish();
        let _ = tracing::subscriber::set_global_default(subscriber);
        buffer
    })
}

struct Capture(&'static Mutex<Vec<u8>>);

impl std::io::Write for Capture {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The canaries in `text`, by what they are.
fn found(w: &World, text: &str) -> Vec<String> {
    w.canaries.iter().filter(|c| text.contains(c.as_str())).cloned().collect()
}

#[tokio::test]
async fn no_secret_comes_back_to_anyone() {
    log();
    let Some(w) = World::new().await else { return };
    let callers = [
        ("the system owner in A", As { who: Who::Person(OWNER.into()), org: w.a.clone(), view_as: None }),
        ("A's Org admin", As { who: Who::Person(w.admin_a.clone()), org: w.a.clone(), view_as: None }),
        ("A's power user", As { who: Who::Person(w.power_a.clone()), org: w.a.clone(), view_as: None }),
        ("A's viewer", As { who: Who::Person(w.viewer_a.clone()), org: w.a.clone(), view_as: None }),
        ("B's Org admin", As { who: Who::Person(w.admin_b.clone()), org: w.b.clone(), view_as: None }),
        ("A's write token", As { who: Who::Token(w.write_token.1.clone()), org: w.a.clone(), view_as: None }),
        ("a guest", As { who: Who::Cookie(w.guest_cookie.clone()), org: String::new(), view_as: None }),
        ("anyone", As { who: Who::Anonymous, org: String::new(), view_as: None }),
    ];
    let mut leaks = vec![];
    let mut check = |what: String, reply: &super::Reply| {
        let headers = format!("{:?}", reply.headers);
        // a guest's own cookie may come back to them; nothing else may
        let text = format!("{}\n{}", reply.body, headers.replace(&w.guest_cookie, ""));
        for c in found(&w, &text) {
            // by design, links can be copied again by the people who manage them: a public link
            // (Org admins; it's meant to be shared), a guest link (Power users; it needs its password)
            if (c == w.share_token && what.starts_with("GET /api/dashboards/{id}/share-links"))
                || (c == w.guest_token && what.starts_with("GET /api/dashboards/{id}/guest-links"))
            {
                continue;
            }
            leaks.push(format!("{what}: {c}"));
        }
    };
    // reading everything there is to read, as everyone
    for &(method, path, min) in MATRIX {
        if method != "GET" || (min == Min::Open && path.contains("/auth/")) {
            continue;
        }
        for (name, who) in &callers {
            let r = w.send(who, method, &fill(&w, path), None).await;
            check(format!("GET {path} as {name}"), &r);
        }
    }
    // the tokens and links lists, everyone's
    let admin = &callers[1].1;
    check("GET /api/tokens?all=true".into(), &w.send(admin, "GET", "/api/tokens?all=true", None).await);
    // errors that involve the secrets: tests against servers that aren't there
    let owner = &callers[0].1;
    for (method, path) in [
        ("POST", "/api/delivery/mail/test"),
        ("POST", "/api/delivery/webhooks/team/test"),
        ("POST", "/api/connections/decoy/test"),
        ("GET", "/api/connections/decoy/tables"),
    ] {
        let r = w.send(owner, method, path, Some(json!({}))).await;
        check(format!("{method} {path} (failing)"), &r);
    }
    // the stored password, tried against a server that isn't there
    let decoy = json!({"config": {"type": "postgres", "host": "127.0.0.1", "port": 1, "database": "x", "user": "x", "tls": false}});
    let r = w.send(owner, "POST", "/api/connections/decoy/test", Some(decoy)).await;
    check("testing a connection".into(), &r);
    // what admins see of the activity
    check("GET /api/usage".into(), &w.send(owner, "GET", "/api/usage", None).await);
    assert!(leaks.is_empty(), "secrets that came back:\n{}", leaks.join("\n"));

    let logged = String::from_utf8_lossy(&log().lock().unwrap()).into_owned();
    let in_log = found(&w, &logged);
    let lines: Vec<&str> = logged.lines().filter(|l| in_log.iter().any(|c| l.contains(c.as_str()))).take(5).collect();
    assert!(in_log.is_empty(), "secrets in the server's log: {in_log:?}\n{}", lines.join("\n"));
    w.clean_up().await;
}
