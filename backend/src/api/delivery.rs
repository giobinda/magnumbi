//! A tenant's delivery settings: its mail server and webhooks (delivery.rs). Each organisation
//! has its own, and the platform workspace its own; none is shared or used for another tenant.
//! Org admins manage them (system owners in the platform workspace). Secrets are write-only.

use axum::Json;
use axum::extract::{Path, Query as UrlQuery, State};
use serde::Deserialize;
use serde_json::{Value, json};

use super::Body;
use super::dashboards::WriteParams;
use crate::auth::{Auth, Caller};
use crate::crypto::SecretKey;
use crate::delivery::{Mail, MailServer, Message, WebhookKind, payload, post_webhook, send_mail, webhook_host};
use crate::error::{ApiError, ApiResult};
use crate::state::{AppState, Shared};
use crate::store::Store;

/// The caller's tenant: its id ("" for the platform workspace) and its own database.
async fn tenant(state: &AppState, caller: &Caller) -> ApiResult<(String, Store)> {
    caller.require("manage_members")?;
    let org = caller.org().to_string();
    let store = state.orgs.store(&org).await?;
    Ok((org, store))
}

fn key(state: &AppState) -> ApiResult<&SecretKey> {
    state.config.secret_key.as_ref().ok_or_else(|| {
        ApiError::bad_request("MAGNUM_SECRET_KEY isn't set, so mail and webhook secrets can't be stored")
    })
}

fn mail_context(org: &str) -> String {
    format!("{org}/mail")
}

fn webhook_context(org: &str, name: &str) -> String {
    format!("{org}/webhook/{name}")
}

/// Who a test message says it comes from.
fn tenant_name(caller: &Caller) -> String {
    match caller.access.platform.organisations.get(caller.org()) {
        Some(o) => o.name.clone(),
        None => "the platform workspace".into(),
    }
}

fn test_message(caller: &Caller) -> Message {
    Message {
        title: "Magnum BI: test message".into(),
        text: format!("Reports and alerts from {} can be delivered here.", tenant_name(caller)),
        link: None,
    }
}

pub async fn get(State(state): State<Shared>, Auth(caller): Auth) -> ApiResult<Json<Value>> {
    let (org, store) = tenant(&state, &caller).await?;
    let mail = store.mail_server(&org).await?.map(|(m, password)| {
        let mut v = serde_json::to_value(m).unwrap_or_default();
        v["has_password"] = json!(password.is_some());
        v
    });
    let webhooks: Vec<Value> = store
        .webhooks(&org)
        .await?
        .into_iter()
        .map(|(name, kind, host)| json!({"name": name, "kind": kind, "host": host}))
        .collect();
    Ok(Json(json!({"mail": mail, "webhooks": webhooks, "secret_key": state.config.secret_key.is_some()})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MailInput {
    host: String,
    port: u16,
    security: crate::delivery::Security,
    #[serde(default)]
    username: Option<String>,
    from: String,
    /// besides members, the email domains reports may go to
    #[serde(default)]
    allowed_domains: Vec<String>,
    /// absent: keep the one stored; empty: none
    #[serde(default)]
    password: Option<String>,
}

/// Set the mail server (`?dry_run=1`: check only).
pub async fn put_mail(
    State(state): State<Shared>,
    Auth(caller): Auth,
    UrlQuery(p): UrlQuery<WriteParams>,
    Body(input): Body<MailInput>,
) -> ApiResult<Json<Value>> {
    let (org, store) = tenant(&state, &caller).await?;
    let i = &input;
    let username = i.username.as_deref().map(str::trim).filter(|u| !u.is_empty()).map(String::from);
    let server = MailServer {
        host: i.host.trim().to_string(),
        port: i.port,
        security: i.security,
        username,
        from: i.from.trim().to_string(),
        allowed_domains: i
            .allowed_domains
            .iter()
            .map(|d| d.trim().trim_start_matches('@').to_lowercase())
            .filter(|d| !d.is_empty())
            .collect(),
    };
    let kept = match input.password {
        Some(_) => None,
        None => store.mail_server(&org).await?.and_then(|(_, sealed)| sealed),
    };
    let sealed = match input.password.as_deref() {
        Some("") => None,
        Some(p) => Some(key(&state)?.seal(p, &mail_context(&org))?),
        None => kept,
    };
    let sealed = if server.username.is_some() { sealed } else { None }; // a password goes with a user name
    server.check(sealed.is_some()).map_err(ApiError::bad_request)?;
    if p.dry_run {
        return Ok(Json(json!({"ok": true, "dry_run": true})));
    }
    store.put_mail_server(&org, &server, sealed.as_deref(), Some(caller.email())).await?;
    Ok(Json(json!({"ok": true})))
}

pub async fn delete_mail(State(state): State<Shared>, Auth(caller): Auth) -> ApiResult<Json<Value>> {
    let (org, store) = tenant(&state, &caller).await?;
    Ok(Json(json!({"ok": store.delete_mail_server(&org).await?})))
}

/// Send a test message to the person asking (and to no one else).
pub async fn test_mail(State(state): State<Shared>, Auth(caller): Auth) -> ApiResult<Json<Value>> {
    let (org, store) = tenant(&state, &caller).await?;
    let (server, sealed) =
        store.mail_server(&org).await?.ok_or_else(|| ApiError::bad_request("no mail server is set up here"))?;
    let password = match sealed {
        Some(s) => Some(key(&state)?.open(&s, &mail_context(&org))?),
        None => None,
    };
    let m = test_message(&caller);
    let mail = Mail { to: vec![caller.email().to_string()], subject: m.title, text: m.text, attachments: vec![] };
    send_mail(state.connectors.net_guard(), &server, password.as_deref(), &mail)
        .await
        .map_err(ApiError::bad_request)?;
    Ok(Json(json!({"sent_to": caller.email()})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebhookInput {
    kind: WebhookKind,
    /// absent: keep the one stored
    #[serde(default)]
    url: Option<String>,
}

fn kind_name(kind: WebhookKind) -> &'static str {
    match kind {
        WebhookKind::Slack => "slack",
        WebhookKind::Teams => "teams",
        WebhookKind::Webhook => "webhook",
    }
}

/// Add or change a webhook (`?dry_run=1`: check only).
pub async fn put_webhook(
    State(state): State<Shared>,
    Auth(caller): Auth,
    Path(name): Path<String>,
    UrlQuery(p): UrlQuery<WriteParams>,
    Body(input): Body<WebhookInput>,
) -> ApiResult<Json<Value>> {
    let (org, store) = tenant(&state, &caller).await?;
    crate::connections::check_name(&name)
        .map_err(|_| ApiError::bad_request("a webhook's name: letters, digits and underscores"))?;
    let key = key(&state)?;
    let url = match input.url {
        Some(url) => url.trim().to_string(),
        None => {
            let (_, sealed) = store
                .webhook(&org, &name)
                .await?
                .ok_or_else(|| ApiError::bad_request("a new webhook needs its address"))?;
            key.open(&sealed, &webhook_context(&org, &name))?
        }
    };
    let host = webhook_host(&url, state.connectors.net_guard().allow_private).map_err(ApiError::bad_request)?;
    if p.dry_run {
        return Ok(Json(json!({"ok": true, "dry_run": true})));
    }
    let sealed = key.seal(&url, &webhook_context(&org, &name))?;
    store.put_webhook(&org, &name, (kind_name(input.kind), &host, &sealed), Some(caller.email())).await?;
    Ok(Json(json!({"ok": true})))
}

pub async fn delete_webhook(
    State(state): State<Shared>,
    Auth(caller): Auth,
    Path(name): Path<String>,
) -> ApiResult<Json<Value>> {
    let (org, store) = tenant(&state, &caller).await?;
    Ok(Json(json!({"ok": store.delete_webhook(&org, &name).await?})))
}

pub async fn test_webhook(
    State(state): State<Shared>,
    Auth(caller): Auth,
    Path(name): Path<String>,
) -> ApiResult<Json<Value>> {
    let (org, store) = tenant(&state, &caller).await?;
    let (kind, sealed) =
        store.webhook(&org, &name).await?.ok_or_else(|| ApiError::not_found(format!("no webhook {name:?}")))?;
    let url = key(&state)?.open(&sealed, &webhook_context(&org, &name))?;
    let kind: WebhookKind = serde_json::from_value(json!(kind)).map_err(|e| ApiError::Internal(e.into()))?;
    post_webhook(state.connectors.net_guard(), &url, &payload(kind, &test_message(&caller)))
        .await
        .map_err(ApiError::bad_request)?;
    Ok(Json(json!({"ok": true})))
}

#[cfg(test)]
pub(crate) mod tests {
    use axum::http::StatusCode;
    use serde_json::{Value, json};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::sync::mpsc;

    use crate::api::share::tests::{clean_up, org_with, send};
    use crate::api::tests::OWNER;

    /// A stand-in mail server: accepts everything, hands over each message's text.
    pub(crate) async fn fake_smtp() -> (u16, mpsc::UnboundedReceiver<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let tx = tx.clone();
                tokio::spawn(async move {
                    let (read, mut write) = socket.into_split();
                    let mut lines = BufReader::new(read).lines();
                    write.write_all(b"220 fake ESMTP\r\n").await.unwrap();
                    let (mut data, mut message) = (false, String::new());
                    while let Ok(Some(line)) = lines.next_line().await {
                        let reply: &[u8] = if data {
                            if line == "." {
                                data = false;
                                tx.send(std::mem::take(&mut message)).unwrap();
                                b"250 queued\r\n"
                            } else {
                                message.push_str(&line);
                                message.push('\n');
                                continue;
                            }
                        } else if line.starts_with("DATA") {
                            data = true;
                            b"354 go on\r\n"
                        } else if line.starts_with("QUIT") {
                            write.write_all(b"221 bye\r\n").await.unwrap();
                            break;
                        } else {
                            b"250 ok\r\n"
                        };
                        write.write_all(reply).await.unwrap();
                    }
                });
            }
        });
        (port, rx)
    }

    /// A stand-in webhook: hands over each JSON body.
    pub(crate) async fn fake_webhook() -> (u16, mpsc::UnboundedReceiver<Value>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let app = axum::Router::new().route(
            "/hook",
            axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
                let tx = tx.clone();
                async move { tx.send(body).unwrap() }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (port, rx)
    }

    #[tokio::test]
    async fn each_tenant_has_its_own_mail_server_and_webhooks() {
        let Some((app, state, cookie, org)) = org_with("title: x").await else { return };
        let other = format!("other{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
        state.orgs.pin(&other, state.store.clone());
        let access = json!({"system_owners": [OWNER], "organisations": {
            org.clone(): {"name": "Delivery test", "models": ["*"]}, other.clone(): {"name": "Other", "models": ["*"]}}});
        state.access.pin(crate::access::AccessConfig::from_document(access).unwrap());
        let c = Some(cookie.as_str());
        let (smtp, mut mails) = fake_smtp().await;
        let (hook, mut posts) = fake_webhook().await;

        let (s, d, _) = send(&app, c, &org, "GET", "/api/delivery", None, None).await;
        assert_eq!(s, StatusCode::OK, "{d}");
        assert_eq!((&d["mail"], &d["webhooks"]), (&Value::Null, &json!([])));

        // a mail server: a password only over TLS, and never sent back
        let server = json!({"host": "127.0.0.1", "port": smtp, "security": "none", "from": "Reports <bi@example.com>"});
        let mut with_password = server.clone();
        with_password["username"] = json!("bi");
        with_password["password"] = json!("s3cret-pw");
        let (s, b, _) = send(&app, c, &org, "PUT", "/api/delivery/mail", Some(with_password), None).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{b}");
        let (s, b, _) = send(&app, c, &org, "PUT", "/api/delivery/mail", Some(server.clone()), None).await;
        assert_eq!(s, StatusCode::OK, "{b}");
        let (_, d, _) = send(&app, c, &org, "GET", "/api/delivery", None, None).await;
        assert_eq!(d["mail"]["host"], json!("127.0.0.1"));
        assert_eq!(d["mail"]["has_password"], json!(false));

        // a test message goes to the person asking, through this organisation's server
        let (s, b, _) = send(&app, c, &org, "POST", "/api/delivery/mail/test", None, None).await;
        assert_eq!(s, StatusCode::OK, "{b}");
        assert_eq!(b["sent_to"], json!(OWNER));
        let mail = mails.recv().await.unwrap();
        assert!(mail.contains(&format!("To: {OWNER}")) && mail.contains("Subject: Magnum BI: test message"), "{mail}");
        assert!(mail.contains("Delivery test"), "names the organisation: {mail}");

        // a webhook: its address is a secret (only its host is shown)
        let url = format!("http://127.0.0.1:{hook}/hook");
        let bad = json!({"kind": "slack", "url": "ftp://example.com/x"});
        let (s, _, _) = send(&app, c, &org, "PUT", "/api/delivery/webhooks/team", Some(bad), None).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        let (s, b, _) =
            send(&app, c, &org, "PUT", "/api/delivery/webhooks/team", Some(json!({"kind": "slack", "url": url})), None)
                .await;
        assert_eq!(s, StatusCode::OK, "{b}");
        let (_, d, _) = send(&app, c, &org, "GET", "/api/delivery", None, None).await;
        assert_eq!(d["webhooks"], json!([{"name": "team", "kind": "slack", "host": "127.0.0.1"}]));
        assert!(!d.to_string().contains("/hook"), "the address isn't sent back: {d}");
        let (s, b, _) = send(&app, c, &org, "POST", "/api/delivery/webhooks/team/test", None, None).await;
        assert_eq!(s, StatusCode::OK, "{b}");
        let post = posts.recv().await.unwrap();
        assert!(post["text"].as_str().unwrap().starts_with("*Magnum BI: test message*"), "{post}");

        // another organisation, and the platform workspace, see none of it
        for tenant in [other.as_str(), ""] {
            let (s, d, _) = send(&app, c, tenant, "GET", "/api/delivery", None, None).await;
            assert_eq!(s, StatusCode::OK, "{d}");
            assert_eq!(d["mail"], Value::Null, "{tenant:?}");
            assert!(!d["webhooks"].as_array().unwrap().iter().any(|w| w["name"] == "team"), "{tenant:?}: {d}");
        }
        let (s, _, _) = send(&app, c, &other, "POST", "/api/delivery/mail/test", None, None).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "no mail server there");

        // Org admins only
        let (s, _, _) = send(&app, c, &org, "GET", "/api/delivery", None, Some("power_user")).await;
        assert_eq!(s, StatusCode::FORBIDDEN);

        // removing
        let (s, _, _) = send(&app, c, &org, "DELETE", "/api/delivery/webhooks/team", None, None).await;
        assert_eq!(s, StatusCode::OK);
        let (s, _, _) = send(&app, c, &org, "DELETE", "/api/delivery/mail", None, None).await;
        assert_eq!(s, StatusCode::OK);
        let (_, d, _) = send(&app, c, &org, "GET", "/api/delivery", None, None).await;
        assert_eq!((&d["mail"], &d["webhooks"]), (&Value::Null, &json!([])));
        clean_up(&state, &org).await;
    }

    #[tokio::test]
    async fn a_kept_password_stays_sealed_and_can_be_replaced() {
        let Some((app, state, cookie, org)) = org_with("title: x").await else { return };
        let c = Some(cookie.as_str());
        let tls = json!({"host": "smtp.example.com", "port": 587, "security": "starttls", "username": "bi",
                         "password": "first-pw", "from": "bi@example.com", "allowed_domains": ["@Partner.example"]});
        let (s, b, _) = send(&app, c, &org, "PUT", "/api/delivery/mail", Some(tls.clone()), None).await;
        assert_eq!(s, StatusCode::OK, "{b}");
        // saving again without a password keeps the one there
        let mut again = tls.clone();
        again.as_object_mut().unwrap().remove("password");
        again["port"] = json!(2525);
        let (s, b, _) = send(&app, c, &org, "PUT", "/api/delivery/mail", Some(again), None).await;
        assert_eq!(s, StatusCode::OK, "{b}");
        let (_, d, _) = send(&app, c, &org, "GET", "/api/delivery", None, None).await;
        assert_eq!((&d["mail"]["port"], &d["mail"]["has_password"]), (&json!(2525), &json!(true)));
        assert_eq!(d["mail"]["allowed_domains"], json!(["partner.example"]));
        assert!(!d.to_string().contains("first-pw"));
        let row = state.store.mail_server(&org).await.unwrap().unwrap();
        assert!(!row.1.unwrap().contains("first-pw"), "sealed at rest");
        clean_up(&state, &org).await;
    }
}
