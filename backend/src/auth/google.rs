//! Google sign-in (OpenID Connect, authorization code + PKCE).
//!
//! Configure MAGNUM_GOOGLE_CLIENT_ID / MAGNUM_GOOGLE_CLIENT_SECRET: a Google Cloud OAuth client of
//! type "Web application" with <public url>/api/auth/google/callback as redirect URI. The ID
//! token is verified here (signature against Google's published keys, audience, issuer, expiry,
//! nonce), and only a Google-verified email is accepted. On first sign-in the email is bound to
//! the Google account id, so a later, different Google account with that email is refused.

use std::time::{Duration, Instant};

use base64::Engine;
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use crate::config::GoogleConfig;

const AUTHORIZE: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN: &str = "https://oauth2.googleapis.com/token";
const CERTS: &str = "https://www.googleapis.com/oauth2/v3/certs";
const ISSUERS: [&str; 2] = ["https://accounts.google.com", "accounts.google.com"];
const KEYS_TTL: Duration = Duration::from_secs(3600);

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct AuthError(pub String);

#[derive(Debug, Clone)]
pub struct GoogleUser {
    pub email: String,
    pub name: String,
    pub sub: String,
}

#[derive(Deserialize)]
struct Claims {
    sub: String,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    email_verified: Option<serde_json::Value>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    nonce: Option<String>,
}

pub struct Google {
    cfg: GoogleConfig,
    http: reqwest::Client,
    keys: Mutex<Option<(Instant, JwkSet)>>,
}

pub fn pkce_challenge(verifier: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

impl Google {
    pub fn new(cfg: GoogleConfig) -> Google {
        let http = reqwest::Client::builder().timeout(Duration::from_secs(15)).build().expect("http client");
        Google { cfg, http, keys: Mutex::new(None) }
    }

    pub fn authorize_url(&self, redirect_uri: &str, state: &str, nonce: &str, verifier: &str) -> String {
        let mut url = url::Url::parse(AUTHORIZE).expect("static url");
        {
            let mut q = url.query_pairs_mut();
            q.append_pair("response_type", "code")
                .append_pair("client_id", &self.cfg.client_id)
                .append_pair("redirect_uri", redirect_uri)
                .append_pair("scope", "openid email profile")
                .append_pair("state", state)
                .append_pair("nonce", nonce)
                .append_pair("code_challenge", &pkce_challenge(verifier))
                .append_pair("code_challenge_method", "S256")
                .append_pair("prompt", "select_account");
            if let Some(hd) = &self.cfg.hosted_domain {
                q.append_pair("hd", hd);
            }
        }
        url.into()
    }

    /// Exchange the code and verify who signed in.
    pub async fn finish(
        &self,
        code: &str,
        redirect_uri: &str,
        verifier: &str,
        nonce: &str,
    ) -> Result<GoogleUser, AuthError> {
        #[derive(Deserialize)]
        struct TokenResponse {
            id_token: Option<String>,
        }
        let r = self
            .http
            .post(TOKEN)
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("client_id", &self.cfg.client_id),
                ("client_secret", &self.cfg.client_secret),
                ("redirect_uri", redirect_uri),
                ("code_verifier", verifier),
            ])
            .send()
            .await
            .map_err(|e| AuthError(format!("couldn't reach Google ({e})")))?;
        let status = r.status();
        let body: TokenResponse =
            r.json().await.map_err(|_| AuthError(format!("Google refused the sign-in ({status})")))?;
        let id_token = body
            .id_token
            .filter(|_| status.is_success())
            .ok_or_else(|| AuthError(format!("Google refused the sign-in ({status})")))?;
        self.verify(&id_token, nonce).await
    }

    async fn verify(&self, token: &str, nonce: &str) -> Result<GoogleUser, AuthError> {
        let header = decode_header(token).map_err(|e| AuthError(format!("Google's answer couldn't be read: {e}")))?;
        let kid = header.kid.ok_or_else(|| AuthError("Google's answer has no key id".into()))?;
        let jwk = self.key(&kid).await?;
        let key = DecodingKey::from_jwk(&jwk).map_err(|e| AuthError(format!("Google key: {e}")))?;
        let mut v = Validation::new(Algorithm::RS256);
        v.set_audience(&[&self.cfg.client_id]);
        v.set_issuer(&ISSUERS);
        let claims = decode::<Claims>(token, &key, &v)
            .map_err(|e| AuthError(format!("Google's answer couldn't be verified: {e}")))?
            .claims;
        if claims.nonce.as_deref() != Some(nonce) {
            return Err(AuthError("sign-in response didn't match this browser; please try again".into()));
        }
        let verified = matches!(&claims.email_verified, Some(serde_json::Value::Bool(true)))
            || matches!(&claims.email_verified, Some(serde_json::Value::String(s)) if s == "true");
        let email = claims
            .email
            .filter(|_| verified)
            .ok_or_else(|| AuthError("your Google account has no verified email address".into()))?;
        let email = email.to_lowercase();
        Ok(GoogleUser { name: claims.name.unwrap_or_else(|| email.clone()), email, sub: claims.sub })
    }

    /// Google's signing key `kid`, from its published key set (cached; refreshed on a miss).
    async fn key(&self, kid: &str) -> Result<jsonwebtoken::jwk::Jwk, AuthError> {
        let mut cached = self.keys.lock().await;
        let fresh = cached.as_ref().is_some_and(|(at, set)| at.elapsed() < KEYS_TTL && set.find(kid).is_some());
        if !fresh {
            let set: JwkSet = self
                .http
                .get(CERTS)
                .send()
                .await
                .and_then(|r| r.error_for_status())
                .map_err(|e| AuthError(format!("couldn't fetch Google's keys ({e})")))?
                .json()
                .await
                .map_err(|e| AuthError(format!("Google's keys couldn't be read ({e})")))?;
            *cached = Some((Instant::now(), set));
        }
        cached
            .as_ref()
            .and_then(|(_, set)| set.find(kid).cloned())
            .ok_or_else(|| AuthError("unknown Google signing key".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_s256() {
        // RFC 7636, appendix B
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn authorize_url_carries_pkce_and_state() {
        let g = Google::new(GoogleConfig {
            client_id: "cid".into(),
            client_secret: "s".into(),
            hosted_domain: Some("acme.example".into()),
        });
        let u = g.authorize_url("http://localhost:8000/api/auth/google/callback", "st", "no", "verifier");
        assert!(u.starts_with(AUTHORIZE));
        for part in [
            "client_id=cid",
            "state=st",
            "nonce=no",
            "code_challenge_method=S256",
            "hd=acme.example",
            "scope=openid+email+profile",
        ] {
            assert!(u.contains(part), "{u} lacks {part}");
        }
    }
}
