//! Settings, from environment variables (see .env.local.example).

use std::path::PathBuf;

use anyhow::{Result, bail};

#[derive(Debug, Clone)]
pub struct GoogleConfig {
    pub client_id: String,
    pub client_secret: String,
    /// only show this Google Workspace's accounts in Google's picker (a hint; membership decides)
    pub hosted_domain: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Config {
    /// Magnum BI's own ScramDB database (MAGNUM_DATABASE_URL)
    pub database_url: String,
    pub project_dir: PathBuf,
    pub frontend_dist: PathBuf,
    pub host: String,
    pub port: u16,
    /// the public address (https in production): OAuth callbacks and origin checks
    pub public_url: Option<String>,
    /// MAGNUM_ENV=production|staging: refuse to start with development-only settings
    pub deployed: bool,
    /// lax (default), strict, or none (cross-site embeds over https)
    pub cookie_samesite: String,
    /// extra origins allowed to send state-changing requests (comma-separated)
    pub trusted_origins: Vec<String>,
    /// who may embed Magnum BI in an iframe (CSP frame-ancestors)
    pub frame_ancestors: String,
    pub password_login: bool,
    pub google: Option<GoogleConfig>,
    /// system owners until the platform settings are saved for the first time
    pub bootstrap_owners: Vec<String>,
    pub auto_migrate: bool,
    pub session_hours: i64,
    /// seals each organisation's ScramDB connection string (crypto.rs)
    pub secret_key: Option<crate::crypto::SecretKey>,
    pub usage_retention_days: i64,
    /// DuckDB connections: one directory per organisation (and _extensions)
    pub duckdb_dir: PathBuf,
    /// download DuckDB's httpfs extension when an organisation first reads object storage
    pub duckdb_install_extensions: bool,
    /// organisations' connections may reach private networks (development; never in production
    /// unless MAGNUM_ALLOW_PRIVATE_CONNECTIONS=1)
    pub allow_private_connections: bool,
    /// private hosts organisations' connections may reach anyway (e.g. a shared warehouse)
    pub connection_allowed_hosts: Vec<String>,
    /// behind a reverse proxy: the header carrying the client address (e.g. x-forwarded-for,
    /// first entry); without it, the TCP peer is the client
    pub client_ip_header: Option<String>,
}

impl Config {
    pub fn from_env() -> Result<Config> {
        Config::from_lookup(|k| std::env::var(k).ok())
    }

    /// Settings from any key -> value lookup (the environment, or a map in tests).
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Config> {
        let env = |key: &str| lookup(key).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        let flag = |key: &str, default: bool| {
            env(key).map_or(default, |v| matches!(v.to_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        };
        let list = |key: &str| -> Vec<String> {
            env(key)
                .map(|v| {
                    v.split(',')
                        .map(|s| s.trim().trim_end_matches('/').to_lowercase())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default()
        };
        let google = match (env("MAGNUM_GOOGLE_CLIENT_ID"), env("MAGNUM_GOOGLE_CLIENT_SECRET")) {
            (Some(client_id), Some(client_secret)) => {
                Some(GoogleConfig { client_id, client_secret, hosted_domain: env("MAGNUM_GOOGLE_HD") })
            }
            (None, None) => None,
            _ => bail!("set both MAGNUM_GOOGLE_CLIENT_ID and MAGNUM_GOOGLE_CLIENT_SECRET (or neither)"),
        };
        let samesite = env("MAGNUM_COOKIE_SAMESITE").unwrap_or_else(|| "lax".into()).to_lowercase();
        if !["lax", "strict", "none"].contains(&samesite.as_str()) {
            bail!("MAGNUM_COOKIE_SAMESITE: lax, strict or none");
        }
        let deployed =
            matches!(env("MAGNUM_ENV").unwrap_or_default().to_lowercase().as_str(), "production" | "staging");
        let config = Config {
            database_url: env("MAGNUM_DATABASE_URL")
                .unwrap_or_else(|| "postgresql://scramdb@127.0.0.1:5432/scramdb".into()),
            project_dir: env("MAGNUM_PROJECT_DIR").unwrap_or_else(|| "project".into()).into(),
            frontend_dist: env("MAGNUM_FRONTEND_DIST").unwrap_or_else(|| "frontend/dist".into()).into(),
            host: env("MAGNUM_HOST").unwrap_or_else(|| "127.0.0.1".into()),
            port: env("MAGNUM_PORT").map(|p| p.parse()).transpose()?.unwrap_or(8000),
            public_url: env("MAGNUM_PUBLIC_URL").map(|u| u.trim_end_matches('/').to_string()),
            deployed,
            cookie_samesite: samesite,
            trusted_origins: list("MAGNUM_TRUSTED_ORIGINS"),
            frame_ancestors: env("MAGNUM_FRAME_ANCESTORS").unwrap_or_else(|| "'self'".into()),
            password_login: flag("MAGNUM_PASSWORD_LOGIN", true),
            google,
            bootstrap_owners: list("MAGNUM_SYSTEM_OWNERS"),
            auto_migrate: flag("MAGNUM_DB_AUTO_MIGRATE", true),
            session_hours: env("MAGNUM_SESSION_HOURS").map(|h| h.parse()).transpose()?.unwrap_or(12),
            secret_key: env("MAGNUM_SECRET_KEY").map(|k| crate::crypto::SecretKey::parse(&k)).transpose()?,
            usage_retention_days: env("MAGNUM_USAGE_RETENTION_DAYS").map(|d| d.parse()).transpose()?.unwrap_or(400),
            duckdb_dir: env("MAGNUM_DUCKDB_DIR").unwrap_or_else(|| "data/duckdb".into()).into(),
            duckdb_install_extensions: flag("MAGNUM_DUCKDB_INSTALL_EXTENSIONS", true),
            allow_private_connections: flag("MAGNUM_ALLOW_PRIVATE_CONNECTIONS", !deployed),
            connection_allowed_hosts: list("MAGNUM_CONNECTION_ALLOWED_HOSTS"),
            client_ip_header: env("MAGNUM_CLIENT_IP_HEADER").map(|h| h.to_lowercase()),
        };
        if config.usage_retention_days < 1 {
            bail!("MAGNUM_USAGE_RETENTION_DAYS: at least 1");
        }
        if !(1..=24 * 30).contains(&config.session_hours) {
            bail!("MAGNUM_SESSION_HOURS: between 1 and 720");
        }
        Ok(config)
    }

    pub fn is_https(&self) -> bool {
        self.public_url.as_deref().is_some_and(|u| u.starts_with("https://"))
    }

    /// Where organisations' connections may connect: never the primary database.
    pub fn connection_guard(&self) -> crate::netguard::NetGuard {
        let primary = url::Url::parse(&self.database_url)
            .ok()
            .and_then(|u| Some(format!("{}:{}", u.host_str()?, u.port().unwrap_or(5432))));
        crate::netguard::NetGuard {
            allow_private: self.allow_private_connections,
            allowed_hosts: self.connection_allowed_hosts.clone(),
            internal: primary.into_iter().collect(),
        }
    }

    /// Problems that make an instance unsafe when shared. Fatal with MAGNUM_ENV=production.
    pub fn deployment_problems(&self) -> Vec<String> {
        let mut p = vec![];
        if !self.password_login && self.google.is_none() {
            p.push("no sign-in method: enable MAGNUM_PASSWORD_LOGIN or configure Google".into());
        }
        if !self.is_https() {
            p.push(
                "MAGNUM_PUBLIC_URL must be the public https address (cookies, OAuth callbacks, origin checks)".into(),
            );
        }
        if self.secret_key.is_none() {
            p.push(
                "MAGNUM_SECRET_KEY is not set (organisation databases need it; create one with `magnum-bi gen-key`)"
                    .into(),
            );
        }
        if self.cookie_samesite == "none" && !self.is_https() {
            p.push("MAGNUM_COOKIE_SAMESITE=none needs https".into());
        }
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(vars: &[(&str, &str)]) -> Config {
        Config::from_lookup(|k| vars.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())).unwrap()
    }

    #[test]
    fn connection_guard_is_strict_in_production() {
        let prod = config(&[
            ("MAGNUM_ENV", "production"),
            ("MAGNUM_DATABASE_URL", "postgresql://u:pw@db.internal:6000/magnum"),
        ]);
        let g = prod.connection_guard();
        assert!(!g.allow_private);
        assert_eq!(g.internal, ["db.internal:6000"], "the primary database is never a connection target");

        let dev = config(&[]);
        assert!(dev.connection_guard().allow_private, "development: local databases are fine");

        let allowed = config(&[
            ("MAGNUM_ENV", "production"),
            ("MAGNUM_ALLOW_PRIVATE_CONNECTIONS", "1"),
            ("MAGNUM_CONNECTION_ALLOWED_HOSTS", "Warehouse.Corp, 10.0.0.5"),
        ]);
        let g = allowed.connection_guard();
        assert!(g.allow_private);
        assert_eq!(g.allowed_hosts, ["warehouse.corp", "10.0.0.5"]);
    }
}
