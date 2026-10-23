//! Materialising query results as typed tables in an organisation's ScramDB.
//!
//! The query cache (cache.rs) stores every cached result this way, as a real table rather than a
//! JSON blob. That is deliberate: the same tables are the bridge for combining sources. A
//! federated query will run each source (BigQuery, Postgres, ...) through its connector,
//! materialise each result here, and join them with ordinary SQL on ScramDB.
//!
//! Tables get neutral column names (c0, c1, ...) plus `_row` for the original order, so any
//! source's column names are safe; the caller keeps the mapping. Values are written as escaped
//! literals (compiler::quote), typed by `ColType`.
//!
//! ScramDB note: `CREATE TABLE schema.t AS SELECT ...` ignores the schema, so tables are
//! always created with explicit columns and then filled with INSERT.

use std::sync::LazyLock;

use anyhow::{Result, bail};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::compiler::quote;
use crate::db::DbPool;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColType {
    Bool,
    Int,
    Float,
    Numeric,
    Text,
    Date,
    Timestamp,
    Timestamptz,
    Json,
}

impl ColType {
    pub fn sql(self) -> &'static str {
        match self {
            ColType::Bool => "BOOLEAN",
            ColType::Int => "BIGINT",
            ColType::Float => "DOUBLE PRECISION",
            // a declared scale: ScramDB 1.0 loses an unconstrained NUMERIC's scale in SUM after
            // a join (see decimals_survive_the_round_trip)
            ColType::Numeric => "NUMERIC(38, 10)",
            ColType::Text => "TEXT",
            ColType::Date => "DATE",
            ColType::Timestamp => "TIMESTAMP",
            ColType::Timestamptz => "TIMESTAMPTZ",
            ColType::Json => "JSONB",
        }
    }
}

static NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^-?\d+(\.\d+)?([eE][-+]?\d+)?$").unwrap());
static IDENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-z_][a-z0-9_]{0,62}$").unwrap());

fn literal(v: &Value, t: ColType) -> Result<String> {
    if v.is_null() {
        return Ok("NULL".into());
    }
    let text = |v: &Value| match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    Ok(match t {
        ColType::Bool => match v {
            Value::Bool(b) => if *b { "TRUE" } else { "FALSE" }.into(),
            _ => bail!("expected a boolean, got {v}"),
        },
        ColType::Int | ColType::Float | ColType::Numeric => {
            let n = text(v);
            if !NUMBER.is_match(&n) {
                bail!("expected a number, got {v}");
            }
            n
        }
        ColType::Json => format!("CAST({} AS JSONB)", quote(&v.to_string()).map_err(|e| anyhow::anyhow!(e.0))?),
        ColType::Text => quote(&text(v)).map_err(|e| anyhow::anyhow!(e.0))?,
        ColType::Date | ColType::Timestamp | ColType::Timestamptz => {
            format!("CAST({} AS {})", quote(&text(v)).map_err(|e| anyhow::anyhow!(e.0))?, t.sql())
        }
    })
}

/// `schema.table` checked to be plain lowercase identifiers (they're spliced into SQL).
fn qualified(schema: &str, table: &str) -> Result<String> {
    if !IDENT.is_match(schema) || !IDENT.is_match(table) {
        bail!("invalid table name {schema}.{table}");
    }
    Ok(format!("{schema}.{table}"))
}

/// Create `schema.table` with the given column types and fill it with `rows` (in order).
pub async fn write(pool: &DbPool, schema: &str, table: &str, types: &[ColType], rows: &[Vec<Value>]) -> Result<()> {
    let name = qualified(schema, table)?;
    let cols: Vec<String> = types.iter().enumerate().map(|(i, t)| format!("c{i} {}", t.sql())).collect();
    let c = pool.get().await?;
    let mut ddl = format!("CREATE TABLE {name} (_row BIGINT NOT NULL");
    for col in &cols {
        ddl.push_str(", ");
        ddl.push_str(col);
    }
    ddl.push(')');
    c.batch_execute(&ddl).await?;
    let filled = async {
        for (n, chunk) in rows.chunks(500).enumerate() {
            let mut values = Vec::with_capacity(chunk.len());
            for (i, row) in chunk.iter().enumerate() {
                if row.len() != types.len() {
                    bail!("row has {} values for {} columns", row.len(), types.len());
                }
                let mut parts = vec![(n * 500 + i).to_string()];
                for (v, t) in row.iter().zip(types) {
                    parts.push(literal(v, *t)?);
                }
                values.push(format!("({})", parts.join(", ")));
            }
            c.batch_execute(&format!("INSERT INTO {name} VALUES {}", values.join(", "))).await?;
        }
        anyhow::Ok(())
    }
    .await;
    if filled.is_err() {
        let _ = c.batch_execute(&format!("DROP TABLE IF EXISTS {name}")).await;
    }
    filled
}

/// Read a materialised table back, in its original order, as JSON rows.
pub async fn read(pool: &DbPool, schema: &str, table: &str, columns: usize) -> Result<Vec<Vec<Value>>> {
    let name = qualified(schema, table)?;
    let cols: Vec<String> = (0..columns).map(|i| format!("c{i}")).collect();
    let select = if cols.is_empty() { "_row".to_string() } else { cols.join(", ") };
    let c = pool.get().await?;
    let rows = c.query(&format!("SELECT {select} FROM {name} ORDER BY _row"), &[]).await?;
    Ok(rows.iter().map(|r| if columns == 0 { vec![] } else { crate::connectors::postgres::row_json(r) }).collect())
}

/// Work tables (a blend's parts) live in the default schema: ScramDB 1.0 only COPYs into it.
/// Their names carry their creation time, so a sweep can drop any left behind.
pub const WORK_PREFIX: &str = "magnum_work_";

pub fn work_table_name() -> String {
    format!("{WORK_PREFIX}{}_{}", chrono::Utc::now().timestamp(), &uuid::Uuid::new_v4().simple().to_string()[..8])
}

/// When a work table was created (unix time), from its name.
pub fn work_table_created(table: &str) -> Option<i64> {
    table.strip_prefix(WORK_PREFIX)?.split('_').next()?.parse().ok()
}

/// A value in COPY's text format.
fn copy_field(v: &Value, t: ColType) -> Result<String> {
    if v.is_null() {
        return Ok("\\N".into());
    }
    let text = match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    if text.contains('\0') {
        bail!("text values can't contain NUL characters");
    }
    Ok(match t {
        ColType::Bool => match v {
            Value::Bool(b) => if *b { "t" } else { "f" }.into(),
            _ => bail!("expected a boolean, got {v}"),
        },
        ColType::Int | ColType::Float | ColType::Numeric => {
            if !NUMBER.is_match(&text) {
                bail!("expected a number, got {v}");
            }
            text
        }
        _ => text.replace('\\', "\\\\").replace('\t', "\\t").replace('\n', "\\n").replace('\r', "\\r"),
    })
}

/// Create `table` in the default schema and fill it with COPY: many times faster than
/// INSERTs (60,000 rows: 0.1 s against 2.7 s on ScramDB 1.0).
pub async fn copy(pool: &DbPool, table: &str, types: &[ColType], rows: &[Vec<Value>]) -> Result<()> {
    use futures_util::SinkExt;
    if !IDENT.is_match(table) {
        bail!("invalid table name {table}");
    }
    let mut text = String::new();
    for (i, row) in rows.iter().enumerate() {
        if row.len() != types.len() {
            bail!("row has {} values for {} columns", row.len(), types.len());
        }
        text.push_str(&i.to_string());
        for (v, t) in row.iter().zip(types) {
            text.push('\t');
            text.push_str(&copy_field(v, *t)?);
        }
        text.push('\n');
    }
    let cols: Vec<String> = types.iter().enumerate().map(|(i, t)| format!(", c{i} {}", t.sql())).collect();
    let c = pool.get().await?;
    c.batch_execute(&format!("CREATE TABLE {table} (_row BIGINT NOT NULL{})", cols.concat())).await?;
    let filled = async {
        let sink = c.copy_in(&format!("COPY {table} FROM STDIN")).await?;
        let mut sink = Box::pin(sink);
        sink.send(bytes::Bytes::from(text)).await?;
        sink.as_mut().finish().await?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    if let Err(e) = filled {
        let _ = c.batch_execute(&format!("DROP TABLE IF EXISTS {table}")).await;
        return Err(e);
    }
    Ok(())
}

pub async fn drop(pool: &DbPool, schema: &str, table: &str) -> Result<()> {
    let name = qualified(schema, table)?;
    pool.get().await?.batch_execute(&format!("DROP TABLE IF EXISTS {name}")).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn literals_are_typed_and_escaped() {
        assert_eq!(literal(&json!("it's"), ColType::Text).unwrap(), "'it''s'");
        assert_eq!(literal(&json!(12.5), ColType::Numeric).unwrap(), "12.5");
        assert!(literal(&json!("1; DROP TABLE x"), ColType::Numeric).is_err());
        assert_eq!(literal(&Value::Null, ColType::Date).unwrap(), "NULL");
        assert_eq!(
            literal(&json!("2026-10-01T00:00:00+00:00"), ColType::Timestamptz).unwrap(),
            "CAST('2026-10-01T00:00:00+00:00' AS TIMESTAMPTZ)"
        );
        assert_eq!(literal(&json!({"a": 1}), ColType::Json).unwrap(), r#"CAST('{"a":1}' AS JSONB)"#);
        assert!(literal(&json!("yes"), ColType::Bool).is_err());
    }

    #[tokio::test]
    async fn decimals_survive_the_round_trip() {
        let Some(pool) = crate::db::test_pool(2).await else { return };
        let (a, b) = (format!("t_{}", uuid::Uuid::new_v4().simple()), format!("t_{}", uuid::Uuid::new_v4().simple()));
        let rows = vec![
            vec![json!("AMER"), json!(2528809.61)],
            vec![json!("AMER"), json!(0.5)],
            vec![json!("APAC"), json!(-3)],
        ];
        write(&pool, "magnum_cache", &a, &[ColType::Text, ColType::Numeric], &rows).await.unwrap();
        write(&pool, "magnum_cache", &b, &[ColType::Text], &[vec![json!("AMER")], vec![json!("APAC")]]).await.unwrap();
        let back = read(&pool, "magnum_cache", &a, 2).await;
        // the shape of a blend's join. ScramDB 1.0 loses an unconstrained NUMERIC's scale in
        // SUM after a join (2528809.61 + 0.5 -> 252880966): columns get a declared scale
        let c = pool.get().await.unwrap();
        let sums = c
            .query(
                &format!(
                    "SELECT s0.c0, SUM(s0.c1) FROM magnum_cache.{a} s0 INNER JOIN magnum_cache.{b} s1 ON s1.c0 = s0.c0 \
                     GROUP BY 1 ORDER BY 1"
                ),
                &[],
            )
            .await
            .unwrap();
        drop(&pool, "magnum_cache", &a).await.unwrap();
        drop(&pool, "magnum_cache", &b).await.unwrap();
        assert_eq!(back.unwrap(), rows);
        let sums: Vec<Vec<Value>> = sums.iter().map(crate::connectors::postgres::row_json).collect();
        assert_eq!(sums, vec![vec![json!("AMER"), json!(2528810.11)], vec![json!("APAC"), json!(-3)]]);
    }

    #[tokio::test]
    async fn copy_writes_any_text_safely() {
        let Ok(url) = std::env::var("MAGNUM_TEST_DATABASE_URL") else { return };
        let pool = crate::db::make_pool(&url, 2).unwrap();
        let table = work_table_name();
        let types = [ColType::Text, ColType::Numeric, ColType::Timestamp, ColType::Bool, ColType::Int];
        let rows = vec![
            vec![
                json!("tab\there, new\nline, back\\slash \\N, it's \"quoted\" ✓"),
                json!(2528809.61),
                json!("2026-07-01T05:00:00"),
                json!(true),
                json!(-7),
            ],
            vec![Value::Null, Value::Null, Value::Null, Value::Null, Value::Null],
            vec![json!("\\."), json!(0), json!("2026-07-01T06:00:00"), json!(false), json!(0)],
        ];
        copy(&pool, &table, &types, &rows).await.unwrap();
        let back = read(&pool, "public", &table, types.len()).await;
        drop(&pool, "public", &table).await.unwrap();
        assert_eq!(back.unwrap(), rows);
        assert!(copy(&pool, "x; DROP TABLE users", &types, &rows).await.is_err());
        assert!(
            copy(&pool, &work_table_name(), &[ColType::Int], &[vec![json!("1\t2")]]).await.is_err(),
            "checked values"
        );
    }

    #[test]
    fn work_tables_carry_their_age() {
        let t = work_table_name();
        assert!(IDENT.is_match(&t) && t.len() <= 63, "{t}");
        assert!(work_table_created(&t).is_some_and(|ts| (chrono::Utc::now().timestamp() - ts).abs() < 5));
        assert_eq!(work_table_created("dashboards"), None);
    }

    #[test]
    fn table_names_are_plain_identifiers() {
        assert!(qualified("magnum_cache", "r_0123abcd").is_ok());
        assert!(qualified("magnum_cache", "r; drop table users").is_err());
        assert!(qualified("Public", "x").is_err());
    }
}
