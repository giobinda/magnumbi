//! Typed access to Magnum BI's tables in ScramDB (see migrations/0001_initial.sql).

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;
use base64::Engine;
use chrono::{DateTime, Duration, Utc};
use rand::RngCore;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio_postgres::Row;

use crate::db::DbPool;

pub fn random_token(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)
}

pub fn sha256_hex(s: &str) -> String {
    Sha256::digest(s.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Debug, Clone)]
pub struct User {
    pub email: String,
    pub name: String,
    pub password_hash: Option<String>,
    pub google_sub: Option<String>,
    /// the organisation whose admin set this account's password (None: platform / self / CLI)
    pub owner_org: Option<String>,
    pub must_change_password: bool,
    pub disabled: bool,
    /// signs in with an authenticator code after the password (auth::totp)
    pub two_factor: bool,
}

const USER_COLUMNS: &str =
    "email, name, password_hash, google_sub, owner_org, must_change_password, disabled, totp_secret_sealed IS NOT NULL";

impl User {
    fn from_row(r: &Row) -> User {
        User {
            email: r.get(0),
            name: r.get(1),
            password_hash: r.get(2),
            google_sub: r.get(3),
            owner_org: r.get(4),
            must_change_password: r.get(5),
            disabled: r.get(6),
            two_factor: r.get(7),
        }
    }
}

/// A person's authenticator settings (sealed secrets, as stored).
pub struct TwoFactor {
    pub secret_sealed: Option<String>,
    pub pending_sealed: Option<String>,
    pub last_step: Option<i64>,
    /// SHA-256 of the unused recovery codes
    pub recovery: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub method: String,
    pub user: User,
    /// a role given by an organisation's provider at sign-in: "<org>:<role>"
    pub granted_role: Option<String>,
    /// set when this is an API token rather than a browser session
    pub token: Option<TokenInfo>,
}

/// An API token's limits.
#[derive(Debug, Clone)]
pub struct TokenInfo {
    pub name: String,
    /// the organisation it works in ('' = the platform workspace)
    pub org: String,
    /// at most this role
    pub role: String,
    /// false: read-only
    pub write: bool,
}

/// An OpenID Connect sign-in in progress.
pub struct OAuthState {
    pub verifier: String,
    pub nonce: String,
    pub redirect_uri: String,
    pub next: String,
    /// the organisation whose provider it's with (None: Google)
    pub provider: Option<String>,
}

#[derive(Clone)]
pub struct Store {
    pub pool: DbPool,
}

impl Store {
    pub fn new(pool: DbPool) -> Store {
        Store { pool }
    }

    // ------------------------------------------------------------------ users

    pub async fn user(&self, email: &str) -> Result<Option<User>> {
        let c = self.pool.get().await?;
        let row = c
            .query_opt(&format!("SELECT {USER_COLUMNS} FROM users WHERE email = $1"), &[&email.to_lowercase()])
            .await?;
        Ok(row.as_ref().map(User::from_row))
    }

    pub async fn user_count(&self) -> Result<i64> {
        let c = self.pool.get().await?;
        Ok(c.query_one("SELECT COUNT(*) FROM users", &[]).await?.get(0))
    }

    /// Create the account, or set a new password on it. Every other session of the person ends.
    pub async fn set_password(
        &self,
        email: &str,
        name: Option<&str>,
        hash: &str,
        owner_org: Option<&str>,
        must_change: bool,
    ) -> Result<()> {
        let email = email.to_lowercase();
        let default_name = email.split('@').next().unwrap_or_default().to_string();
        let c = self.pool.get().await?;
        c.execute(
            "INSERT INTO users (email, name, password_hash, owner_org, must_change_password) VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (email) DO UPDATE SET password_hash = EXCLUDED.password_hash, owner_org = EXCLUDED.owner_org,
               must_change_password = EXCLUDED.must_change_password, updated_at = now(),
               name = CASE WHEN $6::BOOLEAN THEN EXCLUDED.name ELSE users.name END",
            &[&email, &name.unwrap_or(&default_name), &hash, &owner_org, &must_change, &name.is_some()],
        )
        .await?;
        c.execute("DELETE FROM sessions WHERE user_email = $1", &[&email]).await?;
        Ok(())
    }

    /// A Google sign-in: create the account if needed and bind it to the Google account id.
    pub async fn upsert_google_user(&self, email: &str, name: &str, sub: &str) -> Result<User> {
        let c = self.pool.get().await?;
        let row = c
            .query_one(
                &format!(
                    "INSERT INTO users (email, name, google_sub) VALUES ($1, $2, $3)
                     ON CONFLICT (email) DO UPDATE SET google_sub = COALESCE(users.google_sub, EXCLUDED.google_sub),
                       name = CASE WHEN users.name = '' THEN EXCLUDED.name ELSE users.name END, updated_at = now()
                     RETURNING {USER_COLUMNS}"
                ),
                &[&email.to_lowercase(), &name, &sub],
            )
            .await?;
        Ok(User::from_row(&row))
    }

    /// Someone an organisation's provider vouched for: created if new (without a password).
    pub async fn upsert_sso_user(&self, email: &str, name: &str) -> Result<User> {
        let c = self.pool.get().await?;
        let row = c
            .query_one(
                &format!(
                    "INSERT INTO users (email, name) VALUES ($1, $2)
                     ON CONFLICT (email) DO UPDATE SET
                       name = CASE WHEN users.name = '' THEN EXCLUDED.name ELSE users.name END, updated_at = now()
                     RETURNING {USER_COLUMNS}"
                ),
                &[&email.to_lowercase(), &name],
            )
            .await?;
        Ok(User::from_row(&row))
    }

    pub async fn set_disabled(&self, email: &str, disabled: bool) -> Result<bool> {
        let c = self.pool.get().await?;
        let n = c
            .execute(
                "UPDATE users SET disabled = $2, updated_at = now() WHERE email = $1",
                &[&email.to_lowercase(), &disabled],
            )
            .await?;
        if disabled {
            c.execute("DELETE FROM sessions WHERE user_email = $1", &[&email.to_lowercase()]).await?;
        }
        Ok(n > 0)
    }

    pub async fn record_login(&self, email: &str) -> Result<()> {
        let c = self.pool.get().await?;
        c.execute("UPDATE users SET last_login_at = now() WHERE email = $1", &[&email]).await?;
        Ok(())
    }

    /// email -> last sign-in (epoch seconds), and which accounts can sign in with a password.
    pub async fn accounts(&self) -> Result<BTreeMap<String, Value>> {
        let c = self.pool.get().await?;
        let rows = c
            .query(
                "SELECT email, last_login_at, password_hash IS NOT NULL, google_sub IS NOT NULL, disabled FROM users",
                &[],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| {
                let at: Option<DateTime<Utc>> = r.get(1);
                (
                    r.get::<_, String>(0),
                    json!({"last_login": at.map(|t| t.timestamp()), "password": r.get::<_, bool>(2),
                                               "google": r.get::<_, bool>(3), "disabled": r.get::<_, bool>(4)}),
                )
            })
            .collect())
    }

    // ------------------------------------------------------------------ sessions

    /// A new session; returns the cookie value (only its hash is stored).
    /// A session with a role given by the organisation's provider ("<org>:<role>").
    pub async fn create_session_as(
        &self,
        email: &str,
        method: &str,
        ttl: Duration,
        granted_role: Option<&str>,
    ) -> Result<String> {
        let token = random_token(32);
        let c = self.pool.get().await?;
        c.execute(
            "INSERT INTO sessions (id_hash, user_email, method, expires_at, granted_role) VALUES ($1, $2, $3, $4, $5)",
            &[&sha256_hex(&token), &email.to_lowercase(), &method, &(Utc::now() + ttl), &granted_role],
        )
        .await?;
        // now and then, forget expired sessions and sign-ins
        if rand::random::<u8>() < 8 {
            c.execute("DELETE FROM sessions WHERE expires_at < now()", &[]).await?;
            c.execute("DELETE FROM oauth_states WHERE expires_at < now()", &[]).await?;
        }
        Ok(token)
    }

    // ------------------------------------------------------------------ authenticator codes

    pub async fn two_factor(&self, email: &str) -> Result<TwoFactor> {
        let c = self.pool.get().await?;
        let r = c
            .query_opt(
                "SELECT totp_secret_sealed, totp_pending_sealed, totp_last_step, recovery_codes FROM users WHERE email = $1",
                &[&email.to_lowercase()],
            )
            .await?;
        Ok(match r {
            Some(r) => TwoFactor {
                secret_sealed: r.get(0),
                pending_sealed: r.get(1),
                last_step: r.get(2),
                recovery: r
                    .get::<_, Option<String>>(3)
                    .map(|j| serde_json::from_str(&j))
                    .transpose()?
                    .unwrap_or_default(),
            },
            None => TwoFactor { secret_sealed: None, pending_sealed: None, last_step: None, recovery: vec![] },
        })
    }

    pub async fn set_two_factor_pending(&self, email: &str, sealed: Option<&str>) -> Result<()> {
        let c = self.pool.get().await?;
        c.execute("UPDATE users SET totp_pending_sealed = $2 WHERE email = $1", &[&email.to_lowercase(), &sealed])
            .await?;
        Ok(())
    }

    /// The pending secret becomes the secret; every other session ends.
    pub async fn enable_two_factor(&self, email: &str, sealed: &str, step: i64, recovery: &[String]) -> Result<()> {
        let email = email.to_lowercase();
        let c = self.pool.get().await?;
        c.execute(
            "UPDATE users SET totp_secret_sealed = $2, totp_pending_sealed = NULL, totp_last_step = $3::BIGINT,
               recovery_codes = $4, updated_at = now() WHERE email = $1",
            &[&email, &sealed, &step, &serde_json::to_string(recovery)?],
        )
        .await?;
        c.execute("DELETE FROM sessions WHERE user_email = $1", &[&email]).await?;
        Ok(())
    }

    pub async fn disable_two_factor(&self, email: &str) -> Result<()> {
        let c = self.pool.get().await?;
        c.execute(
            "UPDATE users SET totp_secret_sealed = NULL, totp_pending_sealed = NULL, totp_last_step = NULL,
               recovery_codes = NULL, updated_at = now() WHERE email = $1",
            &[&email.to_lowercase()],
        )
        .await?;
        Ok(())
    }

    /// Record a code's step as used; false if that step (or a later one) was used already.
    pub async fn use_two_factor_step(&self, email: &str, step: i64) -> Result<bool> {
        let c = self.pool.get().await?;
        let n = c
            .execute(
                "UPDATE users SET totp_last_step = $2::BIGINT
                 WHERE email = $1 AND (totp_last_step IS NULL OR totp_last_step < $2::BIGINT)",
                &[&email.to_lowercase(), &step],
            )
            .await?;
        Ok(n > 0)
    }

    /// Use up a recovery code (by hash); false if it isn't one of the unused ones.
    pub async fn use_recovery_code(&self, email: &str, hash: &str) -> Result<bool> {
        let current = self.two_factor(email).await?.recovery;
        if !current.iter().any(|h| h == hash) {
            return Ok(false);
        }
        let left: Vec<&String> = current.iter().filter(|h| *h != hash).collect();
        let c = self.pool.get().await?;
        // only if nobody used another code meanwhile
        let n = c
            .execute(
                "UPDATE users SET recovery_codes = $2 WHERE email = $1 AND recovery_codes = $3",
                &[&email.to_lowercase(), &serde_json::to_string(&left)?, &serde_json::to_string(&current)?],
            )
            .await?;
        Ok(n > 0)
    }

    /// A ticket for the second step of a sign-in (5 minutes, 5 attempts).
    pub async fn create_login_challenge(&self, email: &str) -> Result<String> {
        let ticket = random_token(24);
        let c = self.pool.get().await?;
        c.execute(
            "INSERT INTO login_challenges (ticket_hash, user_email, expires_at) VALUES ($1, $2, $3)",
            &[&sha256_hex(&ticket), &email.to_lowercase(), &(Utc::now() + Duration::minutes(5))],
        )
        .await?;
        Ok(ticket)
    }

    /// The email a ticket is for, counting an attempt; None once expired or used up.
    pub async fn login_challenge(&self, ticket: &str) -> Result<Option<String>> {
        if ticket.is_empty() || ticket.len() > 128 {
            return Ok(None);
        }
        let c = self.pool.get().await?;
        let row = c
            .query_opt(
                "UPDATE login_challenges SET attempts = attempts + 1
                 WHERE ticket_hash = $1 AND expires_at > now() AND attempts < 5 RETURNING user_email",
                &[&sha256_hex(ticket)],
            )
            .await?;
        Ok(row.map(|r| r.get(0)))
    }

    pub async fn end_login_challenge(&self, ticket: &str) -> Result<()> {
        let c = self.pool.get().await?;
        c.execute("DELETE FROM login_challenges WHERE ticket_hash = $1 OR expires_at < now()", &[&sha256_hex(ticket)])
            .await?;
        Ok(())
    }

    // ------------------------------------------------------------------ API tokens

    #[allow(clippy::too_many_arguments)]
    pub async fn create_token(
        &self,
        token: &str,
        name: &str,
        email: &str,
        org: &str,
        role: &str,
        write: bool,
        expires: DateTime<Utc>,
    ) -> Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        let c = self.pool.get().await?;
        c.execute(
            "INSERT INTO api_tokens (id, token_hash, name, user_email, org, role, write, expires_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7::BOOLEAN, $8::TIMESTAMPTZ)",
            &[&id, &sha256_hex(token), &name, &email.to_lowercase(), &org, &role, &write, &expires],
        )
        .await?;
        Ok(id)
    }

    /// The person an API token acts for, if it's valid (not revoked or expired, account enabled).
    pub async fn token_session(&self, token: &str) -> Result<Option<SessionInfo>> {
        if !token.starts_with("mbi_") || token.len() > 128 {
            return Ok(None);
        }
        let c = self.pool.get().await?;
        let row = c
            .query_opt(
                &format!(
                    "SELECT t.id, t.name, t.org, t.role, t.write, {}
                     FROM api_tokens t JOIN users u ON u.email = t.user_email
                     WHERE t.token_hash = $1 AND t.revoked_at IS NULL AND t.expires_at > now() AND NOT u.disabled",
                    USER_COLUMNS.split(", ").map(|c| format!("u.{c}")).collect::<Vec<_>>().join(", ")
                ),
                &[&sha256_hex(token)],
            )
            .await?;
        let Some(r) = row else { return Ok(None) };
        let id: String = r.get(0);
        c.execute(
            "UPDATE api_tokens SET last_used_at = now()
             WHERE id = $1 AND (last_used_at IS NULL OR last_used_at < $2::TIMESTAMPTZ)",
            &[&id, &(Utc::now() - Duration::minutes(1))],
        )
        .await?;
        let user = User {
            email: r.get(5),
            name: r.get(6),
            password_hash: r.get(7),
            google_sub: r.get(8),
            owner_org: r.get(9),
            must_change_password: r.get(10),
            disabled: r.get(11),
            two_factor: r.get(12),
        };
        let token = TokenInfo { name: r.get(1), org: r.get(2), role: r.get(3), write: r.get(4) };
        Ok(Some(SessionInfo { method: "token".into(), user, granted_role: None, token: Some(token) }))
    }

    /// Tokens of an organisation (all, or one person's), newest first; never the secrets.
    pub async fn tokens(&self, org: &str, email: Option<&str>) -> Result<Vec<Value>> {
        let c = self.pool.get().await?;
        let rows = c
            .query(
                "SELECT id, name, user_email, role, write, created_at, expires_at, last_used_at, revoked_at
                 FROM api_tokens WHERE org = $1 ORDER BY created_at DESC",
                &[&org],
            )
            .await?;
        let iso = |t: Option<DateTime<Utc>>| t.map(|t| t.to_rfc3339());
        Ok(rows
            .iter()
            .filter(|r| email.is_none_or(|e| r.get::<_, String>(2) == e.to_lowercase()))
            .map(|r| {
                serde_json::json!({
                    "id": r.get::<_, String>(0), "name": r.get::<_, String>(1), "user_email": r.get::<_, String>(2),
                    "role": r.get::<_, String>(3), "scope": if r.get::<_, bool>(4) { "write" } else { "read" },
                    "created_at": iso(r.get(5)), "expires_at": iso(r.get(6)),
                    "last_used_at": iso(r.get(7)), "revoked_at": iso(r.get(8)),
                })
            })
            .collect())
    }

    /// Revoke a token of the organisation (only one person's when `email` is set).
    pub async fn revoke_token(&self, org: &str, id: &str, email: Option<&str>) -> Result<bool> {
        let c = self.pool.get().await?;
        let n = c
            .execute(
                "UPDATE api_tokens SET revoked_at = now()
                 WHERE id = $1 AND org = $2 AND revoked_at IS NULL AND ($3::TEXT IS NULL OR user_email = $3::TEXT)",
                &[&id, &org, &email.map(str::to_lowercase)],
            )
            .await?;
        Ok(n > 0)
    }

    // ------------------------------------------------------------------ passkeys

    /// The person's stable random id for authenticators (created on first use).
    pub async fn webauthn_id(&self, email: &str) -> Result<String> {
        let c = self.pool.get().await?;
        let fresh = uuid::Uuid::new_v4().to_string();
        c.execute(
            "UPDATE users SET webauthn_id = $2 WHERE email = $1 AND webauthn_id IS NULL",
            &[&email.to_lowercase(), &fresh],
        )
        .await?;
        let r = c.query_one("SELECT webauthn_id FROM users WHERE email = $1", &[&email.to_lowercase()]).await?;
        Ok(r.get(0))
    }

    /// (id, name, passkey JSON, created_at, last_used_at)
    pub async fn passkeys(&self, email: &str) -> Result<Vec<(String, String, String, String, Option<String>)>> {
        let c = self.pool.get().await?;
        let rows = c
            .query(
                "SELECT id, name, passkey, created_at, last_used_at FROM passkeys WHERE user_email = $1 ORDER BY created_at",
                &[&email.to_lowercase()],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| {
                let (at, used): (DateTime<Utc>, Option<DateTime<Utc>>) = (r.get(3), r.get(4));
                (r.get(0), r.get(1), r.get(2), at.to_rfc3339(), used.map(|u| u.to_rfc3339()))
            })
            .collect())
    }

    pub async fn add_passkey(&self, email: &str, id: &str, name: &str, passkey: &str) -> Result<()> {
        let c = self.pool.get().await?;
        c.execute(
            "INSERT INTO passkeys (id, user_email, name, passkey) VALUES ($1, $2, $3, $4)",
            &[&id, &email.to_lowercase(), &name, &passkey],
        )
        .await?;
        Ok(())
    }

    /// After a sign-in: the credential's new counter, and when it was used.
    pub async fn used_passkey(&self, id: &str, passkey: &str) -> Result<()> {
        let c = self.pool.get().await?;
        c.execute("UPDATE passkeys SET passkey = $2, last_used_at = now() WHERE id = $1", &[&id, &passkey]).await?;
        Ok(())
    }

    pub async fn delete_passkey(&self, email: &str, id: &str) -> Result<bool> {
        let c = self.pool.get().await?;
        Ok(c.execute("DELETE FROM passkeys WHERE id = $1 AND user_email = $2", &[&id, &email.to_lowercase()]).await?
            > 0)
    }

    /// A WebAuthn ceremony in progress (5 minutes): returns its ticket.
    pub async fn put_webauthn_state(&self, email: &str, kind: &str, state: &str) -> Result<String> {
        let ticket = random_token(24);
        let c = self.pool.get().await?;
        c.execute(
            "INSERT INTO webauthn_states (ticket_hash, user_email, kind, state, expires_at) VALUES ($1, $2, $3, $4, $5)",
            &[&sha256_hex(&ticket), &email.to_lowercase(), &kind, &state, &(Utc::now() + Duration::minutes(5))],
        )
        .await?;
        Ok(ticket)
    }

    /// One use only: (email, state) of a ceremony of `kind`.
    pub async fn take_webauthn_state(&self, ticket: &str, kind: &str) -> Result<Option<(String, String)>> {
        if ticket.is_empty() || ticket.len() > 128 {
            return Ok(None);
        }
        let c = self.pool.get().await?;
        let row = c
            .query_opt(
                "DELETE FROM webauthn_states WHERE ticket_hash = $1 AND kind = $2 AND expires_at > now()
                 RETURNING user_email, state",
                &[&sha256_hex(ticket), &kind],
            )
            .await?;
        c.execute("DELETE FROM webauthn_states WHERE expires_at < now()", &[]).await?;
        Ok(row.map(|r| (r.get(0), r.get(1))))
    }

    pub async fn session(&self, token: &str) -> Result<Option<SessionInfo>> {
        if token.is_empty() || token.len() > 128 {
            return Ok(None);
        }
        let c = self.pool.get().await?;
        let row = c
            .query_opt(
                "SELECT s.method, u.email, u.name, u.password_hash, u.google_sub, u.owner_org, u.must_change_password, u.disabled,
                   u.totp_secret_sealed IS NOT NULL, s.granted_role
                 FROM sessions s JOIN users u ON u.email = s.user_email
                 WHERE s.id_hash = $1 AND s.expires_at > now()",
                &[&sha256_hex(token)],
            )
            .await?;
        Ok(row
            .map(|r| SessionInfo {
                method: r.get(0),
                user: User {
                    email: r.get(1),
                    name: r.get(2),
                    password_hash: r.get(3),
                    google_sub: r.get(4),
                    owner_org: r.get(5),
                    must_change_password: r.get(6),
                    disabled: r.get(7),
                    two_factor: r.get(8),
                },
                granted_role: r.get(9),
                token: None,
            })
            .filter(|s| !s.user.disabled))
    }

    pub async fn delete_session(&self, token: &str) -> Result<()> {
        let c = self.pool.get().await?;
        c.execute("DELETE FROM sessions WHERE id_hash = $1", &[&sha256_hex(token)]).await?;
        Ok(())
    }

    // ------------------------------------------------------------------ Google sign-ins in progress

    pub async fn put_oauth_state(
        &self,
        state: &str,
        verifier: &str,
        nonce: &str,
        redirect_uri: &str,
        next: &str,
        provider: Option<&str>,
    ) -> Result<()> {
        let c = self.pool.get().await?;
        c.execute(
            "INSERT INTO oauth_states (state_hash, verifier, nonce, redirect_uri, next_url, expires_at, provider)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
            &[
                &sha256_hex(state),
                &verifier,
                &nonce,
                &redirect_uri,
                &next,
                &(Utc::now() + Duration::minutes(10)),
                &provider,
            ],
        )
        .await?;
        Ok(())
    }

    /// One use only.
    pub async fn take_oauth_state(&self, state: &str) -> Result<Option<OAuthState>> {
        let c = self.pool.get().await?;
        let row = c
            .query_opt(
                "DELETE FROM oauth_states WHERE state_hash = $1 AND expires_at > now()
                 RETURNING verifier, nonce, redirect_uri, next_url, provider",
                &[&sha256_hex(state)],
            )
            .await?;
        Ok(row.map(|r| OAuthState {
            verifier: r.get(0),
            nonce: r.get(1),
            redirect_uri: r.get(2),
            next: r.get(3),
            provider: r.get(4),
        }))
    }

    // ------------------------------------------------------------------ dashboards

    /// First start: the dashboards shipped in the project folder become platform templates
    /// (only while the table is empty).
    pub async fn seed_dashboards(&self, folder: &Path) -> Result<usize> {
        let c = self.pool.get().await?;
        let n: i64 = c.query_one("SELECT COUNT(*) FROM dashboards", &[]).await?.get(0);
        if n > 0 {
            return Ok(0);
        }
        let mut paths: Vec<_> = std::fs::read_dir(folder)
            .map(|rd| {
                rd.filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| p.extension().is_some_and(|x| x == "yaml"))
                    .collect()
            })
            .unwrap_or_default();
        paths.sort();
        let mut seeded = 0;
        for path in paths {
            let id = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let text = std::fs::read_to_string(&path)?;
            if let Err(e) = crate::project::parse_dashboard(&text) {
                tracing::warn!("not seeding {}: {e}", path.display());
                continue;
            }
            self.write_dashboard("", &id, &text, Some("seed")).await?;
            seeded += 1;
        }
        Ok(seeded)
    }

    pub async fn dashboard_ids(&self, org: &str) -> Result<Vec<String>> {
        let c = self.pool.get().await?;
        let rows = c.query("SELECT id FROM dashboards WHERE org = $1 ORDER BY id", &[&org]).await?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    pub async fn read_dashboard(&self, org: &str, id: &str) -> Result<Option<String>> {
        let c = self.pool.get().await?;
        Ok(c.query_opt("SELECT yaml FROM dashboards WHERE org = $1 AND id = $2", &[&org, &id]).await?.map(|r| r.get(0)))
    }

    pub async fn write_dashboard(&self, org: &str, id: &str, text: &str, by: Option<&str>) -> Result<()> {
        let mut c = self.pool.get().await?;
        let tx = c.transaction().await?;
        tx.execute(
            "INSERT INTO dashboards (org, id, yaml, updated_by) VALUES ($1, $2, $3, $4)
             ON CONFLICT (org, id) DO UPDATE SET yaml = EXCLUDED.yaml, updated_by = EXCLUDED.updated_by, updated_at = now()",
            &[&org, &id, &text, &by],
        )
        .await?;
        tx.execute(
            "INSERT INTO dashboard_versions (id, org, dashboard_id, yaml, saved_by) VALUES ($1, $2, $3, $4, $5)",
            &[&uuid::Uuid::now_v7().to_string(), &org, &id, &text, &by],
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn remove_dashboard(&self, org: &str, id: &str, by: Option<&str>) -> Result<bool> {
        let mut c = self.pool.get().await?;
        let tx = c.transaction().await?;
        let n = tx.execute("DELETE FROM dashboards WHERE org = $1 AND id = $2", &[&org, &id]).await?;
        if n > 0 {
            tx.execute(
                "INSERT INTO dashboard_versions (id, org, dashboard_id, deleted, saved_by) VALUES ($1, $2, $3, true, $4)",
                &[&uuid::Uuid::now_v7().to_string(), &org, &id, &by],
            )
            .await?;
        }
        tx.commit().await?;
        Ok(n > 0)
    }

    pub async fn dashboard_history(&self, org: &str, id: &str, limit: i64) -> Result<Vec<Value>> {
        let c = self.pool.get().await?;
        let rows = c
            .query(
                // the limit is inlined: ScramDB infers a `LIMIT $n` parameter as text
                &format!(
                    "SELECT id, saved_at, saved_by, deleted FROM dashboard_versions WHERE org = $1 AND dashboard_id = $2
                     ORDER BY saved_at DESC, id DESC LIMIT {}",
                    limit.clamp(1, 1000)
                ),
                &[&org, &id],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| {
                let at: DateTime<Utc> = r.get(1);
                json!({"version": r.get::<_, String>(0), "saved_at": at.to_rfc3339(), "saved_by": r.get::<_, Option<String>>(2),
                       "deleted": r.get::<_, bool>(3)})
            })
            .collect())
    }

    /// Every dashboard of an organisation (moving them to its own database).
    pub async fn all_dashboards(&self, org: &str) -> Result<Vec<(String, String)>> {
        let c = self.pool.get().await?;
        let rows = c.query("SELECT id, yaml FROM dashboards WHERE org = $1 ORDER BY id", &[&org]).await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Forget an organisation's dashboards here (after they moved to its own database).
    pub async fn drop_org_dashboards(&self, org: &str) -> Result<u64> {
        let c = self.pool.get().await?;
        Ok(c.execute("DELETE FROM dashboards WHERE org = $1", &[&org]).await?)
    }

    // ------------------------------------------------------------------ organisation databases

    /// (sealed connection string, redacted display) of an organisation's own ScramDB.
    pub async fn org_database(&self, org: &str) -> Result<Option<(String, String)>> {
        let c = self.pool.get().await?;
        let row = c.query_opt("SELECT dsn_sealed, dsn_display FROM org_databases WHERE org = $1", &[&org]).await?;
        Ok(row.map(|r| (r.get(0), r.get(1))))
    }

    pub async fn put_org_database(&self, org: &str, sealed: &str, display: &str, by: Option<&str>) -> Result<()> {
        let c = self.pool.get().await?;
        c.execute(
            "INSERT INTO org_databases (org, dsn_sealed, dsn_display, updated_by) VALUES ($1, $2, $3, $4)
             ON CONFLICT (org) DO UPDATE SET dsn_sealed = EXCLUDED.dsn_sealed, dsn_display = EXCLUDED.dsn_display,
               updated_by = EXCLUDED.updated_by, updated_at = now()",
            &[&org, &sealed, &display, &by],
        )
        .await?;
        Ok(())
    }

    pub async fn delete_org_database(&self, org: &str) -> Result<()> {
        let c = self.pool.get().await?;
        c.execute("DELETE FROM org_databases WHERE org = $1", &[&org]).await?;
        Ok(())
    }

    /// org -> {display, updated_at, updated_by}, for the Organisations page.
    pub async fn org_databases(&self) -> Result<BTreeMap<String, Value>> {
        let c = self.pool.get().await?;
        let rows = c.query("SELECT org, dsn_display, updated_at, updated_by FROM org_databases", &[]).await?;
        Ok(rows
            .iter()
            .map(|r| {
                let at: DateTime<Utc> = r.get(2);
                (
                    r.get::<_, String>(0),
                    json!({"display": r.get::<_, String>(1), "updated_at": at.to_rfc3339(),
                           "updated_by": r.get::<_, Option<String>>(3)}),
                )
            })
            .collect())
    }

    // ------------------------------------------------------------------ data connections

    pub async fn connections(&self, org: &str) -> Result<Vec<crate::connections::Stored>> {
        let c = self.pool.get().await?;
        let rows = c
            .query("SELECT name, config, secret_sealed, updated_at, updated_by FROM connections WHERE org = $1 ORDER BY name", &[&org])
            .await?;
        rows.iter().map(stored_connection).collect()
    }

    pub async fn connection(&self, org: &str, name: &str) -> Result<Option<crate::connections::Stored>> {
        let c = self.pool.get().await?;
        let row = c
            .query_opt(
                "SELECT name, config, secret_sealed, updated_at, updated_by FROM connections WHERE org = $1 AND name = $2",
                &[&org, &name],
            )
            .await?;
        row.as_ref().map(stored_connection).transpose()
    }

    pub async fn put_connection(
        &self,
        org: &str,
        name: &str,
        config: &crate::connections::ConnectionConfig,
        secret_sealed: Option<&str>,
        by: Option<&str>,
    ) -> Result<()> {
        let c = self.pool.get().await?;
        c.execute(
            "INSERT INTO connections (org, name, config, secret_sealed, updated_by) VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (org, name) DO UPDATE SET config = EXCLUDED.config, secret_sealed = EXCLUDED.secret_sealed,
               updated_by = EXCLUDED.updated_by, updated_at = now()",
            &[&org, &name, &serde_json::to_string(config)?, &secret_sealed, &by],
        )
        .await?;
        Ok(())
    }

    pub async fn delete_connection(&self, org: &str, name: &str) -> Result<bool> {
        let c = self.pool.get().await?;
        Ok(c.execute("DELETE FROM connections WHERE org = $1 AND name = $2", &[&org, &name]).await? > 0)
    }

    // ------------------------------------------------------------------ themes

    /// The organisation's own themes and defaults (empty when it has none).
    pub async fn org_themes(&self, org: &str) -> Result<crate::themes::OrgThemes> {
        let c = self.pool.get().await?;
        let row = c.query_opt("SELECT definition FROM theme_settings WHERE org = $1", &[&org]).await?;
        Ok(match row {
            Some(r) => serde_json::from_str(&r.get::<_, String>(0))?,
            None => crate::themes::OrgThemes::default(),
        })
    }

    pub async fn put_org_themes(&self, org: &str, t: &crate::themes::OrgThemes, by: Option<&str>) -> Result<()> {
        let c = self.pool.get().await?;
        c.execute(
            "INSERT INTO theme_settings (org, definition, updated_by) VALUES ($1, $2, $3)
             ON CONFLICT (org) DO UPDATE SET definition = EXCLUDED.definition, updated_by = EXCLUDED.updated_by,
               updated_at = now()",
            &[&org, &serde_json::to_string(t)?, &by],
        )
        .await?;
        Ok(())
    }

    // ------------------------------------------------------------------ blends

    /// The organisation's own blends: (definition, updated_at, updated_by), by name.
    pub async fn blends(&self, org: &str) -> Result<Vec<(crate::blend::Blend, String, Option<String>)>> {
        let c = self.pool.get().await?;
        let rows = c
            .query("SELECT definition, updated_at, updated_by FROM blends WHERE org = $1 ORDER BY name", &[&org])
            .await?;
        rows.iter()
            .map(|r| {
                let at: DateTime<Utc> = r.get(1);
                Ok((serde_json::from_str(&r.get::<_, String>(0))?, at.to_rfc3339(), r.get(2)))
            })
            .collect()
    }

    pub async fn put_blend(&self, org: &str, blend: &crate::blend::Blend, by: Option<&str>) -> Result<()> {
        let c = self.pool.get().await?;
        c.execute(
            "INSERT INTO blends (org, name, definition, updated_by) VALUES ($1, $2, $3, $4)
             ON CONFLICT (org, name) DO UPDATE SET definition = EXCLUDED.definition, updated_by = EXCLUDED.updated_by,
               updated_at = now()",
            &[&org, &blend.name, &serde_json::to_string(blend)?, &by],
        )
        .await?;
        Ok(())
    }

    pub async fn delete_blend(&self, org: &str, name: &str) -> Result<bool> {
        let c = self.pool.get().await?;
        Ok(c.execute("DELETE FROM blends WHERE org = $1 AND name = $2", &[&org, &name]).await? > 0)
    }

    // ------------------------------------------------------------------ settings

    pub async fn setting(&self, key: &str) -> Result<Option<Value>> {
        let c = self.pool.get().await?;
        let row = c.query_opt("SELECT value FROM settings WHERE key = $1", &[&key]).await?;
        Ok(match row {
            Some(r) => Some(serde_json::from_str(&r.get::<_, String>(0))?),
            None => None,
        })
    }

    /// Several keys in one transaction (None: delete the key).
    pub async fn put_settings(&self, values: &[(String, Option<Value>)], by: Option<&str>) -> Result<()> {
        let mut c = self.pool.get().await?;
        let tx = c.transaction().await?;
        for (key, value) in values {
            match value {
                Some(v) => {
                    tx.execute(
                        "INSERT INTO settings (key, value, updated_by) VALUES ($1, $2, $3)
                         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_by = EXCLUDED.updated_by, updated_at = now()",
                        &[key, &v.to_string(), &by],
                    )
                    .await?;
                }
                None => {
                    tx.execute("DELETE FROM settings WHERE key = $1", &[key]).await?;
                }
            }
        }
        tx.commit().await?;
        Ok(())
    }
}

fn stored_connection(r: &Row) -> Result<crate::connections::Stored> {
    let at: DateTime<Utc> = r.get(3);
    Ok(crate::connections::Stored {
        name: r.get(0),
        config: serde_json::from_str(&r.get::<_, String>(1))?,
        secret_sealed: r.get(2),
        updated_at: at.to_rfc3339(),
        updated_by: r.get(4),
    })
}
