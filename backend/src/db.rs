//! ScramDB connections and schema migrations.
//!
//! Two kinds of database, each with its own migrations (plain SQL files in backend/migrations,
//! embedded in the binary):
//! - the **primary** ScramDB (MAGNUM_DATABASE_URL): users, sessions, platform settings,
//!   organisations and their members, platform templates, usage metrics;
//! - one **organisation** ScramDB per organisation (orgdb.rs): its dashboards and query cache.
//!
//! ScramDB speaks the PostgreSQL wire protocol, so this uses tokio-postgres. ScramDB DDL is not
//! transactional, so each statement is idempotent and a version is recorded only once all its
//! statements ran.

use anyhow::{Context, Result};
use deadpool_postgres::{Manager, ManagerConfig, Pool, RecyclingMethod};
use tokio_postgres::NoTls;

pub use deadpool_postgres::Pool as DbPool;

pub type Migrations = &'static [(&'static str, &'static str)];

pub const PRIMARY: Migrations = &[
    ("0001_initial", include_str!("../migrations/primary/0001_initial.sql")),
    ("0002_orgs_usage", include_str!("../migrations/primary/0002_orgs_usage.sql")),
    ("0003_connections", include_str!("../migrations/primary/0003_connections.sql")),
    ("0004_blends", include_str!("../migrations/primary/0004_blends.sql")),
    ("0005_two_factor", include_str!("../migrations/primary/0005_two_factor.sql")),
    ("0006_single_sign_on", include_str!("../migrations/primary/0006_single_sign_on.sql")),
    ("0007_passkeys", include_str!("../migrations/primary/0007_passkeys.sql")),
    ("0008_api_tokens", include_str!("../migrations/primary/0008_api_tokens.sql")),
    ("0009_themes", include_str!("../migrations/primary/0009_themes.sql")),
    ("0010_share_links", include_str!("../migrations/primary/0010_share_links.sql")),
    ("0011_guest_links", include_str!("../migrations/primary/0011_guest_links.sql")),
    ("0012_org_models", include_str!("../migrations/primary/0012_org_models.sql")),
    ("0013_delivery", include_str!("../migrations/primary/0013_delivery.sql")),
    ("0014_schedules", include_str!("../migrations/primary/0014_schedules.sql")),
    ("0015_alerts", include_str!("../migrations/primary/0015_alerts.sql")),
    ("0016_formulas", include_str!("../migrations/primary/0016_formulas.sql")),
    ("0017_governance", include_str!("../migrations/primary/0017_governance.sql")),
    ("0018_docs_glossary", include_str!("../migrations/primary/0018_docs_glossary.sql")),
];
/// Versions are prefixed, so an instance serving as both primary and organisation database keeps
/// the two histories apart.
pub const ORG: Migrations = &[
    ("org/0001_initial", include_str!("../migrations/org/0001_initial.sql")),
    ("org/0002_connections", include_str!("../migrations/org/0002_connections.sql")),
    ("org/0003_blends", include_str!("../migrations/org/0003_blends.sql")),
    ("org/0004_themes", include_str!("../migrations/org/0004_themes.sql")),
    ("org/0005_share_links", include_str!("../migrations/org/0005_share_links.sql")),
    ("org/0006_guest_links", include_str!("../migrations/org/0006_guest_links.sql")),
    ("org/0007_org_models", include_str!("../migrations/org/0007_org_models.sql")),
    ("org/0008_delivery", include_str!("../migrations/org/0008_delivery.sql")),
    ("org/0009_schedules", include_str!("../migrations/org/0009_schedules.sql")),
    ("org/0010_alerts", include_str!("../migrations/org/0010_alerts.sql")),
    ("org/0011_formulas", include_str!("../migrations/org/0011_formulas.sql")),
    ("org/0012_governance", include_str!("../migrations/org/0012_governance.sql")),
    ("org/0013_docs_glossary", include_str!("../migrations/org/0013_docs_glossary.sql")),
];
/// pg_advisory_lock key serializing migrations across replicas (ScramDB: literal keys only)
const MIGRATION_LOCK: i64 = 727_001;

pub fn make_pool(url: &str, max_size: usize) -> Result<Pool> {
    let config: tokio_postgres::Config = url.parse().context("invalid database URL")?;
    let manager = Manager::from_config(config, NoTls, ManagerConfig { recycling_method: RecyclingMethod::Fast });
    Pool::builder(manager).max_size(max_size).build().context("database pool")
}

/// The URL with any password replaced, for logs.
pub fn redact(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(mut u) if u.password().is_some() => {
            let _ = u.set_password(Some("***"));
            u.to_string()
        }
        Ok(u) => u.to_string(),
        Err(_) => url.split_whitespace().filter(|kv| !kv.starts_with("password=")).collect::<Vec<_>>().join(" "),
    }
}

/// Retry `attempt` every `every` until it succeeds or `limit` has passed (then its last error):
/// at start-up the database container may still be coming up.
pub async fn wait_for<T, F, Fut>(limit: std::time::Duration, every: std::time::Duration, mut attempt: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let started = std::time::Instant::now();
    loop {
        match attempt().await {
            Ok(v) => return Ok(v),
            Err(e) if started.elapsed() + every > limit => return Err(e),
            Err(_) => tokio::time::sleep(every).await,
        }
    }
}

fn statements(sql: &str) -> Vec<String> {
    let without_comments: String =
        sql.lines().map(|l| l.split("--").next().unwrap_or("")).collect::<Vec<_>>().join("\n");
    without_comments.split(';').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect()
}

pub async fn migrate(pool: &Pool, migrations: Migrations) -> Result<usize> {
    let client = pool.get().await.context("can't connect to ScramDB")?;
    // the lock comes first: on a new database, replicas (or tests) starting together would
    // otherwise race to create the table, and ScramDB's IF NOT EXISTS doesn't survive that
    client.batch_execute(&format!("SELECT pg_advisory_lock({MIGRATION_LOCK})")).await?;
    let result = async {
        client
            .batch_execute(
                "CREATE TABLE IF NOT EXISTS magnum_migrations (version TEXT PRIMARY KEY, applied_at TIMESTAMPTZ NOT NULL DEFAULT now())",
            )
            .await?;
        let mut applied = 0;
        for (version, sql) in migrations {
            let done =
                client.query_opt("SELECT 1 FROM magnum_migrations WHERE version = $1", &[version]).await?.is_some();
            if done {
                continue;
            }
            for stmt in statements(sql) {
                client.batch_execute(&stmt).await.with_context(|| format!("migration {version}: {stmt}"))?;
            }
            client
                .execute(
                    "INSERT INTO magnum_migrations (version) VALUES ($1) ON CONFLICT (version) DO NOTHING",
                    &[version],
                )
                .await?;
            tracing::info!(version, "applied migration");
            applied += 1;
        }
        anyhow::Ok(applied)
    }
    .await;
    client.batch_execute(&format!("SELECT pg_advisory_unlock({MIGRATION_LOCK})")).await?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn waits_for_the_database_to_come_up() {
        let tries = std::sync::atomic::AtomicU32::new(0);
        let up_on_third = || async {
            let n = tries.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if n < 2 { anyhow::bail!("connection refused") } else { Ok(n) }
        };
        let wait = std::time::Duration::from_millis(5);
        assert_eq!(wait_for(std::time::Duration::from_secs(2), wait, up_on_third).await.unwrap(), 2);
        // never up: the last error, once the time is over
        let started = std::time::Instant::now();
        let e = wait_for(std::time::Duration::from_millis(50), wait, || async {
            Err::<(), _>(anyhow::anyhow!("still down"))
        })
        .await
        .unwrap_err();
        assert!(e.to_string().contains("still down"));
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }

    #[test]
    fn splits_statements_without_comments() {
        let s = statements("-- hello; world\nCREATE TABLE a (x TEXT); -- trailing\n\nCREATE INDEX b ON a (x);\n");
        assert_eq!(s, ["CREATE TABLE a (x TEXT)", "CREATE INDEX b ON a (x)"]);
        assert!(statements(PRIMARY[0].1).len() >= 8);
        assert!(statements(ORG[0].1).len() >= 6);
    }

    #[test]
    fn redacts_passwords() {
        assert_eq!(redact("postgresql://u:secret@h:5432/db"), "postgresql://u:***@h:5432/db");
        assert_eq!(redact("host=h user=u password=secret dbname=d"), "host=h user=u dbname=d");
    }
}
