//! Delivery: how reports and alerts leave Magnum BI, by email or webhook.
//!
//! Every tenant (each organisation, and the platform workspace) has its own mail server and
//! webhooks, in its own database with its secrets sealed for it; nothing is shared or inherited.
//! Their addresses are the tenant's, so they go through the network guard like connections.

use std::str::FromStr;
use std::time::Duration;

use lettre::message::header::ContentType;
use lettre::message::{Attachment, Mailbox, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::netguard::NetGuard;

/// How the connection to the mail server is protected.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Security {
    /// plain connection upgraded to TLS (usually port 587)
    Starttls,
    /// TLS from the start (usually port 465)
    Tls,
    /// no encryption: an internal relay without a password
    None,
}

/// A tenant's mail server (its password is kept apart, sealed).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MailServer {
    pub host: String,
    pub port: u16,
    pub security: Security,
    #[serde(default)]
    pub username: Option<String>,
    /// the sender, e.g. "Reports <reports@example.com>"
    pub from: String,
}

impl MailServer {
    pub fn check(&self, has_password: bool) -> Result<(), String> {
        if self.host.trim().is_empty() || self.port == 0 {
            return Err("the mail server needs a host and a port".into());
        }
        Mailbox::from_str(&self.from).map_err(|_| format!("{:?} isn't an email address", self.from))?;
        let user = self.username.as_deref().is_some_and(|u| !u.trim().is_empty());
        if user && !has_password {
            return Err("a user name needs its password".into());
        }
        if (user || has_password) && self.security == Security::None {
            return Err("a password is only sent over TLS: choose STARTTLS or TLS".into());
        }
        Ok(())
    }
}

/// An email: plain text, and files attached.
pub struct Mail {
    pub to: Vec<String>,
    pub subject: String,
    pub text: String,
    /// (file name, media type, bytes)
    pub attachments: Vec<(String, String, Vec<u8>)>,
}

fn email(server: &MailServer, mail: &Mail) -> Result<lettre::Message, String> {
    let mut b = lettre::Message::builder().from(Mailbox::from_str(&server.from).map_err(|e| e.to_string())?);
    for to in &mail.to {
        b = b.to(Mailbox::from_str(to).map_err(|_| format!("{to:?} isn't an email address"))?);
    }
    let b = b.subject(&mail.subject);
    let built = if mail.attachments.is_empty() {
        b.header(ContentType::TEXT_PLAIN).body(mail.text.clone())
    } else {
        let mut parts = MultiPart::mixed().singlepart(SinglePart::plain(mail.text.clone()));
        for (name, mime, bytes) in &mail.attachments {
            let kind = ContentType::parse(mime).map_err(|e| e.to_string())?;
            parts = parts.singlepart(Attachment::new(name.clone()).body(bytes.clone(), kind));
        }
        b.multipart(parts)
    };
    built.map_err(|e| e.to_string())
}

/// Send through the tenant's server: connect to the address the guard checked, verify TLS
/// against the server's name.
pub async fn send_mail(
    guard: &NetGuard,
    server: &MailServer,
    password: Option<&str>,
    mail: &Mail,
) -> Result<(), String> {
    server.check(password.is_some())?;
    let addr = guard.check(&server.host, server.port).await?;
    let params = || TlsParameters::new(server.host.trim().to_lowercase()).map_err(|e| format!("TLS: {e}"));
    let tls = match server.security {
        Security::Starttls => Tls::Required(params()?),
        Security::Tls => Tls::Wrapper(params()?),
        Security::None => Tls::None,
    };
    let mut b = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(addr.ip().to_string())
        .port(addr.port())
        .tls(tls)
        .timeout(Some(Duration::from_secs(20)));
    if let (Some(user), Some(password)) = (server.username.as_deref().filter(|u| !u.trim().is_empty()), password) {
        b = b.credentials(Credentials::new(user.to_string(), password.to_string()));
    }
    let message = email(server, mail)?;
    b.build().send(message).await.map(|_| ()).map_err(|e| format!("the mail server refused it: {e}"))
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum WebhookKind {
    Slack,
    Teams,
    /// any address taking JSON: {title, text, link}
    Webhook,
}

/// What a webhook says: a title, a few lines, a link back.
pub struct Message {
    pub title: String,
    pub text: String,
    pub link: Option<String>,
}

pub fn payload(kind: WebhookKind, m: &Message) -> Value {
    match kind {
        WebhookKind::Slack => {
            let link = m.link.as_ref().map(|l| format!("\n<{l}|Open in Magnum BI>")).unwrap_or_default();
            json!({"text": format!("*{}*\n{}{link}", m.title, m.text)})
        }
        WebhookKind::Teams => {
            let link = m.link.as_ref().map(|l| format!("\n\n[Open in Magnum BI]({l})")).unwrap_or_default();
            json!({"text": format!("**{}**\n\n{}{link}", m.title, m.text)})
        }
        WebhookKind::Webhook => json!({"title": m.title, "text": m.text, "link": m.link}),
    }
}

/// A webhook's host, if its address is acceptable: https (http in development), no credentials.
pub fn webhook_host(url: &str, allow_http: bool) -> Result<String, String> {
    let u = url::Url::parse(url.trim()).map_err(|_| "that isn't a web address".to_string())?;
    match u.scheme() {
        "https" => {}
        "http" if allow_http => {}
        _ => return Err("a webhook address starts with https://".into()),
    }
    if !u.username().is_empty() || u.password().is_some() {
        return Err("no user name or password in a webhook address".into());
    }
    u.host_str().filter(|h| !h.is_empty()).map(str::to_lowercase).ok_or_else(|| "a webhook address needs a host".into())
}

/// Post to a tenant's webhook. Its address is a secret: it never appears in errors.
pub async fn post_webhook(guard: &NetGuard, url: &str, body: &Value) -> Result<(), String> {
    let host = webhook_host(url, guard.allow_private)?;
    let u = url::Url::parse(url.trim()).map_err(|e| e.to_string())?;
    let addr = guard.check(&host, u.port_or_known_default().unwrap_or(443)).await?;
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none()) // a redirect could lead past the guard
        .resolve(&host, addr) // the address the guard checked, not a second DNS answer
        .build()
        .map_err(|e| e.to_string())?;
    let r =
        http.post(u).json(body).send().await.map_err(|e| format!("couldn't reach the webhook: {}", e.without_url()))?;
    if r.status().is_success() { Ok(()) } else { Err(format!("the webhook answered {}", r.status())) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn server(security: Security, username: Option<&str>) -> MailServer {
        MailServer {
            host: "smtp.example.com".into(),
            port: 587,
            security,
            username: username.map(Into::into),
            from: "Reports <reports@example.com>".into(),
        }
    }

    #[test]
    fn mail_servers_need_an_address_and_never_send_a_password_in_clear() {
        assert_eq!(server(Security::Starttls, Some("u")).check(true), Ok(()));
        assert_eq!(server(Security::None, None).check(false), Ok(()));
        let clear = server(Security::None, Some("u")).check(true).unwrap_err();
        assert!(clear.contains("TLS"), "{clear}");
        assert!(server(Security::Tls, Some("u")).check(false).unwrap_err().contains("password"));
        assert!(MailServer { host: " ".into(), ..server(Security::Tls, None) }.check(false).is_err());
        assert!(MailServer { port: 0, ..server(Security::Tls, None) }.check(false).is_err());
        assert!(MailServer { from: "not an address".into(), ..server(Security::Tls, None) }.check(false).is_err());
    }

    #[test]
    fn webhooks_are_https_without_credentials() {
        assert_eq!(webhook_host("https://hooks.slack.com/services/T/B/x", false), Ok("hooks.slack.com".into()));
        assert!(webhook_host("http://hooks.example.com/x", false).is_err());
        assert_eq!(webhook_host("http://127.0.0.1:9/x", true), Ok("127.0.0.1".into()), "development only");
        assert!(webhook_host("https://user:pw@example.com/x", false).is_err());
        assert!(webhook_host("not a url", false).is_err());
    }

    #[test]
    fn each_kind_of_webhook_gets_its_message_shape() {
        let m = Message {
            title: "Weekly sales".into(),
            text: "Revenue: 1.2M".into(),
            link: Some("https://bi.example.com/d".into()),
        };
        assert_eq!(
            payload(WebhookKind::Slack, &m),
            json!({"text": "*Weekly sales*\nRevenue: 1.2M\n<https://bi.example.com/d|Open in Magnum BI>"})
        );
        assert_eq!(
            payload(WebhookKind::Teams, &m),
            json!({"text": "**Weekly sales**\n\nRevenue: 1.2M\n\n[Open in Magnum BI](https://bi.example.com/d)"})
        );
        assert_eq!(
            payload(WebhookKind::Webhook, &m),
            json!({"title": "Weekly sales", "text": "Revenue: 1.2M", "link": "https://bi.example.com/d"})
        );
    }

    #[tokio::test]
    async fn the_network_guard_applies_to_mail_servers_and_webhooks() {
        let strict = NetGuard::default(); // production: no private networks
        let mail =
            Mail { to: vec!["a@example.com".into()], subject: "s".into(), text: "t".into(), attachments: vec![] };
        let local = MailServer { host: "127.0.0.1".into(), port: 25, ..server(Security::None, None) };
        let refused = send_mail(&strict, &local, None, &mail).await.unwrap_err();
        assert!(refused.contains("aren't allowed"), "{refused}");
        let refused = post_webhook(&strict, "https://127.0.0.1/hook", &json!({})).await.unwrap_err();
        assert!(refused.contains("aren't allowed"), "{refused}");
        assert!(post_webhook(&strict, "http://example.com/hook", &json!({})).await.unwrap_err().contains("https"));
    }
}
