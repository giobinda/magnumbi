//! The query cache, in each organisation's own ScramDB.
//!
//! A model opts in with `cache: 15m` (any duration up to 30 days; off by default, so live data
//! stays live). A cached result is keyed by SHA-256 of organisation + connection + compiled SQL.
//! The compiled SQL already contains the person's data restrictions and the (5-minute rounded)
//! period window, so two people share a cached result only if they'd get the same rows anyway.
//!
//! Each result is a typed table `magnum_cache.r_<key>_<unix time>_<random>` (materialize.rs), indexed by
//! `magnum_cache.entries`. The index row is written last, so a reader never sees a half-written
//! result. Every 5 minutes the server drops expired results, then keeps each organisation within
//! MAGNUM_CACHE_MAX_ROWS rows (least recently used go first). The cache is best-effort: any
//! failure is logged and the query runs uncached.
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
    drop(c); // dropping a table takes a connection of its own
    if let Err(e) = indexed {
        let _ = materialize::drop(&store.pool, SCHEMA, &table).await; // nothing points to it
        return Err(e.into());
    }
    if let Some(old) = old.filter(|o| *o != table) {
        gone_now(store, SCHEMA, &old).await; // the result is saved: a table left behind, a sweep drops
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
    let orphans = c
        .query(
            "SELECT table_name FROM information_schema.tables WHERE table_schema = 'magnum_cache' AND table_name LIKE 'r\\_%'
               AND table_name NOT IN (SELECT table_name FROM magnum_cache.entries)",
            &[],
        )
        .await?;
    let cutoff = (Utc::now() - Duration::minutes(10)).timestamp();
    // blends' work tables (materialize::copy), normally dropped right after the join
    let work = c
        .query(
            "SELECT table_name FROM information_schema.tables WHERE table_schema = 'public' AND table_name LIKE 'magnum\\_work\\_%'",
            &[],
        )
        .await?;
    drop(c); // dropping a table takes a connection of its own
    let mut tables: Vec<(&str, String)> = gone.iter().map(|r| (SCHEMA, r.get(0))).collect();
    let old = |name: &String, at: Option<i64>| at.is_some_and(|t| t < cutoff).then(|| name.clone());
    tables.extend(
        work.iter()
            .filter_map(|r| old(&r.get(0), materialize::work_table_created(&r.get::<_, String>(0))))
            .map(|n| ("public", n)),
    );
    // tables of the last minutes may be filling right now: leave them for a later sweep
    tables
        .extend(orphans.iter().filter_map(|r| old(&r.get(0), created_at(&r.get::<_, String>(0)))).map(|n| (SCHEMA, n)));
    let mut dropped = 0;
    for (schema, name) in &tables {
        dropped += usize::from(gone_now(store, schema, name).await);
    }
    Ok(dropped)
}

/// Drop a table a sweep found. Another sweep (another server) may drop it at the same moment;
/// that's no reason to stop: logged, and the next sweep sees.
async fn gone_now(store: &Store, schema: &str, table: &str) -> bool {
    match materialize::drop(&store.pool, schema, table).await {
        Ok(()) => true,
        Err(e) => {
            tracing::debug!(table, "not dropped (the next sweep will see): {e:#}");
            false
        }
    }
}

/// What the server does every few minutes on each database: expired results and leftovers go,
/// then each organisation is kept within its row budget. Returns how many tables went.
pub async fn tidy(store: &Store, max_rows: i64) -> Result<usize> {
    let mut dropped = sweep(store).await?;
    let orgs = store.pool.get().await?.query("SELECT DISTINCT org FROM magnum_cache.entries", &[]).await?;
    for r in &orgs {
        dropped += trim(store, &r.get::<_, String>(0), max_rows).await?;
    }
    Ok(dropped)
}

/// Keep an organisation's cached results within `max_rows` in all: the most recently used stay,
/// the rest go (MAGNUM_CACHE_MAX_ROWS, so a busy organisation can't fill the disk). Returns how many.
pub async fn trim(store: &Store, org: &str, max_rows: i64) -> Result<usize> {
    let c = store.pool.get().await?;
    let entries = c
        .query(
            "SELECT key, row_count FROM magnum_cache.entries WHERE org = $1
             ORDER BY COALESCE(last_hit_at, created_at) DESC",
            &[&org],
        )
        .await?;
    let mut total = 0;
    let mut tables = vec![];
    for e in &entries {
        total += e.get::<_, i64>(1);
        if total <= max_rows {
            continue;
        }
        let key: String = e.get(0);
        let gone = c.query_opt("DELETE FROM magnum_cache.entries WHERE key = $1 RETURNING table_name", &[&key]).await?;
        tables.extend(gone.map(|r| r.get::<_, String>(0)));
    }
    drop(c); // dropping a table takes a connection of its own
    let mut dropped = 0;
    for t in &tables {
        dropped += usize::from(gone_now(store, SCHEMA, t).await);
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
    drop(c); // the sweep takes connections of its own
    for r in &rows {
        gone_now(store, SCHEMA, &r.get::<_, String>(0)).await;
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

    async fn test_store() -> Option<Store> {
        Some(Store::new(crate::db::test_pool(2).await?))
    }

    #[tokio::test]
    async fn the_cache_works_on_a_single_connection() {
        // nothing holds its connection while asking for another: under load, a pool full of
        // requests each waiting for a second connection waits forever
        let Some(pool) = crate::db::test_pool(1).await else { return };
        let store = Store::new(pool);
        let org = format!("t-{}", uuid::Uuid::new_v4().simple());
        let work = async {
            let key = cache(&store, &org, 2, 3600).await;
            cache(&store, &org, 2, 0).await;
            let p = Put {
                key: &key,
                org: &org,
                model: "m",
                connection: "c",
                ttl_seconds: 3600,
                names: &["n".to_string()],
                types: &[ColType::Numeric],
                rows: &[vec![json!(1)]],
            };
            put(&store, p).await.unwrap(); // replacing a result drops the old table
            assert!(get(&store, &key).await.unwrap().is_some());
            trim(&store, &org, 1).await.unwrap();
            sweep(&store).await.unwrap();
            tidy(&store, 5_000_000).await.unwrap();
            clear(&store, &org, None).await.unwrap();
        };
        // slow on a busy shared database (the sweep goes through every test's results), never stuck
        tokio::time::timeout(std::time::Duration::from_secs(600), work).await.expect("waiting for a second connection");
    }

    async fn cache(store: &Store, org: &str, rows: usize, ttl_seconds: u64) -> String {
        let key = key(org, "c", &uuid::Uuid::new_v4().to_string());
        let rows: Vec<Vec<Value>> = (0..rows).map(|i| vec![json!(i)]).collect();
        let names = ["n".to_string()];
        let p = Put {
            key: &key,
            org,
            model: "m",
            connection: "c",
            ttl_seconds,
            names: &names,
            types: &[ColType::Numeric],
            rows: &rows,
        };
        put(store, p).await.unwrap();
        key
    }

    async fn tables(store: &Store, org: &str) -> Vec<String> {
        let c = store.pool.get().await.unwrap();
        let rows = c.query("SELECT table_name FROM magnum_cache.entries WHERE org = $1", &[&org]).await.unwrap();
        rows.iter().map(|r| r.get(0)).collect()
    }

    async fn exists(store: &Store, table: &str) -> bool {
        let c = store.pool.get().await.unwrap();
        let sql = "SELECT 1 FROM information_schema.tables WHERE table_schema = 'magnum_cache' AND table_name = $1";
        c.query_opt(sql, &[&table]).await.unwrap().is_some()
    }

    #[tokio::test]
    async fn a_sweep_drops_expired_results_and_their_tables() {
        let Some(store) = test_store().await else { return };
        let org = format!("t-{}", uuid::Uuid::new_v4().simple());
        cache(&store, &org, 2, 0).await;
        let expired = tables(&store, &org).await;
        cache(&store, &org, 2, 3600).await;
        sweep(&store).await.unwrap();
        let left = tables(&store, &org).await;
        assert_eq!(left.len(), 1);
        assert!(!left.contains(&expired[0]) && !exists(&store, &expired[0]).await);
        clear(&store, &org, None).await.unwrap();
    }

    #[tokio::test]
    async fn an_organisation_keeps_its_most_recent_rows_within_its_budget() {
        let Some(store) = test_store().await else { return };
        let (org, other) =
            (format!("t-{}", uuid::Uuid::new_v4().simple()), format!("t-{}", uuid::Uuid::new_v4().simple()));
        let oldest = cache(&store, &org, 3, 3600).await;
        let older = cache(&store, &org, 3, 3600).await;
        let newest = cache(&store, &org, 3, 3600).await;
        cache(&store, &other, 3, 3600).await;
        let old_tables = tables(&store, &org).await;

        assert_eq!(trim(&store, &org, 7).await.unwrap(), 1);
        let keys = keys(&store, &org).await;
        assert!(keys.contains(&older) && keys.contains(&newest) && !keys.contains(&oldest), "{keys:?}");
        let gone: Vec<_> = old_tables.iter().filter(|t| !tables_contains(&keys, t)).collect();
        assert_eq!(gone.len(), 1);
        assert!(!exists(&store, gone[0]).await);
        assert_eq!(tables(&store, &other).await.len(), 1, "another organisation's budget is its own");

        // with a budget for none of them, everything goes
        assert_eq!(trim(&store, &org, 0).await.unwrap(), 2);
        assert!(tables(&store, &org).await.is_empty());
        clear(&store, &other, None).await.unwrap();
    }

    async fn keys(store: &Store, org: &str) -> Vec<String> {
        let c = store.pool.get().await.unwrap();
        let rows = c.query("SELECT key FROM magnum_cache.entries WHERE org = $1", &[&org]).await.unwrap();
        rows.iter().map(|r| r.get(0)).collect()
    }

    fn tables_contains(keys: &[String], table: &str) -> bool {
        keys.iter().any(|k| table.starts_with(&format!("r_{}_", &k[..16])))
    }
}
