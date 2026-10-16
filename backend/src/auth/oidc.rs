//! Organisations' own OpenID Connect providers (Entra ID, Okta, Keycloak, Auth0, ...).
//!
//! Authorization code flow with PKCE, state and nonce, like Google sign-in (auth::google), but for
//! any standard provider: its endpoints and keys come from `<issuer>/.well-known/openid-configuration`.
//! The ID token is verified here: signature (asymmetric algorithms only) against the provider's
//! published keys, issuer, audience, expiry, nonce. The provider's addresses are organisation
//! settings, so every request to them goes through the network guard (no internal addresses),
//! and they must be https unless private networks are allowed (development).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::Mutex;

use super::google::{AuthError, pkce_challenge};
use crate::netguard::NetGuard;

const CACHE_TTL: Duration = Duration::from_secs(3600);
const ALGORITHMS: [Algorithm; 7] = [
    Algorithm::RS256,
    Algorithm::RS384,
    Algorithm::RS512,
    Algorithm::PS256,
    Algorithm::PS384,
    Algorithm::ES256,
    Algorithm::ES384,
];

/// A provider's endpoints, from its discovery document.
#[derive(Debug, Clone, Deserialize)]
pub struct Provider {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub jwks_uri: String,
}

/// Who the provider says signed in.
#[derive(Debug, Clone)]
pub struct Identity {
    pub email: String,
    pub name: String,
    pub groups: Vec<String>,
}

fn err<T>(msg: impl Into<String>) -> Result<T, AuthError> {
    Err(AuthError(msg.into()))
}

/// A provider address: https (http only where private networks are allowed, i.e. development),
/// and not an internal address.
pub async fn check_url(guard: &NetGuard, url: &str) -> Result<(), AuthError> {
    let u = url::Url::parse(url).map_err(|_| AuthError(format!("{url:?} isn't a valid address")))?;
    match u.scheme() {
        "https" => {}
        "http" if guard.allow_private => {}
        _ => return err(format!("{url:?}: the provider must be reached over https")),
    }
    if !u.username().is_empty() || u.password().is_some() {
        return err("no credentials in the provider's address");
    }
    guard.check_url(url, 443).await.map_err(AuthError)
}

type Cached = (Instant, Provider, Option<JwkSet>);

#[derive(Default)]
pub struct Oidc {
    http: reqwest::Client,
    /// issuer -> (fetched at, provider, its keys)
    cache: Mutex<HashMap<String, Cached>>,
}

impl Oidc {
    pub fn new() -> Oidc {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none()) // a redirect could lead past the guard
            .build()
            .expect("http client");
        Oidc { http, cache: Mutex::default() }
    }

    /// The provider at `issuer` (discovery, cached for an hour).
    pub async fn provider(&self, issuer: &str, guard: &NetGuard) -> Result<Provider, AuthError> {
        let issuer = issuer.trim_end_matches('/');
        if let Some((at, p, _)) = self.cache.lock().await.get(issuer)
            && at.elapsed() < CACHE_TTL
        {
            return Ok(p.clone());
        }
        let url = format!("{issuer}/.well-known/openid-configuration");
        check_url(guard, &url).await?;
        let p: Provider = self.get_json(&url).await?;
        if p.issuer.trim_end_matches('/') != issuer {
            return err(format!("the provider calls itself {:?}, not {issuer:?}", p.issuer));
        }
        for u in [&p.authorization_endpoint, &p.token_endpoint, &p.jwks_uri] {
            check_url(guard, u).await?;
        }
        self.cache.lock().await.insert(issuer.to_string(), (Instant::now(), p.clone(), None));
        Ok(p)
    }

    async fn get_json<T: serde::de::DeserializeOwned>(&self, url: &str) -> Result<T, AuthError> {
        self.http
            .get(url)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| AuthError(format!("couldn't reach the provider ({e})")))?
            .json()
            .await
            .map_err(|_| AuthError(format!("the provider's answer from {url} couldn't be read")))
    }

    pub fn authorize_url(
        p: &Provider,
        client_id: &str,
        redirect_uri: &str,
        state: &str,
        nonce: &str,
        verifier: &str,
    ) -> String {
        let mut url = url::Url::parse(&p.authorization_endpoint).expect("checked");
        url.query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", client_id)
            .append_pair("redirect_uri", redirect_uri)
            .append_pair("scope", "openid email profile")
            .append_pair("state", state)
            .append_pair("nonce", nonce)
            .append_pair("code_challenge", &pkce_challenge(verifier))
            .append_pair("code_challenge_method", "S256");
        url.into()
    }

    /// Exchange the code and verify who signed in.
    #[allow(clippy::too_many_arguments)]
    pub async fn finish(
        &self,
        p: &Provider,
        client_id: &str,
        client_secret: Option<&str>,
        code: &str,
        redirect_uri: &str,
        verifier: &str,
        nonce: &str,
        groups_claim: &str,
    ) -> Result<Identity, AuthError> {
        let mut form = vec![
            ("grant_type", "authorization_code"),
            ("code", code),
            ("client_id", client_id),
            ("redirect_uri", redirect_uri),
            ("code_verifier", verifier),
        ];
        if let Some(s) = client_secret {
            form.push(("client_secret", s));
        }
        let r = self
            .http
            .post(&p.token_endpoint)
            .form(&form)
            .send()
            .await
            .map_err(|e| AuthError(format!("couldn't reach the provider ({e})")))?;
        let status = r.status();
        let body: Value = r.json().await.unwrap_or(Value::Null);
        let Some(token) = body["id_token"].as_str().filter(|_| status.is_success()) else {
            let why = body["error_description"].as_str().or(body["error"].as_str()).unwrap_or("no ID token");
            return err(format!("the provider refused the sign-in ({status}: {why})"));
        };
        let claims = self.verify(p, client_id, token, nonce).await?;
        identity(&claims, groups_claim)
    }

    async fn verify(&self, p: &Provider, client_id: &str, token: &str, nonce: &str) -> Result<Value, AuthError> {
        let header =
            decode_header(token).map_err(|e| AuthError(format!("the provider's answer couldn't be read: {e}")))?;
        if !ALGORITHMS.contains(&header.alg) {
            return err(format!("the provider signed with {:?}; an asymmetric algorithm is required", header.alg));
        }
        let jwk = self.key(p, header.kid.as_deref()).await?;
        let key = DecodingKey::from_jwk(&jwk).map_err(|e| AuthError(format!("provider key: {e}")))?;
        let mut v = Validation::new(header.alg);
        v.set_audience(&[client_id]);
        v.set_issuer(&[p.issuer.as_str(), p.issuer.trim_end_matches('/')]);
        let claims = decode::<Value>(token, &key, &v)
            .map_err(|e| AuthError(format!("the provider's answer couldn't be verified: {e}")))?
            .claims;
        if claims["nonce"].as_str() != Some(nonce) {
            return err("the sign-in response didn't match this browser; please try again");
        }
        Ok(claims)
    }

    /// The provider's signing key (by key id; the only key when there's no id), refetched on a miss.
    async fn key(&self, p: &Provider, kid: Option<&str>) -> Result<jsonwebtoken::jwk::Jwk, AuthError> {
        let find = |set: &JwkSet| match kid {
            Some(k) => set.find(k).cloned(),
            None if set.keys.len() == 1 => set.keys.first().cloned(),
            None => None,
        };
        let issuer = p.issuer.trim_end_matches('/').to_string();
        if let Some((_, _, Some(set))) = self.cache.lock().await.get(&issuer)
            && let Some(k) = find(set)
        {
            return Ok(k);
        }
        let set: JwkSet = self.get_json(&p.jwks_uri).await?;
        let found = find(&set);
        let mut cache = self.cache.lock().await;
        let entry = cache.entry(issuer).or_insert_with(|| (Instant::now(), p.clone(), None));
        entry.2 = Some(set);
        found.ok_or_else(|| AuthError("unknown provider signing key".into()))
    }
}

/// Email, name and groups from verified claims. The email: `email`, else `preferred_username`
/// or `upn` when they're addresses (Entra ID often sends only those); an email the provider
/// marks unverified is refused.
pub fn identity(claims: &Value, groups_claim: &str) -> Result<Identity, AuthError> {
    if claims["email_verified"] == Value::Bool(false) || claims["email_verified"] == Value::String("false".into()) {
        return err("the provider says this email address isn't verified");
    }
    let email = ["email", "preferred_username", "upn"]
        .iter()
        .filter_map(|c| claims[*c].as_str())
        .find(|e| e.contains('@'))
        .map(|e| e.trim().to_lowercase())
        .ok_or_else(|| AuthError("the provider didn't say which email address signed in".into()))?;
    let name = claims["name"].as_str().map(String::from).unwrap_or_else(|| email.clone());
    let groups = match &claims[groups_claim] {
        Value::Array(a) => a.iter().filter_map(|g| g.as_str().map(String::from)).collect(),
        Value::String(s) => vec![s.clone()],
        _ => vec![],
    };
    Ok(Identity { email, name, groups })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn identity_from_claims() {
        let i = identity(&json!({"email": "Ana@Pol.example", "name": "Ana", "groups": ["a", "b"]}), "groups").unwrap();
        assert_eq!((i.email.as_str(), i.groups.len()), ("ana@pol.example", 2));
        // Entra ID: no email claim, an address in preferred_username; groups as roles
        let i = identity(&json!({"preferred_username": "bo@pol.example", "roles": "BI"}), "roles").unwrap();
        assert_eq!(
            (i.email.as_str(), i.name.as_str(), i.groups),
            ("bo@pol.example", "bo@pol.example", vec!["BI".into()])
        );
        assert!(identity(&json!({"preferred_username": "bo"}), "groups").is_err());
        assert!(
            identity(&json!({"email": "x@y.z", "email_verified": false}), "groups").unwrap_err().0.contains("verified")
        );
    }

    #[tokio::test]
    async fn provider_addresses_are_guarded() {
        let strict = NetGuard::default();
        assert!(check_url(&strict, "http://idp.example.com").await.unwrap_err().0.contains("https"));
        assert!(check_url(&strict, "https://169.254.169.254/x").await.is_err());
        assert!(check_url(&strict, "https://user:pw@idp.example.com").await.is_err());
        assert!(check_url(&strict, "ftp://idp.example.com").await.is_err());
        let dev = NetGuard { allow_private: true, ..NetGuard::default() };
        assert!(check_url(&dev, "http://127.0.0.1:8080/realms/acme").await.is_ok());
    }
}
