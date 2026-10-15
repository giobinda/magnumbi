//! Magnum BI: a lightweight BI tool. Semantic models and dashboards as YAML, queries on ScramDB,
//! a React UI served from the same address.

mod access;
mod api;
mod auth;
mod compiler;
mod config;
mod connectors;
mod db;
mod error;
mod period;
mod project;
mod sample;
mod schema;
mod security;
mod state;
mod store;
mod themes;
mod tile_query;

use std::io::{BufRead, IsTerminal};
use std::sync::Arc;

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
    /// Apply database migrations to ScramDB and exit
    Migrate,
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
            let n = db::migrate(&pool).await?;
            println!("{n} migration(s) applied to {}", db::redact(&config.database_url));
            Ok(())
        }
        Command::SeedSample { rows } => {
            let pool = db::make_pool(&config.database_url, 2)?;
            let n = sample::seed(&pool, rows).await?;
            println!("{n} rows in {} ({})", sample::TABLE, db::redact(&config.database_url));
            Ok(())
        }
        Command::Check => check(&config),
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
                    .extend(s.validate_dashboard(&d, &all).into_iter().map(|w| format!("dashboards/{name}: {w}"))),
                Err(e) => problems.push(format!("dashboards/{name}: {e}")),
            }
        }
    }
    println!("{} connection(s), {} model(s), {} theme(s)", s.connections.len(), s.models.len(), s.themes.len());
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
    db::migrate(&pool).await?;
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
    for p in &problems {
        tracing::warn!("not suitable for a shared deployment: {p}");
    }
    let pool = db::make_pool(&config.database_url, 16)?;
    if config.auto_migrate {
        db::migrate(&pool).await.with_context(|| format!("ScramDB at {}", db::redact(&config.database_url)))?;
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
    if store.user_count().await? == 0 {
        tracing::warn!("no accounts yet: create one with `magnum-bi user set-password you@example.com`");
    }
    if config.bootstrap_owners.is_empty() && store.setting("platform").await?.is_none() {
        tracing::warn!("no system owners yet: set MAGNUM_SYSTEM_OWNERS=you@example.com");
    }
    let addr = format!("{}:{}", config.host, config.port);
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
    let google = config.google.clone().map(auth::google::Google::new);
    let state = Arc::new(AppState {
        store,
        project,
        access: state::AccessCache::new(),
        connectors: connectors::Connectors::new(pool),
        google,
        limiter: auth::password::LoginLimiter::default(),
        config,
    });
    let listener = tokio::net::TcpListener::bind(&addr).await.with_context(|| format!("can't listen on {addr}"))?;
    tracing::info!("Magnum BI on http://{addr}");
    axum::serve(listener, api::router(state)).with_graceful_shutdown(shutdown()).await?;
    Ok(())
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
