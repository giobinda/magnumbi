//! The in-app chat: questions answered by the organisation's own model (its provider and key,
//! set by its Org admins), with Magnum BI's tools.
//!
//! The tools are MCP's (api/mcp.rs), run as the person asking: the API handlers decide what they
//! may do, and their data restrictions apply as everywhere. Only analytics tools are offered
//! (no members, connections, themes or delivery settings; the own attached file of someone allowed to add data is the
//! one way a new table comes in: `api/files.rs::attach`, `add_file_model`). A change is never made in the loop:
//! it's checked (a dry run, where the tool has one) and shown to the person as a proposal, made
//! only when they confirm it. What the provider sees of a result is capped, and the values of
//! sensitive fields can be kept from it (chat.rs).

use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, Query as UrlQuery, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};

use super::Body;
use crate::auth::{Auth, Caller};
use crate::chat::{Call, ChatSettings, ToolResult, ToolSpec, Turn, for_provider, host, parse, request};
use crate::error::{ApiError, ApiResult};
use crate::state::{AppState, Shared};
use crate::store::random_token;

/// The tools the chat offers: reading, and the analytics changes (proposed, then confirmed).
const CHAT_TOOLS: [&str; 27] = [
    "search_catalog",
    "get_glossary",
    "get_catalog",
    "get_lineage",
    "list_models",
    "describe_model",
    "run_query",
    "preview_sql",
    "check_formula",
    "list_dashboards",
    "get_dashboard",
    "dashboard_history",
    "list_blends",
    "list_reports",
    "save_dashboard",
    "delete_dashboard",
    "save_blend",
    "create_report",
    "pause_report",
    "resume_report",
    "send_report_now",
    "delete_report",
    "save_formula",
    "delete_formula",
    "save_docs",
    "save_term",
    "add_file_model",
];

/// Tool rounds per question, before the chat gives up.
const MAX_ROUNDS: usize = 8;
/// The most of a tool's answer (other than a query's rows) the provider sees.
const MAX_TOOL_TEXT: usize = 30_000;

const CHAT_RULES: &str = "\n\nYou are the chat inside Magnum BI, answering people in their organisation. Find \
the right model and measures first (search_catalog with the question's words, then describe_model), then \
run_query. Answer briefly, in the person's language, with the numbers you got: never invent one. Say which \
model you used. A query's chart is shown to the person next to your answer, so don't repeat every row. \
Changes (dashboards, reports, alerts, formulas, blends, the catalog's documentation and glossary) are never \
made by you: calling a tool that changes something shows the person a proposal they confirm or not. Say what \
you proposed; don't call it again. To document a model, look at it (describe_model, get_catalog) and at a few \
of its values (run_query) first, then describe what you saw: never guess a meaning the data doesn't show. \
When the person attaches a file, propose a model for each table they want (add_file_model), with a clear name \
and label: everyone in the organisation will see it once they confirm.";

fn key(state: &AppState) -> ApiResult<&crate::crypto::SecretKey> {
    state
        .config
        .secret_key
        .as_ref()
        .ok_or_else(|| ApiError::bad_request("MAGNUM_SECRET_KEY isn't set, so the chat's API key can't be stored"))
}

fn key_context(org: &str) -> String {
    format!("{org}/chat")
}

fn require_admin(caller: &Caller) -> ApiResult<()> {
    caller.require("manage_members")
}

/// The chat's settings (Org admins): never the key, only whether there is one.
pub async fn settings(State(state): State<Shared>, Auth(caller): Auth) -> ApiResult<Json<Value>> {
    require_admin(&caller)?;
    let saved = state.orgs.store(caller.org()).await?.chat_settings(caller.org()).await?;
    Ok(Json(match saved {
        Some((s, k)) => json!({"settings": s, "has_key": k.is_some()}),
        None => json!({"settings": null, "has_key": false}),
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewSettings {
    settings: ChatSettings,
    /// a new API key (none: keep the one saved; some local models need none)
    #[serde(default)]
    key: Option<String>,
}

/// Save the chat's settings (Org admins; `?dry_run=1`: check only).
pub async fn save_settings(
    State(state): State<Shared>,
    Auth(caller): Auth,
    UrlQuery(p): UrlQuery<super::dashboards::WriteParams>,
    Body(n): Body<NewSettings>,
) -> ApiResult<Json<Value>> {
    require_admin(&caller)?;
    n.settings.check().map_err(ApiError::bad_request)?;
    host(&n.settings.url, state.connectors.net_guard().allow_private).map_err(ApiError::bad_request)?;
    let new_key = n.key.as_deref().map(str::trim).filter(|k| !k.is_empty());
    if new_key.is_some_and(|k| k.len() > 1000 || k.chars().any(char::is_control)) {
        return Err(ApiError::bad_request("that doesn't look like an API key"));
    }
    let org = caller.org();
    let store = state.orgs.store(org).await?;
    // the key never goes to another provider on its own
    if new_key.is_none()
        && let Some((old, Some(_))) = store.chat_settings(org).await?
        && old.url.trim_end_matches('/') != n.settings.url.trim_end_matches('/')
    {
        return Err(ApiError::bad_request("the provider's address changed: give its key again"));
    }
    if p.dry_run {
        return Ok(Json(json!({"ok": true, "dry_run": true})));
    }
    let sealed = match new_key {
        Some(k) => Some(key(&state)?.seal(k, &key_context(org))?),
        None => None,
    };
    store.put_chat_settings(org, &n.settings, sealed.as_deref(), caller.email()).await?;
    state.usage.record(caller.event("chat_settings_saved"));
    Ok(Json(json!({"ok": true})))
}

pub async fn delete_settings(State(state): State<Shared>, Auth(caller): Auth) -> ApiResult<Json<Value>> {
    require_admin(&caller)?;
    state.orgs.store(caller.org()).await?.delete_chat_settings(caller.org()).await?;
    Ok(Json(json!({"ok": true})))
}

/// Whether the chat is on here, the home screen, and the person's conversations.
pub async fn list(State(state): State<Shared>, Auth(caller): Auth) -> ApiResult<Json<Value>> {
    caller.require("view")?;
    let store = state.orgs.store(caller.org()).await?;
    let settings = store.chat_settings(caller.org()).await?.map(|(s, _)| s);
    let mine = store.chat_home(caller.org(), caller.email()).await?;
    let enabled = settings.as_ref().is_some_and(|s| s.enabled);
    let org_home = settings.map_or("dashboards".to_string(), |s| s.home);
    // the chat is a home only while it's on
    let home = if enabled { mine.clone().unwrap_or_else(|| org_home.clone()) } else { "dashboards".into() };
    let conversations: Vec<Value> = store
        .chat_conversations(caller.org(), caller.email())
        .await?
        .into_iter()
        .map(|(id, title, at)| json!({"id": id, "title": title, "updated_at": at}))
        .collect();
    Ok(Json(
        json!({"enabled": enabled, "home": home, "org_home": org_home, "my_home": mine, "conversations": conversations}),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Home {
    home: Option<String>,
}

/// Where the person starts: chat, dashboards, or (null) where their organisation does.
pub async fn set_home(State(state): State<Shared>, Auth(caller): Auth, Body(h): Body<Home>) -> ApiResult<Json<Value>> {
    caller.require("view")?;
    if h.home.as_deref().is_some_and(|x| !["chat", "dashboards"].contains(&x)) {
        return Err(ApiError::bad_request("start on chat or dashboards"));
    }
    state.orgs.store(caller.org()).await?.put_chat_home(caller.org(), caller.email(), h.home.as_deref()).await?;
    Ok(Json(json!({"ok": true})))
}

/// One of the person's conversations, as they saw it.
pub async fn get(State(state): State<Shared>, Auth(caller): Auth, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    caller.require("view")?;
    let (title, _, shown) = conversation(&state, &caller, &id).await?;
    Ok(Json(json!({"id": id, "title": title, "messages": serde_json::from_str::<Value>(&shown).unwrap_or(json!([]))})))
}

pub async fn delete(State(state): State<Shared>, Auth(caller): Auth, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    caller.require("view")?;
    if !state.orgs.store(caller.org()).await?.delete_chat_conversation(caller.org(), caller.email(), &id).await? {
        return Err(ApiError::not_found("no such conversation of yours"));
    }
    Ok(Json(json!({"ok": true})))
}

async fn conversation(state: &AppState, caller: &Caller, id: &str) -> ApiResult<(String, String, String)> {
    let store = state.orgs.store(caller.org()).await?;
    store
        .chat_conversation(caller.org(), caller.email(), id)
        .await?
        .ok_or_else(|| ApiError::not_found("no such conversation of yours"))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Question {
    #[serde(default)]
    conversation: Option<String>,
    message: String,
    /// the connection of a file just attached (`POST /api/chat/file`)
    #[serde(default)]
    file: Option<String>,
}

/// What the chat's model is told of an attached file: its tables' columns, never their rows.
async fn attached_note(state: &AppState, caller: &Caller, connection: &str) -> ApiResult<(String, String)> {
    let (file, tables) = super::files::own_file(state, caller, connection).await?;
    let store = state.orgs.store(caller.org()).await?;
    let (_, c) = state.connectors.get(&store, caller.org(), connection).await?;
    let mut parts = vec![];
    for t in &tables {
        let columns = c.columns(t).await.map_err(|e| super::org_models::conn_error(&format!("table {t}"), e))?;
        let names: Vec<String> = columns.into_iter().map(|(name, _)| name).collect();
        parts.push(format!("{t} ({})", names.join(", ")));
    }
    let note = format!("(I attached the file {file:?}: connection {connection}, tables: {}.)", parts.join("; "));
    Ok((file, note.chars().take(MAX_TOOL_TEXT).collect()))
}

/// Ask a question (in a new conversation, or a follow-up): the answer, its charts and the
/// changes proposed.
pub async fn ask(State(state): State<Shared>, Auth(caller): Auth, Body(q): Body<Question>) -> ApiResult<Json<Value>> {
    caller.require("view")?;
    let message = q.message.trim().to_string();
    if message.is_empty() || message.chars().count() > 4000 {
        return Err(ApiError::bad_request("ask a question (up to 4,000 characters)"));
    }
    let attached = match &q.file {
        Some(connection) => Some(attached_note(&state, &caller, connection).await?),
        None => None,
    };
    let org = caller.org().to_string();
    let store = state.orgs.store(&org).await?;
    let (settings, sealed) = match store.chat_settings(&org).await? {
        Some((s, k)) if s.enabled => (s, k),
        _ => {
            return Err(ApiError::Status(
                StatusCode::CONFLICT,
                "the chat isn't set up here: an Org admin can set it up".into(),
            ));
        }
    };
    let api_key = match sealed {
        Some(k) => Some(key(&state)?.open(&k, &key_context(&org))?),
        None => None,
    };
    // the conversation so far (the person's own), or a new one
    let (id, title, mut turns, mut shown) = match &q.conversation {
        Some(id) => {
            let (title, turns, shown) = conversation(&state, &caller, id).await?;
            let turns: Vec<Turn> = serde_json::from_str(&turns).map_err(anyhow::Error::from)?;
            let shown: Vec<Value> = serde_json::from_str(&shown).map_err(anyhow::Error::from)?;
            (id.clone(), title, turns, shown)
        }
        None => (random_token(9), message.chars().take(80).collect(), vec![], vec![]),
    };
    let month = chrono::Utc::now().format("%Y-%m").to_string();
    let asked = store.count_chat_question(&org, &month).await?;
    if settings.monthly_questions > 0 && asked > i64::from(settings.monthly_questions) {
        return Err(ApiError::Status(
            StatusCode::TOO_MANY_REQUESTS,
            "this organisation has asked all its chat questions for the month: an Org admin can raise the limit".into(),
        ));
    }
    // changes confirmed since the last question: the model should know
    let mut text = message.clone();
    let done: Vec<String> = shown
        .iter_mut()
        .flat_map(|m| m["proposals"].as_array_mut().into_iter().flatten())
        .filter(|p| p["status"] == "applied" && p["told"] != true)
        .map(|p| {
            p["told"] = json!(true);
            p["summary"].as_str().unwrap_or_default().to_string()
        })
        .collect();
    if !done.is_empty() {
        text = format!("(Since your last answer I confirmed: {}.)\n\n{text}", done.join("; "));
    }
    match attached {
        Some((file, note)) => {
            shown.push(json!({"role": "user", "text": message, "file": file}));
            text = format!("{note}\n\n{text}");
        }
        None => shown.push(json!({"role": "user", "text": message})),
    }
    turns.push(Turn::User { text });

    let tools = crate::api::mcp::tools_for(&caller, &CHAT_TOOLS);
    let specs: Vec<ToolSpec> = tools
        .iter()
        .map(|t| ToolSpec { name: t.name.into(), description: t.description.into(), schema: t.schema.clone() })
        .collect();
    let system = format!("{}{CHAT_RULES}", crate::api::mcp::INSTRUCTIONS);
    let caller = Arc::new(caller);
    let (mut charts, mut proposals, mut answer, mut calls_made) = (vec![], vec![], None, 0);
    for _ in 0..MAX_ROUNDS {
        let (url, body) = request(&settings, &system, &turns, &specs);
        let reply = provider(&state, &settings, api_key.as_deref(), &url, &body).await?;
        let turn = parse(&settings, &reply).map_err(|e| ApiError::Status(StatusCode::BAD_GATEWAY, e))?;
        let Turn::Assistant { text, calls } = &turn else { unreachable!("parse gives an assistant turn") };
        let (text, calls) = (text.clone(), calls.clone());
        turns.push(turn);
        if calls.is_empty() {
            answer = Some(text);
            break;
        }
        let mut results = vec![];
        for c in calls {
            calls_made += 1;
            let content = run(&state, &caller, &settings, &tools, &c, &mut charts, &mut proposals).await;
            results.push(ToolResult { id: c.id.clone(), content });
        }
        turns.push(Turn::Results { results });
    }
    let answer =
        answer.unwrap_or_else(|| "I couldn't finish within the steps allowed: try a narrower question.".into());
    shown.push(json!({"role": "assistant", "text": answer, "charts": charts, "proposals": proposals}));
    let (turns, shown_text) = (serde_json::to_string(&turns), serde_json::to_string(&shown));
    let (turns, shown_text) = (turns.map_err(anyhow::Error::from)?, shown_text.map_err(anyhow::Error::from)?);
    store.put_chat_conversation(&org, caller.email(), &id, &title, &turns, &shown_text).await?;
    state.usage.record(crate::usage::Event {
        detail: Some(json!({"tool_calls": calls_made, "proposals": proposals.len()})),
        ..caller.event("chat_question")
    });
    Ok(Json(json!({"conversation": id, "title": title, "reply": shown.last()})))
}

/// One tool call: a read run as the person; a change checked and proposed. The text the model gets.
async fn run(
    state: &Shared,
    caller: &Arc<Caller>,
    settings: &ChatSettings,
    tools: &[crate::api::mcp::ToolInfo],
    c: &Call,
    charts: &mut Vec<Value>,
    proposals: &mut Vec<Value>,
) -> String {
    let Some(tool) = tools.iter().find(|t| t.name == c.name) else {
        return format!("the tool {:?} isn't available to you in the chat", c.name);
    };
    if !c.args.is_object() {
        return "the tool's arguments must be a JSON object".into();
    }
    if tool.writes {
        let mut args = c.args.clone();
        args.as_object_mut().map(|o| o.remove("dry_run"));
        // checked first, where it can be: a mistake goes back to the model, not to the person
        let check = if c.name == "save_dashboard" {
            Some(crate::api::mcp::call_tool(state, caller, "validate_dashboard", args.clone()).await)
        } else if crate::api::mcp::DRY_RUNS.contains(&tool.name) {
            let mut dry = args.clone();
            dry["dry_run"] = json!(true);
            Some(crate::api::mcp::call_tool(state, caller, tool.name, dry).await)
        } else {
            None
        };
        if let Some(Err(e)) = check {
            return format!("checking this change failed: {e}");
        }
        let id = random_token(6);
        let summary = crate::chat::summary(tool.name, &args);
        // what only the check could know (a file's fields), else what the arguments say
        let found = check.as_ref().and_then(|c| c.as_ref().ok()).and_then(|v| v["details"].as_array()).cloned();
        let details = found.map_or_else(|| json!(crate::chat::details(tool.name, &args)), Value::Array);
        proposals.push(json!({"id": id, "tool": tool.name, "args": args, "summary": summary, "details": details,
                              "status": "proposed", "check": check.and_then(Result::ok)}));
        return format!(
            "Proposed to the person as a change to confirm ({summary}; proposal {id}). Nothing is changed unless they \
             confirm it in the chat: don't call this again."
        );
    }
    match crate::api::mcp::call_tool(state, caller, &c.name, c.args.clone()).await {
        Ok(v) if c.name == "run_query" => {
            let query = c.args["query"].clone();
            charts.push(json!({"query": query}));
            let model = query["model"].as_str().unwrap_or_default();
            let sensitive = if settings.keep_sensitive { sensitive_fields(state, caller, model).await } else { vec![] };
            for_provider(&v, &sensitive, settings.rows_to_model as usize).to_string()
        }
        Ok(v) => {
            let text = v.to_string();
            if text.chars().count() > MAX_TOOL_TEXT {
                format!("{} …(cut short)", text.chars().take(MAX_TOOL_TEXT).collect::<String>())
            } else {
                text
            }
        }
        Err(e) => format!("the tool failed: {e}"),
    }
}

/// A model's fields tagged as sensitive, by the model itself or the organisation's documentation.
async fn sensitive_fields(state: &AppState, caller: &Caller, model: &str) -> Vec<String> {
    const SENSITIVE: [&str; 4] = ["pii", "personal", "confidential", "restricted"];
    let Ok(m) = super::model_of(caller, model) else { return vec![] };
    let docs = super::governance::docs_of(state, caller).await.unwrap_or_default();
    let doc = docs.get(model);
    let tagged = |name: &str, own: &[String]| {
        let extra = doc.and_then(|d| d.fields.get(name)).map(|f| f.tags.clone()).unwrap_or_default();
        own.iter().chain(&extra).any(|t| SENSITIVE.contains(&t.as_str()))
    };
    m.dimensions
        .iter()
        .filter(|d| tagged(&d.name, &d.tags))
        .map(|d| d.name.clone())
        .chain(m.measures.iter().filter(|x| tagged(&x.name, &x.tags)).map(|x| x.name.clone()))
        .collect()
}

/// One request to the organisation's provider, through the network guard (the address it
/// checked, no redirects). The key never appears in an error.
async fn provider(
    state: &AppState,
    s: &ChatSettings,
    api_key: Option<&str>,
    url: &str,
    body: &Value,
) -> ApiResult<Value> {
    let failed = |m: String| ApiError::Status(StatusCode::BAD_GATEWAY, m);
    let guard = state.connectors.net_guard();
    let host = host(url, guard.allow_private).map_err(failed)?;
    let u = url::Url::parse(url).map_err(|e| failed(e.to_string()))?;
    let addr = guard.check(&host, u.port_or_known_default().unwrap_or(443)).await.map_err(failed)?;
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(90))
        .redirect(reqwest::redirect::Policy::none())
        .resolve(&host, addr)
        .build()
        .map_err(|e| failed(e.to_string()))?;
    let mut r = http.post(u).json(body);
    if let Some(k) = api_key {
        r = match s.format.as_str() {
            "anthropic" => r.header("x-api-key", k).header("anthropic-version", "2023-06-01"),
            _ => r.bearer_auth(k),
        };
    }
    let resp = r.send().await.map_err(|e| failed(format!("couldn't reach the model's API: {}", e.without_url())))?;
    let status = resp.status();
    let v: Value = resp.json().await.unwrap_or(Value::Null);
    if !status.is_success() && v.get("error").is_none() {
        return Err(failed(format!("the model's API answered {status}")));
    }
    Ok(v)
}

/// Make a proposed change: the person whose conversation it is, with their permissions now.
pub async fn apply(
    State(state): State<Shared>,
    Auth(caller): Auth,
    Path((id, proposal)): Path<(String, String)>,
) -> ApiResult<Json<Value>> {
    caller.require("view")?;
    let (title, turns, shown) = conversation(&state, &caller, &id).await?;
    let mut shown: Vec<Value> = serde_json::from_str(&shown).map_err(anyhow::Error::from)?;
    let Some(p) = shown
        .iter_mut()
        .flat_map(|m| m["proposals"].as_array_mut().into_iter().flatten())
        .find(|p| p["id"] == json!(proposal))
    else {
        return Err(ApiError::not_found("no such proposal in this conversation"));
    };
    if p["status"] != "proposed" {
        return Err(ApiError::Status(
            StatusCode::CONFLICT,
            format!("this change was {} already", p["status"].as_str().unwrap_or("done")),
        ));
    }
    let (tool, args) = (p["tool"].as_str().unwrap_or_default().to_string(), p["args"].clone());
    let caller = Arc::new(caller);
    let tools = crate::api::mcp::tools_for(&caller, &CHAT_TOOLS);
    let result = match tools.iter().find(|t| t.name == tool && t.writes) {
        Some(_) => crate::api::mcp::call_tool(&state, &caller, &tool, args).await,
        None => Err("you can't make this change".into()),
    };
    match &result {
        Ok(v) => {
            p["status"] = json!("applied");
            p["result"] = v.clone();
        }
        Err(e) => {
            p["status"] = json!("failed");
            p["error"] = json!(e);
        }
    }
    let out = p.clone();
    let store = state.orgs.store(caller.org()).await?;
    store
        .put_chat_conversation(
            caller.org(),
            caller.email(),
            &id,
            &title,
            &turns,
            &serde_json::to_string(&shown).map_err(anyhow::Error::from)?,
        )
        .await?;
    state.usage.record(crate::usage::Event {
        detail: Some(json!({"tool": tool, "ok": result.is_ok()})),
        ..caller.event("chat_change")
    });
    Ok(Json(out))
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use axum::http::{HeaderMap, StatusCode};
    use serde_json::{Value, json};

    use crate::api::share::tests::{clean_up, org_with, send};

    const DASHBOARD: &str = "title: Chat test\ntiles: []\n";

    /// What a fake provider was asked: (headers, body), in order.
    pub(crate) type Asked = Arc<Mutex<Vec<(HeaderMap, Value)>>>;

    /// A model's API on 127.0.0.1 that answers from a script, and records what it's asked.
    pub(crate) async fn fake_provider(replies: Vec<Value>) -> (u16, Asked) {
        let script = Arc::new(Mutex::new(VecDeque::from(replies)));
        let asked: Asked = Arc::default();
        let (s, a) = (script.clone(), asked.clone());
        let app = axum::Router::new().fallback(axum::routing::post(
            move |h: HeaderMap, axum::Json(body): axum::Json<Value>| {
                let (s, a) = (s.clone(), a.clone());
                async move {
                    a.lock().unwrap().push((h, body));
                    axum::Json(
                        s.lock().unwrap().pop_front().unwrap_or(json!({"error": {"message": "no more replies"}})),
                    )
                }
            },
        ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (port, asked)
    }

    /// An OpenAI-format answer: some text, or a tool call.
    pub(crate) fn says(text: &str) -> Value {
        json!({"choices": [{"message": {"role": "assistant", "content": text}}]})
    }
    pub(crate) fn calls(id: &str, name: &str, args: Value) -> Value {
        json!({"choices": [{"message": {"role": "assistant", "content": null,
            "tool_calls": [{"id": id, "type": "function", "function": {"name": name, "arguments": args.to_string()}}]}}]})
    }

    async fn set_up(app: &axum::Router, cookie: &str, org: &str, format: &str, port: u16, extra: Value) {
        let mut s = json!({"enabled": true, "format": format, "url": format!("http://127.0.0.1:{port}/v1"), "model": "test-model"});
        for (k, v) in extra.as_object().unwrap() {
            s[k] = v.clone();
        }
        let (st, b, _) = send(
            app,
            Some(cookie),
            org,
            "PUT",
            "/api/chat/settings",
            Some(json!({"settings": s, "key": "sk-test-key"})),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{b}");
    }

    #[tokio::test]
    async fn org_admins_set_up_the_chat_and_its_key_stays_hidden() {
        let Some((app, state, cookie, org)) = org_with(DASHBOARD).await else { return };
        let c = Some(cookie.as_str());
        let s = json!({"enabled": true, "format": "openai", "url": "https://api.openai.com/v1", "model": "gpt-4.1"});
        let body = json!({"settings": s, "key": "sk-very-secret-key"});
        // Org admins only
        let (st, _, _) = send(&app, c, &org, "PUT", "/api/chat/settings", Some(body.clone()), Some("power_user")).await;
        assert_eq!(st, StatusCode::FORBIDDEN);
        let (st, _, _) = send(&app, c, &org, "GET", "/api/chat/settings", None, Some("power_user")).await;
        assert_eq!(st, StatusCode::FORBIDDEN);
        // a dry run checks; saving keeps the key sealed, never shown
        let (st, b, _) = send(&app, c, &org, "PUT", "/api/chat/settings?dry_run=1", Some(body.clone()), None).await;
        assert_eq!((st, &b["dry_run"]), (StatusCode::OK, &json!(true)), "{b}");
        let (_, g, _) = send(&app, c, &org, "GET", "/api/chat/settings", None, None).await;
        assert_eq!(g["settings"], Value::Null);
        send(&app, c, &org, "PUT", "/api/chat/settings", Some(body), None).await;
        let (_, g, _) = send(&app, c, &org, "GET", "/api/chat/settings", None, None).await;
        assert_eq!((&g["settings"]["model"], &g["has_key"]), (&json!("gpt-4.1"), &json!(true)), "{g}");
        assert!(!g.to_string().contains("sk-very-secret"), "{g}");
        let rows = state
            .store
            .pool
            .get()
            .await
            .unwrap()
            .query_one("SELECT key_sealed FROM chat_settings WHERE org = $1", &[&org])
            .await
            .unwrap();
        assert!(!rows.get::<_, String>(0).contains("sk-very-secret"));
        // changed without a key: the key stays
        let mut again = json!({"settings": s});
        again["settings"]["rows_to_model"] = json!(50);
        send(&app, c, &org, "PUT", "/api/chat/settings", Some(again), None).await;
        let (_, g, _) = send(&app, c, &org, "GET", "/api/chat/settings", None, None).await;
        assert_eq!((&g["settings"]["rows_to_model"], &g["has_key"]), (&json!(50), &json!(true)));
        // the key never goes to another provider on its own: a new address needs it again
        let mut moved = json!({"settings": s});
        moved["settings"]["url"] = json!("https://api.elsewhere.example/v1");
        let (status, b, _) = send(&app, c, &org, "PUT", "/api/chat/settings", Some(moved), None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{b}");
        assert!(b["detail"].as_str().unwrap().contains("again"), "{b}");
        // checked
        for bad in [json!({"format": "gemini"}), json!({"url": "ftp://x"}), json!({"model": ""})] {
            let mut b = json!({"settings": s});
            for (k, v) in bad.as_object().unwrap() {
                b["settings"][k] = v.clone();
            }
            let (st, r, _) = send(&app, c, &org, "PUT", "/api/chat/settings", Some(b), None).await;
            assert_eq!(st, StatusCode::BAD_REQUEST, "{bad} -> {r}");
        }
        // members see whether it's on, not how
        let (_, l, _) = send(&app, c, &org, "GET", "/api/chat", None, Some("viewer")).await;
        assert_eq!((&l["enabled"], &l["home"]), (&json!(true), &json!("dashboards")), "{l}");
        assert!(!l.to_string().contains("openai"), "{l}");
        // each person may start somewhere else than the organisation's default, and go back to it
        let (st, _, _) = send(&app, c, &org, "PUT", "/api/chat/home", Some(json!({"home": "chat"})), None).await;
        assert_eq!(st, StatusCode::OK);
        let (_, l, _) = send(&app, c, &org, "GET", "/api/chat", None, None).await;
        assert_eq!(
            (&l["home"], &l["org_home"], &l["my_home"]),
            (&json!("chat"), &json!("dashboards"), &json!("chat")),
            "{l}"
        );
        let (st, _, _) = send(&app, c, &org, "PUT", "/api/chat/home", Some(json!({"home": "attic"})), None).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        send(&app, c, &org, "PUT", "/api/chat/home", Some(json!({"home": null})), None).await;
        let (_, l, _) = send(&app, c, &org, "GET", "/api/chat", None, None).await;
        assert_eq!((&l["home"], &l["my_home"]), (&json!("dashboards"), &Value::Null), "{l}");
        clean_up(&state, &org).await;
    }

    #[tokio::test]
    async fn questions_are_answered_from_the_data_with_little_of_it_sent() {
        let Some((app, state, cookie, org)) = org_with(DASHBOARD).await else { return };
        let c = Some(cookie.as_str());
        // the store is personal here: its values stay home
        let docs = json!({"fields": {"store": {"tags": ["pii"]}}});
        send(&app, c, &org, "PUT", "/api/governance/sales/docs", Some(docs), None).await;
        let query = json!({"model": "sales", "dimensions": ["region", "store"], "measures": ["revenue"]});
        let (port, asked) = fake_provider(vec![
            calls("c1", "search_catalog", json!({"q": "revenue"})),
            calls("c2", "run_query", json!({"query": query})),
            says("Revenue is highest in EMEA."),
            says("APAC comes second."),
        ])
        .await;
        set_up(&app, &cookie, &org, "openai", port, json!({"rows_to_model": 2})).await;

        let (st, r, _) =
            send(&app, c, &org, "POST", "/api/chat", Some(json!({"message": "Where is revenue highest?"})), None).await;
        assert_eq!(st, StatusCode::OK, "{r}");
        assert_eq!(r["reply"]["text"], json!("Revenue is highest in EMEA."));
        assert_eq!(r["reply"]["charts"][0]["query"], query);
        let id = r["conversation"].as_str().unwrap().to_string();
        {
            let asked = asked.lock().unwrap();
            assert_eq!(asked.len(), 3);
            let (headers, first) = &asked[0];
            assert_eq!(headers["authorization"], "Bearer sk-test-key");
            assert_eq!(first["model"], json!("test-model"));
            let tools: Vec<&str> =
                first["tools"].as_array().unwrap().iter().map(|t| t["function"]["name"].as_str().unwrap()).collect();
            assert!(tools.contains(&"run_query") && tools.contains(&"create_report"), "{tools:?}");
            assert!(
                !tools.contains(&"save_members") && !tools.contains(&"save_connection"),
                "admin tools stay out: {tools:?}"
            );
            // what the provider got of the result: two rows, no store, no SQL
            let messages = asked[2].1["messages"].as_array().unwrap();
            let result: Value = serde_json::from_str(messages.last().unwrap()["content"].as_str().unwrap()).unwrap();
            assert_eq!((&result["rows_shown"], &result["left_out"]), (&json!(2), &json!(["store"])), "{result}");
            assert!(result["rows_total"].as_u64().unwrap() > 2 && result.get("sql").is_none(), "{result}");
        }
        // a follow-up carries the conversation
        let (st, r, _) =
            send(&app, c, &org, "POST", "/api/chat", Some(json!({"conversation": id, "message": "And second?"})), None)
                .await;
        assert_eq!((st, &r["reply"]["text"]), (StatusCode::OK, &json!("APAC comes second.")), "{r}");
        assert!(asked.lock().unwrap()[3].1["messages"].as_array().unwrap().len() > 6);
        // the person's own: listed, shown, gone
        let (_, l, _) = send(&app, c, &org, "GET", "/api/chat", None, None).await;
        assert_eq!(l["conversations"][0]["title"], json!("Where is revenue highest?"));
        let (_, conv, _) = send(&app, c, &org, "GET", &format!("/api/chat/{id}"), None, None).await;
        assert_eq!(conv["messages"].as_array().unwrap().len(), 4, "{conv}");
        let (st, _, _) = send(&app, c, &org, "DELETE", &format!("/api/chat/{id}"), None, None).await;
        assert_eq!(st, StatusCode::OK);
        clean_up(&state, &org).await;
    }

    #[tokio::test]
    async fn anthropic_format_works_too() {
        let Some((app, state, cookie, org)) = org_with(DASHBOARD).await else { return };
        let (port, asked) = fake_provider(vec![
            json!({"content": [{"type": "tool_use", "id": "t1", "name": "list_models", "input": {}}], "stop_reason": "tool_use"}),
            json!({"content": [{"type": "text", "text": "There's a sales model."}], "stop_reason": "end_turn"}),
        ])
        .await;
        set_up(&app, &cookie, &org, "anthropic", port, json!({})).await;
        let (st, r, _) =
            send(&app, Some(&cookie), &org, "POST", "/api/chat", Some(json!({"message": "What can I ask?"})), None)
                .await;
        assert_eq!((st, &r["reply"]["text"]), (StatusCode::OK, &json!("There's a sales model.")), "{r}");
        {
            let asked = asked.lock().unwrap();
            assert_eq!(asked[0].0["x-api-key"], "sk-test-key");
            assert_eq!(asked[1].1["messages"][2]["content"][0]["type"], json!("tool_result"));
        }
        clean_up(&state, &org).await;
    }

    #[tokio::test]
    async fn changes_are_proposals_the_person_confirms() {
        let Some((app, state, cookie, org)) = org_with(DASHBOARD).await else { return };
        let c = Some(cookie.as_str());
        let f = json!({"model": "sales", "formula": {"name": "per_sale", "expr": "revenue / transactions"}});
        let wrong = json!({"model": "sales", "formula": {"name": "bad", "expr": "revnue / 2"}});
        let (port, asked) = fake_provider(vec![
            calls("c1", "save_formula", wrong),
            calls("c2", "save_formula", f),
            says("I've proposed a shared formula, per_sale."),
            says("Done."),
        ])
        .await;
        set_up(&app, &cookie, &org, "openai", port, json!({})).await;
        let (st, r, _) =
            send(&app, c, &org, "POST", "/api/chat", Some(json!({"message": "Share revenue per sale"})), None).await;
        assert_eq!(st, StatusCode::OK, "{r}");
        // a mistake went back to the model; the fixed change waits for the person
        let back = asked.lock().unwrap()[1].1["messages"].as_array().unwrap().last().unwrap()["content"].clone();
        assert!(back.as_str().unwrap().contains("did you mean revenue"), "{back}");
        let p = &r["reply"]["proposals"][0];
        assert_eq!(
            (&p["tool"], &p["status"], &p["summary"]),
            (&json!("save_formula"), &json!("proposed"), &json!("Share the formula per_sale on sales")),
            "{r}"
        );
        let (_, list, _) = send(&app, c, &org, "GET", "/api/formulas", None, None).await;
        assert_eq!(list["formulas"], json!([]), "nothing made yet");
        // confirmed: made, once
        let path = format!("/api/chat/{}/proposals/{}", r["conversation"].as_str().unwrap(), p["id"].as_str().unwrap());
        let (st, done, _) = send(&app, c, &org, "POST", &path, None, None).await;
        assert_eq!((st, &done["status"]), (StatusCode::OK, &json!("applied")), "{done}");
        let (_, list, _) = send(&app, c, &org, "GET", "/api/formulas", None, None).await;
        assert_eq!(list["formulas"][0]["formula"]["name"], json!("per_sale"));
        let (st, _, _) = send(&app, c, &org, "POST", &path, None, None).await;
        assert_eq!(st, StatusCode::CONFLICT);
        // the next question tells the model it was done
        send(
            &app,
            c,
            &org,
            "POST",
            "/api/chat",
            Some(json!({"conversation": r["conversation"], "message": "Thanks"})),
            None,
        )
        .await;
        let told = asked.lock().unwrap()[3].1["messages"]
            .as_array()
            .unwrap()
            .iter()
            .rev()
            .find(|m| m["role"] == "user")
            .unwrap()["content"]
            .clone();
        assert!(told.as_str().unwrap().contains("I confirmed: Share the formula per_sale"), "{told}");
        clean_up(&state, &org).await;
    }

    #[test]
    fn the_rules_are_plain_text() {
        // after the organisation's own instructions, then sentences (no stray escapes)
        assert!(super::CHAT_RULES.starts_with("\n\nYou are the chat"));
        assert!(!super::CHAT_RULES.contains('\\'), "{}", super::CHAT_RULES);
        assert!(super::CHAT_RULES.contains("never guess a meaning"));
    }

    #[tokio::test]
    async fn the_chat_fills_the_catalog_once_confirmed() {
        let Some((app, state, cookie, org)) = org_with(DASHBOARD).await else { return };
        let c = Some(cookie.as_str());
        let docs = json!({"model": "sales", "docs": {"description": "Sales in our shops, one row a sale.",
            "fields": {"region": {"description": "The shop's sales region.", "examples": ["EMEA", "APAC"]},
                       "revenue": {"description": "What customers paid, tax included.", "synonyms": ["turnover"]}}}});
        let wrong = json!({"model": "sales", "docs": {"fields": {"nope": {"description": "?"}}}});
        let term = json!({"term": {"name": "Region", "definition": "Where a shop is: EMEA, APAC or AMER."}});
        let (port, asked) = fake_provider(vec![
            calls("c1", "save_docs", wrong),
            calls("c2", "save_docs", docs),
            calls("c3", "save_term", term),
            says("I've proposed the documentation and a glossary term."),
        ])
        .await;
        set_up(&app, &cookie, &org, "openai", port, json!({})).await;
        let (st, r, _) =
            send(&app, c, &org, "POST", "/api/chat", Some(json!({"message": "Fill the catalog for sales"})), None)
                .await;
        assert_eq!(st, StatusCode::OK, "{r}");
        // a field that isn't there went back to the model
        let back = asked.lock().unwrap()[1].1["messages"].as_array().unwrap().last().unwrap()["content"].clone();
        assert!(back.as_str().unwrap().contains("nope"), "{back}");
        let ps = r["reply"]["proposals"].as_array().unwrap().clone();
        assert_eq!(ps.len(), 2, "{r}");
        assert_eq!(ps[0]["summary"], json!("Document the model sales"), "{r}");
        let details = ps[0]["details"].to_string();
        assert!(details.contains("region: The shop's sales region.") && details.contains("revenue"), "{details}");
        assert_eq!(ps[1]["summary"], json!("Add \"Region\" to the glossary"), "{r}");
        let (_, cat, _) = send(&app, c, &org, "GET", "/api/governance", None, None).await;
        assert!(!cat.to_string().contains("one row a sale"), "nothing written yet: {cat}");
        // confirmed: written
        for p in &ps {
            let path =
                format!("/api/chat/{}/proposals/{}", r["conversation"].as_str().unwrap(), p["id"].as_str().unwrap());
            let (st, done, _) = send(&app, c, &org, "POST", &path, None, None).await;
            assert_eq!((st, &done["status"]), (StatusCode::OK, &json!("applied")), "{done}");
        }
        let (_, m, _) = send(&app, c, &org, "GET", "/api/models/sales", None, None).await;
        assert!(m.to_string().contains("one row a sale") && m.to_string().contains("turnover"), "{m}");
        let (_, g, _) = send(&app, c, &org, "GET", "/api/glossary", None, None).await;
        assert!(g.to_string().contains("Where a shop is"), "{g}");
        clean_up(&state, &org).await;
    }

    #[tokio::test]
    async fn the_chat_follows_roles_and_limits() {
        let Some((app, state, cookie, org)) = org_with(DASHBOARD).await else { return };
        let c = Some(cookie.as_str());
        // not set up: said plainly
        let (st, _, _) = send(&app, c, &org, "POST", "/api/chat", Some(json!({"message": "Hi"})), None).await;
        assert_eq!(st, StatusCode::CONFLICT);
        let (port, asked) = fake_provider(vec![
            calls("c1", "run_query", json!({"query": {"model": "sales", "measures": ["revenue"]}})),
            says("I can't run queries for you."),
        ])
        .await;
        set_up(&app, &cookie, &org, "openai", port, json!({"monthly_questions": 1})).await;
        // a viewer doesn't explore: no queries offered, and none run if asked anyway
        let (st, r, _) =
            send(&app, c, &org, "POST", "/api/chat", Some(json!({"message": "Revenue?"})), Some("viewer")).await;
        assert_eq!(st, StatusCode::OK, "{r}");
        assert_eq!(r["reply"]["charts"], json!([]));
        {
            let asked = asked.lock().unwrap();
            let tools: Vec<&str> = asked[0].1["tools"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t["function"]["name"].as_str().unwrap())
                .collect();
            assert!(!tools.contains(&"run_query") && !tools.contains(&"save_formula"), "{tools:?}");
            let back = asked[1].1["messages"].as_array().unwrap().last().unwrap()["content"].clone();
            assert!(back.as_str().unwrap().contains("isn't available"), "{back}");
        }
        // the organisation's monthly questions
        let (st, b, _) = send(&app, c, &org, "POST", "/api/chat", Some(json!({"message": "Again?"})), None).await;
        assert_eq!(st, StatusCode::TOO_MANY_REQUESTS, "{b}");
        clean_up(&state, &org).await;
    }

    /// Attach a file in the chat.
    async fn attach(
        app: &axum::Router,
        cookie: &str,
        org: &str,
        filename: &str,
        bytes: &[u8],
        view_as: Option<&str>,
    ) -> (StatusCode, Value) {
        let mut req = axum::http::Request::builder()
            .method("POST")
            .uri(format!("/api/chat/file?filename={filename}"))
            .header("cookie", cookie)
            .header("x-magnum-org", org)
            .header("content-type", "application/octet-stream");
        if let Some(r) = view_as {
            req = req.header("x-magnum-view-as", r);
        }
        let resp = tower::ServiceExt::oneshot(app.clone(), req.body(axum::body::Body::from(bytes.to_vec())).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let body = http_body_util::BodyExt::collect(resp.into_body()).await.unwrap().to_bytes();
        (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
    }

    /// The organisation's members: `allowed` may add data, `plain` is a power user who may not.
    fn pin_members(state: &crate::state::AppState, org: &str, allowed: &str, plain: &str, on: bool) {
        let access = json!({"system_owners": [crate::api::tests::OWNER], "organisations": {org: {
            "name": "Files", "models": ["*"], "members": {"users": {
                allowed: {"role": "power_user", "add_data": on}, plain: "power_user"}}}}});
        state.access.pin(crate::access::AccessConfig::from_document(access).unwrap());
    }

    /// Two power users of the organisation, signed in: (allowed's email and cookie, plain's cookie).
    async fn power_users(state: &crate::state::AppState, org: &str) -> (String, String, String, String) {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let (allowed, plain) = (format!("allowed-{id}@files.example"), format!("plain-{id}@files.example"));
        pin_members(state, org, &allowed, &plain, true);
        let mut cookies = vec![];
        for e in [&allowed, &plain] {
            state.store.set_password(e, None, "x", None, false).await.unwrap();
            let s = state.store.create_session_as(e, "password", chrono::Duration::hours(1), None).await.unwrap();
            cookies.push(format!("magnum_session={s}"));
        }
        let plain_cookie = cookies.pop().unwrap();
        (allowed, cookies.pop().unwrap(), plain, plain_cookie)
    }

    async fn forget_people(state: &crate::state::AppState, org: &str, people: [&str; 2]) {
        let _ = std::fs::remove_dir_all(crate::connectors::duckdb::org_dir(&state.config.duckdb_dir, org));
        let c = state.store.pool.get().await.unwrap();
        for e in people {
            c.execute("DELETE FROM sessions WHERE user_email = $1::TEXT", &[&e]).await.unwrap();
            c.execute("DELETE FROM users WHERE email = $1::TEXT", &[&e]).await.unwrap();
        }
    }

    const CSV: &[u8] = b"region,amount\nEMEA,10\nAPAC,5\n";

    #[tokio::test]
    async fn a_power_user_allowed_to_add_data_attaches_a_file_and_confirms_its_model() {
        let Some((app, state, cookie, org)) = org_with(DASHBOARD).await else { return };
        let c = Some(cookie.as_str());
        let (allowed, allowed_cookie, plain, plain_cookie) = power_users(&state, &org).await;
        let me = Some(allowed_cookie.as_str());

        // the chat is off: no attaching
        let (st, _) = attach(&app, &allowed_cookie, &org, "orders.csv", CSV, None).await;
        assert_eq!(st, StatusCode::CONFLICT);

        let name = format!("orders_{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
        let (port, _) = fake_provider(vec![]).await;
        set_up(&app, &cookie, &org, "openai", port, json!({})).await;

        // a power user without the flag can't, nor anyone below; nothing is kept
        let (st, b) = attach(&app, &plain_cookie, &org, "orders.csv", CSV, None).await;
        assert_eq!(st, StatusCode::FORBIDDEN, "{b}");
        assert!(b["detail"].as_str().unwrap().contains("Org admin"), "{b}");
        for role in ["viewer", "user"] {
            let (st, _) = attach(&app, &allowed_cookie, &org, "orders.csv", CSV, Some(role)).await;
            assert_eq!(st, StatusCode::FORBIDDEN, "viewing as {role}");
        }
        let (_, before, _) = send(&app, c, &org, "GET", "/api/connections", None, None).await;
        assert!(!before.to_string().contains("file_orders"), "{before}");
        // what the screens go by
        let (_, who, _) = send(&app, me, &org, "GET", "/api/auth/me", None, None).await;
        assert!(who["permissions"].to_string().contains("add_data"), "{who}");
        let (_, who, _) = send(&app, Some(&plain_cookie), &org, "GET", "/api/auth/me", None, None).await;
        assert!(!who["permissions"].to_string().contains("add_data"), "{who}");
        // Org admins can, without the flag
        let (st, b) = attach(&app, &cookie, &org, "admin.csv", CSV, Some("org_admin")).await;
        assert_eq!(st, StatusCode::OK, "{b}");

        let (st, up) = attach(&app, &allowed_cookie, &org, "orders.csv", CSV, None).await;
        assert_eq!(st, StatusCode::OK, "{up}");
        let connection = up["connection"].as_str().unwrap().to_string();
        assert!(connection.starts_with("file_orders_"), "a name of its own: {up}");
        assert_eq!(up["tables"], json!(["orders"]));

        // the wider right is this route and tool only: everything else stays with Org admins
        assert_eq!(
            send(&app, me, &org, "GET", "/api/connections/warehouse/tables", None, None).await.0,
            StatusCode::FORBIDDEN
        );
        let own_tables = format!("/api/connections/{connection}/tables");
        assert_eq!(send(&app, me, &org, "GET", &own_tables, None, None).await.0, StatusCode::FORBIDDEN);

        // the provider is told the columns, not the rows; the model is proposed, not made
        let args = json!({"connection": connection, "table": "orders", "name": name, "label": "Orders"});
        let (port, asked) =
            fake_provider(vec![calls("c1", "add_file_model", args), says("I've proposed a model.")]).await;
        set_up(&app, &cookie, &org, "openai", port, json!({})).await;
        let q = json!({"message": "Add this file", "file": connection});
        let (st, r, _) = send(&app, me, &org, "POST", "/api/chat", Some(q), None).await;
        assert_eq!(st, StatusCode::OK, "{r}");
        {
            let asked = asked.lock().unwrap();
            let first = asked[0].1["messages"].to_string();
            assert!(first.contains(&connection) && first.contains("region") && first.contains("amount"), "{first}");
            assert!(!first.contains("EMEA"), "no rows: {first}");
            assert!(asked[0].1["tools"].to_string().contains("add_file_model"));
        }
        let p = r["reply"]["proposals"][0].clone();
        assert_eq!(p["summary"], json!(format!("Add the file's table orders as the model {name}")), "{r}");
        let details = p["details"].to_string();
        assert!(details.contains("everyone") && details.contains("Region") && details.contains("Amount"), "{details}");
        let conversation = r["conversation"].as_str().unwrap();
        let (_, shown, _) = send(&app, me, &org, "GET", &format!("/api/chat/{conversation}"), None, None).await;
        assert_eq!(shown["messages"][0]["file"], json!("orders.csv"), "{shown}");
        let (_, models, _) = send(&app, me, &org, "GET", "/api/models", None, None).await;
        assert!(!models.to_string().contains(&name), "nothing made yet");

        // confirmed: everyone's, its maker's to own, and it answers
        let path = format!("/api/chat/{conversation}/proposals/{}", p["id"].as_str().unwrap());
        let (st, done, _) = send(&app, me, &org, "POST", &path, None, None).await;
        assert_eq!((st, &done["status"]), (StatusCode::OK, &json!("applied")), "{done}");
        let (_, models, _) = send(&app, Some(&plain_cookie), &org, "GET", "/api/models", None, None).await;
        assert!(models.to_string().contains(&name), "{models}");
        let query = json!({"model": name, "dimensions": ["region"], "measures": ["amount"]});
        let (st, rows, _) = send(&app, me, &org, "POST", "/api/query", Some(query), None).await;
        assert_eq!(st, StatusCode::OK, "{rows}");
        assert!(rows.to_string().contains("EMEA"), "{rows}");
        let (_, g, _) = send(&app, c, &org, "GET", "/api/governance", None, None).await;
        assert!(g.to_string().contains(&allowed), "{g}");
        forget_people(&state, &org, [&allowed, &plain]).await;
        clean_up(&state, &org).await;
    }

    #[tokio::test]
    async fn taking_the_flag_away_stops_what_was_waiting() {
        let Some((app, state, cookie, org)) = org_with(DASHBOARD).await else { return };
        let (allowed, allowed_cookie, plain, _) = power_users(&state, &org).await;
        let me = Some(allowed_cookie.as_str());
        let (port, _) = fake_provider(vec![]).await;
        set_up(&app, &cookie, &org, "openai", port, json!({})).await;
        let (_, up) = attach(&app, &allowed_cookie, &org, "orders.csv", CSV, None).await;
        let connection = up["connection"].as_str().unwrap().to_string();
        let name = format!("orders_{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
        let args = json!({"connection": connection, "table": "orders", "name": name});
        let (port, _) = fake_provider(vec![calls("c1", "add_file_model", args), says("Proposed.")]).await;
        set_up(&app, &cookie, &org, "openai", port, json!({})).await;
        let (_, r, _) = send(&app, me, &org, "POST", "/api/chat", Some(json!({"message": "Add it"})), None).await;
        let p = r["reply"]["proposals"][0].clone();
        assert_eq!(p["status"], json!("proposed"), "{r}");

        pin_members(&state, &org, &allowed, &plain, false);
        let path = format!("/api/chat/{}/proposals/{}", r["conversation"].as_str().unwrap(), p["id"].as_str().unwrap());
        let (st, done, _) = send(&app, me, &org, "POST", &path, None, None).await;
        assert_eq!((st, &done["status"]), (StatusCode::OK, &json!("failed")), "{done}");
        let (_, models, _) = send(&app, me, &org, "GET", "/api/models", None, None).await;
        assert!(!models.to_string().contains(&name), "not made");
        // nor can it name that file in a question, or attach another
        let q = json!({"message": "Add it", "file": connection});
        assert_eq!(send(&app, me, &org, "POST", "/api/chat", Some(q), None).await.0, StatusCode::FORBIDDEN);
        assert_eq!(attach(&app, &allowed_cookie, &org, "x.csv", CSV, None).await.0, StatusCode::FORBIDDEN);
        forget_people(&state, &org, [&allowed, &plain]).await;
        clean_up(&state, &org).await;
    }

    #[tokio::test]
    async fn the_chat_makes_models_only_from_your_own_attached_file() {
        let Some((app, state, cookie, org)) = org_with(DASHBOARD).await else { return };
        let (allowed, allowed_cookie, plain, _) = power_users(&state, &org).await;
        let me = Some(allowed_cookie.as_str());
        // someone else's file, on this organisation's connections
        let store = state.orgs.store(&org).await.unwrap();
        let theirs = format!("salaries_{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
        let config: crate::connections::ConnectionConfig = serde_json::from_value(json!({
            "type": "duckdb", "file": "salaries.csv", "tables": {"salaries": format!("files/{theirs}/salaries.csv")}}))
        .unwrap();
        state.connectors.save(&store, &org, &theirs, config, None, Some("someone@else.example")).await.unwrap();

        let (port, _) = fake_provider(vec![]).await;
        set_up(&app, &cookie, &org, "openai", port, json!({})).await;
        let (st, up) = attach(&app, &allowed_cookie, &org, "orders.csv", CSV, None).await;
        assert_eq!(st, StatusCode::OK, "{up}");
        let mine = up["connection"].as_str().unwrap().to_string();

        // not theirs, not a real connection, not a table outside the file, not a taken name
        let fresh = format!("m_{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
        let attempts = [
            (json!({"connection": theirs, "table": "salaries", "name": fresh}), "isn't a file you attached"),
            (json!({"connection": "warehouse", "table": "sample_sales", "name": fresh}), "isn't a file you attached"),
            (json!({"connection": mine, "table": "sample_sales", "name": fresh}), "has no table"),
            (json!({"connection": mine, "table": "orders", "name": "sales"}), "is taken"),
            (json!({"connection": mine, "table": "orders", "name": "bad name"}), "name"),
        ];
        for (args, says_back) in &attempts {
            let (port, asked) = fake_provider(vec![calls("c1", "add_file_model", args.clone()), says("no")]).await;
            set_up(&app, &cookie, &org, "openai", port, json!({})).await;
            let (st, r, _) = send(&app, me, &org, "POST", "/api/chat", Some(json!({"message": "Add it"})), None).await;
            assert_eq!(st, StatusCode::OK, "{r}");
            assert_eq!(r["reply"]["proposals"], json!([]), "{args} -> {r}");
            let back = asked.lock().unwrap()[1].1["messages"].as_array().unwrap().last().unwrap()["content"].clone();
            assert!(back.as_str().unwrap().contains(says_back), "{args} -> {back}");
        }

        // a file named in a question must be your own, too, and you must still be able to add it
        let (port, _) = fake_provider(vec![says("no")]).await;
        set_up(&app, &cookie, &org, "openai", port, json!({})).await;
        for (file, view_as) in [(theirs.as_str(), None), ("warehouse", None), (mine.as_str(), Some("viewer"))] {
            let q = json!({"message": "Add it", "file": file});
            let (st, r, _) = send(&app, me, &org, "POST", "/api/chat", Some(q), view_as).await;
            assert!(st == StatusCode::BAD_REQUEST || st == StatusCode::FORBIDDEN, "{file}: {st} {r}");
        }
        // what isn't a spreadsheet; a file's name can't reach the path
        let (st, _) = attach(&app, &allowed_cookie, &org, "notes.txt", b"hello", None).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        let (st, up) = attach(&app, &allowed_cookie, &org, "..%2F..%2Fevil.csv", CSV, None).await;
        assert_eq!(st, StatusCode::OK, "{up}");
        assert!(up["connection"].as_str().unwrap().starts_with("file_evil_"), "{up}");
        forget_people(&state, &org, [&allowed, &plain]).await;
        clean_up(&state, &org).await;
    }

    #[test]
    fn an_attached_file_gets_a_name_of_its_own() {
        use super::super::files::attached_name;
        assert_eq!(attached_name("Sales 2026.xlsx", "ab12"), "file_sales_2026_ab12");
        assert_eq!(attached_name("../../etc/passwd.csv", "ab12"), "file_passwd_ab12");
        assert_eq!(attached_name("C:\\docs\\Q1 é.csv", "ab12"), "file_q1_ab12");
        assert_eq!(attached_name(".csv", "ab12"), "file_ab12");
        let long = attached_name(&format!("{}.csv", "x".repeat(200)), "ab12");
        assert!(long.len() <= 63 && crate::connections::check_name(&long).is_ok(), "{long}");
    }
}
