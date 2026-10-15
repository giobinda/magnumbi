//! The query cache, in each organisation's own ScramDB.
//!
//! A model opts in with `cache: 15m` (any duration up to 30 days; off by default, so live data
//! stays live). A cached result is keyed by SHA-256 of organisation + connection + compiled SQL.
//! The compiled SQL already contains the person's data restrictions and the (5-minute rounded)
//! period window, so two people share a cached result only if they'd get the same rows anyway.
//!
//! Each result is a typed table `magnum_cache.r_<key>_<unix time>_<random>` (materialize.rs), indexed by
//! `magnum_cache.entries`. The index row is written last, so a reader never sees a half-written
//! result. The cache is best-effort: any failure is logged and the query runs uncached.
//!
//! ScramDB infers some parameter types as text (e.g. in INSERTs into schema-qualified tables),
//! so non-text parameters carry explicit casts.

use std::collections::BTreeMap;

use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};

use crate::materialize::{self, ColType};
use crate::store::{Store, random_token, sha256_hex};

const SCHEMA: &str = "magnum_cache";
/// cached results kept per organisation instance (least recently used go first)
const MAX_ENTRIES: i64 = 2_000;
/// results larger than this are not cached
const MAX_ROWS: usize = 50_000;

pub fn key(org: &str, connection: &str, sql: &str) -> String {
    sha256_hex(&format!("{org}\n{connection}\n{sql}"))
}

pub struct Hit {
    pub rows: Vec<Vec<Value>>,
    pub created_at: DateTime<Utc>,
}

pub async fn get(store: &Store, key: &str) -> Result<Option<Hit>> {
    let entry = {
        let c = store.pool.get().await?;
        c.query_opt(
            "SELECT table_name, columns, created_at FROM magnum_cache.entries WHERE key = $1 AND expires_at > now()",
            &[&key],
        )
        .await?
    };
    let Some(e) = entry else { return Ok(None) };
    let (table, columns, created_at): (String, String, DateTime<Utc>) = (e.get(0), e.get(1), e.get(2));
    let n = serde_json::from_str::<Vec<Value>>(&columns)?.len();
    let rows = materialize::read(&store.pool, SCHEMA, &table, n).await?;
    let pool = store.pool.clone();
    let key = key.to_string();
    tokio::spawn(async move {
        if let Ok(c) = pool.get().await {
            let _ = c
                .execute("UPDATE magnum_cache.entries SET hits = hits + 1, last_hit_at = now() WHERE key = $1", &[&key])
                .await;
        }
    });
    Ok(Some(Hit { rows, created_at }))
}

pub struct Put<'a> {
    pub key: &'a str,
    pub org: &'a str,
    pub model: &'a str,
    pub connection: &'a str,
    pub ttl_seconds: u64,
    pub names: &'a [String],
    pub types: &'a [ColType],
    pub rows: &'a [Vec<Value>],
}

pub async fn put(store: &Store, p: Put<'_>) -> Result<()> {
    if p.rows.len() > MAX_ROWS {
        return Ok(());
    }
    // an empty result says nothing about types: text columns are fine for zero rows
    let types: Vec<ColType> =
        if p.types.len() == p.names.len() { p.types.to_vec() } else { vec![ColType::Text; p.names.len()] };
    let table = table_name(p.key, Utc::now());
    materialize::write(&store.pool, SCHEMA, &table, &types, p.rows).await?;
    let columns: Vec<Value> = p.names.iter().zip(&types).map(|(n, t)| json!({"name": n, "type": t})).collect();
    let expires = Utc::now() + Duration::seconds(p.ttl_seconds as i64);
    let c = store.pool.get().await?;
    let old: Option<String> =
        c.query_opt("SELECT table_name FROM magnum_cache.entries WHERE key = $1", &[&p.key]).await?.map(|r| r.get(0));
    let indexed = c.execute(
        "INSERT INTO magnum_cache.entries (key, org, model, connection, table_name, columns, row_count, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7::BIGINT, $8::TIMESTAMPTZ)
         ON CONFLICT (key) DO UPDATE SET table_name = EXCLUDED.table_name, columns = EXCLUDED.columns,
           row_count = EXCLUDED.row_count, created_at = now(), expires_at = EXCLUDED.expires_at, hits = 0, last_hit_at = NULL",
        &[&p.key, &p.org, &p.model, &p.connection, &table, &Value::Array(columns).to_string(), &(p.rows.len() as i64), &expires],
    )
    .await;
    if let Err(e) = indexed {
        let _ = materialize::drop(&store.pool, SCHEMA, &table).await; // nothing points to it
        return Err(e.into());
    }
    if let Some(old) = old.filter(|o| *o != table) {
        materialize::drop(&store.pool, SCHEMA, &old).await?;
    }
    if rand::random::<u8>() < 16 {
        sweep(store).await?;
    }
    Ok(())
}

/// Drop expired results, results beyond MAX_ENTRIES (least recently used first), and tables
/// no entry points to (a replica that stopped halfway).
pub async fn sweep(store: &Store) -> Result<usize> {
    let c = store.pool.get().await?;
    let gone = c
        .query(
            &format!(
                "DELETE FROM magnum_cache.entries WHERE expires_at <= now() OR key IN (
                   SELECT key FROM magnum_cache.entries ORDER BY COALESCE(last_hit_at, created_at) DESC OFFSET {MAX_ENTRIES})
                 RETURNING table_name"
            ),
            &[],
        )
        .await?;
    let mut dropped = 0;
    for r in &gone {
        materialize::drop(&store.pool, SCHEMA, &r.get::<_, String>(0)).await?;
        dropped += 1;
    }
    let orphans = c
        .query(
            "SELECT table_name FROM information_schema.tables WHERE table_schema = 'magnum_cache' AND table_name LIKE 'r\\_%'
               AND table_name NOT IN (SELECT table_name FROM magnum_cache.entries)",
            &[],
        )
        .await?;
    let cutoff = (Utc::now() - Duration::minutes(10)).timestamp();
    for r in &orphans {
        let name: String = r.get(0);
        // tables of the last minutes may be filling right now: leave them for a later sweep
        if created_at(&name).is_some_and(|t| t < cutoff) {
            materialize::drop(&store.pool, SCHEMA, &name).await?;
            dropped += 1;
        }
    }
    Ok(dropped)
}

/// A fresh table per result (two replicas filling the same key never collide), named with its
/// creation time so a sweep can tell an abandoned table from one being filled.
fn table_name(key: &str, now: DateTime<Utc>) -> String {
    let suffix: String = random_token(6).to_lowercase().chars().filter(char::is_ascii_alphanumeric).take(4).collect();
    format!("r_{}_{}_{suffix}", &key[..16], now.timestamp())
}

fn created_at(table: &str) -> Option<i64> {
    table.split('_').nth(2)?.parse().ok()
}

/// Drop an organisation's cached results (all, or one model's). Returns how many.
pub async fn clear(store: &Store, org: &str, model: Option<&str>) -> Result<usize> {
    let c = store.pool.get().await?;
    let rows = match model {
        Some(m) => {
            c.query("DELETE FROM magnum_cache.entries WHERE org = $1 AND model = $2 RETURNING table_name", &[&org, &m])
                .await?
        }
        None => c.query("DELETE FROM magnum_cache.entries WHERE org = $1 RETURNING table_name", &[&org]).await?,
    };
    for r in &rows {
        materialize::drop(&store.pool, SCHEMA, &r.get::<_, String>(0)).await?;
    }
    sweep(store).await?; // expired results and abandoned tables too
    Ok(rows.len())
}

/// Per model: cached results, rows, hits (live entries only).
pub async fn stats(store: &Store, org: &str) -> Result<BTreeMap<String, Value>> {
    let c = store.pool.get().await?;
    let rows = c
        .query(
            "SELECT model, COUNT(*), CAST(SUM(row_count) AS BIGINT), CAST(SUM(hits) AS BIGINT) FROM magnum_cache.entries
             WHERE org = $1 AND expires_at > now() GROUP BY model",
            &[&org],
        )
        .await?;
    Ok(rows
        .iter()
        .map(|r| {
            let (n, rows, hits): (i64, Option<i64>, Option<i64>) = (r.get(1), r.get(2), r.get(3));
            (r.get::<_, String>(0), json!({"results": n, "rows": rows, "hits": hits}))
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_separate_organisations_and_sql() {
        let a = key("acme", "scramdb", "SELECT 1");
        assert_eq!(a, key("acme", "scramdb", "SELECT 1"));
        assert_ne!(a, key("globex", "scramdb", "SELECT 1"));
        assert_ne!(a, key("acme", "scramdb", "SELECT 2"));
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn table_names_carry_their_age() {
        let now = Utc::now();
        let t = table_name(&key("acme", "c", "SELECT 1"), now);
        assert!(t.starts_with("r_") && t.len() <= 63, "{t}");
        assert!(t.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'), "{t}");
        assert_eq!(created_at(&t), Some(now.timestamp()));
    }
}
