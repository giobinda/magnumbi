//! Data connectors: where compiled queries run.
//!
//! Each connection in connections.yaml gets one connector. Only ScramDB exists today; adding a
//! database means a new `Connection` variant (schema.rs), a `Dialect` if its SQL differs
//! (compiler.rs, chosen in ProjectState::dialect), and an implementation of `Connector` here.

pub mod scramdb;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::Value;

use crate::db::DbPool;
use crate::schema::Connection;

#[derive(Debug, Clone, Default)]
pub struct QueryResult {
    pub rows: Vec<Vec<Value>>,
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
}

pub fn make_connector(conn: &Connection, own_db: &DbPool) -> anyhow::Result<Arc<dyn Connector>> {
    Ok(match conn {
        Connection::Scramdb(c) => Arc::new(scramdb::ScramDbConnector::new(c, own_db)?),
    })
}

type Live = BTreeMap<String, (Connection, Arc<dyn Connector>)>;

/// One connector per connection, rebuilt when its configuration changes.
pub struct Connectors {
    own_db: DbPool,
    live: Mutex<Live>,
}

impl Connectors {
    pub fn new(own_db: DbPool) -> Connectors {
        Connectors { own_db, live: Mutex::new(BTreeMap::new()) }
    }

    pub fn get(&self, name: &str, conn: &Connection) -> anyhow::Result<Arc<dyn Connector>> {
        let mut live = self.live.lock().expect("connectors lock");
        if let Some((cfg, c)) = live.get(name)
            && cfg == conn
        {
            return Ok(c.clone());
        }
        let c = make_connector(conn, &self.own_db)?;
        live.insert(name.into(), (conn.clone(), c.clone()));
        Ok(c)
    }
}
