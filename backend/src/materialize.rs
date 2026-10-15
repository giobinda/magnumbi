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
            ColType::Numeric => "NUMERIC",
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
    Ok(rows.iter().map(|r| if columns == 0 { vec![] } else { crate::connectors::scramdb::row_json(r) }).collect())
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

    #[test]
    fn table_names_are_plain_identifiers() {
        assert!(qualified("magnum_cache", "r_0123abcd").is_ok());
        assert!(qualified("magnum_cache", "r; drop table users").is_err());
        assert!(qualified("Public", "x").is_err());
    }
}
