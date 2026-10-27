//! Magnum BI: a lightweight BI tool. Semantic models and dashboards as YAML, queries on ScramDB,
//! a React UI served from the same address.
// Windows: the desktop build is an app (no console window behind it); the server build is a command
#![cfg_attr(all(windows, feature = "desktop"), windows_subsystem = "windows")]

mod access;
mod api;
#[cfg(test)]
mod attack;
mod auth;
mod blend;
mod cache;
mod chat;
mod code_cli;
mod compiler;
mod config;
mod connections;
mod connectors;
mod crypto;
mod db;
mod delivery;
mod demo;
#[cfg(feature = "desktop")]
mod desktop;
/// The built UI, in desktop builds (build.rs); the server serves frontend/dist from disk.
#[cfg(feature = "desktop")]
mod ui {
    include!(concat!(env!("OUT_DIR"), "/ui.rs"));
}
#[cfg(not(feature = "desktop"))]
mod ui {
    pub static FILES: &[(&str, &[u8])] = &[];
}
mod error;
mod export;
mod formula;
mod governance;
mod lineage;
mod lite;
mod materialize;
mod netguard;
mod org_models;
mod orgdb;
mod period;
mod project;
mod results;
mod sample;
mod schedule;
mod schema;
mod security;
mod setup;
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
    /// An organisation's setup as code, from a running server (an API token in MAGNUM_TOKEN;
    /// secrets in MAGNUM_SECRET_<FILE>, e.g. MAGNUM_SECRET_CONNECTIONS_WAREHOUSE)
    Code {
        #[command(subcommand)]
        command: CodeCommand,
        /// the server, e.g. https://bi.example.com
        #[arg(long, env = "MAGNUM_URL", global = true)]
        url: Option<String>,
        /// the organisation (default: the token's)
        #[arg(long, env = "MAGNUM_ORG", global = true)]
        org: Option<String>,
    },
    /// Magnum BI lite in a window of its own (built with --features desktop)
    #[cfg(feature = "desktop")]
    Desktop {
        /// where it keeps everything (default: ~/.magnum-bi)
        #[arg(long)]
        dir: Option<std::path::PathBuf>,
    },
    /// Magnum BI for one person on this computer: its own PostgreSQL (fetched the first time), no
    /// organisations, accounts or sign-in, reachable from this computer only
    Lite {
        /// default: a free one, picked the first time and kept
        #[arg(long)]
        port: Option<u16>,
        /// where it keeps everything (default: ~/.magnum-bi)
        #[arg(long)]
        dir: Option<std::path::PathBuf>,
        /// don't open the browser
        #[arg(long)]
        no_open: bool,
        /// lock it with a PIN (asked for here), then exit; it asks for the PIN from the next start
        #[arg(long, conflicts_with = "remove_pin")]
        set_pin: bool,
        /// remove the PIN, then exit
        #[arg(long)]
        remove_pin: bool,
    },
}

#[derive(Subcommand)]
enum CodeCommand {
    /// Write the organisation's setup into a folder, as YAML files
    Export { dir: std::path::PathBuf },
    /// Show what applying the folder would change (nothing is changed)
    Plan {
        dir: std::path::PathBuf,
        /// also delete what the folder doesn't have
        #[arg(long)]
        prune: bool,
    },
    /// Apply the folder, after showing the plan and asking
    Apply {
        dir: std::path::PathBuf,
        #[arg(long)]
        prune: bool,
        /// don't ask (scripts and CI)
        #[arg(long)]
        yes: bool,
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

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("MAGNUM_LOG")
                .unwrap_or_else(|_| "magnum_bi=info,tower_http=warn".into()),
        )
        .init();
    let cli = Cli::parse();
    // the window runs on the main thread (macOS insists), the server beside it
    #[cfg(feature = "desktop")]
    match &cli.command {
        // the installed app, opened from the menu or by a double click: its window
        None => return desktop::run(lite_dir(None)?),
        Some(Command::Desktop { dir }) => return desktop::run(lite_dir(dir.clone())?),
        _ => {}
    }
    tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(run(cli))
}

/// Where lite keeps everything: the folder asked for, else ~/.magnum-bi.
fn lite_dir(dir: Option<std::path::PathBuf>) -> Result<std::path::PathBuf> {
    match dir {
        Some(d) => Ok(d),
        None => Ok(std::env::home_dir().context("no home folder: say where with --dir")?.join(".magnum-bi")),
    }
}

async fn run(cli: Cli) -> Result<()> {
    // a client of another server: none of this one's settings
    if let Some(Command::Code { command, url, org }) = cli.command {
        return code(command, url, org).await;
    }
    // its own database and settings
    if let Some(Command::Lite { port, dir, no_open, set_pin, remove_pin }) = cli.command {
        let dir = lite_dir(dir)?;
        if set_pin || remove_pin {
            return lite_pin(&dir, set_pin);
        }
        let _one = lite::lock(&dir)?;
        let open = move |address: &str| {
            if !no_open {
                lite::open_browser(address);
            }
        };
        return lite(port, dir, open, shutdown()).await;
    }
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
        Command::Code { .. } | Command::Lite { .. } => unreachable!("handled above"),
        #[cfg(feature = "desktop")]
        Command::Desktop { .. } => unreachable!("handled above"),
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
    serve_until(config, shutdown(), |_| {}).await
}

/// `serve`, until `stop`; `listening` hears the address once requests can come.
async fn serve_until(
    config: Config,
    stop: impl Future<Output = ()> + Send + 'static,
    listening: impl FnOnce(&str),
) -> Result<()> {
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
    listening(&addr);
    // scheduled reports: what's due, every minute (each run is claimed, so replicas don't repeat it)
    let reports = state.clone();
    tokio::spawn(async move {
        let mut every = tokio::time::interval(std::time::Duration::from_secs(60));
        // a slow round (many organisations, a slow mail server) isn't followed by a burst of them
        every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            every.tick().await;
            api::run_scheduled_reports(&reports).await;
        }
    });
    // the query cache: expired results and each organisation's row budget, every 5 minutes
    let caches = state.clone();
    tokio::spawn(async move {
        let mut every = tokio::time::interval(std::time::Duration::from_secs(300));
        // a slow round (many organisations, big caches) isn't followed by a burst of them
        every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            every.tick().await;
            api::tidy_caches(&caches).await;
        }
    });
    let app = api::router(state).into_make_service_with_connect_info::<std::net::SocketAddr>();
    axum::serve(listener, app).with_graceful_shutdown(stop).await?;
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
        // lite: the platform workspace is the only one, and keeps what an organisation's database
        // would (its query cache among them) in the one database
        if config.lite {
            db::migrate(&pool, db::ORG).await?;
        }
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
    if config.lite {
        lite::ensure_owner(&store).await?; // no first password: nobody signs in
    } else if let Some((email, password, generated)) =
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
        "storage: {} at {}; sign-in: {}",
        if config.lite { "its own PostgreSQL" } else { "ScramDB" },
        db::redact(&config.database_url),
        match (config.password_login && !config.lite, config.google.is_some() && !config.lite) {
            (true, true) => "password + Google",
            (true, false) => "password",
            (false, true) => "Google",
            (false, false) if config.lite => "none (lite: this computer only)",
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
    // (lite: connecting to this computer's own databases is the point)
    if guard.allow_private && !config.lite {
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
        models: Default::default(),
        formulas: Default::default(),
        governance: Default::default(),
        docs: Default::default(),
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

/// `magnum-bi code …`: export, plan or apply through the server's API.
async fn code(command: CodeCommand, url: Option<String>, org: Option<String>) -> Result<()> {
    let url = url.context("which server? --url or MAGNUM_URL")?;
    let token = std::env::var("MAGNUM_TOKEN").ok().filter(|t| !t.is_empty());
    let token = token.context("an API token in MAGNUM_TOKEN (Security › API tokens; Org admins)")?;
    let remote = code_cli::Remote { url, token, org };
    match command {
        CodeCommand::Export { dir } => {
            let n = code_cli::export(&remote, &dir).await?;
            println!("{n} files in {}", dir.display());
        }
        CodeCommand::Plan { dir, prune } => {
            let plan = code_cli::send(&remote, &dir, prune, false).await?;
            println!("{}", code_cli::describe(&plan));
            if plan["ok"] == false {
                bail!("the plan has problems: fix them, then plan again");
            }
        }
        CodeCommand::Apply { dir, prune, yes } => {
            let plan = code_cli::send(&remote, &dir, prune, false).await?;
            println!("{}", code_cli::describe(&plan));
            if plan["ok"] == false {
                bail!("the plan has problems: nothing was changed");
            }
            if plan["changes"].as_array().is_none_or(|c| c.is_empty()) {
                return Ok(());
            }
            if !yes {
                use std::io::IsTerminal;
                if !std::io::stdin().is_terminal() {
                    bail!("not applied: pass --yes to apply without asking");
                }
                print!("Apply these changes? Type yes: ");
                std::io::Write::flush(&mut std::io::stdout())?;
                let mut answer = String::new();
                std::io::stdin().read_line(&mut answer)?;
                if answer.trim() != "yes" {
                    bail!("not applied");
                }
            }
            code_cli::send(&remote, &dir, prune, true).await?;
            println!("Applied.");
        }
    }
    Ok(())
}

/// `magnum-bi lite`: its own PostgreSQL in the folder, then the server for its one person.
/// `magnum-bi lite` (and the desktop window's server): until `stop`; `ready` hears the address once
/// it can be opened.
async fn lite(
    port: Option<u16>,
    dir: std::path::PathBuf,
    ready: impl FnOnce(&str),
    stop: impl Future<Output = ()> + Send + 'static,
) -> Result<()> {
    let port = lite::port(&dir, port)?;
    println!("Starting Magnum BI lite in {} (the first start downloads PostgreSQL)…", dir.display());
    let (database, url) = lite::database(&dir).await.context("starting its PostgreSQL")?;
    let config = lite::config(&dir, &url, port, |k| std::env::var(k).ok())?;
    std::fs::create_dir_all(&config.project_dir)?;
    let address = format!("http://localhost:{port}");
    let served = serve_until(config, stop, |_| {
        println!("Magnum BI lite: {address} (this computer only; Ctrl+C to stop)");
        ready(&address);
    })
    .await;
    database.stop().await.context("stopping its PostgreSQL")?;
    served
}

/// `magnum-bi lite --set-pin | --remove-pin`: lock lite with a PIN, or unlock it for good.
fn lite_pin(dir: &std::path::Path, set: bool) -> Result<()> {
    if !set {
        lite::set_pin(dir, None)?;
        println!("No PIN any more (from the next start).");
        return Ok(());
    }
    let pin = rpassword::prompt_password("PIN (at least 4 characters): ")?;
    if rpassword::prompt_password("Again: ")? != pin {
        bail!("the two PINs differ: nothing changed");
    }
    lite::set_pin(dir, Some(&pin))?;
    println!("PIN set: Magnum BI lite asks for it from the next start.");
    Ok(())
}
