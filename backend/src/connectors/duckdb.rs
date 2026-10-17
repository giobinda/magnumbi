//! DuckDB, in a sandbox per query.
//!
//! Every query gets a fresh DuckDB: in memory, or the organisation's `.duckdb` file opened
//! read-only. The connection's tables become views over files: in the organisation's own
//! directory (MAGNUM_DUCKDB_DIR/<org>/) or in its object storage (s3://, gs://, https://, with
//! its own keys). Then the sandbox is locked: only those directories and prefixes are
//! reachable (`allowed_directories` with `enable_external_access = false`), extensions can't
//! be loaded, and the configuration can't be changed. A query can't read another
//! organisation's files, the server's files, or anything else on the network.
//!
//! Object storage, Avro and Iceberg need DuckDB extensions (`httpfs`, `avro`, `iceberg`): loaded
//! from MAGNUM_DUCKDB_DIR/_extensions, downloaded there once (DuckDB's signed builds) when
//! MAGNUM_DUCKDB_INSTALL_EXTENSIONS allows. They're loaded before the sandbox is locked.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate};
use duckdb::arrow::datatypes::DataType;
use duckdb::types::{TimeUnit, ValueRef};
use duckdb::{AccessMode, Config, Connection};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Connector, ConnectorError, QueryResult};
use crate::compiler::{ident, quote};
use crate::connections::{DuckDb, Format, TableSource};
use crate::materialize::ColType;

/// S3 or GCS (HMAC) keys for the organisation's object storage.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StorageKeys {
    pub key_id: String,
    pub secret: String,
}

impl StorageKeys {
    pub fn parse(s: &str) -> Result<StorageKeys, String> {
        let k: StorageKeys = serde_json::from_str(s)
            .map_err(|_| "storage keys: {\"key_id\": \"...\", \"secret\": \"...\"}".to_string())?;
        if k.key_id.is_empty() || k.secret.is_empty() {
            return Err("storage keys: key_id and secret are required".into());
        }
        Ok(k)
    }
}

#[derive(Clone)]
pub struct DuckDbConnector {
    cfg: DuckDb,
    /// the organisation's directory: the only local place it can read
    root: PathBuf,
    extensions: PathBuf,
    install_extensions: bool,
    keys: Option<StorageKeys>,
}

impl DuckDbConnector {
    pub fn new(
        cfg: &DuckDb,
        root: PathBuf,
        extensions: PathBuf,
        install_extensions: bool,
        secret: Option<String>,
    ) -> anyhow::Result<DuckDbConnector> {
        let keys = secret.as_deref().map(StorageKeys::parse).transpose().map_err(anyhow::Error::msg)?;
        Ok(DuckDbConnector { cfg: cfg.clone(), root, extensions, install_extensions, keys })
    }

    fn sources(&self) -> Result<Vec<(&String, TableSource<'_>)>, ConnectorError> {
        self.cfg
            .tables
            .iter()
            .map(|(name, l)| TableSource::parse(l).map(|s| (name, s)).map_err(ConnectorError::Query))
            .collect()
    }

    /// Load an extension from MAGNUM_DUCKDB_DIR/_extensions, downloading it first if allowed.
    fn load(&self, conn: &Connection, ext: &str) -> Result<(), ConnectorError> {
        if conn.execute_batch(&format!("LOAD {ext}")).is_ok() {
            return Ok(());
        }
        if !self.install_extensions {
            return Err(ConnectorError::Unavailable(format!(
                "this needs DuckDB's {ext} extension; install it into MAGNUM_DUCKDB_DIR/_extensions \
                 or allow MAGNUM_DUCKDB_INSTALL_EXTENSIONS"
            )));
        }
        conn.execute_batch(&format!("INSTALL {ext}; LOAD {ext}"))
            .map_err(|e| ConnectorError::Unavailable(format!("can't install DuckDB's {ext} extension: {e}")))
    }

    /// A configured, locked sandbox (blocking: call from a blocking thread). `on_open` gets the
    /// connection as soon as it exists: configuring it already reads remote files.
    fn open(&self, on_open: impl FnOnce(&Connection)) -> Result<Connection, ConnectorError> {
        let q = |e: duckdb::Error| ConnectorError::Query(e.to_string());
        std::fs::create_dir_all(&self.root).map_err(|e| ConnectorError::Unavailable(format!("data directory: {e}")))?;
        std::fs::create_dir_all(&self.extensions).ok();
        let root = self.root.canonicalize().map_err(|e| ConnectorError::Unavailable(format!("data directory: {e}")))?;
        let config = Config::default()
            .with("extension_directory", self.extensions.to_string_lossy())
            .and_then(|c| c.with("autoinstall_known_extensions", "false"))
            .and_then(|c| c.with("autoload_known_extensions", "false"))
            .and_then(|c| c.with("threads", "4"))
            .and_then(|c| c.with("memory_limit", "2GB"))
            .map_err(q)?;
        let conn = match &self.cfg.database {
            Some(db) => {
                let path = root.join(db);
                if !path.exists() {
                    return Err(ConnectorError::Unavailable(format!(
                        "database file {db:?} isn't in the organisation's directory"
                    )));
                }
                Connection::open_with_flags(&path, config.access_mode(AccessMode::ReadOnly).map_err(q)?).map_err(q)?
            }
            None => Connection::open_in_memory_with_flags(config).map_err(q)?,
        };
        on_open(&conn);
        let mut allowed: Vec<String> = vec![format!("{}/", root.display())];
        let sources = self.sources()?;
        let remote = sources.iter().any(|(_, s)| s.remote());
        let mut extensions: Vec<&str> = if remote { vec!["httpfs"] } else { vec![] };
        for ext in sources.iter().flat_map(|(_, s)| s.extensions()) {
            if !extensions.contains(ext) {
                extensions.push(ext);
            }
        }
        for ext in extensions {
            self.load(&conn, ext)?;
        }
        if remote {
            let secs = self.cfg.statement_timeout_ms.div_ceil(1000).max(1);
            conn.execute_batch(&format!("SET http_timeout = {secs}")).map_err(q)?;
        }
        if sources.iter().any(|(_, s)| s.format == Format::Iceberg) {
            // a table folder without version-hint.text: read its latest metadata file
            conn.execute_batch("SET unsafe_enable_version_guessing = true").map_err(q)?;
        }
        if remote && let Some(k) = &self.keys {
            let lit = |s: &str| quote(s).map_err(|e| ConnectorError::Query(e.0));
            let mut extra = String::new();
            if let Some(r) = &self.cfg.s3_region {
                extra.push_str(&format!(", REGION {}", lit(r)?));
            }
            if let Some(e) = &self.cfg.s3_endpoint {
                extra.push_str(&format!(", ENDPOINT {}, URL_STYLE 'path'", lit(e)?));
            }
            let (id, secret) = (lit(&k.key_id)?, lit(&k.secret)?);
            let schemes: Vec<&str> =
                sources.iter().filter_map(|(_, s)| s.path.split_once("://").map(|(s, _)| s)).collect();
            if schemes.contains(&"s3") {
                conn.execute_batch(&format!("CREATE SECRET magnum_s3 (TYPE s3, KEY_ID {id}, SECRET {secret}{extra})"))
                    .map_err(q)?;
            }
            if schemes.iter().any(|s| *s == "gs" || *s == "gcs") {
                conn.execute_batch(&format!("CREATE SECRET magnum_gcs (TYPE gcs, KEY_ID {id}, SECRET {secret})"))
                    .map_err(q)?;
            }
        }
        for (name, source) in &sources {
            let path = if source.remote() {
                // only this table's folder (or file) becomes reachable
                allowed.push(source.reachable());
                source.path.to_string()
            } else {
                root.join(source.path).to_string_lossy().into_owned()
            };
            let src = quote(&path).map_err(|e| ConnectorError::Query(e.0))?;
            // Iceberg: data files are found relative to the table's folder, so reads stay inside it
            let options = if source.format == Format::Iceberg { ", allow_moved_paths = true" } else { "" };
            conn.execute_batch(&format!(
                "CREATE TEMP VIEW {} AS SELECT * FROM {}({src}{options})",
                ident(name),
                source.reader()
            ))
            .map_err(q)?;
        }
        let list =
            allowed.iter().map(|a| quote(a).map_err(|e| ConnectorError::Query(e.0))).collect::<Result<Vec<_>, _>>()?;
        conn.execute_batch(&format!(
            "SET allowed_directories = [{}]; SET enable_external_access = false; SET lock_configuration = true",
            list.join(", ")
        ))
        .map_err(q)?;
        Ok(conn)
    }

    fn query(&self, conn: &Connection, sql: &str) -> Result<(Vec<Vec<Value>>, Vec<ColType>), ConnectorError> {
        let q = |e: duckdb::Error| ConnectorError::Query(e.to_string());
        let mut stmt = conn.prepare(sql).map_err(q)?;
        let mut rows = stmt.query([]).map_err(q)?;
        let n = rows.as_ref().map_or(0, |s| s.column_count());
        let types: Vec<(ColType, bool)> =
            (0..n).map(|i| rows.as_ref().map_or((ColType::Text, false), |s| col_type(&s.column_type(i)))).collect();
        let mut out = vec![];
        while let Some(row) = rows.next().map_err(q)? {
            let mut r = Vec::with_capacity(n);
            for (i, (_, tz)) in types.iter().enumerate() {
                r.push(cell(row.get_ref(i).map_err(q)?, *tz));
            }
            out.push(r);
        }
        Ok((out, types.into_iter().map(|(t, _)| t).collect()))
    }
}

#[async_trait]
impl Connector for DuckDbConnector {
    async fn run(&self, sql: &str, _columns: &[String]) -> Result<QueryResult, ConnectorError> {
        let started = Instant::now();
        let this = self.clone();
        let sql = sql.to_string();
        let timeout = Duration::from_millis(self.cfg.statement_timeout_ms);
        let (tx, mut rx) = tokio::sync::oneshot::channel();
        let worker = tokio::task::spawn_blocking(move || {
            let conn = this.open(|c| {
                let _ = tx.send(c.interrupt_handle());
            })?;
            this.query(&conn, &sql)
        });
        let (rows, types) = match tokio::time::timeout(timeout, worker).await {
            Ok(joined) => joined.map_err(|e| ConnectorError::Query(e.to_string()))??,
            Err(_) => {
                if let Ok(h) = rx.try_recv() {
                    h.interrupt();
                }
                return Err(ConnectorError::Query(format!("the query took longer than {} s", timeout.as_secs())));
            }
        };
        Ok(QueryResult { rows, types, duration_ms: started.elapsed().as_millis() as u64 })
    }

    async fn tables(&self) -> Result<Vec<String>, ConnectorError> {
        // the connection's tables (views over its files) and those of its database file
        let r = self
            .run("SELECT table_name FROM information_schema.tables WHERE table_schema = 'main' ORDER BY 1", &[])
            .await?;
        let mut names: std::collections::BTreeSet<String> = self.cfg.tables.keys().cloned().collect();
        names.extend(r.rows.iter().filter_map(|row| row.first().and_then(|v| v.as_str()).map(String::from)));
        Ok(names.into_iter().collect())
    }

    async fn columns(&self, table: &str) -> Result<Vec<(String, ColType)>, ConnectorError> {
        let parts = crate::org_models::table_parts(table)
            .ok_or_else(|| ConnectorError::Query(format!("invalid table name {table:?}")))?;
        let quoted = parts.iter().map(|p| ident(p)).collect::<Vec<_>>().join(".");
        let r = self.run(&format!("DESCRIBE SELECT * FROM {quoted}"), &[]).await?;
        Ok(r.rows
            .iter()
            .filter_map(|row| Some((row.first()?.as_str()?.to_string(), type_named(row.get(1)?.as_str()?))))
            .collect())
    }

    async fn test(&self) -> Result<String, ConnectorError> {
        let r = self.run("SELECT version()", &[]).await?;
        let version = r.rows.first().and_then(|row| row.first()).and_then(|v| v.as_str()).unwrap_or("?").to_string();
        let names: Vec<&String> = self.cfg.tables.keys().collect();
        for n in &names {
            self.run(&format!("SELECT * FROM {} LIMIT 0", ident(n)), &[])
                .await
                .map_err(|e| ConnectorError::Query(format!("table {n}: {e}")))?;
        }
        let tables = if names.is_empty() {
            "from the database file".to_string()
        } else {
            names.iter().map(|n| n.as_str()).collect::<Vec<_>>().join(", ")
        };
        Ok(format!("DuckDB {version}; tables: {tables}"))
    }
}

/// The neutral type of a DuckDB type name, as DESCRIBE writes it.
fn type_named(t: &str) -> ColType {
    let t = t.to_uppercase();
    match t.as_str() {
        "BOOLEAN" => ColType::Bool,
        "DATE" => ColType::Date,
        "FLOAT" | "DOUBLE" | "REAL" => ColType::Float,
        "TIMESTAMP WITH TIME ZONE" | "TIMESTAMPTZ" => ColType::Timestamptz,
        _ if t.starts_with("TIMESTAMP") => ColType::Timestamp,
        _ if t.starts_with("DECIMAL") || t.starts_with("NUMERIC") => ColType::Numeric,
        _ if t.ends_with("INT") || t == "INTEGER" || t == "HUGEINT" || t == "UHUGEINT" => ColType::Int,
        _ => ColType::Text,
    }
}

/// (neutral type, is a timestamp with time zone)
fn col_type(t: &DataType) -> (ColType, bool) {
    match t {
        DataType::Boolean => (ColType::Bool, false),
        DataType::Int8 | DataType::Int16 | DataType::Int32 | DataType::Int64 => (ColType::Int, false),
        DataType::UInt8 | DataType::UInt16 | DataType::UInt32 | DataType::UInt64 => (ColType::Int, false),
        DataType::Float16 | DataType::Float32 | DataType::Float64 => (ColType::Float, false),
        DataType::Decimal128(..) | DataType::Decimal256(..) => (ColType::Numeric, false),
        DataType::Date32 | DataType::Date64 => (ColType::Date, false),
        DataType::Timestamp(_, Some(_)) => (ColType::Timestamptz, true),
        DataType::Timestamp(_, None) => (ColType::Timestamp, false),
        _ => (ColType::Text, false),
    }
}

fn micros(unit: TimeUnit, v: i64) -> i64 {
    match unit {
        TimeUnit::Second => v.saturating_mul(1_000_000),
        TimeUnit::Millisecond => v.saturating_mul(1_000),
        TimeUnit::Microsecond => v,
        TimeUnit::Nanosecond => v / 1_000,
    }
}

fn cell(v: ValueRef<'_>, tz: bool) -> Value {
    match v {
        ValueRef::Null => Value::Null,
        ValueRef::Boolean(b) => json!(b),
        ValueRef::TinyInt(i) => json!(i),
        ValueRef::SmallInt(i) => json!(i),
        ValueRef::Int(i) => json!(i),
        ValueRef::BigInt(i) => json!(i),
        ValueRef::HugeInt(i) => i64::try_from(i).map_or_else(|_| json!(i.to_string()), |i| json!(i)),
        ValueRef::UTinyInt(i) => json!(i),
        ValueRef::USmallInt(i) => json!(i),
        ValueRef::UInt(i) => json!(i),
        ValueRef::UBigInt(i) => json!(i),
        ValueRef::Float(f) => serde_json::Number::from_f64(f64::from(f)).map_or(Value::Null, Value::Number),
        ValueRef::Double(f) => serde_json::Number::from_f64(f).map_or(Value::Null, Value::Number),
        ValueRef::Decimal(d) => {
            d.to_string().parse::<rust_decimal::Decimal>().map_or(Value::Null, super::postgres::decimal)
        }
        ValueRef::Date32(days) => {
            json!(NaiveDate::from_num_days_from_ce_opt(days + 719_163).map(|d| d.to_string()))
        }
        ValueRef::Timestamp(unit, v) => match DateTime::from_timestamp_micros(micros(unit, v)) {
            Some(t) if tz => json!(t.to_rfc3339()),
            Some(t) => json!(t.naive_utc().format("%Y-%m-%dT%H:%M:%S%.f").to_string()),
            None => Value::Null,
        },
        ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
        _ => Value::Null, // blobs, intervals, nested types: not shown in charts
    }
}

/// The organisation's DuckDB directory: MAGNUM_DUCKDB_DIR/<org> ("_platform" for the platform).
pub fn org_dir(base: &Path, org: &str) -> PathBuf {
    base.join(if org.is_empty() { "_platform" } else { org })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connections::ConnectionConfig;

    fn sandbox(tables: serde_json::Value, database: Option<&str>) -> (DuckDbConnector, PathBuf) {
        let base = std::env::temp_dir().join(format!("magnum-duck-{}", uuid::Uuid::new_v4()));
        let root = org_dir(&base, "acme");
        std::fs::create_dir_all(root.join("sales")).unwrap();
        let mut cfg = json!({"type": "duckdb", "tables": tables});
        if let Some(db) = database {
            cfg["database"] = json!(db);
        }
        let ConnectionConfig::Duckdb(c) = serde_json::from_value(cfg).unwrap() else { unreachable!() };
        (DuckDbConnector::new(&c, root.clone(), base.join("_extensions"), false, None).unwrap(), base)
    }

    fn write_parquet(root: &Path) {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(&format!(
            "COPY (SELECT i AS id, CAST(TIMESTAMP '2026-09-01 00:00:00' + INTERVAL (i) HOUR AS TIMESTAMPTZ) AS sold_at,
                     CASE WHEN i % 2 = 0 THEN 'EMEA' ELSE 'AMER' END AS region, CAST(i * 1.5 AS DECIMAL(10,2)) AS amount
                   FROM range(100) t(i)) TO '{}' (FORMAT parquet)",
            root.join("sales/part1.parquet").display()
        ))
        .unwrap();
        std::fs::write(root.join("secret.csv"), "a\n1\n").unwrap();
    }

    #[tokio::test]
    async fn queries_files_in_the_sandbox() {
        let (c, base) = sandbox(json!({"sales": "sales/*.parquet"}), None);
        write_parquet(&org_dir(&base, "acme"));
        let r = c
            .run(
                // what the DuckDB dialect generates for a `time` column (UTC timestamps)
                "SELECT region, DATE_TRUNC('day', CAST(sold_at AS TIMESTAMP)) AS d, SUM(amount) AS revenue, COUNT(*) AS n
                 FROM sales WHERE CAST(sold_at AS TIMESTAMP) >= CAST('2026-09-01 00:00:00' AS TIMESTAMP)
                 GROUP BY 1, 2 ORDER BY 1, 2",
                &[],
            )
            .await
            .unwrap();
        assert_eq!(r.rows.len(), 10, "{:?}", r.rows);
        assert_eq!(r.types, [ColType::Text, ColType::Timestamp, ColType::Numeric, ColType::Int]);
        assert_eq!(r.rows[0][0], json!("AMER"));
        assert_eq!(r.rows[0][1], json!("2026-09-01T00:00:00"));
        let message = c.test().await.unwrap();
        assert!(message.starts_with("DuckDB v") && message.contains("sales"), "{message}");
        std::fs::remove_dir_all(base).ok();
    }

    #[tokio::test]
    async fn lists_tables_and_their_columns() {
        let (c, base) = sandbox(json!({"sales": "sales/*.parquet"}), None);
        write_parquet(&org_dir(&base, "acme"));
        assert_eq!(c.tables().await.unwrap(), ["sales"]);
        let cols = c.columns("sales").await.unwrap();
        assert_eq!(
            cols,
            [
                ("id".into(), ColType::Int),
                ("sold_at".into(), ColType::Timestamptz),
                ("region".into(), ColType::Text),
                ("amount".into(), ColType::Numeric)
            ]
        );
        assert!(c.columns("nope").await.is_err());
        assert!(c.columns("sales; SELECT 1").await.is_err());
        std::fs::remove_dir_all(base).ok();
    }

    #[tokio::test]
    async fn the_sandbox_is_locked() {
        let (c, base) = sandbox(json!({"sales": "sales/*.parquet"}), None);
        write_parquet(&org_dir(&base, "acme"));
        for escape in [
            "SELECT * FROM read_csv_auto('/etc/passwd')".to_string(),
            format!("SELECT * FROM read_csv_auto('{}')", base.join("other_org/x.csv").display()),
            "SET enable_external_access = true".to_string(),
            "SET allowed_directories = ['/']".to_string(),
            "INSTALL httpfs".to_string(),
            "LOAD httpfs".to_string(),
            "ATTACH '/tmp/x.duckdb'".to_string(),
            "COPY (SELECT 1) TO '/tmp/magnum-escape.csv'".to_string(),
        ] {
            let r = c.run(&escape, &[]).await;
            assert!(r.is_err(), "{escape} should be refused, got {:?}", r.map(|r| r.rows));
        }
        // its own directory stays readable
        let ok =
            format!("SELECT COUNT(*) FROM read_csv_auto('{}')", org_dir(&base, "acme").join("secret.csv").display());
        assert!(c.run(&ok, &[]).await.is_ok());
        std::fs::remove_dir_all(base).ok();
    }

    #[tokio::test]
    async fn empty_results_still_have_column_types() {
        let (c, base) = sandbox(json!({}), None);
        let r = c.run("SELECT 'x' AS a, 1.5 AS b WHERE false", &[]).await.unwrap();
        assert!(r.rows.is_empty());
        assert_eq!(r.types, [ColType::Text, ColType::Numeric]);
        std::fs::remove_dir_all(base).ok();
    }

    #[tokio::test]
    async fn missing_extensions_are_explained() {
        let (c, base) = sandbox(json!({"events": "events/*.avro"}), None);
        let e = c.run("SELECT 1", &[]).await.unwrap_err();
        assert!(matches!(&e, ConnectorError::Unavailable(m) if m.contains("avro extension")), "{e:?}");
        std::fs::remove_dir_all(base).ok();
    }

    /// Avro and Iceberg need DuckDB extensions: these tests run when MAGNUM_TEST_DUCKDB_EXTENSIONS
    /// names an extension directory (downloaded into it if needed).
    fn with_extensions(tables: serde_json::Value) -> Option<(DuckDbConnector, PathBuf)> {
        let Ok(ext) = std::env::var("MAGNUM_TEST_DUCKDB_EXTENSIONS") else {
            eprintln!("skipped: MAGNUM_TEST_DUCKDB_EXTENSIONS isn't set");
            return None;
        };
        let (c, base) = sandbox(tables, None);
        Some((DuckDbConnector { extensions: PathBuf::from(ext), install_extensions: true, ..c }, base))
    }

    #[tokio::test]
    async fn the_timeout_covers_opening_remote_files() {
        // a server that accepts connections and never answers
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let _held = tokio::spawn(async move {
            let mut open = vec![];
            while let Ok((socket, _)) = listener.accept().await {
                open.push(socket);
            }
        });
        let Some((c, base)) = with_extensions(json!({"t": format!("https://127.0.0.1:{port}/x.parquet")})) else {
            return;
        };
        let c = DuckDbConnector { cfg: DuckDb { statement_timeout_ms: 1000, ..c.cfg.clone() }, ..c };
        let started = Instant::now();
        let e = c.run("SELECT 1", &[]).await.unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(5), "took {:?}: {e:?}", started.elapsed());
        std::fs::remove_dir_all(base).ok();
    }

    fn copy_dir(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                copy_dir(&entry.path(), &to.join(entry.file_name()));
            } else {
                std::fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
            }
        }
    }

    #[tokio::test]
    async fn reads_avro_and_iceberg() {
        let Some((c, base)) =
            with_extensions(json!({"events": "events/*.avro", "orders": "iceberg:lake/retail/sales"}))
        else {
            return;
        };
        let root = org_dir(&base, "acme");
        std::fs::create_dir_all(root.join("events")).unwrap();
        let writer = Connection::open_in_memory().unwrap();
        writer
            .execute_batch(&format!(
                "SET extension_directory = '{}'; INSTALL avro; LOAD avro;
                 COPY (SELECT i AS id, TIMESTAMP '2026-09-01 00:00:00' + INTERVAL (i) HOUR AS seen_at FROM range(5) t(i))
                 TO '{}' (FORMAT avro)",
                c.extensions.display(),
                root.join("events/part1.avro").display()
            ))
            .unwrap();
        // a table written elsewhere (its metadata names other paths): read relative to its folder
        copy_dir(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/iceberg/sales").as_path(),
            &root.join("lake/retail/sales"),
        );

        let r = c.run("SELECT COUNT(*), MAX(seen_at) FROM events", &[]).await.unwrap();
        assert_eq!(r.rows, vec![vec![json!(5), json!("2026-09-01T04:00:00")]]);
        assert_eq!(r.types, [ColType::Int, ColType::Timestamp]);
        let r = c.run("SELECT region, COUNT(*) FROM orders GROUP BY 1 ORDER BY 1", &[]).await.unwrap();
        assert_eq!(r.rows, vec![vec![json!("AMER"), json!(6)], vec![json!("EMEA"), json!(6)]]);
        assert!(c.test().await.unwrap().contains("events, orders"));

        // other Iceberg tables stay out of reach
        let other = base.join("other_org/lake/sales");
        copy_dir(&root.join("lake/retail/sales"), &other);
        let escape = format!("SELECT COUNT(*) FROM iceberg_scan('{}', allow_moved_paths = true)", other.display());
        assert!(c.run(&escape, &[]).await.is_err(), "{escape} should be refused");
        std::fs::remove_dir_all(base).ok();
    }

    #[tokio::test]
    async fn database_files_are_read_only() {
        let (_, base) = sandbox(json!({}), None);
        let root = org_dir(&base, "acme");
        Connection::open(root.join("wh.duckdb"))
            .unwrap()
            .execute_batch("CREATE TABLE stores AS SELECT 'Store 01' AS store, 3 AS staff")
            .unwrap();
        let cfg: DuckDb = serde_json::from_value(json!({"database": "wh.duckdb"})).unwrap();
        let c = DuckDbConnector::new(&cfg, root.clone(), base.join("_extensions"), false, None).unwrap();
        let r = c.run("SELECT store, staff FROM stores", &[]).await.unwrap();
        assert_eq!(r.rows, vec![vec![json!("Store 01"), json!(3)]]);
        assert!(c.run("DELETE FROM stores", &[]).await.is_err());
        std::fs::remove_dir_all(base).ok();
    }
}
