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
}

const USER_COLUMNS: &str = "email, name, password_hash, google_sub, owner_org, must_change_password, disabled";

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
        }
    }
}

#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub method: String,
    pub user: User,
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
    pub async fn create_session(&self, email: &str, method: &str, ttl: Duration) -> Result<String> {
        let token = random_token(32);
        let c = self.pool.get().await?;
        c.execute(
            "INSERT INTO sessions (id_hash, user_email, method, expires_at) VALUES ($1, $2, $3, $4)",
            &[&sha256_hex(&token), &email.to_lowercase(), &method, &(Utc::now() + ttl)],
        )
        .await?;
        // now and then, forget expired sessions and sign-ins
        if rand::random::<u8>() < 8 {
            c.execute("DELETE FROM sessions WHERE expires_at < now()", &[]).await?;
            c.execute("DELETE FROM oauth_states WHERE expires_at < now()", &[]).await?;
        }
        Ok(token)
    }

    pub async fn session(&self, token: &str) -> Result<Option<SessionInfo>> {
        if token.is_empty() || token.len() > 128 {
            return Ok(None);
        }
        let c = self.pool.get().await?;
        let row = c
            .query_opt(
                "SELECT s.method, u.email, u.name, u.password_hash, u.google_sub, u.owner_org, u.must_change_password, u.disabled
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
                },
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
    ) -> Result<()> {
        let c = self.pool.get().await?;
        c.execute(
            "INSERT INTO oauth_states (state_hash, verifier, nonce, redirect_uri, next_url, expires_at) VALUES ($1, $2, $3, $4, $5, $6)",
            &[&sha256_hex(state), &verifier, &nonce, &redirect_uri, &next, &(Utc::now() + Duration::minutes(10))],
        )
        .await?;
        Ok(())
    }

    /// One use only: (verifier, nonce, redirect_uri, next).
    pub async fn take_oauth_state(&self, state: &str) -> Result<Option<(String, String, String, String)>> {
        let c = self.pool.get().await?;
        let row = c
            .query_opt(
                "DELETE FROM oauth_states WHERE state_hash = $1 AND expires_at > now() RETURNING verifier, nonce, redirect_uri, next_url",
                &[&sha256_hex(state)],
            )
            .await?;
        Ok(row.map(|r| (r.get(0), r.get(1), r.get(2), r.get(3))))
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
