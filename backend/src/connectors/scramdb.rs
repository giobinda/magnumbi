//! ScramDB data connector (PostgreSQL wire protocol). Queries run in a READ ONLY transaction with
//! a statement timeout. Without a `dsn` it uses Magnum BI's own ScramDB database.

use std::time::Instant;

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use serde_json::{Value, json};
use tokio_postgres::Row;
use tokio_postgres::types::Type;

use super::{Connector, ConnectorError, QueryResult};
use crate::db::{DbPool, make_pool};
use crate::materialize::ColType;
use crate::schema::ScramDbConnection;

pub struct ScramDbConnector {
    pool: DbPool,
    statement_timeout_ms: u64,
}

impl ScramDbConnector {
    pub fn new(cfg: &ScramDbConnection, own_db: &DbPool) -> anyhow::Result<ScramDbConnector> {
        let pool = match &cfg.dsn {
            Some(dsn) => make_pool(dsn, 8)?,
            None => own_db.clone(),
        };
        Ok(ScramDbConnector { pool, statement_timeout_ms: cfg.statement_timeout_ms })
    }
}

#[async_trait]
impl Connector for ScramDbConnector {
    async fn run(&self, sql: &str, _columns: &[String]) -> Result<QueryResult, ConnectorError> {
        let started = Instant::now();
        let mut client =
            self.pool.get().await.map_err(|e| ConnectorError::Unavailable(format!("can't reach ScramDB: {e}")))?;
        let tx = client
            .build_transaction()
            .read_only(true)
            .start()
            .await
            .map_err(|e| ConnectorError::Query(e.to_string()))?;
        tx.batch_execute(&format!("SET LOCAL statement_timeout = {}", self.statement_timeout_ms))
            .await
            .map_err(|e| ConnectorError::Query(e.to_string()))?;
        let rows = tx.query(sql, &[]).await.map_err(|e| ConnectorError::Query(db_error(&e)))?;
        tx.commit().await.map_err(|e| ConnectorError::Query(e.to_string()))?;
        let types = match rows.first() {
            Some(r) => r.columns().iter().map(|c| col_type(c.type_())).collect(),
            None => vec![],
        };
        let rows = rows.iter().map(row_json).collect();
        Ok(QueryResult { rows, types, duration_ms: started.elapsed().as_millis() as u64 })
    }
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
fn decimal(d: Decimal) -> Value {
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
