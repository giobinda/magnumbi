//! Lite: Magnum BI for one person on their own computer (`magnum-bi lite`). No organisations, no
//! accounts, no sign-in: the platform workspace, used by one built-in owner. Safe only because it
//! can't be reached from anywhere else: it listens on loopback only and answers only requests
//! addressed to localhost (a page elsewhere can't reach it by pointing its own name at
//! 127.0.0.1). Switched on by the command only, never by a setting.

use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};

use anyhow::Context;

use crate::config::Config;
use crate::store::Store;

/// The one person of a lite installation (a system owner of the platform workspace).
pub const OWNER: &str = "you@magnum-bi.local";

/// Make sure the owner's account exists (it has no password: nobody signs in as it).
pub async fn ensure_owner(store: &Store) -> anyhow::Result<()> {
    // nor a password to change: there's no sign-in to change it at
    if store.user(OWNER).await?.is_none_or(|u| u.must_change_password) {
        // a password nobody knows: the account is used without signing in, on this computer only
        let hash = crate::auth::password::hash(&crate::store::random_token(32))?;
        store.set_password(OWNER, Some("You"), &hash, None, false).await?;
    }
    Ok(())
}

/// The owner, acting without signing in (lite only).
pub async fn session(store: &Store) -> anyhow::Result<Option<crate::store::SessionInfo>> {
    let Some(user) = store.user(OWNER).await? else { return Ok(None) };
    Ok(Some(crate::store::SessionInfo { method: "local".into(), user, granted_role: None, token: None }))
}

/// Lite only listens on loopback.
pub fn check_bind(host: &str) -> Result<(), String> {
    let loopback = host == "localhost" || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback());
    if loopback { Ok(()) } else { Err(format!("lite listens on this computer only (127.0.0.1), not {host:?}")) }
}

/// In lite, a request addressed to anything but localhost is refused.
pub fn host_rejection(config: &Config, headers: &HeaderMap) -> Option<Response> {
    if !config.lite {
        return None;
    }
    let host = headers.get(axum::http::header::HOST).and_then(|h| h.to_str().ok()).unwrap_or_default().to_lowercase();
    // the name without its port ("[::1]:8000" -> "[::1]", "localhost:8000" -> "localhost")
    let name = match host.rfind(':') {
        Some(i) if !host[i..].contains(']') => &host[..i],
        _ => host.as_str(),
    };
    if matches!(name, "localhost" | "127.0.0.1" | "[::1]") {
        return None;
    }
    let body =
        serde_json::json!({"detail": "Magnum BI lite answers only on this computer: open it at http://localhost"});
    Some((axum::http::StatusCode::FORBIDDEN, axum::Json(body)).into_response())
}

/// What needs other people (organisations, accounts and how they sign in, links for people
/// without one) isn't in lite. The person's own page (`/api/auth/me`) and signing out stay.
pub fn route_rejection(config: &Config, path: &str) -> Option<Response> {
    if !config.lite {
        return None;
    }
    let parts: Vec<&str> = path.trim_start_matches("/api/").split('/').collect();
    let gone = match parts.as_slice() {
        ["platform", "demo"] | ["auth", "me" | "logout"] => false,
        ["platform", ..] | ["members", ..] | ["auth", ..] | ["public", ..] | ["guest", ..] => true,
        ["share-links" | "guest-links", ..] | ["dashboards", _, "share-links" | "guest-links", ..] => true,
        _ => false,
    };
    let body = serde_json::json!({"detail": "not in Magnum BI lite: it has no organisations, accounts or sign-in"});
    gone.then(|| (axum::http::StatusCode::NOT_FOUND, axum::Json(body)).into_response())
}

/// The settings of a lite server: everything in `dir`, on this computer only, as the owner.
/// Other settings (MAGNUM_...) still apply, except where it listens and who owns it.
pub fn config(
    dir: &std::path::Path,
    database_url: &str,
    port: u16,
    env: impl Fn(&str) -> Option<String>,
) -> anyhow::Result<Config> {
    let path = |p: &str| dir.join(p).display().to_string();
    let fixed = [
        ("MAGNUM_DATABASE_URL", database_url.to_string()),
        ("MAGNUM_HOST", "127.0.0.1".to_string()),
        ("MAGNUM_PORT", port.to_string()),
        ("MAGNUM_SYSTEM_OWNERS", OWNER.to_string()),
        ("MAGNUM_SECRET_KEY_FILE", path("secret.key")),
        ("MAGNUM_PROJECT_DIR", path("project")),
        ("MAGNUM_DUCKDB_DIR", path("duckdb")),
        // the rules for a server shared with others don't apply to one person's computer
        ("MAGNUM_ENV", "development".to_string()),
    ];
    let lookup = |k: &str| fixed.iter().find(|(n, _)| *n == k).map(|(_, v)| v.clone()).or_else(|| env(k));
    let mut c = Config::from_lookup(lookup)?;
    c.lite = true;
    // one person's own spreadsheets, on their own disk (MAGNUM_UPLOAD_MAX_MB still decides, if set)
    if env("MAGNUM_UPLOAD_MAX_MB").is_none() {
        c.upload_max_bytes = 2048 << 20;
    }
    c.lite_pin = std::fs::read_to_string(dir.join("pin")).ok().map(|h| h.trim().to_string()).filter(|h| !h.is_empty());
    check_bind(&c.host).map_err(anyhow::Error::msg)?;
    Ok(c)
}

/// Lock lite with a PIN (`None`: remove it). Only its hash is kept, in `dir`, by its owner only.
pub fn set_pin(dir: &std::path::Path, pin: Option<&str>) -> anyhow::Result<()> {
    use std::io::Write;
    let path = dir.join("pin");
    let Some(pin) = pin else {
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        return Ok(());
    };
    if pin.chars().count() < 4 {
        anyhow::bail!("a PIN has at least 4 characters");
    }
    std::fs::create_dir_all(dir)?;
    let hash = crate::auth::password::hash(pin)?;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    opts.open(&path)?.write_all(hash.as_bytes())?;
    Ok(())
}

/// Holds `dir` for this lite until dropped: a second one on the same folder (opened twice) would
/// fight over its PostgreSQL and its port, so it is told where the first one is instead. The folder
/// becomes its owner's only, on every start (one made before, by hand or with --dir, too): it holds
/// the database, its password and everything shown.
pub fn lock(dir: &std::path::Path) -> anyhow::Result<std::fs::File> {
    std::fs::create_dir_all(dir)?;
    owner_only(dir, 0o700)?;
    let file = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(dir.join("lock"))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => {
            let port = std::fs::read_to_string(dir.join("port")).unwrap_or_default();
            anyhow::bail!("Magnum BI lite is already open from {}: http://localhost:{}", dir.display(), port.trim())
        }
        Err(std::fs::TryLockError::Error(e)) => Err(e.into()),
    }
}

/// Readable (and, for a folder, enterable) by its owner only. Windows: a user's own folders are
/// theirs already.
fn owner_only(path: &std::path::Path, mode: u32) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
    Ok(())
}

/// The port lite listens on: the one asked for, else the one it had last time (kept in `dir`:
/// the browser or window keeps preferences per address), else a free one picked now. A port
/// something else has taken since is replaced.
pub fn port(dir: &std::path::Path, asked: Option<u16>) -> anyhow::Result<u16> {
    if let Some(p) = asked {
        return Ok(p);
    }
    let free = |p: u16| std::net::TcpListener::bind(("127.0.0.1", p)).is_ok();
    let path = dir.join("port");
    let kept = std::fs::read_to_string(&path).ok().and_then(|p| p.trim().parse::<u16>().ok());
    if let Some(p) = kept.filter(|p| free(*p)) {
        return Ok(p);
    }
    let p = std::net::TcpListener::bind(("127.0.0.1", 0))?.local_addr()?.port();
    std::fs::create_dir_all(dir)?;
    std::fs::write(&path, p.to_string())?;
    Ok(p)
}

/// The password of lite's own PostgreSQL: made the first time, kept in `dir`, readable by its
/// owner only (the database keeps the one it was set up with).
pub fn database_password(dir: &std::path::Path) -> anyhow::Result<String> {
    use std::io::Write;
    let path = dir.join("database.password");
    if !path.exists() {
        std::fs::create_dir_all(dir)?;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
        opts.open(&path)?.write_all(crate::store::random_token(32).as_bytes())?;
    }
    Ok(std::fs::read_to_string(&path)?.trim().to_string())
}

/// Lite's own PostgreSQL in `dir`, started: fetched the first time (version 16, the one Magnum BI
/// is tested on; a data folder only opens with its own major version), on this computer only.
/// Returns it (stop it when done) and its address.
pub async fn database(
    dir: &std::path::Path,
    progress: &(dyn Fn(u64, u64) + Sync),
) -> anyhow::Result<(postgresql_embedded::PostgreSQL, String)> {
    let version = match installed_version(dir) {
        Some(v) => v,
        None => download(dir, progress).await.context("downloading its PostgreSQL")?,
    };
    let mut s = postgresql_embedded::Settings::new();
    // exactly the one installed: nothing asked of the network once it's there
    s.version = postgresql_embedded::VersionReq::parse(&format!("={version}"))?;
    s.installation_dir = dir.join("postgresql").join(version.to_string());
    s.data_dir = dir.join("data");
    s.password_file = dir.join(".pgpass");
    s.password = database_password(dir)?;
    s.host = "127.0.0.1".into();
    s.temporary = false;
    s.timeout = Some(std::time::Duration::from_secs(120)); // the first start sets up its data
    s.configuration.insert("listen_addresses".into(), "127.0.0.1".into());
    let mut pg = postgresql_embedded::PostgreSQL::new(s);
    pg.setup().await?;
    // the library writes the database password to .pgpass readable by everyone
    owner_only(&dir.join(".pgpass"), 0o600)?;
    // one left running in this folder (the program killed, or crashed) is no other lite's: the
    // folder is locked (`lock`). Stop it, or it holds the data and this start fails.
    if pg.status() == postgresql_embedded::Status::Started {
        tracing::info!("stopping the PostgreSQL left running in {}", dir.display());
        pg.stop().await?;
    }
    pg.start().await?;
    if !pg.database_exists("magnum").await? {
        pg.create_database("magnum").await?;
    }
    let url = pg.settings().url("magnum");
    Ok((pg, url))
}

/// Where its PostgreSQL builds come from (as postgresql_embedded's own).
const ARCHIVES: &str = postgresql_archive::configuration::theseus::URL;

pub const MB: u64 = 1024 * 1024;

/// The newest PostgreSQL 16 installed in `dir` (a folder per version; `.partial`: still being
/// extracted, or left half-done).
fn installed_version(dir: &std::path::Path) -> Option<postgresql_embedded::Version> {
    std::fs::read_dir(dir.join("postgresql"))
        .ok()?
        .filter_map(|e| postgresql_embedded::Version::parse(&e.ok()?.file_name().to_string_lossy()).ok())
        .filter(|v| v.major == 16 && v.pre.is_empty())
        .max()
}

/// The archive of one version, for this computer.
fn archive_url(version: &postgresql_embedded::Version) -> String {
    let target = postgresql_archive::matcher::target();
    format!("{ARCHIVES}/releases/download/{version}/postgresql-{version}-{target}.tar.gz")
}

/// What the first start shows while it downloads (`total` 0: not known).
pub fn downloading(done: u64, total: u64) -> String {
    if total == 0 {
        format!("Downloading PostgreSQL: {} MB", done / MB)
    } else {
        format!("Downloading PostgreSQL: {} of {} MB", done / MB, total / MB)
    }
}

/// The first start: the newest PostgreSQL 16, downloaded (telling `progress` the bytes so far and
/// the total), checked against its published SHA-256, and extracted into `dir`.
async fn download(
    dir: &std::path::Path,
    progress: &(dyn Fn(u64, u64) + Sync),
) -> anyhow::Result<postgresql_embedded::Version> {
    use sha2::Digest;
    let version = postgresql_archive::get_version(ARCHIVES, &postgresql_embedded::VersionReq::parse("=16")?).await?;
    let url = archive_url(&version);
    let http = reqwest::Client::new();
    let mut response = http.get(&url).send().await?.error_for_status()?;
    let total = response.content_length().unwrap_or(0);
    let mut bytes = Vec::with_capacity(total as usize);
    progress(0, total);
    // told once a megabyte, and at the end: enough for a progress bar
    let mut told = 0;
    while let Some(chunk) = response.chunk().await? {
        bytes.extend_from_slice(&chunk);
        let done = bytes.len() as u64;
        if done / MB > told || done == total {
            told = done / MB;
            progress(done, total);
        }
    }
    let published = http.get(format!("{url}.sha256")).send().await?.error_for_status()?.text().await?;
    let published = published.split_whitespace().next().unwrap_or_default().to_ascii_lowercase();
    let got = format!("{:x}", sha2::Sha256::digest(&bytes));
    anyhow::ensure!(got == published, "its PostgreSQL archive doesn't match its published SHA-256: try again");
    // extracted next to where it goes, then moved there: never half there under its own name
    let installed = dir.join("postgresql").join(version.to_string());
    let partial = dir.join("postgresql").join(format!("{version}.partial"));
    let _ = std::fs::remove_dir_all(&partial);
    postgresql_archive::extract(ARCHIVES, &bytes, &partial).await?;
    std::fs::rename(&partial, &installed)?;
    Ok(version)
}

/// Open the app in the person's browser (best effort: the address is printed too).
pub fn open_browser(url: &str) {
    let opened = if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(url).spawn()
    } else if cfg!(windows) {
        // not `cmd /C start`: cmd would read a `&` in the address as another command
        std::process::Command::new("rundll32.exe").args(["url.dll,FileProtocolHandler", url]).spawn()
    } else {
        std::process::Command::new("xdg-open").arg(url).spawn()
    };
    if let Err(e) = opened {
        tracing::debug!("couldn't open a browser: {e}");
    }
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use serde_json::{Value, json};

    use super::*;
    use crate::api::tests::{app, app_lite};

    async fn ask(app: &axum::Router, host: Option<&str>, method: &str, path: &str) -> (StatusCode, Value) {
        let mut req = axum::http::Request::builder().method(method).uri(path);
        if let Some(h) = host {
            req = req.header("host", h);
        }
        let resp = tower::ServiceExt::oneshot(app.clone(), req.body(axum::body::Body::empty()).unwrap()).await.unwrap();
        let status = resp.status();
        let bytes = http_body_util::BodyExt::collect(resp.into_body()).await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    #[tokio::test]
    async fn lite_is_the_local_owner_without_signing_in() {
        let Some((app, _)) = app_lite().await else { return };
        let (s, me) = ask(&app, Some("localhost:8000"), "GET", "/api/auth/me").await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!((&me["authenticated"], &me["user"]["email"]), (&json!(true), &json!(OWNER)), "{me}");
        let (s, d) = ask(&app, Some("127.0.0.1:8000"), "GET", "/api/dashboards").await;
        assert_eq!(s, StatusCode::OK, "{d}");
    }

    #[tokio::test]
    async fn lite_answers_only_requests_addressed_to_localhost() {
        let Some((app, _)) = app_lite().await else { return };
        for host in ["localhost", "localhost:8000", "127.0.0.1:8000", "[::1]:8000"] {
            assert_eq!(ask(&app, Some(host), "GET", "/api/health").await.0, StatusCode::OK, "{host}");
        }
        // a page whose own name points at 127.0.0.1 sends its own name
        for host in [Some("evil.example"), Some("evil.example:8000"), Some("localhost.evil.example"), None] {
            let (s, _) = ask(&app, host, "GET", "/api/dashboards").await;
            assert_eq!(s, StatusCode::FORBIDDEN, "{host:?}");
        }
    }

    #[tokio::test]
    async fn the_server_still_asks_everyone_to_sign_in() {
        let Some((app, _, _)) = app().await else { return };
        assert_eq!(ask(&app, Some("localhost"), "GET", "/api/dashboards").await.0, StatusCode::UNAUTHORIZED);
        let (_, me) = ask(&app, Some("localhost"), "GET", "/api/auth/me").await;
        assert_eq!(me["authenticated"], json!(false));
    }

    async fn query(app: &axum::Router, cookie: Option<&str>) -> Value {
        let q = json!({"model": "sales", "dimensions": ["region"], "measures": ["revenue"]});
        let mut req = axum::http::Request::post("/api/query")
            .header("host", "localhost")
            .header("content-type", "application/json");
        if let Some(c) = cookie {
            req = req.header("cookie", c);
        }
        let resp = tower::ServiceExt::oneshot(app.clone(), req.body(axum::body::Body::from(q.to_string())).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = http_body_util::BodyExt::collect(resp.into_body()).await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    /// Whether asking again comes from the cache (it's filled in the background: given a moment).
    async fn cached_again(app: &axum::Router, cookie: Option<&str>) -> bool {
        query(app, cookie).await;
        for _ in 0..50 {
            if query(app, cookie).await["stats"]["cache_hit"] == json!(true) {
                return true;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        false
    }

    #[tokio::test]
    async fn lite_caches_results_in_the_platform_workspace() {
        let Some((lite, _)) = app_lite().await else { return };
        assert!(cached_again(&lite, None).await, "a model with cache: is cached in lite");
        let (s, stats) = ask(&lite, Some("localhost"), "GET", "/api/cache").await;
        assert_eq!(s, StatusCode::OK, "the cache's own page works: {stats}");
        let (s, cleared) = ask(&lite, Some("localhost"), "DELETE", "/api/cache").await;
        assert_eq!(s, StatusCode::OK, "{cleared}");
        // the online platform workspace holds templates: not cached there
        let Some((online, state, cookie)) = app().await else { return };
        let owners = json!({"system_owners": [crate::api::tests::OWNER]});
        state.access.pin(crate::access::AccessConfig::from_document(owners).unwrap());
        assert!(!cached_again(&online, Some(&cookie)).await);
    }

    #[tokio::test]
    async fn what_needs_other_people_isnt_in_lite() {
        let Some((lite, _)) = app_lite().await else { return };
        for (method, path) in [
            ("GET", "/api/platform"),
            ("PUT", "/api/platform/organisations/acme/database"),
            ("POST", "/api/platform/dashboards/d/publish"),
            ("GET", "/api/members"),
            ("GET", "/api/members/sign-in"),
            ("POST", "/api/auth/login"),
            ("POST", "/api/auth/password"),
            ("POST", "/api/auth/two-factor/setup"),
            ("GET", "/api/auth/passkeys"),
            ("GET", "/api/auth/google/login"),
            ("GET", "/api/dashboards/d/share-links"),
            ("GET", "/api/dashboards/d/guest-links"),
            ("GET", "/api/public/x/y"),
            ("GET", "/api/guest/x/y"),
        ] {
            let (s, body) = ask(&lite, Some("localhost"), method, path).await;
            assert_eq!(s, StatusCode::NOT_FOUND, "{method} {path}: {body}");
            assert!(body["detail"].as_str().unwrap_or_default().contains("lite"), "{method} {path}: {body}");
        }
        // the rest is there, the person's own page included
        for path in ["/api/auth/me", "/api/platform/demo", "/api/tokens", "/api/code", "/api/connections"] {
            assert_eq!(ask(&lite, Some("localhost"), "GET", path).await.0, StatusCode::OK, "{path}");
        }
        let (_, me) = ask(&lite, Some("localhost"), "GET", "/api/auth/me").await;
        assert_eq!(me["lite"], json!(true), "the app hides what isn't there: {me}");
    }

    #[test]
    fn a_lite_server_keeps_everything_in_its_folder_and_to_this_computer() {
        let dir = std::env::temp_dir().join(format!("magnum-lite-{}", uuid::Uuid::new_v4().simple()));
        let env = |k: &str| match k {
            "MAGNUM_HOST" => Some("0.0.0.0".to_string()), // can't widen it
            "MAGNUM_SYSTEM_OWNERS" => Some("someone@else.example".to_string()),
            "MAGNUM_CACHE_MAX_ROWS" => Some("1000".to_string()), // the rest still applies
            _ => None,
        };
        let c = config(&dir, "postgresql://magnum:pw@localhost:5433/magnum", 8123, env).unwrap();
        assert!(c.lite);
        assert_eq!((c.host.as_str(), c.port), ("127.0.0.1", 8123));
        assert_eq!(c.bootstrap_owners, [OWNER]);
        assert_eq!(c.database_url, "postgresql://magnum:pw@localhost:5433/magnum");
        assert_eq!(c.cache_max_rows, 1000);
        assert!(c.duckdb_dir.starts_with(&dir) && c.project_dir.starts_with(&dir));
        assert!(c.secret_key.is_some() && dir.join("secret.key").exists(), "its own key, kept in the folder");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn the_owner_never_has_a_password_to_change() {
        // a folder from an earlier start that made it choose one: there's no sign-in to choose it at
        let Some((lite, state)) = app_lite().await else { return };
        let hash = crate::auth::password::hash("one-time").unwrap();
        state.store.set_password(OWNER, None, &hash, None, true).await.unwrap();
        let (s, body) = ask(&lite, Some("localhost"), "GET", "/api/dashboards").await;
        assert_eq!(s, StatusCode::OK, "never stuck on a password it can't change: {body}");
        ensure_owner(&state.store).await.unwrap();
        assert!(!state.store.user(OWNER).await.unwrap().unwrap().must_change_password, "and cleared at start");
    }

    async fn post(
        app: &axum::Router,
        path: &str,
        body: Value,
        cookie: Option<&str>,
    ) -> (StatusCode, Value, Option<String>) {
        let mut req =
            axum::http::Request::post(path).header("host", "localhost").header("content-type", "application/json");
        if let Some(c) = cookie {
            req = req.header("cookie", c);
        }
        let resp = tower::ServiceExt::oneshot(app.clone(), req.body(axum::body::Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let cookie = resp
            .headers()
            .get("set-cookie")
            .and_then(|v| v.to_str().ok())
            .map(|v| v.split(';').next().unwrap().to_string());
        let bytes = http_body_util::BodyExt::collect(resp.into_body()).await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null), cookie)
    }

    async fn get_with(app: &axum::Router, path: &str, cookie: &str) -> StatusCode {
        let req = axum::http::Request::get(path).header("host", "localhost").header("cookie", cookie);
        tower::ServiceExt::oneshot(app.clone(), req.body(axum::body::Body::empty()).unwrap()).await.unwrap().status()
    }

    #[tokio::test]
    async fn a_pin_locks_lite_until_it_is_given() {
        let Some((lite, _)) = crate::api::tests::app_lite_with_pin(Some("4321")).await else { return };
        assert_eq!(ask(&lite, Some("localhost"), "GET", "/api/dashboards").await.0, StatusCode::UNAUTHORIZED);
        let (_, me) = ask(&lite, Some("localhost"), "GET", "/api/auth/me").await;
        assert_eq!(
            (&me["authenticated"], &me["lite"], &me["pin"]),
            (&json!(false), &json!(true), &json!(true)),
            "{me}"
        );

        let (s, body, cookie) = post(&lite, "/api/lite/unlock", json!({"pin": "1111"}), None).await;
        assert_eq!((s, cookie), (StatusCode::BAD_REQUEST, None), "{body}");
        let (s, _, cookie) = post(&lite, "/api/lite/unlock", json!({"pin": "4321"}), None).await;
        assert_eq!(s, StatusCode::OK);
        let cookie = cookie.expect("a session for the browser");
        assert_eq!(get_with(&lite, "/api/dashboards", &cookie).await, StatusCode::OK);
        // locking again ends it
        assert_eq!(post(&lite, "/api/auth/logout", json!({}), Some(&cookie)).await.0, StatusCode::OK);
        assert_eq!(get_with(&lite, "/api/dashboards", &cookie).await, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn guessing_the_pin_locks_it() {
        let Some((lite, _)) = crate::api::tests::app_lite_with_pin(Some("4321")).await else { return };
        let mut last = StatusCode::OK;
        for _ in 0..12 {
            last = post(&lite, "/api/lite/unlock", json!({"pin": "0000"}), None).await.0;
        }
        assert_eq!(last, StatusCode::TOO_MANY_REQUESTS);
        let (s, _, cookie) = post(&lite, "/api/lite/unlock", json!({"pin": "4321"}), None).await;
        assert_eq!((s, cookie), (StatusCode::TOO_MANY_REQUESTS, None), "not even the right one, for a while");
    }

    #[tokio::test]
    async fn without_a_pin_there_is_nothing_to_unlock() {
        let Some((lite, _)) = app_lite().await else { return };
        assert_eq!(post(&lite, "/api/lite/unlock", json!({"pin": "4321"}), None).await.0, StatusCode::NOT_FOUND);
        let Some((online, _, _)) = app().await else { return };
        assert_eq!(post(&online, "/api/lite/unlock", json!({"pin": "4321"}), None).await.0, StatusCode::NOT_FOUND);
    }

    #[test]
    fn a_pin_is_kept_as_a_hash_by_its_owner_only() {
        let dir = std::env::temp_dir().join(format!("magnum-lite-{}", uuid::Uuid::new_v4().simple()));
        assert!(set_pin(&dir, Some("12")).is_err(), "at least 4 characters");
        set_pin(&dir, Some("4321")).unwrap();
        let stored = std::fs::read_to_string(dir.join("pin")).unwrap();
        assert!(!stored.contains("4321") && crate::auth::password::verify("4321", Some(stored.trim())));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(dir.join("pin")).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let c = config(&dir, "postgresql://x@localhost/m", 8000, |_| None).unwrap();
        assert!(c.lite_pin.is_some(), "a lite server started in the folder asks for it");
        set_pin(&dir, None).unwrap();
        assert!(config(&dir, "postgresql://x@localhost/m", 8000, |_| None).unwrap().lite_pin.is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn getting_started_is_about_data_not_organisations() {
        let Some((lite, _)) = app_lite().await else { return };
        let (s, r) = ask(&lite, Some("localhost"), "GET", "/api/setup").await;
        assert_eq!(s, StatusCode::OK);
        let ids: Vec<&str> = r["steps"].as_array().unwrap().iter().map(|s| s["id"].as_str().unwrap()).collect();
        // sample data first, to look around before connecting one's own
        assert_eq!(ids, ["demo", "connection", "model", "dashboard", "theme", "delivery", "agent"], "{r}");
        assert_eq!(r["steps"][0]["optional"], json!(true), "{r}");
    }

    #[test]
    fn an_installed_postgresql_is_used_without_asking_the_network() {
        let dir = std::env::temp_dir().join(format!("magnum-lite-{}", uuid::Uuid::new_v4().simple()));
        assert_eq!(installed_version(&dir), None, "nothing yet: it's downloaded");
        for d in ["16.4.0", "16.15.1", "16.9.0.partial", "15.8.0", "notes"] {
            std::fs::create_dir_all(dir.join("postgresql").join(d)).unwrap();
        }
        // the newest PostgreSQL 16 there (never one half extracted)
        assert_eq!(installed_version(&dir), Some(postgresql_embedded::Version::new(16, 15, 1)));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn its_postgresql_comes_from_the_theseus_builds() {
        let url = archive_url(&postgresql_embedded::Version::new(16, 15, 1));
        let target = postgresql_archive::matcher::target();
        assert_eq!(
            url,
            format!(
                "https://github.com/theseus-rs/postgresql-binaries/releases/download/16.15.1/postgresql-16.15.1-{target}.tar.gz"
            )
        );
    }

    #[test]
    fn the_download_says_how_far_it_is() {
        assert_eq!(downloading(12 * MB, 31 * MB), "Downloading PostgreSQL: 12 of 31 MB");
        assert_eq!(downloading(5 * MB, 0), "Downloading PostgreSQL: 5 MB");
    }

    /// Needs its PostgreSQL (downloaded): MAGNUM_TEST_LITE_DATABASE=1.
    #[tokio::test]
    async fn a_postgresql_left_running_is_stopped_before_starting() {
        if std::env::var_os("MAGNUM_TEST_LITE_DATABASE").is_none() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("magnum-lite-{}", uuid::Uuid::new_v4().simple()));
        let (left, _) = database(&dir, &|_, _| {}).await.unwrap();
        // the program went away without stopping it (killed, crashed, an older build)
        std::mem::forget(left);
        let (pg, url) = database(&dir, &|_, _| {}).await.expect("starts over the one left running");
        // its password, as the PostgreSQL library writes it, readable by its owner only
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join(".pgpass")).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "{mode:o}");
        }
        let (client, conn) = tokio_postgres::connect(&url, tokio_postgres::NoTls).await.unwrap();
        tokio::spawn(conn);
        client.simple_query("SELECT 1").await.unwrap();
        pg.stop().await.unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn its_folder_is_its_owners_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("magnum-lite-{}", uuid::Uuid::new_v4().simple()));
        // one made before (or by hand, or with --dir) open to everyone
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        let _one = lock(&dir).unwrap();
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "{mode:o}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn one_lite_at_a_time_per_folder() {
        let dir = std::env::temp_dir().join(format!("magnum-lite-{}", uuid::Uuid::new_v4().simple()));
        let p = port(&dir, None).unwrap();
        let first = lock(&dir).unwrap();
        let again = lock(&dir).unwrap_err().to_string();
        assert!(again.contains("already open") && again.contains(&format!("http://localhost:{p}")), "{again}");
        // another folder is another lite
        let other = dir.join("other");
        let _other = lock(&other).unwrap();
        drop(first);
        lock(&dir).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_port_is_picked_once_and_kept_unless_taken() {
        let dir = std::env::temp_dir().join(format!("magnum-lite-{}", uuid::Uuid::new_v4().simple()));
        let first = port(&dir, None).unwrap();
        assert!(first > 1024, "{first}");
        assert_eq!(port(&dir, None).unwrap(), first, "the same address next time");
        // something else took it: another one, kept from then on
        let taken = std::net::TcpListener::bind(("127.0.0.1", first)).unwrap();
        let second = port(&dir, None).unwrap();
        assert_ne!(second, first);
        drop(taken);
        assert_eq!(port(&dir, None).unwrap(), second);
        // asked for: that one, whatever was kept
        assert_eq!(port(&dir, Some(8123)).unwrap(), 8123);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_database_password_is_made_once_and_kept_private() {
        let dir = std::env::temp_dir().join(format!("magnum-lite-{}", uuid::Uuid::new_v4().simple()));
        let first = database_password(&dir).unwrap();
        assert!(first.len() >= 24);
        assert_eq!(database_password(&dir).unwrap(), first, "the same one next time");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("database.password")).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "its owner only");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn lite_listens_on_loopback_only_and_no_setting_turns_it_on() {
        for ok in ["127.0.0.1", "localhost", "::1"] {
            assert!(check_bind(ok).is_ok(), "{ok}");
        }
        for bad in ["0.0.0.0", "::", "192.168.1.10", "bi.example.com"] {
            assert!(check_bind(bad).is_err(), "{bad}");
        }
        let c = Config::from_lookup(|k| matches!(k, "MAGNUM_LITE" | "LITE").then(|| "1".to_string())).unwrap();
        assert!(!c.lite, "only `magnum-bi lite` turns it on");
    }
}
