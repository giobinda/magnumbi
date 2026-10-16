//! Each organisation's dedicated ScramDB instance: its dashboards and its query cache.
//!
//! System owners register an instance per organisation (Organisations page, or
//! `magnum-bi org set-database`). The connection string is sealed with MAGNUM_SECRET_KEY in the
//! primary database (crypto.rs). Pools are opened on first use, migrated with the organisation
//! migrations, and kept; the registry is re-read every few seconds so every replica follows a
//! change. The platform workspace ("") lives in the primary database.
//!
//! Fails closed: an organisation without a registered instance has no dashboards to show
//! (never a silent fallback to the primary database), and one unreachable instance only
//! affects its own organisation.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use crate::crypto::SecretKey;
use crate::db::{self, DbPool};
use crate::store::Store;

/// How long a known registration is trusted before re-reading it (another replica may change it).
const RECHECK: Duration = Duration::from_secs(10);

#[derive(Debug, thiserror::Error)]
pub enum OrgDbError {
    #[error("{0} has no database yet: a system owner must set its ScramDB on the Organisations page")]
    NotConfigured(String),
    #[error("{0}")]
    Unavailable(String),
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

struct Open {
    sealed: String,
    store: Store,
    checked: Instant,
}

pub struct OrgDbs {
    primary: Store,
    key: Option<SecretKey>,
    open: Mutex<HashMap<String, Open>>,
    /// tests: organisations given a store directly (no registered database)
    #[cfg(test)]
    pinned: std::sync::Mutex<HashMap<String, Store>>,
}

impl OrgDbs {
    pub fn new(primary: Store, key: Option<SecretKey>) -> OrgDbs {
        OrgDbs {
            primary,
            key,
            open: Mutex::new(HashMap::new()),
            #[cfg(test)]
            pinned: Default::default(),
        }
    }

    /// Tests: `org`'s data goes to `store` (rows carry the organisation, so the test database's
    /// own stay apart), without registering a database.
    #[cfg(test)]
    pub fn pin(&self, org: &str, store: Store) {
        self.pinned.lock().expect("pinned").insert(org.to_string(), store);
    }

    fn key(&self) -> Result<&SecretKey, OrgDbError> {
        self.key.as_ref().ok_or_else(|| {
            OrgDbError::Unavailable("MAGNUM_SECRET_KEY isn't set, so organisation databases can't be used".into())
        })
    }

    /// Where `org`'s dashboards and cache live: the primary database for the platform workspace
    /// (""), else the organisation's own instance.
    pub async fn store(&self, org: &str) -> Result<Store, OrgDbError> {
        if org.is_empty() {
            return Ok(self.primary.clone());
        }
        #[cfg(test)]
        if let Some(s) = self.pinned.lock().expect("pinned").get(org) {
            return Ok(s.clone());
        }
        let mut open = self.open.lock().await;
        if let Some(o) = open.get(org).filter(|o| o.checked.elapsed() < RECHECK) {
            return Ok(o.store.clone());
        }
        let Some((sealed, _)) = self.primary.org_database(org).await? else {
            open.remove(org);
            return Err(OrgDbError::NotConfigured(org.into()));
        };
        if let Some(o) = open.get_mut(org).filter(|o| o.sealed == sealed) {
            o.checked = Instant::now();
            return Ok(o.store.clone());
        }
        let dsn =
            self.key()?.open(&sealed, org).map_err(|e| OrgDbError::Unavailable(format!("{org}'s database: {e}")))?;
        let store = connect(&dsn).await.map_err(|e| OrgDbError::Unavailable(format!("{org}'s database: {e:#}")))?;
        open.insert(org.into(), Open { sealed, store: store.clone(), checked: Instant::now() });
        tracing::info!(org, "opened organisation database {}", db::redact(&dsn));
        Ok(store)
    }

    /// Point `org` at a ScramDB instance: test it, migrate it, move the organisation's existing
    /// dashboards there (when it has none yet), then save the sealed connection string.
    /// Returns (redacted connection string, server version, dashboards moved).
    pub async fn register(
        &self,
        org: &str,
        dsn: &str,
        by: Option<&str>,
    ) -> Result<(String, String, usize), OrgDbError> {
        let key = self.key()?;
        let dsn = dsn.trim();
        let new = connect(dsn).await.map_err(|e| OrgDbError::Unavailable(format!("can't use that database: {e:#}")))?;
        let version: String = {
            let c = new.pool.get().await.map_err(|e| OrgDbError::Unavailable(e.to_string()))?;
            c.query_one("SELECT version()", &[]).await.map_err(|e| OrgDbError::Unavailable(e.to_string()))?.get(0)
        };
        // the organisation's dashboards so far: in its previous instance, or in the primary
        // database (organisations created before they had their own)
        let previous = match self.store(org).await {
            Ok(s) => Some(s),
            Err(OrgDbError::NotConfigured(_)) => None,
            Err(e) => return Err(e),
        };
        let from = previous.clone().unwrap_or_else(|| self.primary.clone());
        let mut moved = 0;
        if !same_instance(&from.pool, &new.pool).await && new.dashboard_ids(org).await?.is_empty() {
            for (id, yaml) in from.all_dashboards(org).await? {
                new.write_dashboard(org, &id, &yaml, Some("moved")).await?;
                moved += 1;
            }
            // they live in the organisation's instance now; a previous dedicated instance keeps its
            // copy, but the primary database only holds the platform's own dashboards
            if same_instance(&from.pool, &self.primary.pool).await {
                self.primary.drop_org_dashboards(org).await?;
            }
        }
        let shown = db::redact(dsn);
        let sealed = key.seal(dsn, org)?;
        self.primary.put_org_database(org, &sealed, &shown, by).await?;
        self.open.lock().await.insert(org.into(), Open { sealed, store: new, checked: Instant::now() });
        tracing::info!(org, moved, "organisation database set to {}", shown);
        Ok((shown, version, moved))
    }

    /// The organisation is gone: forget where its database is (its data stays in the instance).
    pub async fn forget(&self, org: &str) -> anyhow::Result<()> {
        self.primary.delete_org_database(org).await?;
        self.open.lock().await.remove(org);
        Ok(())
    }
}

/// Open and migrate an organisation instance (a few connections: dashboards and cache only).
async fn connect(dsn: &str) -> anyhow::Result<Store> {
    let pool = db::make_pool(dsn, 8)?;
    db::migrate(&pool, db::ORG).await?;
    Ok(Store::new(pool))
}

/// Two pools on the same ScramDB instance (a development setup sharing one instance)?
async fn same_instance(a: &DbPool, b: &DbPool) -> bool {
    async fn id(p: &DbPool) -> Option<String> {
        let c = p.get().await.ok()?;
        // a marker table each instance keeps for itself
        c.batch_execute("CREATE TABLE IF NOT EXISTS magnum_instance (id TEXT PRIMARY KEY)").await.ok()?;
        let fresh = uuid::Uuid::new_v4().to_string();
        c.execute(
            "INSERT INTO magnum_instance (id) SELECT $1::TEXT WHERE NOT EXISTS (SELECT 1 FROM magnum_instance)",
            &[&fresh],
        )
        .await
        .ok()?;
        Some(c.query_one("SELECT id FROM magnum_instance LIMIT 1", &[]).await.ok()?.get(0))
    }
    matches!((id(a).await, id(b).await), (Some(x), Some(y)) if x == y)
}
