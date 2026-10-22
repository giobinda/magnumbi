//! The in-app chat: questions answered by the organisation's own model (its provider and key,
//! set by its Org admins), with Magnum BI's tools.
//!
//! The tools are MCP's (api/mcp.rs), run as the person asking: the API handlers decide what they
//! may do, and their data restrictions apply as everywhere. Only analytics tools are offered
//! (no members, connections, themes or delivery settings). A change is never made in the loop:
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
const CHAT_TOOLS: [&str; 24] = [
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
];

/// Tool rounds per question, before the chat gives up.
const MAX_ROUNDS: usize = 8;
/// The most of a tool's answer (other than a query's rows) the provider sees.
const MAX_TOOL_TEXT: usize = 30_000;

const CHAT_RULES: &str = "\\n\\nYou are the chat inside Magnum BI, answering people in their organisation. Find \\
the right model and measures first (search_catalog with the question's words, then describe_model), then \\
run_query. Answer briefly, in the person's language, with the numbers you got: never invent one. Say which \\
model you used. A query's chart is shown to the person next to your answer, so don't repeat every row. \\
Changes (dashboards, reports, alerts, formulas, blends) are never made by you: calling a tool that changes \\
something shows the person a proposal they confirm or not. Say what you proposed; don't call it again.";

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
    if p.dry_run {
        return Ok(Json(json!({"ok": true, "dry_run": true})));
    }
    let org = caller.org();
    let sealed = match new_key {
        Some(k) => Some(key(&state)?.seal(k, &key_context(org))?),
        None => None,
    };
    let store = state.orgs.store(org).await?;
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
}

/// Ask a question (in a new conversation, or a follow-up): the answer, its charts and the
/// changes proposed.
pub async fn ask(State(state): State<Shared>, Auth(caller): Auth, Body(q): Body<Question>) -> ApiResult<Json<Value>> {
    caller.require("view")?;
    let message = q.message.trim().to_string();
    if message.is_empty() || message.chars().count() > 4000 {
        return Err(ApiError::bad_request("ask a question (up to 4,000 characters)"));
    }
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
    shown.push(json!({"role": "user", "text": message}));
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
        proposals.push(json!({"id": id, "tool": tool.name, "args": args, "summary": summary, "status": "proposed",
                              "check": check.and_then(Result::ok)}));
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
}
