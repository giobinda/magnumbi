//! Data connections, owned by each organisation.
//!
//! An organisation's connections live in its own ScramDB (the platform workspace's in the
//! primary), managed by its Org admins and by system owners on the Connections page. Settings
//! are plain JSON; the secret (a password, a service-account key, storage keys) is sealed with
//! MAGNUM_SECRET_KEY, bound to the organisation and the connection name, and never shown again.
//!
//! Models are platform YAML and name a connection (`connection: warehouse`) plus unqualified
//! tables. Each organisation's own `warehouse` decides where those tables are: its Postgres
//! schema, its BigQuery dataset, or its files for DuckDB. One model, every organisation's own
//! data, each with its own credentials (docs/org-isolation.md).

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::compiler::Dialect;
use crate::schema::NAME_RE;

/// Logical table names (DuckDB views), schemas and datasets: plain identifiers.
static IDENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z_][A-Za-z0-9_]{0,127}$").unwrap());
static HOST: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9.\-:\[\]]{1,253}$").unwrap());
static GCP_PROJECT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([a-z][a-z0-9.\-]{0,61}:)?[a-z][a-z0-9\-]{4,28}[a-z0-9]$").unwrap());
static LOCATION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9\-]{2,40}$").unwrap());

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ConnectionConfig {
    Scramdb(SqlServer),
    Postgres(SqlServer),
    Bigquery(BigQuery),
    Duckdb(DuckDb),
}

/// ScramDB or PostgreSQL. Secret: the password.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SqlServer {
    pub host: String,
    #[serde(default = "pg_port")]
    pub port: u16,
    pub database: String,
    pub user: String,
    /// where the models' unqualified tables are (search_path); default: the server's default
    #[serde(default)]
    pub schema: Option<String>,
    /// TLS with certificate and host name verification (public CAs)
    #[serde(default = "yes")]
    pub tls: bool,
    #[serde(default = "minute")]
    pub statement_timeout_ms: u64,
}

/// BigQuery. Secret: a service-account JSON key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BigQuery {
    /// where the data is (unqualified tables: `project.dataset.table`)
    pub project: String,
    /// who pays for queries; default: `project`
    #[serde(default)]
    pub billing_project: Option<String>,
    /// default dataset for unqualified table names
    #[serde(default)]
    pub dataset: Option<String>,
    /// e.g. US, EU, europe-west1
    #[serde(default)]
    pub location: Option<String>,
    /// hard cap per query, checked with a dry run first (default 10 GiB)
    #[serde(default = "ten_gib")]
    pub max_bytes_billed: u64,
    #[serde(default = "two_minutes")]
    pub timeout_s: u64,
}

/// DuckDB, in a sandbox per query. Secret (optional): S3 or GCS HMAC keys as
/// {"key_id": "...", "secret": "..."}.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DuckDb {
    /// a .duckdb file in the organisation's directory, opened read-only
    #[serde(default)]
    pub database: Option<String>,
    /// logical table -> files: a path in the organisation's directory, or s3:// gs:// https://
    /// (Parquet, CSV or JSON; globs allowed), e.g. {"sales": "s3://acme-data/sales/*.parquet"}
    #[serde(default)]
    pub tables: BTreeMap<String, String>,
    #[serde(default)]
    pub s3_region: Option<String>,
    /// S3-compatible endpoint (MinIO, R2, ...), host[:port]
    #[serde(default)]
    pub s3_endpoint: Option<String>,
    #[serde(default = "minute")]
    pub statement_timeout_ms: u64,
}

fn pg_port() -> u16 {
    5432
}
fn yes() -> bool {
    true
}
fn minute() -> u64 {
    60_000
}
fn ten_gib() -> u64 {
    10 * 1024 * 1024 * 1024
}
fn two_minutes() -> u64 {
    120
}

impl ConnectionConfig {
    pub fn kind(&self) -> &'static str {
        match self {
            ConnectionConfig::Scramdb(_) => "scramdb",
            ConnectionConfig::Postgres(_) => "postgres",
            ConnectionConfig::Bigquery(_) => "bigquery",
            ConnectionConfig::Duckdb(_) => "duckdb",
        }
    }

    pub fn dialect(&self) -> Dialect {
        match self {
            ConnectionConfig::Scramdb(_) | ConnectionConfig::Postgres(_) => Dialect::Postgres,
            ConnectionConfig::Bigquery(_) => Dialect::BigQuery,
            ConnectionConfig::Duckdb(_) => Dialect::DuckDb,
        }
    }

    /// Does this kind of connection need a secret before it can be used?
    pub fn needs_secret(&self) -> bool {
        matches!(self, ConnectionConfig::Bigquery(_))
    }

    pub fn validate(&self) -> Result<(), String> {
        match self {
            ConnectionConfig::Scramdb(c) | ConnectionConfig::Postgres(c) => {
                if !HOST.is_match(&c.host) {
                    return Err("host: a host name or IP address".into());
                }
                if c.database.trim().is_empty() || c.user.trim().is_empty() {
                    return Err("database and user are required".into());
                }
                if let Some(s) = &c.schema
                    && !IDENT.is_match(s)
                {
                    return Err("schema: letters, digits and underscores".into());
                }
                check_timeout(c.statement_timeout_ms)
            }
            ConnectionConfig::Bigquery(c) => {
                for p in std::iter::once(&c.project).chain(c.billing_project.iter()) {
                    if !GCP_PROJECT.is_match(p) {
                        return Err(format!("{p:?} isn't a Google Cloud project id"));
                    }
                }
                if c.dataset.as_deref().is_some_and(|d| !IDENT.is_match(d)) {
                    return Err("dataset: letters, digits and underscores".into());
                }
                if c.location.as_deref().is_some_and(|l| !LOCATION.is_match(l)) {
                    return Err("location: e.g. US, EU or europe-west1".into());
                }
                if c.max_bytes_billed == 0 {
                    return Err("max_bytes_billed: more than 0".into());
                }
                if !(5..=600).contains(&c.timeout_s) {
                    return Err("timeout_s: between 5 and 600".into());
                }
                Ok(())
            }
            ConnectionConfig::Duckdb(c) => {
                if c.database.is_none() && c.tables.is_empty() {
                    return Err("set a database file, tables, or both".into());
                }
                if let Some(db) = &c.database {
                    local_path(db)?;
                    if !db.ends_with(".duckdb") && !db.ends_with(".db") {
                        return Err("database: a .duckdb file".into());
                    }
                }
                for (name, location) in &c.tables {
                    if !IDENT.is_match(name) {
                        return Err(format!("table {name:?}: letters, digits and underscores"));
                    }
                    file_reader(location).map_err(|e| format!("table {name}: {e}"))?;
                }
                if c.s3_region.as_deref().is_some_and(|r| !LOCATION.is_match(r)) {
                    return Err("s3_region: e.g. eu-west-1".into());
                }
                if c.s3_endpoint.as_deref().is_some_and(|e| !HOST.is_match(e)) {
                    return Err("s3_endpoint: host[:port]".into());
                }
                check_timeout(c.statement_timeout_ms)
            }
        }
    }

    /// Check a secret entered for this connection (never stored or logged in clear).
    pub fn validate_secret(&self, secret: &str) -> Result<(), String> {
        match self {
            ConnectionConfig::Scramdb(_) | ConnectionConfig::Postgres(_) => {
                if secret.len() > 1024 {
                    return Err("password too long".into());
                }
                Ok(())
            }
            ConnectionConfig::Bigquery(_) => crate::connectors::bigquery::ServiceAccount::parse(secret).map(|_| ()),
            ConnectionConfig::Duckdb(_) => crate::connectors::duckdb::StorageKeys::parse(secret).map(|_| ()),
        }
    }
}

fn check_timeout(ms: u64) -> Result<(), String> {
    if (1_000..=600_000).contains(&ms) { Ok(()) } else { Err("statement_timeout_ms: between 1000 and 600000".into()) }
}

/// A path inside the organisation's own directory: relative, no `..`.
pub fn local_path(p: &str) -> Result<(), String> {
    let path = std::path::Path::new(p);
    if p.is_empty()
        || path.is_absolute()
        || p.starts_with('~')
        || p.contains('\\')
        || path.components().any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err(format!("{p:?}: a path inside the organisation's directory (relative, no ..)"));
    }
    Ok(())
}

/// The DuckDB function reading a table location, from its extension; checks the location.
pub fn file_reader(location: &str) -> Result<&'static str, String> {
    let remote = ["s3://", "gs://", "gcs://", "https://"].iter().any(|s| location.starts_with(s));
    if !remote {
        local_path(location)?;
    }
    if location.contains('\'') || location.chars().any(char::is_control) {
        return Err("invalid characters".into());
    }
    let lower = location.to_lowercase();
    let lower = lower.strip_suffix(".gz").unwrap_or(&lower);
    Ok(if lower.ends_with(".parquet") {
        "read_parquet"
    } else if lower.ends_with(".csv") || lower.ends_with(".tsv") {
        "read_csv_auto"
    } else if lower.ends_with(".json") || lower.ends_with(".ndjson") || lower.ends_with(".jsonl") {
        "read_json_auto"
    } else {
        return Err("files must be .parquet, .csv or .json (globs allowed, e.g. sales/*.parquet)".into());
    })
}

pub fn check_name(name: &str) -> Result<(), String> {
    if NAME_RE.is_match(name) && name.len() <= 63 {
        Ok(())
    } else {
        Err(format!("connection name {name:?}: letters, digits and underscores"))
    }
}

/// What the secret is bound to when sealed: this organisation's connection of this name.
pub fn seal_context(org: &str, name: &str) -> String {
    format!("{org}/connection/{name}")
}

/// A stored connection: settings, and the sealed secret (if any).
#[derive(Debug, Clone)]
pub struct Stored {
    pub name: String,
    pub config: ConnectionConfig,
    pub secret_sealed: Option<String>,
    pub updated_at: String,
    pub updated_by: Option<String>,
}

impl Stored {
    /// For the UI: settings, whether a secret is set; never the secret.
    pub fn public(&self) -> Value {
        let mut v = serde_json::to_value(&self.config).unwrap_or(Value::Null);
        let kind = v.as_object_mut().and_then(|o| o.remove("type")).unwrap_or(Value::Null);
        serde_json::json!({"name": self.name, "type": kind, "config": v, "has_secret": self.secret_sealed.is_some(),
                           "dialect": format!("{:?}", self.config.dialect()).to_lowercase(),
                           "updated_at": self.updated_at, "updated_by": self.updated_by})
    }

    /// Changes whenever the settings or the secret change (connectors are rebuilt then).
    pub fn fingerprint(&self) -> String {
        crate::store::sha256_hex(&format!(
            "{}\n{}",
            serde_json::to_string(&self.config).unwrap_or_default(),
            self.secret_sealed.as_deref().unwrap_or("")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cfg(v: Value) -> Result<ConnectionConfig, String> {
        let c: ConnectionConfig = serde_json::from_value(v).map_err(|e| e.to_string())?;
        c.validate()?;
        Ok(c)
    }

    #[test]
    fn sql_servers() {
        let c = cfg(json!({"type": "postgres", "host": "db.acme.example", "database": "dw", "user": "reader", "schema": "sales"}))
            .unwrap();
        assert_eq!(c.dialect(), Dialect::Postgres);
        assert!(matches!(&c, ConnectionConfig::Postgres(s) if s.tls && s.port == 5432));
        assert!(cfg(json!({"type": "postgres", "host": "db; rm -rf /", "database": "dw", "user": "r"})).is_err());
        assert!(cfg(json!({"type": "postgres", "host": "h", "database": "dw", "user": "r", "schema": "x;y"})).is_err());
        assert!(
            cfg(json!({"type": "postgres", "host": "h", "database": "dw", "user": "r", "sslmode": "off"})).is_err()
        );
    }

    #[test]
    fn bigquery() {
        let c = cfg(json!({"type": "bigquery", "project": "acme-analytics", "dataset": "sales", "location": "EU"}))
            .unwrap();
        assert_eq!(c.dialect(), Dialect::BigQuery);
        assert!(c.needs_secret());
        assert!(cfg(json!({"type": "bigquery", "project": "Not A Project"})).is_err());
    }

    #[test]
    fn duckdb_locations_stay_inside() {
        let ok = json!({"type": "duckdb", "database": "warehouse.duckdb",
                        "tables": {"sales": "sales/*.parquet", "stores": "s3://acme-data/stores.csv", "events": "https://x.example/e.json"}});
        assert_eq!(cfg(ok).unwrap().dialect(), Dialect::DuckDb);
        for bad in [
            "/etc/passwd.csv",
            "../other_org/x.parquet",
            "a/../../x.parquet",
            "~/x.parquet",
            "x.txt",
            "s3://b/x.parquet'); --",
        ] {
            assert!(cfg(json!({"type": "duckdb", "tables": {"t": bad}})).is_err(), "{bad}");
        }
        assert!(cfg(json!({"type": "duckdb", "database": "/var/db.duckdb"})).is_err());
        assert!(cfg(json!({"type": "duckdb"})).is_err());
        assert_eq!(file_reader("sales/2026-*.parquet").unwrap(), "read_parquet");
        assert_eq!(file_reader("x.csv.gz").unwrap(), "read_csv_auto");
    }

    #[test]
    fn public_view_has_no_secret() {
        let s = Stored {
            name: "warehouse".into(),
            config: cfg(json!({"type": "scramdb", "host": "h", "database": "d", "user": "u"})).unwrap(),
            secret_sealed: Some("v1:sealed".into()),
            updated_at: "now".into(),
            updated_by: None,
        };
        let p = s.public();
        assert_eq!(p["type"], "scramdb");
        assert_eq!(p["has_secret"], true);
        assert!(!p.to_string().contains("v1:sealed"));
    }
}
