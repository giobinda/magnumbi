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
    /// logical table -> files or an Iceberg table (see `TableSource`): a path in the organisation's
    /// directory, or s3:// gs:// https://, e.g. {"sales": "s3://acme-data/sales/*.parquet",
    /// "orders": "iceberg:s3://acme-lake/retail/orders"}
    #[serde(default)]
    pub tables: BTreeMap<String, String>,
    #[serde(default)]
    pub s3_region: Option<String>,
    /// S3-compatible endpoint (MinIO, R2, ...), host[:port]
    #[serde(default)]
    pub s3_endpoint: Option<String>,
    #[serde(default = "minute")]
    pub statement_timeout_ms: u64,
    /// an uploaded file's name (api/files.rs): its tables are its sheets, kept in files/<connection>/
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// a Google Sheet shared with anyone with the link, by its id: its tabs, refreshed hourly
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub google_sheet: Option<String>,
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

    /// Whether a secret given for `self` may go with `other` too: to the same host (as the same
    /// user; another port on it is fine) or storage endpoint. A stored secret never follows a
    /// changed address on its own.
    pub fn same_destination(&self, other: &ConnectionConfig) -> bool {
        use ConnectionConfig::*;
        match (self, other) {
            (Scramdb(a), Scramdb(b)) | (Postgres(a), Postgres(b)) => {
                a.host.eq_ignore_ascii_case(&b.host) && a.user == b.user
            }
            // a service account key only ever goes to Google
            (Bigquery(_), Bigquery(_)) => true,
            (Duckdb(a), Duckdb(b)) => a.s3_endpoint == b.s3_endpoint,
            _ => false,
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
                    TableSource::parse(location).map_err(|e| format!("table {name}: {e}"))?;
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

/// Where a DuckDB table's data is: files (Parquet, CSV, JSON, Avro; globs allowed) or an Iceberg
/// table's folder (`iceberg:<folder>`), in the organisation's directory or its object storage.
#[derive(Debug, Clone, Copy)]
pub struct TableSource<'a> {
    pub format: Format,
    /// without the `iceberg:` prefix
    pub path: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Format {
    Parquet,
    Csv,
    Json,
    Avro,
    Iceberg,
}

impl<'a> TableSource<'a> {
    pub fn parse(location: &'a str) -> Result<TableSource<'a>, String> {
        let (iceberg, path) = match location.strip_prefix("iceberg:") {
            Some(p) => (true, p),
            None => (false, location),
        };
        if path.contains('\'') || path.chars().any(char::is_control) {
            return Err("invalid characters".into());
        }
        let source = |format| TableSource { format, path };
        if !source(Format::Parquet).remote() {
            local_path(path.trim_end_matches('/'))?;
        }
        if iceberg {
            return Ok(source(Format::Iceberg));
        }
        let lower = path.to_lowercase();
        let lower = lower.strip_suffix(".gz").unwrap_or(&lower);
        let format = if lower.ends_with(".parquet") {
            Format::Parquet
        } else if lower.ends_with(".csv") || lower.ends_with(".tsv") {
            Format::Csv
        } else if lower.ends_with(".json") || lower.ends_with(".ndjson") || lower.ends_with(".jsonl") {
            Format::Json
        } else if lower.ends_with(".avro") {
            Format::Avro
        } else {
            return Err("files must be .parquet, .csv, .json or .avro (globs allowed, e.g. sales/*.parquet), \
                        or an Iceberg table's folder as iceberg:<folder>"
                .into());
        };
        Ok(source(format))
    }

    pub fn remote(&self) -> bool {
        ["s3://", "gs://", "gcs://", "https://"].iter().any(|s| self.path.starts_with(s))
    }

    /// The DuckDB function reading it.
    pub fn reader(&self) -> &'static str {
        match self.format {
            Format::Parquet => "read_parquet",
            Format::Csv => "read_csv_auto",
            Format::Json => "read_json_auto",
            Format::Avro => "read_avro",
            Format::Iceberg => "iceberg_scan",
        }
    }

    /// DuckDB extensions it needs, besides httpfs for object storage (Iceberg manifests are Avro).
    pub fn extensions(&self) -> &'static [&'static str] {
        match self.format {
            Format::Avro => &["avro"],
            Format::Iceberg => &["avro", "iceberg"],
            _ => &[],
        }
    }

    /// The prefix its reads stay in: an Iceberg table's folder, or the folder of the files.
    pub fn reachable(&self) -> String {
        if self.format == Format::Iceberg {
            return format!("{}/", self.path.trim_end_matches('/'));
        }
        match self.path.rfind('/') {
            Some(i) if !self.path[..i].ends_with(':') && !self.path[..i].ends_with(":/") => self.path[..=i].to_string(),
            _ => self.path.to_string(),
        }
    }
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

    #[test]
    fn secrets_stay_with_their_server() {
        let pg = |host: &str, port: u16, user: &str| -> ConnectionConfig {
            serde_json::from_value(
                serde_json::json!({"type": "postgres", "host": host, "port": port, "database": "d", "user": user}),
            )
            .unwrap()
        };
        assert!(pg("db.acme.example", 5432, "bi").same_destination(&pg("DB.acme.example", 5432, "bi")));
        assert!(!pg("db.acme.example", 5432, "bi").same_destination(&pg("evil.example", 5432, "bi")));
        assert!(pg("db.acme.example", 5432, "bi").same_destination(&pg("db.acme.example", 6543, "bi")), "another port");
        assert!(!pg("db.acme.example", 5432, "bi").same_destination(&pg("db.acme.example", 5432, "admin")));
        let scram: ConnectionConfig = serde_json::from_value(
            serde_json::json!({"type": "scramdb", "host": "db.acme.example", "database": "d", "user": "bi"}),
        )
        .unwrap();
        assert!(!pg("db.acme.example", 5432, "bi").same_destination(&scram), "another kind");
        let duck = |endpoint: Option<&str>| -> ConnectionConfig {
            serde_json::from_value(serde_json::json!({"type": "duckdb", "s3_endpoint": endpoint})).unwrap()
        };
        assert!(duck(Some("minio.acme.example")).same_destination(&duck(Some("minio.acme.example"))));
        assert!(!duck(Some("minio.acme.example")).same_destination(&duck(Some("evil.example"))));
    }
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
                        "tables": {"sales": "sales/*.parquet", "stores": "s3://acme-data/stores.csv", "events": "https://x.example/e.json",
                                   "clicks": "gs://acme-data/clicks/*.avro", "orders": "iceberg:s3://acme-lake/retail/orders"}});
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
        assert!(cfg(json!({"type": "duckdb", "tables": {"t": "iceberg:../lake/sales"}})).is_err());
    }

    #[test]
    fn table_sources() {
        let reader = |l: &str| TableSource::parse(l).map(|s| s.reader());
        assert_eq!(reader("sales/2026-*.parquet").unwrap(), "read_parquet");
        assert_eq!(reader("x.csv.gz").unwrap(), "read_csv_auto");
        assert_eq!(reader("s3://acme/events/*.avro").unwrap(), "read_avro");
        assert_eq!(reader("iceberg:gs://acme-lake/retail/sales").unwrap(), "iceberg_scan");
        assert_eq!(reader("iceberg:lake/retail/sales").unwrap(), "iceberg_scan");
        assert!(reader("iceberg:").is_err());
        assert!(reader("x.txt").unwrap_err().contains(".avro"));

        let s = TableSource::parse("iceberg:s3://acme-lake/retail/sales/").unwrap();
        assert_eq!((s.path, s.remote()), ("s3://acme-lake/retail/sales/", true));
        assert_eq!(s.extensions(), ["avro", "iceberg"]);
        // the files a table may read: an Iceberg table's whole folder, else the files' folder
        assert_eq!(s.reachable(), "s3://acme-lake/retail/sales/");
        assert_eq!(TableSource::parse("iceberg:s3://b/t").unwrap().reachable(), "s3://b/t/");
        assert_eq!(TableSource::parse("s3://b/events/*.avro").unwrap().reachable(), "s3://b/events/");
        assert_eq!(TableSource::parse("https://x.example/e.json").unwrap().reachable(), "https://x.example/");
        assert!(TableSource::parse("events/*.avro").unwrap().extensions() == ["avro"]);
        assert!(TableSource::parse("sales.parquet").unwrap().extensions().is_empty());
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
