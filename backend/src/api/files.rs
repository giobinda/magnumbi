//! Spreadsheets and CSV files as connections (files.rs), a table per sheet, read by DuckDB in the
//! workspace's own folder: `POST /api/connections/{name}/file` uploads a copy, `.../sheet` takes a
//! Google Sheet shared with anyone with the link, `.../refresh` reads that sheet again (as every
//! hour does, `refresh_sheets`).

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, Query as UrlQuery, State};
use serde::Deserialize;
use serde_json::{Value, json};

use super::Body;
use super::dashboards::WriteParams;
use crate::auth::{Auth, Caller};
use crate::connections::{ConnectionConfig, check_name};
use crate::error::{ApiError, ApiResult};
use crate::files::Table;
use crate::state::{AppState, Shared};
use crate::store::Store;
use crate::usage::Event;

#[derive(Deserialize)]
pub struct FileParams {
    /// the file's own name: its kind (.csv, .xlsx, ...), and the table's name for a CSV (checked
    /// once the caller may upload: refusing it first would answer people who may not)
    #[serde(default)]
    filename: Option<String>,
    #[serde(default, deserialize_with = "super::dashboards::flag")]
    dry_run: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SheetBody {
    /// the sheet's link (or its id)
    link: String,
}

/// Where a file connection's tables are, in the workspace's folder (what DuckDB may read).
fn folder(state: &AppState, org: &str, name: &str) -> std::path::PathBuf {
    crate::connectors::duckdb::org_dir(&state.config.duckdb_dir, org).join("files").join(name)
}

/// Where it comes from: an uploaded file's name, or a Google Sheet's id.
enum Source {
    File(String),
    Sheet(String),
}

/// Upload a spreadsheet or CSV as the connection `name` (new, or replacing the file it had): a
/// table per sheet. `?dry_run=1`: read it and say its tables, keep nothing.
pub async fn upload(
    State(state): State<Shared>,
    Auth(caller): Auth,
    Path(name): Path<String>,
    UrlQuery(p): UrlQuery<FileParams>,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let store = allowed(&state, &caller, &name).await?;
    let filename = p.filename.as_deref().unwrap_or_default().trim().to_string();
    if filename.is_empty() || filename.len() > 200 {
        return Err(ApiError::bad_request("filename: the file's name, at most 200 characters"));
    }
    let tables = read(&state, &filename, body.to_vec()).await?;
    let names =
        keep(&state, &store, caller.org(), &name, tables, Source::File(filename), p.dry_run, Some(caller.email()))
            .await?;
    if !p.dry_run {
        state.usage.record(Event {
            connection: Some(name.clone()),
            detail: Some(json!({"tables": names.len()})),
            ..caller.event("file_uploaded")
        });
    }
    Ok(Json(json!({"ok": true, "dry_run": p.dry_run, "tables": names})))
}

/// A Google Sheet shared with anyone with the link, as the connection `name`: a table per tab.
pub async fn sheet(
    State(state): State<Shared>,
    Auth(caller): Auth,
    Path(name): Path<String>,
    UrlQuery(p): UrlQuery<WriteParams>,
    Body(b): Body<SheetBody>,
) -> ApiResult<Json<Value>> {
    let store = allowed(&state, &caller, &name).await?;
    let id = crate::files::sheet_id(&b.link).ok_or_else(|| {
        ApiError::bad_request("that isn't a Google Sheet's link (docs.google.com/spreadsheets/d/...)")
    })?;
    let tables = read(&state, "sheet.xlsx", download(&state, &id).await.map_err(ApiError::bad_request)?).await?;
    let names =
        keep(&state, &store, caller.org(), &name, tables, Source::Sheet(id), p.dry_run, Some(caller.email())).await?;
    if !p.dry_run {
        state.usage.record(Event { connection: Some(name.clone()), ..caller.event("sheet_connected") });
    }
    Ok(Json(json!({"ok": true, "dry_run": p.dry_run, "tables": names})))
}

/// Read a Google Sheet connection again, now.
pub async fn refresh(
    State(state): State<Shared>,
    Auth(caller): Auth,
    Path(name): Path<String>,
) -> ApiResult<Json<Value>> {
    let store = allowed(&state, &caller, &name).await?;
    let id = sheet_of(&state, &store, caller.org(), &name).await?.ok_or_else(|| {
        ApiError::bad_request(format!("{name:?} isn't a Google Sheet: upload the file again instead"))
    })?;
    let tables = read(&state, "sheet.xlsx", download(&state, &id).await.map_err(ApiError::bad_request)?).await?;
    let names =
        keep(&state, &store, caller.org(), &name, tables, Source::Sheet(id), false, Some(caller.email())).await?;
    Ok(Json(json!({"ok": true, "tables": names})))
}

/// Every workspace's Google Sheets, read again (hourly, main.rs). One that fails keeps its last
/// tables and waits for the next round; the others go on.
pub async fn refresh_sheets(state: &AppState) {
    let mut tenants = vec![String::new()];
    match state.store.org_databases().await {
        Ok(dbs) => tenants.extend(dbs.into_keys()),
        Err(e) => tracing::warn!("Google Sheets: organisations unavailable: {e:#}"),
    }
    for org in tenants {
        let Ok(store) = state.orgs.store(&org).await else { continue };
        let Ok(list) = state.connectors.list(&store, &org).await else { continue };
        for c in list {
            let ConnectionConfig::Duckdb(d) = &c.config else { continue };
            let Some(id) = d.google_sheet.clone() else { continue };
            let result = match download(state, &id).await {
                Ok(bytes) => match read(state, "sheet.xlsx", bytes).await {
                    Ok(tables) => {
                        keep(state, &store, &org, &c.name, tables, Source::Sheet(id), false, None).await.map(drop)
                    }
                    Err(e) => Err(e),
                },
                Err(e) => Err(ApiError::bad_request(e)),
            };
            if let Err(e) = result {
                tracing::warn!(org, connection = c.name, "Google Sheet not refreshed: {e:?}");
            }
        }
    }
}

/// Who may (as for connections), a valid name, and not a connection of another kind.
async fn allowed(state: &AppState, caller: &Caller, name: &str) -> ApiResult<Store> {
    caller.require("manage_members")?;
    check_name(name).map_err(ApiError::bad_request)?;
    let store = state.orgs.store(caller.org()).await?;
    let existing = state.connectors.list(&store, caller.org()).await?.into_iter().find(|c| c.name == name);
    if existing.is_some_and(|c| !is_file(&c.config)) {
        return Err(ApiError::Status(
            axum::http::StatusCode::CONFLICT,
            format!("the connection {name:?} isn't a file: choose another name"),
        ));
    }
    Ok(store)
}

async fn sheet_of(state: &AppState, store: &Store, org: &str, name: &str) -> ApiResult<Option<String>> {
    let list = state.connectors.list(store, org).await?;
    Ok(list.into_iter().find(|c| c.name == name).and_then(|c| match c.config {
        ConnectionConfig::Duckdb(d) => d.google_sheet,
        _ => None,
    }))
}

/// A file's tables (reading a workbook is work: off the async threads).
async fn read(state: &AppState, filename: &str, bytes: Vec<u8>) -> ApiResult<Vec<Table>> {
    let (filename, max_unpacked) = (filename.to_string(), state.config.upload_max_bytes as u64 * 10);
    let tables = tokio::task::spawn_blocking(move || crate::files::read(&filename, &bytes, max_unpacked))
        .await
        .map_err(|e| ApiError::Internal(e.into()))?
        .map_err(ApiError::bad_request)?;
    if tables.is_empty() {
        return Err(ApiError::bad_request("this file has no data"));
    }
    Ok(tables)
}

/// The tables saved as the connection `name` (checked first; `dry_run`: only checked). Their names.
#[allow(clippy::too_many_arguments)]
async fn keep(
    state: &AppState,
    store: &Store,
    org: &str,
    name: &str,
    tables: Vec<Table>,
    source: Source,
    dry_run: bool,
    by: Option<&str>,
) -> ApiResult<Vec<String>> {
    let names: Vec<String> = tables.iter().map(|t| t.name.clone()).collect();
    let paths: serde_json::Map<String, Value> =
        names.iter().map(|t| (t.clone(), json!(format!("files/{name}/{t}.csv")))).collect();
    let mut config = json!({"type": "duckdb", "tables": paths});
    match source {
        Source::File(f) => config["file"] = json!(f),
        Source::Sheet(id) => config["google_sheet"] = json!(id),
    }
    let config: ConnectionConfig = serde_json::from_value(config).map_err(|e| ApiError::Internal(e.into()))?;
    state.connectors.check(store, org, name, &config, None).await?;
    if dry_run {
        return Ok(names);
    }
    // the new tables beside the old, then in their place: never half one, half the other
    let dir = folder(state, org, name);
    let new = dir.with_file_name(format!(".{name}.new"));
    let write = || -> std::io::Result<()> {
        let _ = std::fs::remove_dir_all(&new);
        std::fs::create_dir_all(&new)?;
        for t in &tables {
            std::fs::write(new.join(format!("{}.csv", t.name)), &t.csv)?;
        }
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::rename(&new, &dir)
    };
    write().map_err(|e| ApiError::Internal(anyhow::anyhow!("saving the file's tables: {e}")))?;
    state.connectors.save(store, org, name, config, None, by).await?;
    Ok(names)
}

/// A Google Sheet as an Excel workbook, by its id: from Google only (the address is built here,
/// never given), following redirects to Google's download hosts only, at most the upload limit.
async fn download(state: &AppState, id: &str) -> Result<Vec<u8>, String> {
    let base = state.config.google_sheets_url.trim_end_matches('/').to_string();
    let own = url::Url::parse(&base).ok().and_then(|u| u.host_str().map(str::to_string)).unwrap_or_default();
    let policy = reqwest::redirect::Policy::custom(move |a| {
        let host = a.url().host_str().unwrap_or_default().to_string();
        let google = host == "docs.google.com" || host.ends_with(".googleusercontent.com") || host == own;
        if google && a.previous().len() < 5 { a.follow() } else { a.stop() }
    });
    let http = reqwest::Client::builder()
        .redirect(policy)
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| e.to_string())?;
    let unreachable = |e: reqwest::Error| format!("Google Sheets didn't answer: {e}");
    let mut r = http.get(format!("{base}/spreadsheets/d/{id}/export?format=xlsx")).send().await.map_err(unreachable)?;
    if r.status() == reqwest::StatusCode::NOT_FOUND {
        return Err("no Google Sheet at that link".into());
    }
    if !r.status().is_success() {
        return Err(format!("Google Sheets answered {}", r.status()));
    }
    let max = state.config.upload_max_bytes;
    let mut bytes = Vec::new();
    while let Some(chunk) = r.chunk().await.map_err(unreachable)? {
        bytes.extend_from_slice(&chunk);
        if bytes.len() > max {
            return Err(format!("this sheet is over {} MB", max >> 20));
        }
    }
    // a sheet that isn't shared answers with Google's sign-in page, not a workbook (a zip: "PK")
    if !bytes.starts_with(b"PK") {
        return Err("this sheet isn't shared: in Google Sheets, Share › General access › Anyone with the link".into());
    }
    Ok(bytes)
}

/// A connection made from a file or a Google Sheet (its tables are kept by Magnum BI).
pub fn is_file(c: &ConnectionConfig) -> bool {
    matches!(c, ConnectionConfig::Duckdb(d) if d.file.is_some() || d.google_sheet.is_some())
}

/// A file connection's tables, gone with it (after it's deleted).
pub fn remove(state: &AppState, caller: &Caller, name: &str) {
    if check_name(name).is_ok() {
        let _ = std::fs::remove_dir_all(folder(state, caller.org(), name));
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use axum::body::Body as HttpBody;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use crate::api::tests::{app, app_configured, call};

    /// Upload `bytes` as `filename` to the connection `name`.
    pub async fn upload(
        app: &axum::Router,
        cookie: &str,
        name: &str,
        filename: &str,
        bytes: &[u8],
        query: &str,
    ) -> (StatusCode, Value) {
        let uri = format!("/api/connections/{name}/file?filename={}{query}", urlencoding(filename));
        let req = Request::builder()
            .method("POST")
            .uri(uri)
            .header("cookie", cookie)
            .header("content-type", "application/octet-stream")
            .body(HttpBody::from(bytes.to_vec()))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
    }

    fn urlencoding(s: &str) -> String {
        url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
    }

    fn unique() -> String {
        format!("f{}", uuid::Uuid::new_v4().simple())
    }

    #[tokio::test]
    async fn a_csv_becomes_a_connection_with_its_table() {
        let Some((app, state, cookie)) = app().await else { return };
        let name = unique();
        let (s, r) = upload(&app, &cookie, &name, "Sales 2026.csv", b"region,amount\nEMEA,10\nAPAC,5\n", "").await;
        assert_eq!(s, StatusCode::OK, "{r}");
        assert_eq!(r["tables"], json!(["sales_2026"]));
        // a DuckDB connection like any other: its tables, and a model proposed from the columns
        let (_, t) = call(&app, &cookie, "GET", &format!("/api/connections/{name}/tables"), None).await;
        assert_eq!(t["tables"], json!(["sales_2026"]), "{t}");
        let (s, m) =
            call(&app, &cookie, "GET", &format!("/api/connections/{name}/tables/sales_2026/model?name=sales_x"), None)
                .await;
        assert_eq!(s, StatusCode::OK, "{m}");
        assert!(m.to_string().contains("region") && m.to_string().contains("amount"), "{m}");
        let (_, list) = call(&app, &cookie, "GET", "/api/connections", None).await;
        let c = list["connections"].as_array().unwrap().iter().find(|c| c["name"] == json!(name)).unwrap().clone();
        assert_eq!(c["config"]["file"], json!("Sales 2026.csv"), "{c}");
        // deleted: its files too
        let dir = crate::connectors::duckdb::org_dir(&state.config.duckdb_dir, "").join("files").join(&name);
        assert!(dir.join("sales_2026.csv").exists());
        call(&app, &cookie, "DELETE", &format!("/api/connections/{name}"), None).await;
        assert!(!dir.exists(), "its tables went with it");
    }

    #[tokio::test]
    async fn replacing_a_file_replaces_its_tables() {
        let Some((app, state, cookie)) = app().await else { return };
        let name = unique();
        upload(&app, &cookie, &name, "old.csv", b"a\n1\n", "").await;
        let (s, r) = upload(&app, &cookie, &name, "new.csv", b"b\n2\n", "").await;
        assert_eq!(s, StatusCode::OK, "{r}");
        let (_, t) = call(&app, &cookie, "GET", &format!("/api/connections/{name}/tables"), None).await;
        assert_eq!(t["tables"], json!(["new"]), "the old file's table is gone: {t}");
        let dir = crate::connectors::duckdb::org_dir(&state.config.duckdb_dir, "").join("files").join(&name);
        let files: Vec<String> =
            std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(files, ["new.csv"]);
        clean_up(&app, &cookie, &state, &name).await;
    }

    #[tokio::test]
    async fn a_dry_run_reads_the_file_and_keeps_nothing() {
        let Some((app, state, cookie)) = app().await else { return };
        let name = unique();
        let (s, r) = upload(&app, &cookie, &name, "x.csv", b"a\n1\n", "&dry_run=1").await;
        assert_eq!((s, r["tables"].clone(), r["dry_run"].clone()), (StatusCode::OK, json!(["x"]), json!(true)), "{r}");
        let (_, list) = call(&app, &cookie, "GET", "/api/connections", None).await;
        assert!(!list.to_string().contains(&name), "nothing saved");
        assert!(!crate::connectors::duckdb::org_dir(&state.config.duckdb_dir, "").join("files").join(&name).exists());
    }

    #[tokio::test]
    async fn what_isnt_a_file_connection_is_left_alone() {
        let Some((app, state, cookie)) = app().await else { return };
        let name = unique();
        // a connection of another kind: never turned into a file
        let pg = json!({"config": {"type": "postgres", "host": "db.example", "database": "d", "user": "u"}});
        let (s, r) = call(&app, &cookie, "PUT", &format!("/api/connections/{name}"), Some(pg)).await;
        assert_eq!(s, StatusCode::OK, "{r}");
        let (s, r) = upload(&app, &cookie, &name, "x.csv", b"a\n1\n", "").await;
        assert_eq!(s, StatusCode::CONFLICT, "{r}");
        // names that aren't connection names are refused before anything is written
        for bad in ["..", "a%2Fb", "x%20y"] {
            let (s, _) = upload(&app, &cookie, bad, "x.csv", b"a\n1\n", "").await;
            assert!(s == StatusCode::BAD_REQUEST || s == StatusCode::NOT_FOUND, "{bad}: {s}");
        }
        // and what isn't a spreadsheet
        let (s, r) = upload(&app, &cookie, &unique(), "notes.txt", b"hello", "").await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{r}");
        clean_up(&app, &cookie, &state, &name).await;
    }

    #[tokio::test]
    async fn a_file_over_the_limit_is_refused() {
        let Some((app, _, cookie)) =
            app_configured(concat!(env!("CARGO_MANIFEST_DIR"), "/../project"), |c| c.upload_max_bytes = 2 << 20).await
        else {
            return;
        };
        // over the general 1 MB request limit, under this one: fine
        let mut ok = b"a\n".to_vec();
        ok.extend(std::iter::repeat_n(b"1\n".as_slice(), 700_000).flatten());
        let name = unique();
        let (s, r) = upload(&app, &cookie, &name, "ok.csv", &ok, "&dry_run=1").await;
        assert_eq!(s, StatusCode::OK, "{r}");
        let big = vec![b'1'; 3 << 20];
        let (s, _) = upload(&app, &cookie, &name, "big.csv", &big, "&dry_run=1").await;
        assert_eq!(s, StatusCode::PAYLOAD_TOO_LARGE);
        // the general limit still holds everywhere else
        let req = Request::builder()
            .method("PUT")
            .uri(format!("/api/connections/{name}"))
            .header("cookie", cookie.as_str())
            .header("content-type", "application/json")
            .body(HttpBody::from(vec![b' '; 3 << 20]))
            .unwrap();
        assert_eq!(app.clone().oneshot(req).await.unwrap().status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    const SHARED: &str = "1SharedSheetAbcdefghijklmnopqrstuvwxyz0123";
    const PRIVATE: &str = "1PrivateSheetAbcdefghijklmnopqrstuvwxyz012";

    /// A stand-in for Google: SHARED is a workbook (its second version once `changed`), anything else
    /// the sign-in page Google shows for a sheet that isn't shared.
    async fn google(changed: std::sync::Arc<std::sync::atomic::AtomicBool>) -> String {
        use axum::extract::Path as P;
        let router = axum::Router::new().route(
            "/spreadsheets/d/{id}/export",
            axum::routing::get(move |P(id): P<String>| {
                let changed = changed.clone();
                async move {
                    if id != SHARED {
                        return (StatusCode::OK, [("content-type", "text/html")], b"<html>Sign in</html>".to_vec());
                    }
                    let book = if changed.load(std::sync::atomic::Ordering::SeqCst) {
                        crate::files::tests::xlsx(&[
                            ("Orders", &[&["order"], &["A-1"]]),
                            ("Returns", &[&["order"], &["A-1"]]),
                        ])
                    } else {
                        crate::files::tests::xlsx(&[("Orders", &[&["order"], &["A-1"]])])
                    };
                    (
                        StatusCode::OK,
                        [("content-type", "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet")],
                        book,
                    )
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        base
    }

    #[tokio::test]
    async fn a_shared_google_sheet_becomes_a_connection_and_refreshes() {
        let changed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let base = google(changed.clone()).await;
        let Some((app, state, cookie)) =
            app_configured(concat!(env!("CARGO_MANIFEST_DIR"), "/../project"), |c| c.google_sheets_url = base.clone())
                .await
        else {
            return;
        };
        let name = unique();
        let link = format!("https://docs.google.com/spreadsheets/d/{SHARED}/edit#gid=0");
        let (s, r) =
            call(&app, &cookie, "POST", &format!("/api/connections/{name}/sheet"), Some(json!({"link": link}))).await;
        assert_eq!(s, StatusCode::OK, "{r}");
        assert_eq!(r["tables"], json!(["orders"]));
        let (_, list) = call(&app, &cookie, "GET", "/api/connections", None).await;
        let c = list["connections"].as_array().unwrap().iter().find(|c| c["name"] == json!(name)).unwrap().clone();
        assert_eq!(c["config"]["google_sheet"], json!(SHARED), "{c}");
        // the sheet changed: a refresh brings its new tab
        changed.store(true, std::sync::atomic::Ordering::SeqCst);
        let (s, r) = call(&app, &cookie, "POST", &format!("/api/connections/{name}/refresh"), None).await;
        assert_eq!((s, r["tables"].clone()), (StatusCode::OK, json!(["orders", "returns"])), "{r}");
        let (_, t) = call(&app, &cookie, "GET", &format!("/api/connections/{name}/tables"), None).await;
        assert_eq!(t["tables"], json!(["orders", "returns"]), "{t}");
        // and every hour, for every workspace
        changed.store(false, std::sync::atomic::Ordering::SeqCst);
        super::refresh_sheets(&state).await;
        let (_, t) = call(&app, &cookie, "GET", &format!("/api/connections/{name}/tables"), None).await;
        assert_eq!(t["tables"], json!(["orders"]), "{t}");
        clean_up(&app, &cookie, &state, &name).await;
    }

    #[tokio::test]
    async fn only_shared_google_sheets_are_read() {
        let base = google(Default::default()).await;
        let Some((app, _, cookie)) =
            app_configured(concat!(env!("CARGO_MANIFEST_DIR"), "/../project"), |c| c.google_sheets_url = base.clone())
                .await
        else {
            return;
        };
        let name = unique();
        let private = format!("https://docs.google.com/spreadsheets/d/{PRIVATE}/edit");
        let (s, r) =
            call(&app, &cookie, "POST", &format!("/api/connections/{name}/sheet"), Some(json!({"link": private})))
                .await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{r}");
        assert!(r["detail"].as_str().unwrap().to_lowercase().contains("anyone with the link"), "{r}");
        // a link that isn't a Google Sheet is never fetched
        let elsewhere = format!("{base}/spreadsheets/d/{SHARED}/edit");
        let (s, r) =
            call(&app, &cookie, "POST", &format!("/api/connections/{name}/sheet"), Some(json!({"link": elsewhere})))
                .await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{r}");
        let (_, list) = call(&app, &cookie, "GET", "/api/connections", None).await;
        assert!(!list.to_string().contains(&name), "nothing saved");
    }

    async fn clean_up(app: &axum::Router, cookie: &str, state: &crate::state::AppState, name: &str) {
        call(app, cookie, "DELETE", &format!("/api/connections/{name}"), None).await;
        let dir = crate::connectors::duckdb::org_dir(&state.config.duckdb_dir, "").join("files").join(name);
        let _ = std::fs::remove_dir_all(dir);
    }
}
