//! Usage metrics of the platform, in the primary ScramDB (`usage_events`).
//!
//! Requests hand events to a bounded channel and move on; a background task writes them in
//! batches. Recording never fails or slows a request: when ScramDB is unreachable or the buffer
//! is full, events are dropped and a warning is logged. Events older than the retention period
//! (MAGNUM_USAGE_RETENTION_DAYS, default 400) are deleted daily.

use std::time::Duration;

use anyhow::Result;
use chrono::{DateTime, NaiveDate, Utc};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::store::Store;

const BUFFER: usize = 10_000;
const BATCH: usize = 500;
const FLUSH_EVERY: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Default)]
pub struct Event {
    /// sign_in | query | dashboard_view | dashboard_saved | dashboard_deleted | members_changed |
    /// platform_changed | org_database_set | template_published | password_issued | cache_cleared
    pub kind: &'static str,
    pub org: String,
    pub user_email: Option<String>,
    pub role: Option<String>,
    /// queries: explore | tile | filter_values
    pub source: Option<&'static str>,
    pub dashboard_id: Option<String>,
    pub model: Option<String>,
    pub connection: Option<String>,
    pub row_count: Option<i64>,
    pub duration_ms: Option<i64>,
    pub cache_hit: Option<bool>,
    pub ok: bool,
    pub detail: Option<Value>,
    /// through an API token: "token:<name>"
    pub via: Option<String>,
}

impl Event {
    pub fn new(kind: &'static str) -> Event {
        Event { kind, ok: true, ..Event::default() }
    }
}

#[derive(Clone)]
pub struct Usage {
    tx: mpsc::Sender<Event>,
}

impl Usage {
    /// Start the writer (and the daily retention clean-up) on the primary database.
    pub fn start(store: Store, retention_days: i64) -> Usage {
        let (tx, rx) = mpsc::channel(BUFFER);
        tokio::spawn(writer(store.clone(), rx));
        tokio::spawn(async move {
            let mut every = tokio::time::interval(Duration::from_secs(24 * 3600));
            loop {
                every.tick().await;
                if let Err(e) = purge(&store, retention_days).await {
                    tracing::warn!("usage retention clean-up failed: {e:#}");
                }
            }
        });
        Usage { tx }
    }

    pub fn record(&self, e: Event) {
        match self.tx.try_send(e) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => tracing::warn!("usage buffer full: event dropped"),
            Err(mpsc::error::TrySendError::Closed(_)) => tracing::error!("usage writer stopped: event dropped"),
        }
    }
}

async fn writer(store: Store, mut rx: mpsc::Receiver<Event>) {
    let mut batch: Vec<Event> = Vec::with_capacity(BATCH);
    loop {
        let closed = tokio::select! {
            got = rx.recv_many(&mut batch, BATCH) => got == 0,
            _ = tokio::time::sleep(FLUSH_EVERY) => false,
        };
        if !batch.is_empty() {
            if let Err(e) = insert(&store, &batch).await {
                tracing::warn!("{} usage event(s) not recorded: {e:#}", batch.len());
            }
            batch.clear();
        }
        if closed {
            return;
        }
    }
}

async fn insert(store: &Store, events: &[Event]) -> Result<()> {
    let mut c = store.pool.get().await?;
    let tx = c.transaction().await?;
    let stmt = tx
        .prepare(
            "INSERT INTO usage_events (id, kind, org, user_email, role, source, dashboard_id, model, connection,
               row_count, duration_ms, cache_hit, ok, detail, via)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)",
        )
        .await?;
    for e in events {
        let id = uuid::Uuid::now_v7().to_string();
        let detail = e.detail.as_ref().map(Value::to_string);
        tx.execute(
            &stmt,
            &[
                &id,
                &e.kind,
                &e.org,
                &e.user_email,
                &e.role,
                &e.source,
                &e.dashboard_id,
                &e.model,
                &e.connection,
                &e.row_count,
                &e.duration_ms,
                &e.cache_hit,
                &e.ok,
                &detail,
                &e.via,
            ],
        )
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

async fn purge(store: &Store, retention_days: i64) -> Result<()> {
    let cutoff = Utc::now() - chrono::Duration::days(retention_days);
    let n = store.pool.get().await?.execute("DELETE FROM usage_events WHERE at < $1", &[&cutoff]).await?;
    if n > 0 {
        tracing::info!("usage: removed {n} event(s) older than {retention_days} days");
    }
    Ok(())
}

/// The usage summary for the last `days` days: one organisation, or (None) the whole platform.
/// A model's queries in an organisation over the last `days`: (queries, people, last query).
pub async fn of_model(store: &Store, org: &str, model: &str, days: i64) -> Result<(i64, i64, Option<DateTime<Utc>>)> {
    let since: DateTime<Utc> = Utc::now() - chrono::Duration::days(days);
    let c = store.pool.get().await?;
    let r = c
        .query_one(
            "SELECT COUNT(*), COUNT(DISTINCT user_email), MAX(at) FROM usage_events
             WHERE org = $1::TEXT AND model = $2::TEXT AND kind = 'query' AND at >= $3::TIMESTAMPTZ",
            &[&org, &model, &since],
        )
        .await?;
    Ok((r.get(0), r.get(1), r.get(2)))
}

pub async fn summary(store: &Store, org: Option<&str>, days: i64) -> Result<Value> {
    let since: DateTime<Utc> = Utc::now() - chrono::Duration::days(days);
    let c = store.pool.get().await?;
    // the organisation filter is a bound parameter; only the shape of the WHERE clause varies
    let (scope, params): (&str, Vec<&(dyn tokio_postgres::types::ToSql + Sync)>) = match &org {
        Some(o) => ("at >= $1 AND org = $2", vec![&since, o]),
        None => ("at >= $1", vec![&since]),
    };
    let totals = c
        .query_one(
            &format!(
                "SELECT COUNT(DISTINCT user_email),
                   COUNT(CASE WHEN kind = 'query' THEN 1 END),
                   COUNT(CASE WHEN kind = 'query' AND cache_hit THEN 1 END),
                   COUNT(CASE WHEN kind = 'query' AND NOT ok THEN 1 END),
                   CAST(AVG(CASE WHEN kind = 'query' AND ok AND NOT COALESCE(cache_hit, false) THEN duration_ms END) AS DOUBLE PRECISION),
                   COUNT(CASE WHEN kind = 'dashboard_view' THEN 1 END),
                   COUNT(CASE WHEN kind = 'sign_in' THEN 1 END)
                 FROM usage_events WHERE {scope}"
            ),
            &params,
        )
        .await?;
    let by_day = c
        .query(
            &format!(
                "SELECT CAST(DATE_TRUNC('day', at) AS DATE), COUNT(DISTINCT user_email),
                   COUNT(CASE WHEN kind = 'query' THEN 1 END), COUNT(CASE WHEN kind = 'dashboard_view' THEN 1 END)
                 FROM usage_events WHERE {scope} GROUP BY 1 ORDER BY 1"
            ),
            &params,
        )
        .await?;
    let dashboards = c
        .query(
            &format!(
                "SELECT org, dashboard_id, COUNT(*), COUNT(DISTINCT user_email) FROM usage_events
                 WHERE {scope} AND kind = 'dashboard_view' GROUP BY 1, 2 ORDER BY 3 DESC LIMIT 20"
            ),
            &params,
        )
        .await?;
    let people = c
        .query(
            &format!(
                "SELECT user_email, org, MAX(role), COUNT(CASE WHEN kind = 'query' THEN 1 END),
                   COUNT(CASE WHEN kind = 'dashboard_view' THEN 1 END), MAX(at)
                 FROM usage_events WHERE {scope} AND user_email IS NOT NULL GROUP BY 1, 2 ORDER BY 6 DESC LIMIT 50"
            ),
            &params,
        )
        .await?;
    let models = c
        .query(
            &format!(
                "SELECT model, COUNT(*), COUNT(CASE WHEN cache_hit THEN 1 END), COUNT(CASE WHEN NOT ok THEN 1 END),
                   CAST(AVG(CASE WHEN ok AND NOT COALESCE(cache_hit, false) THEN duration_ms END) AS DOUBLE PRECISION)
                 FROM usage_events WHERE {scope} AND kind = 'query' GROUP BY 1 ORDER BY 2 DESC"
            ),
            &params,
        )
        .await?;
    let orgs = if org.is_none() {
        let rows = c
            .query(
                "SELECT org, COUNT(DISTINCT user_email), COUNT(CASE WHEN kind = 'query' THEN 1 END),
                   COUNT(CASE WHEN kind = 'dashboard_view' THEN 1 END)
                 FROM usage_events WHERE at >= $1 GROUP BY 1 ORDER BY 3 DESC",
                &[&since],
            )
            .await?;
        rows.iter()
            .map(|r| {
                json!({"org": r.get::<_, String>(0), "people": r.get::<_, i64>(1), "queries": r.get::<_, i64>(2),
                            "dashboard_views": r.get::<_, i64>(3)})
            })
            .collect::<Vec<_>>()
    } else {
        vec![]
    };
    let (queries, hits): (i64, i64) = (totals.get(1), totals.get(2));
    Ok(json!({
        "days": days,
        "totals": {
            "people": totals.get::<_, i64>(0),
            "queries": queries,
            "cache_hit_rate": if queries > 0 { Some(hits as f64 / queries as f64) } else { None },
            "failed_queries": totals.get::<_, i64>(3),
            "avg_query_ms": totals.get::<_, Option<f64>>(4),
            "dashboard_views": totals.get::<_, i64>(5),
            "sign_ins": totals.get::<_, i64>(6),
        },
        "by_day": by_day.iter().map(|r| json!({"day": r.get::<_, NaiveDate>(0).to_string(), "people": r.get::<_, i64>(1),
                                                "queries": r.get::<_, i64>(2), "dashboard_views": r.get::<_, i64>(3)})).collect::<Vec<_>>(),
        "dashboards": dashboards.iter().map(|r| json!({"org": r.get::<_, String>(0), "dashboard_id": r.get::<_, Option<String>>(1),
                                                        "views": r.get::<_, i64>(2), "people": r.get::<_, i64>(3)})).collect::<Vec<_>>(),
        "people": people.iter().map(|r| json!({"email": r.get::<_, String>(0), "org": r.get::<_, String>(1),
                                                "role": r.get::<_, Option<String>>(2), "queries": r.get::<_, i64>(3),
                                                "dashboard_views": r.get::<_, i64>(4),
                                                "last_seen": r.get::<_, DateTime<Utc>>(5).to_rfc3339()})).collect::<Vec<_>>(),
        "models": models.iter().map(|r| json!({"model": r.get::<_, Option<String>>(0), "queries": r.get::<_, i64>(1),
                                                "cache_hits": r.get::<_, i64>(2), "failed": r.get::<_, i64>(3),
                                                "avg_ms": r.get::<_, Option<f64>>(4)})).collect::<Vec<_>>(),
        "orgs": orgs,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn count(store: &Store, kind: &str) -> i64 {
        let c = store.pool.get().await.unwrap();
        c.query_one("SELECT COUNT(*) FROM usage_events WHERE kind = $1", &[&kind]).await.unwrap().get(0)
    }

    /// Needs a ScramDB (MAGNUM_TEST_DATABASE_URL); skipped without it.
    #[tokio::test]
    async fn events_keep_being_written_after_quiet_periods() {
        let Ok(url) = std::env::var("MAGNUM_TEST_DATABASE_URL") else { return };
        let pool = crate::db::make_pool(&url, 4).unwrap();
        crate::db::migrate(&pool, crate::db::PRIMARY).await.unwrap();
        let store = Store::new(pool);
        let usage = Usage::start(store.clone(), 400);
        let kind: &'static str = Box::leak(format!("test_{}", uuid::Uuid::new_v4().simple()).into_boxed_str());
        for _ in 0..3 {
            usage.record(Event::new(kind));
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
        assert_eq!(count(&store, kind).await, 3);
        // quiet for longer than a flush interval, then more events
        tokio::time::sleep(FLUSH_EVERY * 3).await;
        usage.record(Event::new(kind));
        tokio::time::sleep(Duration::from_secs(3)).await;
        assert_eq!(count(&store, kind).await, 4, "the writer must survive quiet periods");
    }
}
