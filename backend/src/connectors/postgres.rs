//! ScramDB and PostgreSQL (the PostgreSQL wire protocol). Queries run in a READ ONLY
//! transaction with a statement timeout and, when the connection names a schema, that
//! search_path. TLS (when on) verifies the server certificate and host name against the public
//! certificate authorities.

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use deadpool_postgres::{Manager, ManagerConfig, Pool, RecyclingMethod};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use serde_json::{Value, json};
use tokio_postgres::Row;
use tokio_postgres::config::SslMode;
use tokio_postgres::types::Type;

use super::{Connector, ConnectorError, QueryResult};
use crate::compiler::ident;
use crate::connections::SqlServer;
use crate::materialize::ColType;

pub struct PgConnector {
    pool: Pool,
    schema: Option<String>,
    statement_timeout_ms: u64,
    label: &'static str,
}

/// A TLS connector that verifies certificates against the public roots (webpki-roots).
pub fn tls() -> tokio_postgres_rustls::MakeRustlsConnect {
    let roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .expect("TLS versions")
        .with_root_certificates(roots)
        .with_no_client_auth();
    tokio_postgres_rustls::MakeRustlsConnect::new(config)
}

impl PgConnector {
    /// `label`: "ScramDB" or "Postgres", for messages.
    /// `pinned`: the address the host was checked to resolve to (netguard.rs); connecting there
    /// (the host name still names the server for TLS) means a later DNS answer can't redirect it.
    pub fn new(
        c: &SqlServer,
        password: Option<String>,
        label: &'static str,
        pinned: Option<std::net::IpAddr>,
    ) -> anyhow::Result<PgConnector> {
        let mut config = tokio_postgres::Config::new();
        config
            .host(&c.host)
            .port(c.port)
            .dbname(&c.database)
            .user(&c.user)
            .application_name("magnum-bi")
            .connect_timeout(Duration::from_secs(10))
            .ssl_mode(if c.tls { SslMode::Require } else { SslMode::Disable });
        if let Some(p) = password {
            config.password(p);
        }
        if let Some(ip) = pinned {
            config.hostaddr(ip);
        }
        let manager = if c.tls {
            Manager::from_config(config, tls(), ManagerConfig { recycling_method: RecyclingMethod::Fast })
        } else {
            Manager::from_config(
                config,
                tokio_postgres::NoTls,
                ManagerConfig { recycling_method: RecyclingMethod::Fast },
            )
        };
        let pool = Pool::builder(manager).max_size(8).build()?;
        Ok(PgConnector { pool, schema: c.schema.clone(), statement_timeout_ms: c.statement_timeout_ms, label })
    }
}

#[async_trait]
impl Connector for PgConnector {
    async fn run(&self, sql: &str, _columns: &[String]) -> Result<QueryResult, ConnectorError> {
        let started = Instant::now();
        let mut client = self
            .pool
            .get()
            .await
            .map_err(|e| ConnectorError::Unavailable(format!("can't reach {}: {}", self.label, pool_error(&e))))?;
        let tx = client
            .build_transaction()
            .read_only(true)
            .start()
            .await
            .map_err(|e| ConnectorError::Query(e.to_string()))?;
        let mut setup = format!("SET LOCAL statement_timeout = {}", self.statement_timeout_ms);
        if let Some(schema) = &self.schema {
            setup.push_str(&format!("; SET LOCAL search_path TO {}", ident(schema)));
        }
        tx.batch_execute(&setup).await.map_err(|e| ConnectorError::Query(db_error(&e)))?;
        let rows = tx.query(sql, &[]).await.map_err(|e| ConnectorError::Query(db_error(&e)))?;
        tx.commit().await.map_err(|e| ConnectorError::Query(e.to_string()))?;
        let types = match rows.first() {
            Some(r) => r.columns().iter().map(|c| col_type(c.type_())).collect(),
            None => vec![],
        };
        let rows = rows.iter().map(row_json).collect();
        Ok(QueryResult { rows, types, duration_ms: started.elapsed().as_millis() as u64 })
    }

    async fn test(&self) -> Result<String, ConnectorError> {
        let client = self
            .pool
            .get()
            .await
            .map_err(|e| ConnectorError::Unavailable(format!("can't reach {}: {}", self.label, pool_error(&e))))?;
        let version: String =
            client.query_one("SELECT version()", &[]).await.map_err(|e| ConnectorError::Query(db_error(&e)))?.get(0);
        if let Some(schema) = &self.schema {
            let found = client
                .query_opt("SELECT 1 FROM information_schema.schemata WHERE schema_name = $1::TEXT", &[schema])
                .await
                .map_err(|e| ConnectorError::Query(db_error(&e)))?;
            if found.is_none() {
                return Err(ConnectorError::Query(format!(
                    "schema {schema:?} doesn't exist (or isn't visible to this user)"
                )));
            }
        }
        Ok(version)
    }
}

/// The root cause of a pool error (the TLS or connection error, not "error creating a new object").
fn pool_error(e: &deadpool_postgres::PoolError) -> String {
    let mut msg = e.to_string();
    let mut source = std::error::Error::source(e);
    while let Some(s) = source {
        msg = s.to_string();
        source = s.source();
    }
    msg
}

fn db_error(e: &tokio_postgres::Error) -> String {
    match e.as_db_error() {
        Some(d) => format!("{}: {}", d.code().code(), d.message()),
        None => e.to_string(),
    }
}

fn num(f: Option<f64>) -> Value {
    f.and_then(serde_json::Number::from_f64).map_or(Value::Null, Value::Number)
}

/// Integers stay integers; whole decimals become integers (like the UI expects); NaN/inf: null.
pub fn decimal(d: Decimal) -> Value {
    if d.fract().is_zero()
        && let Some(i) = d.to_i64()
    {
        return json!(i);
    }
    num(d.to_f64())
}

/// The neutral type of a ScramDB column (anything unusual travels as text).
pub fn col_type(t: &Type) -> ColType {
    match *t {
        Type::BOOL => ColType::Bool,
        Type::INT2 | Type::INT4 | Type::INT8 => ColType::Int,
        Type::FLOAT4 | Type::FLOAT8 => ColType::Float,
        Type::NUMERIC => ColType::Numeric,
        Type::DATE => ColType::Date,
        Type::TIMESTAMP => ColType::Timestamp,
        Type::TIMESTAMPTZ => ColType::Timestamptz,
        Type::JSON | Type::JSONB => ColType::Json,
        _ => ColType::Text,
    }
}

pub fn row_json(row: &Row) -> Vec<Value> {
    (0..row.len()).map(|i| cell(row, i)).collect()
}

fn cell(row: &Row, i: usize) -> Value {
    let t = row.columns()[i].type_().clone();
    let v = match t {
        Type::BOOL => row.try_get::<_, Option<bool>>(i).map(|v| json!(v)),
        Type::INT2 => row.try_get::<_, Option<i16>>(i).map(|v| json!(v)),
        Type::INT4 => row.try_get::<_, Option<i32>>(i).map(|v| json!(v)),
        Type::INT8 => row.try_get::<_, Option<i64>>(i).map(|v| json!(v)),
        Type::FLOAT4 => row.try_get::<_, Option<f32>>(i).map(|v| num(v.map(f64::from))),
        Type::FLOAT8 => row.try_get::<_, Option<f64>>(i).map(num),
        Type::NUMERIC => row.try_get::<_, Option<Decimal>>(i).map(|v| v.map_or(Value::Null, decimal)),
        Type::DATE => row.try_get::<_, Option<NaiveDate>>(i).map(|v| json!(v.map(|d| d.to_string()))),
        Type::TIMESTAMP => row
            .try_get::<_, Option<NaiveDateTime>>(i)
            .map(|v| json!(v.map(|d| d.format("%Y-%m-%dT%H:%M:%S%.f").to_string()))),
        Type::TIMESTAMPTZ => row.try_get::<_, Option<DateTime<Utc>>>(i).map(|v| json!(v.map(|d| d.to_rfc3339()))),
        Type::JSON | Type::JSONB => row.try_get::<_, Option<Value>>(i).map(|v| v.unwrap_or(Value::Null)),
        _ => row.try_get::<_, Option<String>>(i).map(|v| json!(v)),
    };
    v.unwrap_or_else(|e| {
        tracing::debug!(column = row.columns()[i].name(), "unsupported column type {t}: {e}");
        Value::Null
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn decimals() {
        assert_eq!(decimal(Decimal::from_str("12.00").unwrap()), json!(12));
        assert_eq!(decimal(Decimal::from_str("12.5").unwrap()), json!(12.5));
    }
}

#[cfg(test)]
mod pg_tests {
    use super::*;

    /// A real PostgreSQL (MAGNUM_TEST_POSTGRES_URL, e.g. postgresql://postgres:pw@127.0.0.1:55440/postgres);
    /// skipped without it. Each test gets its own schema (tests run in parallel).
    async fn server() -> Option<(SqlServer, String)> {
        let url = url::Url::parse(&std::env::var("MAGNUM_TEST_POSTGRES_URL").ok()?).unwrap();
        let schema = format!("magnum_test_{}", uuid::Uuid::new_v4().simple());
        let cfg: SqlServer = serde_json::from_value(json!({
            "host": url.host_str().unwrap(), "port": url.port().unwrap_or(5432),
            "database": url.path().trim_start_matches('/'), "user": url.username(), "tls": false, "schema": schema,
        }))
        .unwrap();
        let password = url.password().unwrap_or_default().to_string();
        let admin =
            PgConnector::new(&SqlServer { schema: None, ..cfg.clone() }, Some(password.clone()), "Postgres", None)
                .unwrap();
        let c = admin.pool.get().await.unwrap();
        c.batch_execute(&format!(
            "CREATE SCHEMA {schema};
             CREATE TABLE {schema}.sales (region TEXT, amount NUMERIC(10,2), sold_at TIMESTAMPTZ);
             INSERT INTO {schema}.sales VALUES ('EMEA', 10.50, '2026-09-01 00:00:00+00'), ('AMER', 3, NULL);",
        ))
        .await
        .unwrap();
        Some((cfg, password))
    }

    #[tokio::test]
    async fn unqualified_tables_resolve_in_the_connections_schema() {
        let Some((cfg, pw)) = server().await else { return };
        let c = PgConnector::new(&cfg, Some(pw), "Postgres", None).unwrap();
        let r = c.run("SELECT region, amount, sold_at FROM sales ORDER BY region", &[]).await.unwrap();
        assert_eq!(r.types, [ColType::Text, ColType::Numeric, ColType::Timestamptz]);
        assert_eq!(r.rows[1], vec![json!("EMEA"), json!(10.5), json!("2026-09-01T00:00:00+00:00")]);
        assert!(c.test().await.unwrap().contains("PostgreSQL"));
        // read-only: no writes, whatever the SQL says
        assert!(c.run("DELETE FROM sales", &[]).await.is_err());
    }

    #[tokio::test]
    async fn a_missing_schema_is_reported() {
        let Some((cfg, pw)) = server().await else { return };
        let c =
            PgConnector::new(&SqlServer { schema: Some("nope".into()), ..cfg }, Some(pw), "Postgres", None).unwrap();
        let e = c.test().await.unwrap_err().to_string();
        assert!(e.contains("schema \"nope\" doesn't exist"), "{e}");
    }

    #[tokio::test]
    async fn tls_is_required_when_asked_never_downgraded() {
        let Some((cfg, pw)) = server().await else { return };
        // the test server has no TLS: asking for it must fail, not fall back to plain text
        let c = PgConnector::new(&SqlServer { tls: true, ..cfg }, Some(pw), "Postgres", None).unwrap();
        let e = c.test().await.unwrap_err().to_string();
        assert!(e.contains("can't reach Postgres"), "{e}");
    }

    #[tokio::test]
    async fn wrong_passwords_are_refused() {
        let Some((cfg, _)) = server().await else { return };
        let c = PgConnector::new(&cfg, Some("wrong".into()), "Postgres", None).unwrap();
        assert!(c.test().await.is_err());
    }
}
