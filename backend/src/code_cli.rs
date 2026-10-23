//! Setup as code from the command line: `magnum-bi code export|plan|apply <folder>`, a client of
//! a running server's /api/code (api/code.rs) with an API token. The folder holds the YAML files;
//! secret values come from environment variables (MAGNUM_SECRET_<FILE>), never from files.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

/// Where the server is, and who's asking.
pub struct Remote {
    pub url: String,
    pub token: String,
    /// the organisation to act in (none: the token's)
    pub org: Option<String>,
}

/// The variable holding a file's secret: `connections/warehouse.yaml` -> MAGNUM_SECRET_CONNECTIONS_WAREHOUSE.
pub fn secret_var(path: &str) -> String {
    let stem = path.strip_suffix(".yaml").unwrap_or(path);
    let name: String =
        stem.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { '_' }).collect();
    format!("MAGNUM_SECRET_{name}")
}

/// The secrets set in the environment for these files.
pub fn secrets(files: &BTreeMap<String, String>, env: impl Fn(&str) -> Option<String>) -> BTreeMap<String, String> {
    files.keys().filter_map(|p| env(&secret_var(p)).filter(|v| !v.is_empty()).map(|v| (p.clone(), v))).collect()
}

/// The folder's YAML files by their path in it (`/`-separated), skipping hidden folders (.git).
pub fn read_dir(dir: &Path) -> Result<BTreeMap<String, String>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, String>) -> Result<()> {
        for entry in std::fs::read_dir(dir).with_context(|| format!("can't read {}", dir.display()))? {
            let entry = entry?;
            let path = entry.path();
            let hidden = path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with('.'));
            // links aren't followed: a cloned repository can't send a file from elsewhere
            let kind = entry.file_type()?;
            if hidden || kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                walk(root, &path, out)?;
            } else if path.extension().is_some_and(|e| e == "yaml") {
                let rel: Vec<String> = path
                    .strip_prefix(root)?
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect();
                let text = std::fs::read_to_string(&path).with_context(|| format!("can't read {}", path.display()))?;
                out.insert(rel.join("/"), text);
            }
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out)?;
    Ok(out)
}

/// Write the files into the folder (making folders as needed). Returns how many.
pub fn write_dir(dir: &Path, files: &BTreeMap<String, String>) -> Result<usize> {
    for (path, text) in files {
        // the server names the files; still, nothing is written outside the folder
        if path.split('/').any(|p| p.is_empty() || p == "." || p == "..") || path.starts_with('/') {
            bail!("the server sent a file named {path:?}: not written");
        }
        // nor through a link the folder has (a repository's link to a file elsewhere)
        let mut at = dir.to_path_buf();
        for part in path.split('/') {
            at.push(part);
            if std::fs::symlink_metadata(&at).is_ok_and(|m| m.file_type().is_symlink()) {
                bail!("{} is a link: not written through", at.display());
            }
        }
        let target = dir.join(path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).with_context(|| format!("can't make {}", parent.display()))?;
        }
        std::fs::write(&target, text).with_context(|| format!("can't write {}", target.display()))?;
    }
    Ok(files.len())
}

/// A plan, one line per change: `+` create, `~` update, `-` delete, `!` a problem.
pub fn describe(plan: &Value) -> String {
    let changes = plan["changes"].as_array().cloned().unwrap_or_default();
    let unchanged = plan["unchanged"].as_u64().unwrap_or(0);
    if changes.is_empty() {
        return format!("Nothing to change ({unchanged} files)");
    }
    let mut lines: Vec<String> = changes
        .iter()
        .map(|c| {
            let path = c["path"].as_str().unwrap_or("?");
            match (c["error"].as_str(), c["action"].as_str()) {
                (Some(e), _) => format!("! {path}: {e}"),
                (None, Some("create")) => format!("+ {path}"),
                (None, Some("delete")) => format!("- {path}"),
                _ => format!("~ {path}"),
            }
        })
        .collect();
    lines.push(format!("{} changes, {unchanged} files unchanged", changes.len()));
    lines.join("\n")
}

/// The token goes over https only (plain http only to this computer).
pub fn check_url(url: &str) -> Result<()> {
    let u = url::Url::parse(url).map_err(|_| anyhow::anyhow!("{url:?} isn't a web address"))?;
    let local = matches!(u.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    match u.scheme() {
        "https" => Ok(()),
        "http" if local => Ok(()),
        _ => bail!("the server's address starts with https:// (the token would travel in clear)"),
    }
}

fn client() -> Result<reqwest::Client> {
    // no redirects: the token goes to the server named, nowhere else
    let c = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .redirect(reqwest::redirect::Policy::none());
    Ok(c.build()?)
}

/// The request, with the token and the organisation.
fn request(remote: &Remote, method: reqwest::Method, query: &str) -> Result<reqwest::RequestBuilder> {
    check_url(&remote.url)?;
    let url = format!("{}/api/code{query}", remote.url.trim_end_matches('/'));
    let mut r = client()?.request(method, url).bearer_auth(&remote.token);
    if let Some(org) = &remote.org {
        r = r.header("x-magnum-org", org);
    }
    Ok(r)
}

/// The answer as JSON; a refusal as an error with the server's words.
async fn answer(r: reqwest::Response) -> Result<Value> {
    let status = r.status();
    let body: Value = r.json().await.unwrap_or(Value::Null);
    if status.is_success() || body.get("changes").is_some() {
        return Ok(body);
    }
    bail!("{status}: {}", body["detail"].as_str().unwrap_or("the server refused"))
}

pub async fn export(remote: &Remote, dir: &Path) -> Result<usize> {
    let r = request(remote, reqwest::Method::GET, "")?.send().await.context("can't reach the server")?;
    let body = answer(r).await?;
    let files: BTreeMap<String, String> = serde_json::from_value(body["files"].clone())?;
    write_dir(dir, &files)
}

/// The plan for the folder (`apply`: and apply it).
pub async fn send(remote: &Remote, dir: &Path, prune: bool, apply: bool) -> Result<Value> {
    let files = read_dir(dir)?;
    let secrets = secrets(&files, |k| std::env::var(k).ok());
    let mut query = vec![];
    if !apply {
        query.push("dry_run=1");
    }
    if prune {
        query.push("prune=1");
    }
    let query = if query.is_empty() { String::new() } else { format!("?{}", query.join("&")) };
    let body = json!({"files": files, "secrets": secrets});
    let r =
        request(remote, reqwest::Method::POST, &query)?.json(&body).send().await.context("can't reach the server")?;
    answer(r).await
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::Json;
    use axum::extract::{Query, State};
    use axum::http::HeaderMap;
    use axum::routing::get;

    use super::*;

    fn folder() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("magnum-code-{}", uuid::Uuid::new_v4().simple()))
    }

    #[test]
    fn secrets_come_from_variables_named_after_their_file() {
        assert_eq!(secret_var("connections/warehouse.yaml"), "MAGNUM_SECRET_CONNECTIONS_WAREHOUSE");
        assert_eq!(secret_var("mail.yaml"), "MAGNUM_SECRET_MAIL");
        assert_eq!(
            secret_var("reports/sales-overview/monday-numbers.yaml"),
            "MAGNUM_SECRET_REPORTS_SALES_OVERVIEW_MONDAY_NUMBERS"
        );
        let files: BTreeMap<String, String> =
            [("mail.yaml", ""), ("chat.yaml", "")].iter().map(|(p, t)| (p.to_string(), t.to_string())).collect();
        let env = |k: &str| (k == "MAGNUM_SECRET_MAIL").then(|| "pw".to_string());
        assert_eq!(secrets(&files, env), BTreeMap::from([("mail.yaml".to_string(), "pw".to_string())]));
    }

    #[test]
    fn a_folder_of_files_and_back() {
        let dir = folder();
        let files: BTreeMap<String, String> = [("models/sales.yaml", "name: sales\n"), ("mail.yaml", "host: m\n")]
            .iter()
            .map(|(p, t)| (p.to_string(), t.to_string()))
            .collect();
        assert_eq!(write_dir(&dir, &files).unwrap(), 2);
        // not setup: git's own files, notes, other files
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join(".git/config.yaml"), "x").unwrap();
        std::fs::write(dir.join("README.md"), "notes").unwrap();
        assert_eq!(read_dir(&dir).unwrap(), files);
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(read_dir(&dir).is_err(), "a folder that isn't there");
    }

    #[cfg(unix)]
    #[test]
    fn links_are_neither_followed_nor_written_through() {
        let (dir, elsewhere) = (folder(), folder());
        std::fs::create_dir_all(dir.join("models")).unwrap();
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("private.yaml"), "not to be sent").unwrap();
        std::os::unix::fs::symlink(elsewhere.join("private.yaml"), dir.join("models/x.yaml")).unwrap();
        std::os::unix::fs::symlink(&elsewhere, dir.join("dashboards")).unwrap();
        assert!(read_dir(&dir).unwrap().is_empty(), "nothing from outside the folder is read");
        for path in ["models/x.yaml", "dashboards/d.yaml"] {
            let files = BTreeMap::from([(path.to_string(), "overwritten".to_string())]);
            assert!(write_dir(&dir, &files).is_err(), "{path}");
        }
        assert_eq!(std::fs::read_to_string(elsewhere.join("private.yaml")).unwrap(), "not to be sent");
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::remove_dir_all(&elsewhere).unwrap();
    }

    #[test]
    fn the_token_goes_over_https_only() {
        assert!(check_url("https://bi.example.com").is_ok());
        assert!(check_url("http://localhost:8000").is_ok() && check_url("http://127.0.0.1:8000").is_ok());
        assert!(check_url("http://bi.example.com").is_err());
        assert!(check_url("ftp://bi.example.com").is_err() && check_url("not a url").is_err());
    }

    #[test]
    fn plans_read_as_a_list_of_changes() {
        let plan = json!({"changes": [{"path": "models/a.yaml", "action": "create"},
            {"path": "mail.yaml", "action": "update"}, {"path": "blends/b.yaml", "action": "delete"},
            {"path": "chat.yaml", "action": "create", "error": "a bad address"}], "unchanged": 3});
        assert_eq!(
            describe(&plan),
            "+ models/a.yaml\n~ mail.yaml\n- blends/b.yaml\n! chat.yaml: a bad address\n4 changes, 3 files unchanged"
        );
        assert_eq!(describe(&json!({"changes": [], "unchanged": 2})), "Nothing to change (2 files)");
    }

    /// What the stand-in was sent: the query and the body of each POST.
    type Seen = Arc<Mutex<Vec<(String, Value)>>>;

    /// A stand-in server: the files it has, and what it was sent.
    async fn server(seen: Seen) -> String {
        async fn export(headers: HeaderMap) -> Json<Value> {
            assert_eq!(headers["authorization"], "Bearer mbi_test");
            assert_eq!(headers["x-magnum-org"], "acme");
            Json(json!({"files": {"models/sales.yaml": "name: sales\n", "mail.yaml": "host: m\n"}}))
        }
        async fn apply(
            State(seen): State<Seen>,
            Query(q): Query<BTreeMap<String, String>>,
            Json(body): Json<Value>,
        ) -> Json<Value> {
            seen.lock().unwrap().push((format!("{q:?}"), body));
            Json(json!({"ok": true, "changes": [{"path": "mail.yaml", "action": "update"}], "unchanged": 1}))
        }
        let app = axum::Router::new().route("/api/code", get(export).post(apply)).with_state(seen);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        url
    }

    #[tokio::test]
    async fn export_plan_and_apply_through_the_api() {
        let seen = Arc::new(Mutex::new(vec![]));
        let url = server(seen.clone()).await;
        let remote = Remote { url, token: "mbi_test".into(), org: Some("acme".into()) };
        let dir = folder();
        assert_eq!(export(&remote, &dir).await.unwrap(), 2);
        assert_eq!(std::fs::read_to_string(dir.join("models/sales.yaml")).unwrap(), "name: sales\n");

        let plan = send(&remote, &dir, true, false).await.unwrap();
        assert_eq!(plan["changes"][0]["action"], "update");
        send(&remote, &dir, false, true).await.unwrap();
        let seen = seen.lock().unwrap();
        assert!(seen[0].0.contains("\"dry_run\": \"1\"") && seen[0].0.contains("\"prune\": \"1\""), "{:?}", seen[0].0);
        assert!(!seen[1].0.contains("dry_run") && !seen[1].0.contains("prune"), "{:?}", seen[1].0);
        assert_eq!(seen[1].1["files"]["mail.yaml"], "host: m\n");
        assert_eq!(seen[1].1["secrets"], json!({}));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
