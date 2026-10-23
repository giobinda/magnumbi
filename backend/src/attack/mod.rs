//! The attack suite: uses and misuses the API the way an outsider, a curious colleague or another
//! organisation would, and fails when anything gets through.
//!
//! Every test builds a [`World`] on the test ScramDB (MAGNUM_TEST_DATABASE_URL; skipped without
//! it): two organisations with real people at every role, dashboards with the same id in both,
//! public and guest links, API tokens, delivery settings, and a unique marker planted in every
//! secret. The parts:
//!
//! - `routes`: every route of the router is in the access matrix; who may call each one, and that
//!   nobody else gets through (other roles, other organisations, forged headers, read tokens).
//! - `secrets`: no planted secret comes back in any response or log line.
//! - `abuse`: brute force, cross-site requests, floods of visitors, sessions and tokens that ended.
//! - `hostile`: injection, traversal, bombs and absurd values in every field are refused cleanly.
//! - `row_filters`: a restricted organisation's answers equal an unrestricted person's answers
//!   with the restriction written into the query, for random queries, on every path to data.

mod abuse;
mod hostile;
mod routes;
mod row_filters;
mod secrets;

use std::sync::{Arc, OnceLock};

use axum::http::{HeaderMap, StatusCode};
use serde_json::{Value, json};

use crate::api::tests::{OWNER, app, scramdb_config};
use crate::state::AppState;

/// Who is asking.
#[derive(Clone, Debug)]
pub enum Who {
    Anonymous,
    /// a signed-in person (session cookie)
    Person(String),
    /// an API token
    Token(String),
    /// a raw Cookie header (guest links, forged sessions)
    Cookie(String),
}

/// A request as someone, in an organisation ("" for the platform workspace or none).
#[derive(Clone, Debug)]
pub struct As {
    pub who: Who,
    pub org: String,
    pub view_as: Option<String>,
}

pub struct Reply {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: String,
}

impl Reply {
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }
}

/// Marks a planted secret: whatever contains it leaked.
pub fn canary(what: &str) -> String {
    format!("canary-{what}-{}", uuid::Uuid::new_v4().simple())
}

pub struct World {
    pub app: axum::Router,
    pub state: Arc<AppState>,
    /// organisation ids
    pub a: String,
    pub b: String,
    /// people, by role in A (and B's Org admin): email -> session cookie
    pub cookies: Vec<(String, String)>,
    pub admin_a: String,
    pub power_a: String,
    pub user_a: String,
    pub viewer_a: String,
    pub admin_b: String,
    /// A's dashboard (B has one with the same id)
    pub dashboard: String,
    pub share_id: String,
    pub share_token: String,
    pub guest_id: String,
    pub guest_token: String,
    pub guest_cookie: String,
    /// admin_a's tokens: (id, token)
    pub read_token: (String, String),
    pub write_token: (String, String),
    /// B's Org admin's write token
    pub token_b: String,
    /// a signed-link report (its password is a planted secret)
    pub schedule_id: String,
    /// every secret planted, to look for in what comes back
    pub canaries: Vec<String>,
    suffix: String,
}

/// One password hash for every test person (hashing is slow on purpose).
fn test_hash() -> &'static str {
    static HASH: OnceLock<String> = OnceLock::new();
    HASH.get_or_init(|| crate::auth::password::hash("attack-suite-password").unwrap())
}

pub const DASHBOARD: &str = "title: Attack target\nfilters:\n  - {model: sales, field: region}\ntiles:\n  - {id: t, type: bar, drill: location, query: {model: sales, dimensions: [region], measures: [transactions]}}\n  - {id: n, type: kpi, query: {model: sales, measures: [transactions]}}\n  - {id: note, type: text, text: hi}\n";

impl World {
    pub async fn new() -> Option<World> {
        let (app, state, owner_cookie) = app().await?;
        let suffix = uuid::Uuid::new_v4().simple().to_string()[..10].to_string();
        let (a, b) = (format!("atka{suffix}"), format!("atkb{suffix}"));
        let email = |role: &str| format!("{role}-{suffix}@attack.example");
        let (admin_a, power_a, user_a, viewer_a, admin_b) =
            (email("admin-a"), email("power-a"), email("user-a"), email("viewer-a"), email("admin-b"));
        let key = state.config.secret_key.clone().unwrap();
        let oidc_secret = canary("oidc");
        let access = json!({"system_owners": [OWNER], "organisations": {
            a.clone(): {"name": "Attack A", "models": ["*"], "members": {"users": {
                admin_a.clone(): "org_admin", power_a.clone(): "power_user", user_a.clone(): "user", viewer_a.clone(): "viewer"},
                "oidc": {"label": "A's provider", "issuer": "https://login.a.example", "client_id": "a",
                         "client_secret_sealed": key.seal(&oidc_secret, &format!("{a}/oidc")).unwrap()}}},
            // everyone in B, its Org admin included, sees only EMEA's sales (attack::row_filters)
            b.clone(): {"name": "Attack B", "models": ["*"], "filters": [{"model": "sales", "dimension": "region", "values": ["EMEA"]}],
                        "members": {"users": {admin_b.clone(): "org_admin"}}}}});
        state.access.pin(crate::access::AccessConfig::from_document(access).unwrap());
        state.orgs.pin(&a, state.store.clone());
        state.orgs.pin(&b, state.store.clone());

        let mut cookies = vec![(OWNER.to_string(), owner_cookie)];
        for e in [&admin_a, &power_a, &user_a, &viewer_a, &admin_b] {
            state.store.set_password(e, None, test_hash(), None, false).await.unwrap();
            let s = state.store.create_session_as(e, "password", chrono::Duration::hours(1), None).await.unwrap();
            cookies.push((e.clone(), format!("magnum_session={s}")));
        }
        let mut w = World {
            app,
            state,
            a: a.clone(),
            b: b.clone(),
            cookies,
            admin_a,
            power_a,
            user_a,
            viewer_a,
            admin_b,
            dashboard: "target".into(),
            share_id: String::new(),
            share_token: String::new(),
            guest_id: String::new(),
            guest_token: String::new(),
            guest_cookie: String::new(),
            read_token: Default::default(),
            write_token: Default::default(),
            token_b: String::new(),
            schedule_id: String::new(),
            canaries: vec![oidc_secret],
            suffix,
        };
        w.furnish().await;
        Some(w)
    }

    /// The organisations' data, dashboards, links, tokens and secrets.
    async fn furnish(&mut self) {
        let (a, b) = (self.a.clone(), self.b.clone());
        let owner = |org: &str| As { who: Who::Person(OWNER.into()), org: org.into(), view_as: None };
        let ok = |r: Reply, what: &str| {
            assert!(r.status.is_success(), "setting up {what}: {} {}", r.status, r.body);
            r.json()
        };
        let warehouse = json!({"config": scramdb_config()});
        for org in [&a, &b] {
            ok(self.send(&owner(org), "PUT", "/api/connections/warehouse", Some(warehouse.clone())).await, "warehouse");
        }
        let decoy = canary("connection-password");
        let config =
            json!({"type": "postgres", "host": "127.0.0.1", "port": 1, "database": "x", "user": "x", "tls": false});
        let r = self
            .send(&owner(&a), "PUT", "/api/connections/decoy", Some(json!({"config": config, "secret": decoy})))
            .await;
        ok(r, "decoy connection");
        self.canaries.push(decoy);
        ok(
            self.yaml(&owner(&a), &format!("/api/dashboards/{}/yaml", self.dashboard), DASHBOARD).await,
            "A's dashboard",
        );
        let b_dashboard = DASHBOARD.replace("Attack target", "B's private dashboard");
        ok(self.yaml(&owner(&b), &format!("/api/dashboards/{}/yaml", self.dashboard), &b_dashboard).await, "B's");

        let links = format!("/api/dashboards/{}/share-links", self.dashboard);
        let share = ok(self.send(&owner(&a), "POST", &links, Some(json!({"cache_seconds": 300}))).await, "share link");
        self.share_id = share["id"].as_str().unwrap().into();
        self.share_token = share["url"].as_str().unwrap().rsplit('/').next().unwrap().into();
        self.canaries.push(self.share_token.clone());

        let password = canary("guest-password");
        let body = json!({"expires_in_hours": 1, "password": password, "role": "user"});
        let guests = format!("/api/dashboards/{}/guest-links", self.dashboard);
        let guest = ok(self.send(&owner(&a), "POST", &guests, Some(body)).await, "guest link");
        self.guest_id = guest["id"].as_str().unwrap().into();
        self.guest_token = guest["url"].as_str().unwrap().rsplit('/').next().unwrap().into();
        let unlock = format!("/api/guest/{a}/{}/unlock", self.guest_token);
        let anyone = As { who: Who::Anonymous, org: String::new(), view_as: None };
        let r = self.send(&anyone, "POST", &unlock, Some(json!({"password": password}))).await;
        assert_eq!(r.status, StatusCode::OK, "unlocking the guest link: {}", r.body);
        let set = r.headers["set-cookie"].to_str().unwrap();
        self.guest_cookie = set.split(';').next().unwrap().to_string();
        self.canaries.extend([password, self.guest_token.clone()]);

        let admin = |org: &str, who: &str| As { who: Who::Person(who.into()), org: org.into(), view_as: None };
        let token =
            |r: Value| (r["id"].to_string().trim_matches('"').to_string(), r["token"].as_str().unwrap().to_string());
        let body = |scope: &str| json!({"name": format!("attack {scope}"), "scope": scope});
        self.read_token =
            token(ok(self.send(&admin(&a, &self.admin_a), "POST", "/api/tokens", Some(body("read"))).await, "token"));
        self.write_token =
            token(ok(self.send(&admin(&a, &self.admin_a), "POST", "/api/tokens", Some(body("write"))).await, "token"));
        self.token_b =
            token(ok(self.send(&admin(&b, &self.admin_b), "POST", "/api/tokens", Some(body("write"))).await, "token"))
                .1;
        self.canaries.extend([self.read_token.1.clone(), self.write_token.1.clone(), self.token_b.clone()]);

        let mail_password = canary("mail-password");
        let mail = json!({"host": "smtp.attack.example", "port": 587, "security": "starttls", "username": "bi",
                          "password": mail_password, "from": "bi@attack.example"});
        ok(self.send(&owner(&a), "PUT", "/api/delivery/mail", Some(mail)).await, "mail server");
        let hook = format!("https://hooks.attack.example/{}", canary("webhook"));
        let r = self
            .send(&owner(&a), "PUT", "/api/delivery/webhooks/team", Some(json!({"kind": "slack", "url": hook})))
            .await;
        ok(r, "webhook");
        self.canaries.extend([mail_password, hook]);

        let report_password = canary("report-password");
        let report = json!({"name": "Attack report", "format": "signed", "recipients": [OWNER], "password": report_password,
                            "when": {"every": "daily", "at": "08:00", "timezone": "UTC"}});
        let path = format!("/api/dashboards/{}/schedules", self.dashboard);
        let r = ok(self.send(&owner(&a), "POST", &path, Some(report)).await, "scheduled report");
        self.schedule_id = r["id"].as_str().unwrap().into();
        self.canaries.push(report_password);

        // the chat's API key: set, never seen again (a provider nobody can reach: never called)
        let chat_key = canary("chat-key");
        let chat = json!({"settings": {"enabled": false, "format": "openai", "url": "https://llm.attack.example/v1",
                                       "model": "attack-model"}, "key": chat_key});
        ok(self.send(&owner(&a), "PUT", "/api/chat/settings", Some(chat)).await, "chat settings");
        self.canaries.push(chat_key);
    }

    /// The cookie of someone in the world.
    pub fn cookie(&self, email: &str) -> &str {
        &self.cookies.iter().find(|(e, _)| e == email).unwrap_or_else(|| panic!("no session for {email}")).1
    }

    pub async fn send(&self, who: &As, method: &str, path: &str, body: Option<Value>) -> Reply {
        let (ct, body) = match body {
            Some(b) => (Some("application/json"), b.to_string()),
            None => (None, String::new()),
        };
        self.raw(who, method, path, ct, body).await
    }

    pub async fn yaml(&self, who: &As, path: &str, yaml: &str) -> Reply {
        self.raw(who, "PUT", path, Some("text/plain"), yaml.to_string()).await
    }

    pub async fn raw(&self, who: &As, method: &str, path: &str, content_type: Option<&str>, body: String) -> Reply {
        self.raw_with(who, method, path, &[], content_type, body).await
    }

    /// A request with extra headers (Origin, Sec-Fetch-Site, ...).
    pub async fn raw_with(
        &self,
        who: &As,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        content_type: Option<&str>,
        body: String,
    ) -> Reply {
        let mut req = axum::http::Request::builder().method(method).uri(path);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        match &who.who {
            Who::Anonymous => {}
            Who::Person(email) => req = req.header("cookie", self.cookie(email)),
            Who::Token(t) => req = req.header("authorization", format!("Bearer {t}")),
            Who::Cookie(c) => req = req.header("cookie", c.as_str()),
        }
        if !who.org.is_empty() {
            req = req.header("x-magnum-org", who.org.as_str());
        }
        if let Some(r) = &who.view_as {
            req = req.header("x-magnum-view-as", r.as_str());
        }
        if let Some(ct) = content_type {
            req = req.header("content-type", ct);
        }
        let resp = tower::ServiceExt::oneshot(self.app.clone(), req.body(axum::body::Body::from(body)).unwrap())
            .await
            .unwrap();
        let (status, headers) = (resp.status(), resp.headers().clone());
        let bytes = http_body_util::BodyExt::collect(resp.into_body()).await.unwrap().to_bytes();
        if status == axum::http::StatusCode::UNAUTHORIZED
            && let Who::Person(email) = &who.who
        {
            self.why_signed_out(email, &format!("{method} {path}")).await;
        }
        Reply { status, headers, body: String::from_utf8_lossy(&bytes).into_owned() }
    }

    /// A person of this world was told to sign in again: what the database holds of them then (a
    /// session that vanished mid-run once failed the suite; this tells a deleted session, a deleted
    /// account and a read that missed rows that are there apart).
    async fn why_signed_out(&self, email: &str, request: &str) {
        let token = self.cookie(email).trim_start_matches("magnum_session=").to_string();
        let c = self.state.store.pool.get().await.unwrap();
        let session = c
            .query_opt(
                "SELECT expires_at::TEXT, now()::TEXT FROM sessions WHERE id_hash = $1::TEXT",
                &[&crate::store::sha256_hex(&token)],
            )
            .await
            .map(|r| r.map(|r| (r.get::<_, String>(0), r.get::<_, String>(1))));
        let user = c
            .query_opt("SELECT disabled FROM users WHERE email = $1::TEXT", &[&email])
            .await
            .map(|r| r.map(|r| r.get::<_, bool>(0)));
        let again = self.state.store.session(&token).await.map(|s| s.is_some());
        let sessions = c
            .query_one("SELECT COUNT(*) FROM sessions WHERE user_email = $1::TEXT", &[&email])
            .await
            .map(|r| r.get::<_, i64>(0));
        eprintln!(
            "SIGNED OUT {email} on {request}: session (expires, now) {session:?}; account disabled? {user:?}; \
             looked up again: {again:?}; their sessions: {sessions:?}"
        );
    }

    /// Remove what the world made in the shared database.
    pub async fn clean_up(self) {
        for org in [&self.a, &self.b] {
            crate::api::share::tests::clean_up(&self.state, org).await;
        }
        let c = self.state.store.pool.get().await.unwrap();
        let people = format!("%-{}@attack.example", self.suffix);
        for sql in [
            "DELETE FROM api_tokens WHERE user_email LIKE $1",
            "DELETE FROM sessions WHERE user_email LIKE $1",
            "DELETE FROM users WHERE email LIKE $1",
        ] {
            c.execute(sql, &[&people]).await.unwrap();
        }
    }
}
