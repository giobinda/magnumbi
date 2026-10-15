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
];
/// Versions are prefixed, so an instance serving as both primary and organisation database keeps
/// the two histories apart.
pub const ORG: Migrations = &[("org/0001_initial", include_str!("../migrations/org/0001_initial.sql"))];
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

fn statements(sql: &str) -> Vec<String> {
    let without_comments: String =
        sql.lines().map(|l| l.split("--").next().unwrap_or("")).collect::<Vec<_>>().join("\n");
    without_comments.split(';').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect()
}

pub async fn migrate(pool: &Pool, migrations: Migrations) -> Result<usize> {
    let client = pool.get().await.context("can't connect to ScramDB")?;
    client
        .batch_execute(
            "CREATE TABLE IF NOT EXISTS magnum_migrations (version TEXT PRIMARY KEY, applied_at TIMESTAMPTZ NOT NULL DEFAULT now())",
        )
        .await?;
    client.batch_execute(&format!("SELECT pg_advisory_lock({MIGRATION_LOCK})")).await?;
    let result = async {
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
