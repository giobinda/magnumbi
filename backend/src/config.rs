//! Settings, from environment variables (see .env.local.example).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

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
    /// Org admins may make dashboards public (anyone with the link); 0 forbids it platform-wide
    pub public_links: bool,
    pub google: Option<GoogleConfig>,
    /// system owners until the platform settings are saved for the first time
    pub bootstrap_owners: Vec<String>,
    /// the first system owner's one-time password, when there are no accounts yet
    /// (MAGNUM_INITIAL_PASSWORD; none: generated and logged once)
    pub initial_password: Option<String>,
    pub auto_migrate: bool,
    pub session_hours: i64,
    /// seals each organisation's ScramDB connection string (crypto.rs)
    pub secret_key: Option<crate::crypto::SecretKey>,
    pub usage_retention_days: i64,
    /// rows of cached results each organisation may keep (MAGNUM_CACHE_MAX_ROWS; cache.rs)
    pub cache_max_rows: i64,
    /// how big an uploaded spreadsheet or CSV may be (MAGNUM_UPLOAD_MAX_MB, 100; lite: 2048)
    pub upload_max_bytes: usize,
    /// where Google Sheets are downloaded from: Google's, never a setting (tests use a stand-in)
    pub google_sheets_url: String,
    /// one person on their own computer, no sign-in (lite.rs): set by `magnum-bi lite` only, never
    /// by a setting
    pub lite: bool,
    /// lite only: the hash of the PIN that unlocks it (none: it isn't locked)
    pub lite_pin: Option<String>,
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
            public_links: flag("MAGNUM_PUBLIC_LINKS", true),
            google,
            bootstrap_owners: list("MAGNUM_SYSTEM_OWNERS"),
            initial_password: env("MAGNUM_INITIAL_PASSWORD").filter(|p| !p.is_empty()),
            auto_migrate: flag("MAGNUM_DB_AUTO_MIGRATE", true),
            session_hours: env("MAGNUM_SESSION_HOURS").map(|h| h.parse()).transpose()?.unwrap_or(12),
            secret_key: match (env("MAGNUM_SECRET_KEY"), env("MAGNUM_SECRET_KEY_FILE")) {
                (Some(k), _) => Some(crate::crypto::SecretKey::parse(&k)?),
                (None, Some(path)) => Some(key_file(Path::new(&path))?),
                (None, None) => None,
            },
            usage_retention_days: env("MAGNUM_USAGE_RETENTION_DAYS").map(|d| d.parse()).transpose()?.unwrap_or(400),
            lite: false,
            lite_pin: None,
            cache_max_rows: env("MAGNUM_CACHE_MAX_ROWS").map(|n| n.parse()).transpose()?.unwrap_or(5_000_000),
            google_sheets_url: "https://docs.google.com".into(),
            upload_max_bytes: env("MAGNUM_UPLOAD_MAX_MB").map(|n| n.parse::<usize>()).transpose()?.unwrap_or(100) << 20,
            duckdb_dir: env("MAGNUM_DUCKDB_DIR").unwrap_or_else(|| "data/duckdb".into()).into(),
            duckdb_install_extensions: flag("MAGNUM_DUCKDB_INSTALL_EXTENSIONS", true),
            allow_private_connections: flag("MAGNUM_ALLOW_PRIVATE_CONNECTIONS", !deployed),
            connection_allowed_hosts: list("MAGNUM_CONNECTION_ALLOWED_HOSTS"),
            client_ip_header: env("MAGNUM_CLIENT_IP_HEADER").map(|h| h.to_lowercase()),
        };
        if config.usage_retention_days < 1 {
            bail!("MAGNUM_USAGE_RETENTION_DAYS: at least 1");
        }
        if config.cache_max_rows < 0 {
            bail!("MAGNUM_CACHE_MAX_ROWS: 0 or more");
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

/// MAGNUM_SECRET_KEY_FILE: the key in a file, made (owner-only) on the first start if missing.
/// Containers keep it on its own volume, apart from the database.
fn key_file(path: &Path) -> Result<crate::crypto::SecretKey> {
    use std::io::Write;
    if !path.exists() {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
        let mut f = opts.open(path).with_context(|| format!("creating {}", path.display()))?;
        f.write_all(crate::crypto::SecretKey::generate().as_bytes())?;
        tracing::warn!(
            "generated a new MAGNUM_SECRET_KEY in {}: back it up (without it, organisation databases and secrets \
             must be entered again)",
            path.display()
        );
    }
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    crate::crypto::SecretKey::parse(&text).with_context(|| format!("MAGNUM_SECRET_KEY_FILE {}", path.display()))
}

/// The first administrator: with no accounts yet, the first system owner gets a one-time password
/// (the one given, else a generated one, to show once): (email, password, generated).
pub fn first_admin(user_count: i64, owners: &[String], given: Option<&str>) -> Option<(String, String, bool)> {
    let email = owners.first().filter(|_| user_count == 0)?;
    Some(match given.filter(|p| !p.is_empty()) {
        Some(p) => (email.clone(), p.to_string(), false),
        None => (email.clone(), crate::store::random_token(18), true),
    })
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

    #[test]
    fn the_cache_has_a_row_budget_per_organisation() {
        assert_eq!(config(&[]).cache_max_rows, 5_000_000);
        assert_eq!(config(&[("MAGNUM_CACHE_MAX_ROWS", "200000")]).cache_max_rows, 200_000);
        assert!(Config::from_lookup(|k| (k == "MAGNUM_CACHE_MAX_ROWS").then(|| "-1".into())).is_err());
    }

    #[test]
    fn secret_key_from_a_file_made_on_first_start() {
        let dir = std::env::temp_dir().join(format!("magnum-key-{}", uuid::Uuid::new_v4().simple()));
        let path = dir.join("secrets/key");
        let p = path.to_str().unwrap();
        let first = config(&[("MAGNUM_SECRET_KEY_FILE", p)]);
        assert!(first.secret_key.is_some(), "generated");
        let written = std::fs::read_to_string(&path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600, "owner only");
        }
        // the next start reads the same key
        let again = config(&[("MAGNUM_SECRET_KEY_FILE", p)]);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), written);
        let sealed = first.secret_key.as_ref().unwrap().seal("x", "c").unwrap();
        assert_eq!(again.secret_key.as_ref().unwrap().open(&sealed, "c").unwrap(), "x");
        // MAGNUM_SECRET_KEY wins; a bad file is an error, not a silent new key
        std::fs::write(&path, "not a key").unwrap();
        assert!(Config::from_lookup(|k| (k == "MAGNUM_SECRET_KEY_FILE").then(|| p.to_string())).is_err());
        let key = crate::crypto::SecretKey::generate();
        let both = config(&[("MAGNUM_SECRET_KEY_FILE", p), ("MAGNUM_SECRET_KEY", &key)]);
        assert!(both.secret_key.is_some());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn the_first_administrator() {
        let owners = vec!["admin@example.com".to_string()];
        // no accounts yet: the owner gets the given password, or a generated one (shown once)
        let (email, pw, generated) = first_admin(0, &owners, Some("a given passphrase")).unwrap();
        assert_eq!((email.as_str(), pw.as_str(), generated), ("admin@example.com", "a given passphrase", false));
        let (_, pw, generated) = first_admin(0, &owners, None).unwrap();
        assert!(generated && pw.len() >= 16);
        assert!(first_admin(0, &owners, Some("")).unwrap().2, "empty: generated");
        // accounts exist, or nobody to give it to: nothing
        assert!(first_admin(1, &owners, None).is_none());
        assert!(first_admin(0, &[], Some("x")).is_none());
    }
}
