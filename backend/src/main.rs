//! Magnum BI: a lightweight BI tool. Semantic models and dashboards as YAML, queries on ScramDB,
//! a React UI served from the same address.

mod access;
mod api;
mod auth;
mod blend;
mod cache;
mod compiler;
mod config;
mod connections;
mod connectors;
mod crypto;
mod db;
mod demo;
mod error;
mod materialize;
mod netguard;
mod orgdb;
mod period;
mod project;
mod sample;
mod schema;
mod security;
mod state;
mod store;
mod themes;
mod tile_query;
mod usage;

use std::io::{BufRead, IsTerminal};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

use crate::config::Config;
use crate::state::AppState;
use crate::store::Store;

#[derive(Parser)]
#[command(name = "magnum-bi", version, about = "Magnum BI: semantic models and dashboards on ScramDB")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the server (the default)
    Serve,
    /// Apply the primary database's migrations and exit (organisation databases migrate when used)
    Migrate,
    /// Print a new random MAGNUM_SECRET_KEY
    GenKey,
    /// (Re)create the `sample_sales` demo table in Magnum BI's database
    SeedSample {
        #[arg(long, default_value_t = 50_000)]
        rows: usize,
    },
    /// Check the project files (connections, models, themes, dashboards) and exit
    Check,
    /// Manage sign-in accounts
    User {
        #[command(subcommand)]
        command: UserCommand,
    },
    /// Set up the NYC demo: taxi trips and borough weather, connections and dashboards
    /// (downloads about 60 MB per month; months already there are skipped)
    Demo {
        /// the organisation to set it up for (default: the platform workspace)
        #[arg(long, default_value = "")]
        org: String,
        /// how many months of trips, up to the latest published (13: a month and the same month a year earlier)
        #[arg(long, default_value_t = 13)]
        months: u32,
    },
    /// Manage organisations' own ScramDB instances
    Org {
        #[command(subcommand)]
        command: OrgCommand,
    },
}

#[derive(Subcommand)]
enum OrgCommand {
    /// Point an organisation at its ScramDB (connection string read from stdin; needs
    /// MAGNUM_SECRET_KEY). Tests and migrates it, and moves the organisation's dashboards there.
    SetDatabase { org: String },
}

#[derive(Subcommand)]
enum UserCommand {
    /// Create an account or set its password (asked for, or read from stdin with --stdin)
    SetPassword {
        email: String,
        #[arg(long)]
        name: Option<String>,
        /// read the password from the first line of stdin (scripts)
        #[arg(long)]
        stdin: bool,
    },
    /// Stop an account from signing in (ends its sessions)
    Disable { email: String },
    /// Allow a disabled account to sign in again
    Enable { email: String },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("MAGNUM_LOG")
                .unwrap_or_else(|_| "magnum_bi=info,tower_http=warn".into()),
        )
        .init();
    let cli = Cli::parse();
    let config = Config::from_env()?;
    match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => serve(config).await,
        Command::Migrate => {
            let pool = db::make_pool(&config.database_url, 2)?;
            let n = db::migrate(&pool, db::PRIMARY).await?;
            println!("{n} migration(s) applied to {}", db::redact(&config.database_url));
            Ok(())
        }
        Command::SeedSample { rows } => {
            let pool = db::make_pool(&config.database_url, 2)?;
            db::migrate(&pool, db::PRIMARY).await?;
            let n = sample::seed(&pool, rows).await?;
            println!("{n} rows in {} ({})", sample::TABLE, db::redact(&config.database_url));
            // the sample model reads `warehouse`: give the platform workspace one, if it has none
            let store = Store::new(pool);
            if store.connection("", "warehouse").await?.is_none() {
                let warehouse = sample::warehouse(&config.database_url)?;
                store.put_connection("", "warehouse", &warehouse, None, Some("seed")).await?;
                println!("added the platform workspace's `warehouse` connection (to this database)");
            }
            Ok(())
        }
        Command::GenKey => {
            println!("{}", crypto::SecretKey::generate());
            Ok(())
        }
        Command::Check => check(&config),
        Command::Demo { org, months } => {
            let state = app_state(config).await?;
            let progress = state.demo.clone();
            let by = state.config.bootstrap_owners.first().cloned().unwrap_or_else(|| "magnum-bi demo".into());
            let today = chrono::Utc::now().date_naive();
            demo::run(&state, &org, months, &by, today, &demo::Sources::default(), &progress).await?;
            let where_ =
                if org.is_empty() { "the platform workspace".to_string() } else { format!("organisation {org}") };
            println!("NYC demo ready in {where_}: dashboards nyc_taxi and nyc_taxi_weather");
            Ok(())
        }
        Command::Org { command: OrgCommand::SetDatabase { org } } => {
            let pool = db::make_pool(&config.database_url, 2)?;
            db::migrate(&pool, db::PRIMARY).await?;
            let store = Store::new(pool);
            let known = match store.setting("platform").await? {
                Some(p) => serde_json::from_value::<access::Platform>(p)?.organisations.contains_key(&org),
                None => false,
            };
            if !known {
                bail!("no organisation {org:?}: create it on the Organisations page first");
            }
            let mut dsn = String::new();
            std::io::stdin().lock().read_line(&mut dsn)?;
            let orgs = orgdb::OrgDbs::new(store, config.secret_key.clone());
            let (display, version, moved) = orgs.register(&org, dsn.trim(), Some("cli")).await?;
            println!("{org}: {display} ({version}); {moved} dashboard(s) moved");
            Ok(())
        }
        Command::User { command } => user(&config, command).await,
    }
}

fn check(config: &Config) -> Result<()> {
    let p = project::Project::new(&config.project_dir);
    let s = p.current();
    let mut problems: Vec<String> = s.errors.iter().map(|(k, v)| format!("{k}: {v}")).collect();
    let all = s.models.keys().cloned().collect();
    if let Ok(rd) = std::fs::read_dir(p.dashboards_dir()) {
        for path in rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|x| x == "yaml")) {
            let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
            match project::parse_dashboard(&std::fs::read_to_string(&path)?) {
                Ok(d) => problems
                    // connections belong to organisations: check models and fields, in Postgres SQL
                    .extend(
                        s.validate_dashboard(&d, &all, &|_| Some(compiler::Dialect::Postgres))
                            .into_iter()
                            .map(|w| format!("dashboards/{name}: {w}")),
                    ),
                Err(e) => problems.push(format!("dashboards/{name}: {e}")),
            }
        }
    }
    println!("{} model(s), {} theme(s)", s.models.len(), s.themes.len());
    if problems.is_empty() {
        println!("no problems found");
        return Ok(());
    }
    for p in &problems {
        eprintln!("✗ {p}");
    }
    bail!("{} problem(s)", problems.len())
}

fn read_password(stdin: bool) -> Result<String> {
    if stdin || !std::io::stdin().is_terminal() {
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line)?;
        return Ok(line.trim_end_matches(['\r', '\n']).to_string());
    }
    let first = rpassword::prompt_password("New password: ")?;
    let again = rpassword::prompt_password("Again: ")?;
    if first != again {
        bail!("the passwords don't match");
    }
    Ok(first)
}

async fn user(config: &Config, command: UserCommand) -> Result<()> {
    let pool = db::make_pool(&config.database_url, 2)?;
    db::migrate(&pool, db::PRIMARY).await?;
    let store = Store::new(pool);
    match command {
        UserCommand::SetPassword { email, name, stdin } => {
            let email = email.trim().to_lowercase();
            if !email.contains('@') {
                bail!("{email:?} isn't an email address");
            }
            let pw = read_password(stdin)?;
            auth::password::check_policy(&pw, &email).map_err(anyhow::Error::msg)?;
            let hash = auth::password::hash(&pw)?;
            // set on the server: not bound to any organisation
            store.set_password(&email, name.as_deref(), &hash, None, false).await?;
            println!("password set for {email}");
            if !config.bootstrap_owners.contains(&email) {
                println!("(to make them a system owner before any organisation exists: MAGNUM_SYSTEM_OWNERS={email})");
            }
        }
        UserCommand::Disable { email } => {
            if !store.set_disabled(&email, true).await? {
                bail!("no account {email}");
            }
            println!("{email} can no longer sign in");
        }
        UserCommand::Enable { email } => {
            if !store.set_disabled(&email, false).await? {
                bail!("no account {email}");
            }
            println!("{email} can sign in again");
        }
    }
    Ok(())
}

async fn serve(config: Config) -> Result<()> {
    let problems = config.deployment_problems();
    if config.deployed && !problems.is_empty() {
        bail!("MAGNUM_ENV=production refuses to start: {}", problems.join("; "));
    }
    // development (the default): fine on one computer, so a note rather than a warning
    if !problems.is_empty() {
        tracing::info!("development mode, fine on this computer; before sharing it: {}", problems.join("; "));
    }
    let state = app_state(config).await?;
    let addr = format!("{}:{}", state.config.host, state.config.port);
    let listener = tokio::net::TcpListener::bind(&addr).await.with_context(|| format!("can't listen on {addr}"))?;
    tracing::info!("Magnum BI on http://{addr}");
    let app = api::router(state).into_make_service_with_connect_info::<std::net::SocketAddr>();
    axum::serve(listener, app).with_graceful_shutdown(shutdown()).await?;
    Ok(())
}

/// The server's state: the primary database (migrated), the project, the guard, everything a
/// request needs. Shared by `serve` and commands that act like the server (`demo`).
async fn app_state(config: Config) -> Result<Arc<AppState>> {
    let pool = db::make_pool(&config.database_url, 16)?;
    if pool.get().await.is_err() {
        tracing::info!("waiting for ScramDB at {}", db::redact(&config.database_url));
        db::wait_for(Duration::from_secs(60), Duration::from_secs(1), || async {
            pool.get().await.map(drop).map_err(anyhow::Error::from)
        })
        .await
        .with_context(|| format!("ScramDB at {} didn't come up", db::redact(&config.database_url)))?;
    }
    if config.auto_migrate {
        db::migrate(&pool, db::PRIMARY)
            .await
            .with_context(|| format!("ScramDB at {}", db::redact(&config.database_url)))?;
    }
    let store = Store::new(pool.clone());
    let project = project::Project::new(&config.project_dir);
    for (file, e) in &project.current().errors {
        tracing::warn!("{file}: {e}");
    }
    let seeded = store.seed_dashboards(&project.dashboards_dir()).await?;
    if seeded > 0 {
        tracing::info!("seeded {seeded} dashboard(s) from {}", project.dashboards_dir().display());
    }
    if let Some((email, password, generated)) =
        config::first_admin(store.user_count().await?, &config.bootstrap_owners, config.initial_password.as_deref())
    {
        let hash = auth::password::hash(&password)?;
        store.set_password(&email, None, &hash, None, true).await?;
        if generated {
            tracing::warn!(
                "first start: sign in as {email} with the one-time password {password} (you'll choose your own)"
            );
        } else {
            tracing::info!(
                "first start: {email} can sign in with MAGNUM_INITIAL_PASSWORD (once: they'll choose their own)"
            );
        }
    }
    if store.user_count().await? == 0 {
        tracing::warn!("no accounts yet: create one with `magnum-bi user set-password you@example.com`");
    }
    if config.bootstrap_owners.is_empty() && store.setting("platform").await?.is_none() {
        tracing::warn!("no system owners yet: set MAGNUM_SYSTEM_OWNERS=you@example.com");
    }
    tracing::info!(
        "storage: ScramDB at {}; sign-in: {}",
        db::redact(&config.database_url),
        match (config.password_login, config.google.is_some()) {
            (true, true) => "password + Google",
            (true, false) => "password",
            (false, true) => "Google",
            (false, false) => "none configured",
        }
    );
    // organisations' connections never reach the platform's own databases
    let mut guard = config.connection_guard();
    let org_dbs = store.org_databases().await?;
    if config.secret_key.is_none() && !org_dbs.is_empty() {
        tracing::warn!(
            "{} organisation database(s) registered but MAGNUM_SECRET_KEY isn't set: those organisations \
             will get 503s (set the key they were saved with, e.g. in .env.local)",
            org_dbs.len()
        );
    }
    for (org, db) in org_dbs {
        let shown = db["display"].as_str().and_then(|d| url::Url::parse(d).ok());
        match shown.and_then(|u| Some(format!("{}:{}", u.host_str()?, u.port().unwrap_or(5432)))) {
            Some(endpoint) => guard.internal.push(endpoint),
            None => tracing::warn!(org, "can't tell where this organisation's database is; not guarded against"),
        }
    }
    if guard.allow_private {
        tracing::warn!("organisations' connections may reach private networks (MAGNUM_ALLOW_PRIVATE_CONNECTIONS)");
    }
    let google = config.google.clone().map(auth::google::Google::new);
    Ok(Arc::new(AppState {
        orgs: orgdb::OrgDbs::new(store.clone(), config.secret_key.clone()),
        usage: usage::Usage::start(store.clone(), config.usage_retention_days),
        store,
        project,
        access: state::AccessCache::new(),
        connectors: connectors::Connectors::new(
            config.secret_key.clone(),
            config.duckdb_dir.clone(),
            config.duckdb_install_extensions,
        )
        .with_guard(guard),
        google,
        limiter: auth::password::LoginLimiter::default(),
        ip_limiter: auth::password::LoginLimiter::new(20),
        hashing: tokio::sync::Semaphore::new(state::hashing_permits()),
        blends: state::OrgBlends::default(),
        themes: Default::default(),
        oidc: crate::auth::oidc::Oidc::new(),
        passkeys: crate::auth::passkeys::relying_party(&config),
        demo: Default::default(),
        flights: Default::default(),
        config,
    }))
}

async fn shutdown() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! { _ = ctrl_c => {}, _ = term => {} }
}
