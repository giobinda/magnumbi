//! Data connectors: where compiled queries run.
//!
//! Connections belong to an organisation (connections.rs): stored in its own ScramDB, the secret
//! sealed for that organisation. `Connectors` turns a stored connection into a live connector
//! and keeps it until the connection changes. Adding a database means a `ConnectionConfig`
//! variant, a `Dialect` if its SQL differs, and an implementation of `Connector` here.

pub mod bigquery;
pub mod duckdb;
pub mod postgres;

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::Value;

use crate::compiler::Dialect;
use crate::connections::{ConnectionConfig, Stored, check_name, seal_context};
use crate::crypto::SecretKey;
use crate::materialize::ColType;
use crate::netguard::NetGuard;
use crate::store::Store;

#[derive(Debug, Clone, Default)]
pub struct QueryResult {
    pub rows: Vec<Vec<Value>>,
    /// each column's type, in a connector-neutral form: what materialising the result into
    /// ScramDB (cache, federation) needs
    pub types: Vec<ColType>,
    pub duration_ms: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum ConnectorError {
    #[error("{0}")]
    Unavailable(String),
    #[error("{0}")]
    Query(String),
}

#[async_trait]
pub trait Connector: Send + Sync {
    /// Run a read-only query; rows are JSON values in `columns` order.
    async fn run(&self, sql: &str, columns: &[String]) -> Result<QueryResult, ConnectorError>;
    /// Check the connection works (and say what it is connected to).
    async fn test(&self) -> Result<String, ConnectorError>;
}

#[derive(Debug, thiserror::Error)]
pub enum ConnError {
    #[error("this organisation has no connection named {0:?}: an admin can add it on the Connections page")]
    Missing(String),
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

type Live = (String, Arc<dyn Connector>);

/// Live connectors per (organisation, connection), rebuilt when the stored connection changes.
pub struct Connectors {
    key: Option<SecretKey>,
    /// MAGNUM_DUCKDB_DIR: one directory per organisation, plus _extensions
    duckdb_dir: PathBuf,
    install_extensions: bool,
    /// where organisations' connections may connect (netguard.rs)
    guard: NetGuard,
    /// (organisation, connection) -> (fingerprint of its settings and secret, connector)
    live: Mutex<HashMap<(String, String), Live>>,
}

/// What a connection failure says: the same for a closed port, a timeout or an unknown host, so
/// trying settings can't be used to map a network. Credentials refused by the server are named.
fn connect_failure(e: ConnectorError) -> ConnError {
    match e {
        ConnectorError::Unavailable(m)
            if m.contains("password authentication failed") || m.contains("refused the service account") =>
        {
            ConnError::Invalid("the server refused these credentials".into())
        }
        ConnectorError::Unavailable(m) => {
            tracing::info!("connection test failed: {m}");
            ConnError::Invalid(
                "can't connect: check the host, port and credentials, and that it accepts connections from Magnum BI"
                    .into(),
            )
        }
        ConnectorError::Query(m) => ConnError::Invalid(m),
    }
}

impl Connectors {
    pub fn new(key: Option<SecretKey>, duckdb_dir: PathBuf, install_extensions: bool) -> Connectors {
        // development default: private networks allowed; the server sets the real guard
        let guard = NetGuard { allow_private: true, ..NetGuard::default() };
        Connectors { key, duckdb_dir, install_extensions, guard, live: Mutex::new(HashMap::new()) }
    }

    pub fn with_guard(mut self, guard: NetGuard) -> Connectors {
        self.guard = guard;
        self
    }

    /// Organisations' connections may only reach allowed addresses; the platform workspace's
    /// connections are the system owners'. Returns the checked address to connect to (SQL servers).
    async fn guard(&self, org: &str, config: &ConnectionConfig) -> Result<Option<IpAddr>, ConnError> {
        if org.is_empty() {
            return Ok(None);
        }
        let refuse = ConnError::Invalid;
        match config {
            ConnectionConfig::Scramdb(c) | ConnectionConfig::Postgres(c) => {
                Ok(Some(self.guard.check(&c.host, c.port).await.map_err(refuse)?.ip()))
            }
            ConnectionConfig::Duckdb(c) => {
                for location in c.tables.values().filter(|l| l.starts_with("https://")) {
                    self.guard.check_url(location, 443).await.map_err(refuse)?;
                }
                if let Some(endpoint) = &c.s3_endpoint {
                    self.guard.check_url(endpoint, 443).await.map_err(refuse)?;
                }
                Ok(None)
            }
            ConnectionConfig::Bigquery(_) => Ok(None), // Google's own endpoints only
        }
    }

    fn key(&self) -> Result<&SecretKey, ConnError> {
        self.key.as_ref().ok_or_else(|| {
            ConnError::Invalid("MAGNUM_SECRET_KEY isn't set, so connection secrets can't be stored".into())
        })
    }

    pub async fn list(&self, store: &Store, org: &str) -> Result<Vec<Stored>, ConnError> {
        Ok(store.connections(org).await?)
    }

    /// The live connector for `org`'s connection `name`, and the SQL dialect to compile for it.
    pub async fn get(&self, store: &Store, org: &str, name: &str) -> Result<(Dialect, Arc<dyn Connector>), ConnError> {
        let stored = store.connection(org, name).await?.ok_or_else(|| ConnError::Missing(name.into()))?;
        let fingerprint = stored.fingerprint();
        let key = (org.to_string(), name.to_string());
        if let Some((f, c)) = self.live.lock().expect("connectors lock").get(&key)
            && *f == fingerprint
        {
            return Ok((stored.config.dialect(), c.clone()));
        }
        let pinned = self.guard(org, &stored.config).await?;
        let c = self.build(org, &stored.config, stored.secret_sealed.as_deref().map(|s| (s, name)), pinned)?;
        self.live.lock().expect("connectors lock").insert(key, (fingerprint, c.clone()));
        Ok((stored.config.dialect(), c))
    }

    /// A connector for settings and a sealed secret (`(sealed, connection name)`).
    pub fn build(
        &self,
        org: &str,
        config: &ConnectionConfig,
        sealed: Option<(&str, &str)>,
        pinned: Option<IpAddr>,
    ) -> Result<Arc<dyn Connector>, ConnError> {
        let secret = match sealed {
            Some((s, name)) => Some(self.key()?.open(s, &seal_context(org, name))?),
            None => None,
        };
        self.build_plain(org, config, secret, pinned)
    }

    fn build_plain(
        &self,
        org: &str,
        config: &ConnectionConfig,
        secret: Option<String>,
        pinned: Option<IpAddr>,
    ) -> Result<Arc<dyn Connector>, ConnError> {
        let invalid = |e: anyhow::Error| ConnError::Invalid(e.to_string());
        Ok(match config {
            ConnectionConfig::Scramdb(c) => {
                Arc::new(postgres::PgConnector::new(c, secret, "ScramDB", pinned).map_err(invalid)?)
            }
            ConnectionConfig::Postgres(c) => {
                Arc::new(postgres::PgConnector::new(c, secret, "Postgres", pinned).map_err(invalid)?)
            }
            ConnectionConfig::Bigquery(c) => Arc::new(bigquery::BigQueryConnector::new(c, secret).map_err(invalid)?),
            ConnectionConfig::Duckdb(c) => Arc::new(
                duckdb::DuckDbConnector::new(
                    c,
                    duckdb::org_dir(&self.duckdb_dir, org),
                    self.duckdb_dir.join("_extensions"),
                    self.install_extensions,
                    secret,
                )
                .map_err(invalid)?,
            ),
        })
    }

    /// Validate and store a connection. `secret`: None keeps the stored one.
    pub async fn save(
        &self,
        store: &Store,
        org: &str,
        name: &str,
        config: ConnectionConfig,
        secret: Option<&str>,
        by: Option<&str>,
    ) -> Result<(), ConnError> {
        check_name(name).map_err(ConnError::Invalid)?;
        config.validate().map_err(ConnError::Invalid)?;
        self.guard(org, &config).await?;
        let sealed = match secret.filter(|s| !s.is_empty()) {
            Some(s) => {
                config.validate_secret(s).map_err(ConnError::Invalid)?;
                Some(self.key()?.seal(s, &seal_context(org, name))?)
            }
            None => store.connection(org, name).await?.and_then(|c| c.secret_sealed),
        };
        if sealed.is_none() && config.needs_secret() {
            return Err(ConnError::Invalid(format!("a {} connection needs its key", config.kind())));
        }
        store.put_connection(org, name, &config, sealed.as_deref(), by).await?;
        Ok(())
    }

    /// Check settings (and a new secret, or the stored one) work, without saving them.
    pub async fn try_out(
        &self,
        store: &Store,
        org: &str,
        name: &str,
        config: &ConnectionConfig,
        secret: Option<&str>,
    ) -> Result<String, ConnError> {
        config.validate().map_err(ConnError::Invalid)?;
        let pinned = self.guard(org, config).await?;
        let connector = match secret.filter(|s| !s.is_empty()) {
            Some(s) => {
                config.validate_secret(s).map_err(ConnError::Invalid)?;
                self.build_plain(org, config, Some(s.to_string()), pinned)?
            }
            None => {
                let stored = store.connection(org, name).await?.and_then(|c| c.secret_sealed);
                self.build(org, config, stored.as_deref().map(|s| (s, name)), pinned)?
            }
        };
        connector.test().await.map_err(connect_failure)
    }

    pub async fn delete(&self, store: &Store, org: &str, name: &str) -> Result<bool, ConnError> {
        self.live.lock().expect("connectors lock").remove(&(org.to_string(), name.to_string()));
        Ok(store.delete_connection(org, name).await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connections::ConnectionConfig;
    use crate::crypto::SecretKey;
    use crate::store::Store;
    use serde_json::json;

    /// A real ScramDB (e.g. the dev primary: postgresql://scramdb@127.0.0.1:55432/scramdb);
    /// these tests are skipped without MAGNUM_TEST_DATABASE_URL.
    async fn store() -> Option<Store> {
        let url = std::env::var("MAGNUM_TEST_DATABASE_URL").ok()?;
        let pool = crate::db::make_pool(&url, 4).unwrap();
        crate::db::migrate(&pool, crate::db::ORG).await.unwrap();
        Some(Store::new(pool))
    }

    fn registry() -> Connectors {
        let key = SecretKey::parse(&SecretKey::generate()).unwrap();
        Connectors::new(Some(key), std::env::temp_dir().join("magnum-test-duckdb"), false)
    }

    fn scramdb(url: &str) -> ConnectionConfig {
        let u = url::Url::parse(url).unwrap();
        serde_json::from_value(json!({"type": "scramdb", "host": u.host_str().unwrap(), "port": u.port().unwrap_or(5432),
                                      "database": u.path().trim_start_matches('/'), "user": u.username(), "tls": false}))
        .unwrap()
    }

    #[tokio::test]
    async fn organisations_own_their_connections() {
        let Some(store) = store().await else { return };
        let url = std::env::var("MAGNUM_TEST_DATABASE_URL").unwrap();
        let (acme, globex) =
            (format!("t{}", uuid::Uuid::new_v4().simple()), format!("t{}", uuid::Uuid::new_v4().simple()));
        let reg = registry();

        reg.save(&store, &acme, "warehouse", scramdb(&url), Some("not-needed"), Some("test")).await.unwrap();
        let listed = reg.list(&store, &acme).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].public()["has_secret"], json!(true));
        assert!(!listed[0].public().to_string().contains("not-needed"), "secrets are never listed");

        let (dialect, c) = reg.get(&store, &acme, "warehouse").await.unwrap();
        assert_eq!(dialect, crate::compiler::Dialect::Postgres);
        assert_eq!(c.run("SELECT 1 AS one", &[]).await.unwrap().rows, vec![vec![json!(1)]]);

        // another organisation doesn't have it
        assert!(matches!(reg.get(&store, &globex, "warehouse").await, Err(ConnError::Missing(_))));

        // saving without a secret keeps the stored one; a changed config gives a new connector
        let mut changed = scramdb(&url);
        if let ConnectionConfig::Scramdb(s) = &mut changed {
            s.statement_timeout_ms = 5_000;
        }
        reg.save(&store, &acme, "warehouse", changed, None, Some("test")).await.unwrap();
        assert_eq!(reg.list(&store, &acme).await.unwrap()[0].public()["has_secret"], json!(true));
        let (_, c2) = reg.get(&store, &acme, "warehouse").await.unwrap();
        assert!(!Arc::ptr_eq(&c, &c2), "rebuilt after a change");

        reg.delete(&store, &acme, "warehouse").await.unwrap();
        assert!(matches!(reg.get(&store, &acme, "warehouse").await, Err(ConnError::Missing(_))));
    }

    #[tokio::test]
    async fn invalid_settings_and_secrets_are_refused() {
        let Some(store) = store().await else { return };
        let reg = registry();
        let bq: ConnectionConfig =
            serde_json::from_value(json!({"type": "bigquery", "project": "acme-analytics"})).unwrap();
        assert!(reg.save(&store, "acme", "bq", bq.clone(), Some("not a key"), None).await.is_err());
        assert!(reg.save(&store, "acme", "bad name!", bq, None, None).await.is_err());
    }

    fn strict(internal: &str) -> Connectors {
        let key = SecretKey::parse(&SecretKey::generate()).unwrap();
        let guard = crate::netguard::NetGuard { internal: vec![internal.into()], ..Default::default() };
        Connectors::new(Some(key), std::env::temp_dir().join("magnum-test-duckdb"), false).with_guard(guard)
    }

    #[tokio::test]
    async fn organisations_cannot_reach_internal_addresses() {
        let Some(store) = store().await else { return };
        let url = std::env::var("MAGNUM_TEST_DATABASE_URL").unwrap();
        let u = url::Url::parse(&url).unwrap();
        let reg = strict(&format!("{}:{}", u.host_str().unwrap(), u.port().unwrap_or(5432)));
        let org = format!("t{}", uuid::Uuid::new_v4().simple());

        let e = reg.save(&store, &org, "w", scramdb(&url), None, None).await.unwrap_err().to_string();
        assert!(e.contains("internal"), "{e}");
        let e = reg.try_out(&store, &org, "w", &scramdb(&url), None).await.unwrap_err().to_string();
        assert!(e.contains("internal"), "{e}");

        // stored before the guard (or by hand): refused when it connects
        store.put_connection(&org, "w", &scramdb(&url), None, None).await.unwrap();
        assert!(reg.get(&store, &org, "w").await.is_err());

        // object storage locations too
        let duck: ConnectionConfig =
            serde_json::from_value(json!({"type": "duckdb", "tables": {"t": "https://127.0.0.1:8011/x.parquet"}}))
                .unwrap();
        assert!(reg.save(&store, &org, "d", duck, None, None).await.is_err());

        // the platform workspace is the system owners': exempt
        assert!(
            reg.save(&store, "", &format!("p{}", uuid::Uuid::new_v4().simple()), scramdb(&url), None, None)
                .await
                .is_ok()
        );
        store.delete_connection(&org, "w").await.unwrap();
    }

    #[tokio::test]
    async fn connection_failures_dont_reveal_open_ports() {
        let Some(store) = store().await else { return };
        let reg = registry(); // development: private addresses allowed
        let closed: ConnectionConfig = serde_json::from_value(
            json!({"type": "postgres", "host": "127.0.0.1", "port": 1, "database": "d", "user": "u", "tls": false}),
        )
        .unwrap();
        let e = reg.try_out(&store, "acme", "x", &closed, None).await.unwrap_err().to_string();
        assert!(!e.to_lowercase().contains("refused") && e.contains("can't connect"), "{e}");
    }
}
