//! MCP (Model Context Protocol) for agents: Claude Desktop, Claude Code, Cursor, ...
//!
//! `POST /api/mcp`, stateless JSON-RPC 2.0 (the streamable HTTP transport, answering with JSON):
//! `initialize`, `ping`, `tools/list`, `tools/call`; notifications are accepted. It sits behind
//! the same session middleware as the rest of the API, so an agent connects with an API token
//! (`Authorization: Bearer mbi_...`) and is that person, in that organisation, with that role.
//!
//! Every tool calls the API handler a person's browser would, with the caller's identity: the
//! permissions, row filters, organisation isolation, dry runs and version checks are the API's
//! own, never re-implemented here. Tools a caller can't use aren't listed; read-only tokens get
//! no writing tools (and are refused them if they call one anyway).

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, Query as UrlQuery, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::dashboards::WriteParams;
use super::{Body, admin, blends, connections, dashboards, models, themes};
use crate::auth::{Auth, Caller};
use crate::error::{ApiError, ApiResult};
use crate::state::Shared;

/// Newest first; a client asking for one of these gets it, others get the newest.
const PROTOCOL_VERSIONS: [&str; 3] = ["2025-11-25", "2025-06-18", "2025-03-26"];

pub(crate) const INSTRUCTIONS: &str = "Magnum BI is a BI tool: semantic models (dimensions and measures over a database \
connection), blends (models joined across databases; their fields are written model.field), and dashboards \
(YAML: tiles, each a query on a model, or `type: text` with markdown `text:`; optional `tabs:`). You act as the person whose API token you use, in their organisation \
and role; data restrictions apply to you as to them.\n\
Explore first: list_models (or search_catalog with the question's words), then describe_model for field names; a model's governance says who \
owns it and its status: prefer certified models, avoid deprecated ones (the note says why); run_query takes a semantic query \
({model, dimensions, measures, filters, sort, limit, period, formulas}), never SQL. Time dimensions take a grain: \
\"sold_at:month\" (hour, day, week, month, quarter, year).\n\
To change a dashboard: get_dashboard (YAML and its version), edit the YAML, validate_dashboard (a dry_run that \
saves nothing and lists warnings such as unknown fields), then save_dashboard with the version you read; if it \
was changed meanwhile the save is refused: read it again and redo your change. New dashboards: version \"none\". \
Members and blends work the same way (dry_run, version). Deleted and overwritten dashboards stay in \
dashboard_history. Never ask people for passwords or secrets in chat.";

struct Tool {
    name: &'static str,
    description: &'static str,
    /// the permission it needs (the handler checks it again)
    perm: &'static str,
    /// changes something (refused to read-only tokens, unless it's a dry run)
    writes: bool,
    schema: fn() -> Value,
}

fn obj(props: Value, required: &[&str]) -> Value {
    json!({"type": "object", "properties": props, "required": required})
}

fn query_schema() -> Value {
    json!({"type": "object", "description": "A semantic query", "properties": {
        "model": {"type": "string", "description": "a model or blend name (list_models)"},
        "dimensions": {"type": "array", "items": {"type": "string"}, "description": "e.g. [\"region\", \"sold_at:month\"]; blends: \"model.field\""},
        "measures": {"type": "array", "items": {"type": "string"}},
        "filters": {"type": "array", "items": {"type": "object", "properties": {
            "field": {"type": "string"},
            "op": {"type": "string", "enum": ["equals", "not_equals", "contains", "gt", "gte", "lt", "lte", "between", "is_null", "not_null", "last_n_days"]},
            "values": {"type": "array"}}, "required": ["field"]}},
        "sort": {"type": "array", "items": {"type": "string"}, "description": "field names; \"-\" prefix for descending"},
        "limit": {"type": "integer", "minimum": 1, "maximum": 50000},
        "period": {"type": "object", "description": "{field, current: {last_n_days | preset | from, to}, compare: previous_period | previous_year}"},
        "top": {"type": "object", "description": "{dimension, n, by, bottom?}"},
        "formulas": {"type": "array", "description": "calculations on the model's measures, each a column: \
[{name, expr, label?, format?}]; expr like \"(revenue - cost) / revenue\" or \"sum(amount) / count_distinct(store)\"; \
+ - * / (x / 0 is empty), comparisons, and/or/not, if(cond, a, b), coalesce, round(x, places), abs, least, greatest. \
per_row: true makes a field of each row from the model's columns (e.g. \"amount * qty\", \"if(region = 'EMEA', 'home', 'away')\", \
with 'text', lower, upper, concat): group by it, filter on it, or sum(it). \
Calculations on the results (computed after the query, on its rows): running_sum(x), moving_avg(x, n), previous(x), \
rank(x), percent_of_total(x): along the first date dimension (else the first dimension), for each value of the others",
            "items": {"type": "object", "properties": {"name": {"type": "string"}, "expr": {"type": "string"},
                "label": {"type": "string"}, "format": {"type": "string", "enum": ["number", "currency", "percent"]},
                "per_row": {"type": "boolean"}},
                "required": ["name", "expr"]}}},
        "required": ["model"]})
}

fn dashboard_id() -> Value {
    json!({"type": "string", "description": "dashboard id (letters, digits, - and _)"})
}

fn tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "list_models",
            perm: "view",
            writes: false,
            schema: || obj(json!({}), &[]),
            description: "The models and blends you can query, with their dimensions and measures.",
        },
        Tool {
            name: "describe_model",
            perm: "view",
            writes: false,
            schema: || obj(json!({"name": {"type": "string"}}), &["name"]),
            description: "One model or blend: its dimensions (with types), measures (with formats) and drill hierarchies, \
with the organisation's documentation (descriptions, synonyms, examples, tags, glossary terms) and governance.",
        },
        Tool {
            name: "search_catalog",
            perm: "view",
            writes: false,
            schema: || {
                obj(json!({"q": {"type": "string", "description": "words, e.g. \"churn\" or \"net revenue\""}}), &["q"])
            },
            description: "Find models, fields, glossary terms and dashboards by name, other words people use (synonyms, \
terms, tags) or description: to find the right measure for a question.",
        },
        Tool {
            name: "get_glossary",
            perm: "view",
            writes: false,
            schema: || obj(json!({}), &[]),
            description: "The organisation's business terms: definitions, synonyms, and the fields (model.field) that implement each.",
        },
        Tool {
            name: "run_query",
            perm: "explore",
            writes: false,
            schema: || obj(json!({"query": query_schema()}), &["query"]),
            description: "Run a semantic query and get the rows (and the SQL if your role may see it). Data restrictions apply.",
        },
        Tool {
            name: "preview_sql",
            perm: "view_sql",
            writes: false,
            schema: || obj(json!({"query": query_schema()}), &["query"]),
            description: "The SQL a semantic query would run, without running it.",
        },
        Tool {
            name: "list_dashboards",
            perm: "view",
            writes: false,
            schema: || obj(json!({}), &[]),
            description: "The organisation's dashboards: id, title, number of tiles.",
        },
        Tool {
            name: "get_dashboard",
            perm: "view",
            writes: false,
            schema: || obj(json!({"id": dashboard_id()}), &["id"]),
            description: "A dashboard as YAML (if you may edit), its parsed form, warnings, and its version (needed to save changes).",
        },
        Tool {
            name: "validate_dashboard",
            perm: "edit",
            writes: false,
            schema: || obj(json!({"id": dashboard_id(), "yaml": {"type": "string"}}), &["id", "yaml"]),
            description: "Check a dashboard's YAML without saving (dry run): errors, and warnings such as tiles using unknown fields.",
        },
        Tool {
            name: "save_dashboard",
            perm: "edit",
            writes: true,
            schema: || {
                obj(
                    json!({"id": dashboard_id(), "yaml": {"type": "string"},
                "version": {"type": "string", "description": "from get_dashboard; \"none\" to create a new one"}}),
                    &["id", "yaml"],
                )
            },
            description: "Create or replace a dashboard from YAML. Pass the version you read: the save is refused if someone changed it meanwhile.",
        },
        Tool {
            name: "delete_dashboard",
            perm: "edit",
            writes: true,
            schema: || obj(json!({"id": dashboard_id(), "version": {"type": "string"}}), &["id"]),
            description: "Delete a dashboard (it stays in its history). Pass the version you read to delete only that.",
        },
        Tool {
            name: "dashboard_history",
            perm: "edit",
            writes: false,
            schema: || obj(json!({"id": dashboard_id()}), &["id"]),
            description: "A dashboard's saved versions (who, when), newest first.",
        },
        Tool {
            name: "list_reports",
            perm: "share",
            writes: false,
            schema: || obj(json!({"dashboard": {"type": "string", "description": "a dashboard id"}}), &["dashboard"]),
            description: "A dashboard's scheduled reports and alerts, the webhooks they can use, and whether email is set up.",
        },
        Tool {
            name: "create_report",
            perm: "share",
            writes: true,
            schema: || {
                obj(
                    json!({"dashboard": {"type": "string"},
                "report": {"type": "object", "description": "{name, format: csv | signed | public | alert, \
when: {every: daily | weekly | monthly | hourly, day?, at: \"HH:MM\", timezone}, recipients: [emails of members], \
webhooks?: [names], request?: {filters, period} (the dashboard's inputs), guest_role?, password? (signed), \
share_link? (public: one of the dashboard's links), alert? ({tile, measure?, op: above | below | rises_by | falls_by, \
value}: for format alert)}"},
                "dry_run": {"type": "boolean", "description": "check it and say what it would be, without making it"}}),
                    &["dashboard", "report"],
                )
            },
            description: "Schedule a report (the dashboard's data, a guest link or a public link, by email or webhooks) \
or an alert (when a tile's number crosses a line). Recipients must be members or allowed domains.",
        },
        Tool {
            name: "pause_report",
            perm: "share",
            writes: true,
            schema: || obj(json!({"id": {"type": "string"}}), &["id"]),
            description: "Pause one of your scheduled reports or alerts (Org admins: anyone's).",
        },
        Tool {
            name: "resume_report",
            perm: "share",
            writes: true,
            schema: || obj(json!({"id": {"type": "string"}}), &["id"]),
            description: "Resume a paused report or alert, from now on.",
        },
        Tool {
            name: "send_report_now",
            perm: "share",
            writes: true,
            schema: || obj(json!({"id": {"type": "string"}}), &["id"]),
            description: "Send a scheduled report now (an alert: check it now).",
        },
        Tool {
            name: "delete_report",
            perm: "share",
            writes: true,
            schema: || obj(json!({"id": {"type": "string"}}), &["id"]),
            description: "Delete a scheduled report or alert.",
        },
        Tool {
            name: "check_formula",
            perm: "explore",
            writes: false,
            schema: || {
                obj(
                    json!({"model": {"type": "string"}, "expr": {"type": "string"},
                "per_row": {"type": "boolean"}, "formulas": {"type": "array", "description": "the query's other formulas"}}),
                    &["model", "expr"],
                )
            },
            description: "Check a formula before using it in run_query: {ok} or {ok: false, error, at (where)}.",
        },
        Tool {
            name: "save_formula",
            perm: "manage_members",
            writes: true,
            schema: || {
                obj(
                    json!({"model": {"type": "string"},
                "formula": {"type": "object", "description": "{name, expr, label?, format?: number | currency | percent, per_row?, description?}"},
                "dry_run": {"type": "boolean"}}),
                    &["model", "formula"],
                )
            },
            description: "Share a formula with the whole organisation, as one of the model's measures (or, per_row, fields).",
        },
        Tool {
            name: "delete_formula",
            perm: "manage_members",
            writes: true,
            schema: || obj(json!({"model": {"type": "string"}, "name": {"type": "string"}}), &["model", "name"]),
            description: "Remove one of the organisation's shared formulas.",
        },
        Tool {
            name: "get_catalog",
            perm: "view",
            writes: false,
            schema: || obj(json!({}), &[]),
            description: "Every model you can see: its owner, stewards, status (certified, draft, deprecated) and note, \
and the organisation's documentation of it.",
        },
        Tool {
            name: "get_lineage",
            perm: "view",
            writes: false,
            schema: || obj(json!({"model": {"type": "string"}}), &["model"]),
            description: "Where a model comes from and where it's used: dashboards, tiles, reports, blends, shared formulas, \
each field's uses, and its queries in the last 30 days.",
        },
        Tool {
            name: "list_blends",
            perm: "view",
            writes: false,
            schema: || obj(json!({}), &[]),
            description: "Blends (models joined on shared fields, across databases): definitions, and which you may edit.",
        },
        Tool {
            name: "save_blend",
            perm: "edit",
            writes: true,
            schema: || {
                obj(
                    json!({"blend": {"type": "object", "description": "{name, label?, base, joins: [{model, on: [\"base_field = field\", \"time:hour = time:hour\"]}], cache?}"},
                "dry_run": {"type": "boolean"}}),
                    &["blend"],
                )
            },
            description: "Create or replace one of the organisation's blends (dry_run: check only).",
        },
        Tool {
            name: "delete_blend",
            perm: "edit",
            writes: true,
            schema: || obj(json!({"name": {"type": "string"}}), &["name"]),
            description: "Delete one of the organisation's blends.",
        },
        Tool {
            name: "list_connections",
            perm: "manage_members",
            writes: false,
            schema: || obj(json!({}), &[]),
            description: "The organisation's data connections (settings only; secrets are never shown).",
        },
        Tool {
            name: "save_connection",
            perm: "manage_members",
            writes: true,
            schema: || {
                obj(
                    json!({"name": {"type": "string"}, "config": {"type": "object", "description": "{type: postgres|scramdb|bigquery|duckdb, ...}"},
                "secret": {"type": "string", "description": "omit to keep the stored one"}}),
                    &["name", "config"],
                )
            },
            description: "Create or update a data connection (it's tested first). Secrets are write-only.",
        },
        Tool {
            name: "delete_connection",
            perm: "manage_members",
            writes: true,
            schema: || obj(json!({"name": {"type": "string"}}), &["name"]),
            description: "Delete a data connection: models reading it stop working here.",
        },
        Tool {
            name: "get_members",
            perm: "manage_members",
            writes: false,
            schema: || obj(json!({}), &[]),
            description: "The organisation's members (roles, personal filters), the default role for its email domains, and the version (needed to save).",
        },
        Tool {
            name: "save_members",
            perm: "manage_members",
            writes: true,
            schema: || {
                obj(
                    json!({"members": {"type": "object", "description": "{default_role, default_filters, users: {email: role or {role, filters}}}: the whole list"},
                "version": {"type": "string"}, "dry_run": {"type": "boolean"}}),
                    &["members"],
                )
            },
            description: "Replace the organisation's member list (the whole of it: read it first). Pass the version you read; dry_run to check only.",
        },
        Tool {
            name: "list_themes",
            perm: "view",
            writes: false,
            schema: || obj(json!({}), &[]),
            description: "The themes here (built-in, project and the organisation's own) and the default light and dark theme.",
        },
        Tool {
            name: "export_theme_css",
            perm: "view",
            writes: false,
            schema: || obj(json!({"id": {"type": "string"}}), &["id"]),
            description: "A theme as CSS design tokens (--mbi-accent, --mbi-chart-1, ...) plus helper classes (.mbi-card, \
                          .mbi-button-primary, ...), for a company's own site or design system.",
        },
        Tool {
            name: "import_theme_css",
            perm: "manage_members",
            writes: false,
            schema: || {
                obj(
                    json!({"css": {"type": "string", "description": "CSS with --mbi-* tokens (as export_theme_css writes)"},
                    "base": {"type": "string", "description": "the theme filling in tokens the CSS leaves out"}}),
                    &["css"],
                )
            },
            description: "Turn CSS design tokens into a theme (checked, not saved): give it an id and label, then save_org_themes.",
        },
        Tool {
            name: "get_org_themes",
            perm: "manage_members",
            writes: false,
            schema: || obj(json!({}), &[]),
            description: "The organisation's own themes, its default light and dark theme, the version (needed to save) and \
                          the themes it can start from.",
        },
        Tool {
            name: "save_org_themes",
            perm: "manage_members",
            writes: true,
            schema: || {
                obj(
                    json!({"themes": {"type": "object", "description": "{themes: [theme, ...], default_light?, default_dark?}: the whole set (read it first)"},
                    "version": {"type": "string"}, "dry_run": {"type": "boolean"}}),
                    &["themes"],
                )
            },
            description: "Replace the organisation's own themes and its defaults (the whole set). Pass the version you read; \
                          dry_run to check only. Theme ids of built-in themes are taken.",
        },
        Tool {
            name: "usage_summary",
            perm: "manage_members",
            writes: false,
            schema: || obj(json!({"days": {"type": "integer", "minimum": 1, "maximum": 400}}), &[]),
            description: "Usage of the organisation: sign-ins, queries, dashboards, failures, over the last days.",
        },
    ]
}

/// Read-only tokens get no writing tools.
fn read_only(caller: &Caller) -> bool {
    caller.session.token.as_ref().is_some_and(|t| !t.write)
}

fn visible(caller: &Caller, t: &Tool) -> bool {
    caller.can(t.perm) && !(t.writes && read_only(caller))
}

fn rpc_result(id: Value, result: Value) -> Response {
    Json(json!({"jsonrpc": "2.0", "id": id, "result": result})).into_response()
}

fn rpc_error(id: Value, code: i64, message: &str) -> Response {
    Json(json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})).into_response()
}

pub async fn endpoint(State(state): State<Shared>, Auth(caller): Auth, body: Bytes) -> Response {
    let Ok(msg) = serde_json::from_slice::<Value>(&body) else {
        return rpc_error(Value::Null, -32700, "parse error");
    };
    if msg.is_array() {
        return rpc_error(Value::Null, -32600, "batches aren't supported");
    }
    let id = msg.get("id").cloned();
    let method = msg["method"].as_str();
    if msg["jsonrpc"] != json!("2.0")
        || (method.is_none() && id.is_some() && msg.get("result").is_none() && msg.get("error").is_none())
    {
        return rpc_error(id.unwrap_or(Value::Null), -32600, "invalid request (JSON-RPC 2.0)");
    }
    // notifications and responses: nothing to answer
    let (Some(method), Some(id)) = (method, id) else { return StatusCode::ACCEPTED.into_response() };
    let params = msg.get("params").cloned().unwrap_or(json!({}));
    match method {
        "initialize" => {
            let asked = params["protocolVersion"].as_str().unwrap_or_default();
            let version = PROTOCOL_VERSIONS.iter().find(|v| **v == asked).unwrap_or(&PROTOCOL_VERSIONS[0]);
            let mut r = rpc_result(
                id,
                json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {"name": "magnum-bi", "title": "Magnum BI", "version": env!("CARGO_PKG_VERSION")},
                    "instructions": INSTRUCTIONS,
                }),
            );
            r.headers_mut().insert("mcp-protocol-version", HeaderValue::from_static(version));
            r
        }
        "ping" => rpc_result(id, json!({})),
        "tools/list" => {
            let list: Vec<Value> = tools()
                .iter()
                .filter(|t| visible(&caller, t))
                .map(|t| json!({"name": t.name, "description": t.description, "inputSchema": (t.schema)(),
                                "annotations": {"readOnlyHint": !t.writes, "destructiveHint": t.name.starts_with("delete_")}}))
                .collect();
            rpc_result(id, json!({"tools": list}))
        }
        "tools/call" => {
            let name = params["name"].as_str().unwrap_or_default();
            let Some(tool) = tools().into_iter().find(|t| t.name == name) else {
                return rpc_error(id, -32602, &format!("unknown tool {name:?}"));
            };
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            let mut out = match run_tool(&state, &caller, &tool, args).await {
                Ok(v) => {
                    json!({"content": [{"type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default()}],
                                "structuredContent": v, "isError": false})
                }
                Err(e) => json!({"content": [{"type": "text", "text": e}], "isError": true}),
            };
            if !out["structuredContent"].is_object() {
                out.as_object_mut().expect("object").remove("structuredContent");
            }
            state.usage.record(crate::usage::Event { detail: Some(json!({"tool": name})), ..caller.event("mcp_tool") });
            rpc_result(id, out)
        }
        _ => rpc_error(id, -32601, &format!("method {method:?} isn't supported")),
    }
}

/// The writing tools that check without saving when given `dry_run: true`.
pub(crate) const DRY_RUNS: [&str; 4] = ["save_blend", "save_members", "create_report", "save_formula"];

/// A tool run as `tools/call` runs it: Ok(what the API answered), or its error as text.
async fn run_tool(state: &Shared, caller: &std::sync::Arc<Caller>, tool: &Tool, args: Value) -> Result<Value, String> {
    // a read-only token may only check: a dry run, on a tool that honours it
    let dry = args["dry_run"].as_bool().unwrap_or(false) && DRY_RUNS.contains(&tool.name);
    if tool.writes && !dry && read_only(caller) {
        return Err(error_text(ApiError::forbidden("this API token is read-only")));
    }
    call(state, caller, tool.name, args).await.map_err(error_text)
}

/// A tool the caller may use, for the in-app chat (api/chat.rs).
pub(crate) struct ToolInfo {
    pub name: &'static str,
    pub description: &'static str,
    pub writes: bool,
    pub schema: Value,
}

/// The tools among `names` this caller may use (as tools/list would show them).
pub(crate) fn tools_for(caller: &Caller, names: &[&str]) -> Vec<ToolInfo> {
    tools()
        .into_iter()
        .filter(|t| names.contains(&t.name) && visible(caller, t))
        .map(|t| ToolInfo { name: t.name, description: t.description, writes: t.writes, schema: (t.schema)() })
        .collect()
}

/// One tool, for the in-app chat: the same checks and handlers as tools/call.
pub(crate) async fn call_tool(
    state: &Shared,
    caller: &std::sync::Arc<Caller>,
    name: &str,
    args: Value,
) -> Result<Value, String> {
    let Some(tool) = tools().into_iter().find(|t| t.name == name && visible(caller, t)) else {
        return Err(format!("the tool {name:?} isn't available to you"));
    };
    run_tool(state, caller, &tool, args).await
}

/// What the API would have answered, as text the agent can act on.
fn error_text(e: ApiError) -> String {
    let (status, body) = e.into_parts();
    let detail = body["detail"].as_str().unwrap_or("failed").to_string();
    match body.as_object().map(|o| o.len()) {
        Some(1) | None => format!("{detail} ({})", status.as_u16()),
        _ => format!("{detail} ({}): {body}", status.as_u16()),
    }
}

fn arg<T: DeserializeOwned>(args: &Value, key: &str) -> ApiResult<T> {
    let v = args.get(key).cloned().ok_or_else(|| ApiError::bad_request(format!("missing argument {key:?}")))?;
    serde_json::from_value(v).map_err(|e| ApiError::bad_request(format!("argument {key:?}: {e}")))
}

fn opt<T: DeserializeOwned>(args: &Value, key: &str) -> ApiResult<Option<T>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(_) => arg(args, key).map(Some),
    }
}

fn if_match(version: Option<String>) -> HeaderMap {
    let mut h = HeaderMap::new();
    if let Some(v) = version.and_then(|v| HeaderValue::from_str(&v).ok()) {
        h.insert(header::IF_MATCH, v);
    }
    h
}

async fn call(state: &Shared, caller: &std::sync::Arc<Caller>, name: &str, args: Value) -> ApiResult<Value> {
    let (s, a) = (|| State(state.clone()), || Auth(caller.clone()));
    let Json(v) = match name {
        "list_models" => models::list(s(), a()).await?,
        "describe_model" => models::get_model(s(), a(), Path(arg(&args, "name")?)).await?,
        "get_glossary" => super::governance::glossary(s(), a()).await?,
        "search_catalog" => {
            let q = arg::<String>(&args, "q")?;
            super::governance::search(s(), a(), UrlQuery(super::governance::SearchParams::of(q))).await?
        }
        "run_query" => models::query(s(), a(), Body(arg(&args, "query")?)).await?,
        "preview_sql" => models::compile_only(s(), a(), Body(arg(&args, "query")?)).await?,
        "list_dashboards" => dashboards::list(s(), a()).await?,
        "get_dashboard" => dashboards::get_dashboard(s(), a(), Path(arg(&args, "id")?)).await?,
        "validate_dashboard" | "save_dashboard" => {
            let dry_run = name == "validate_dashboard";
            let headers = if_match(if dry_run { None } else { opt(&args, "version")? });
            let p = UrlQuery(WriteParams { dry_run });
            dashboards::save_yaml(s(), a(), Path(arg(&args, "id")?), p, headers, arg(&args, "yaml")?).await?
        }
        "delete_dashboard" => {
            dashboards::delete(s(), a(), Path(arg(&args, "id")?), if_match(opt(&args, "version")?)).await?
        }
        "dashboard_history" => dashboards::history(s(), a(), Path(arg(&args, "id")?)).await?,
        "list_blends" => blends::list(a()).await?,
        "list_reports" => super::schedules::list(s(), a(), Path(arg(&args, "dashboard")?)).await?,
        "create_report" => {
            let p = UrlQuery(WriteParams { dry_run: opt(&args, "dry_run")?.unwrap_or(false) });
            super::schedules::create(s(), a(), Path(arg(&args, "dashboard")?), p, Body(arg(&args, "report")?)).await?
        }
        "pause_report" | "resume_report" => {
            let active =
                serde_json::from_value(json!({"active": name == "resume_report"})).map_err(anyhow::Error::from)?;
            super::schedules::set_active(s(), a(), Path(arg(&args, "id")?), Body(active)).await?
        }
        "send_report_now" => super::schedules::run_now(s(), a(), Path(arg(&args, "id")?)).await?,
        "delete_report" => super::schedules::delete(s(), a(), Path(arg(&args, "id")?)).await?,
        "check_formula" => {
            let mut c = args.clone();
            c.as_object_mut().map(|o| o.remove("dry_run"));
            super::formulas::check(
                a(),
                Body(serde_json::from_value(c).map_err(|e| ApiError::bad_request(e.to_string()))?),
            )
            .await?
        }
        "save_formula" => {
            let f: crate::schema::Formula = arg(&args, "formula")?;
            let p = UrlQuery(WriteParams { dry_run: opt(&args, "dry_run")?.unwrap_or(false) });
            super::formulas::save(s(), a(), Path((arg(&args, "model")?, f.name.clone())), p, Body(f)).await?
        }
        "delete_formula" => {
            super::formulas::delete(s(), a(), Path((arg(&args, "model")?, arg(&args, "name")?))).await?
        }
        "get_catalog" => super::governance::list(s(), a()).await?,
        "get_lineage" => super::governance::lineage(s(), a(), Path(arg(&args, "model")?)).await?,
        "save_blend" => {
            let b: crate::blend::Blend = arg(&args, "blend")?;
            let p = UrlQuery(WriteParams { dry_run: opt(&args, "dry_run")?.unwrap_or(false) });
            blends::save(s(), a(), Path(b.name.clone()), p, Body(b)).await?
        }
        "delete_blend" => blends::delete(s(), a(), Path(arg(&args, "name")?)).await?,
        "list_connections" => connections::list(s(), a()).await?,
        "save_connection" => {
            let body = serde_json::from_value(json!({"config": args["config"], "secret": args.get("secret")}))
                .map_err(|e| ApiError::bad_request(format!("config: {e}")))?;
            let p = UrlQuery(WriteParams { dry_run: opt(&args, "dry_run")?.unwrap_or(false) });
            connections::save(s(), a(), Path(arg(&args, "name")?), p, Body(body)).await?
        }
        "delete_connection" => connections::delete(s(), a(), Path(arg(&args, "name")?)).await?,
        "get_members" => admin::get_members(s(), a()).await?,
        "save_members" => {
            let p = UrlQuery(WriteParams { dry_run: opt(&args, "dry_run")?.unwrap_or(false) });
            admin::put_members(s(), a(), p, if_match(opt(&args, "version")?), Body(arg(&args, "members")?)).await?
        }
        "list_themes" => {
            caller.require("view")?;
            let p = &caller.project;
            Json(json!({"themes": p.themes, "default_light": p.theme_defaults.0, "default_dark": p.theme_defaults.1}))
        }
        "export_theme_css" => {
            caller.require("view")?;
            let id: String = arg(&args, "id")?;
            let t = caller
                .project
                .themes
                .iter()
                .find(|t| t.id == id)
                .ok_or_else(|| ApiError::not_found(format!("no theme {id:?}")))?;
            Json(json!({"id": t.id, "css": crate::themes::to_css(t)}))
        }
        "import_theme_css" => {
            let body = serde_json::from_value(json!({"css": args["css"], "base": args.get("base")}))
                .map_err(|e| ApiError::bad_request(e.to_string()))?;
            let Json(t) = themes::import(a(), Body(body)).await?;
            Json(json!(t))
        }
        "get_org_themes" => themes::get_org(s(), a()).await?,
        "save_org_themes" => {
            let p = UrlQuery(WriteParams { dry_run: opt(&args, "dry_run")?.unwrap_or(false) });
            themes::put_org(s(), a(), p, if_match(opt(&args, "version")?), Body(arg(&args, "themes")?)).await?
        }
        "usage_summary" => {
            let days = serde_json::from_value(json!({"days": opt::<i64>(&args, "days")?.unwrap_or(30)}))
                .map_err(|e| ApiError::bad_request(e.to_string()))?;
            admin::usage_summary(s(), a(), UrlQuery(days)).await?
        }
        other => return Err(ApiError::bad_request(format!("unknown tool {other:?}"))),
    };
    Ok(v)
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use serde_json::{Value, json};

    use crate::api::tests::{app, call};

    async fn rpc(app: &axum::Router, token: &str, msg: Value) -> (StatusCode, Value) {
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/api/mcp")
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .body(axum::body::Body::from(msg.to_string()))
            .unwrap();
        let resp = tower::ServiceExt::oneshot(app.clone(), req).await.unwrap();
        let status = resp.status();
        let bytes = http_body_util::BodyExt::collect(resp.into_body()).await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn tool(app: &axum::Router, token: &str, name: &str, args: Value) -> Value {
        let (s, r) = rpc(
            app,
            token,
            json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call",
                                            "params": {"name": name, "arguments": args}}),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{r}");
        assert_eq!(r["id"], json!(7));
        r["result"].clone()
    }

    async fn token(app: &axum::Router, cookie: &str, scope: &str) -> String {
        let (_, t) =
            call(app, cookie, "POST", "/api/tokens", Some(json!({"name": format!("mcp {scope}"), "scope": scope})))
                .await;
        t["token"].as_str().unwrap().to_string()
    }

    #[tokio::test]
    async fn agents_work_through_mcp() {
        let Some((app, state, cookie)) = app().await else { return };
        let (rw, ro) = (token(&app, &cookie, "write").await, token(&app, &cookie, "read").await);

        // handshake
        let (s, init) = rpc(&app, &rw, json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "1"}}})).await;
        assert_eq!(s, StatusCode::OK, "{init}");
        assert_eq!(init["result"]["protocolVersion"], json!("2025-06-18"));
        assert_eq!(init["result"]["serverInfo"]["name"], json!("magnum-bi"));
        assert!(init["result"]["capabilities"]["tools"].is_object());
        assert!(init["result"]["instructions"].as_str().unwrap().contains("dry_run"));
        let (s, _) = rpc(&app, &rw, json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).await;
        assert_eq!(s, StatusCode::ACCEPTED);
        let (_, pong) = rpc(&app, &rw, json!({"jsonrpc": "2.0", "id": 2, "method": "ping"})).await;
        assert_eq!(pong["result"], json!({}));

        // tools: what this caller may do; read-only tokens see no writing tools
        let names = |r: &Value| -> Vec<String> {
            r["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect()
        };
        let (_, all) = rpc(&app, &rw, json!({"jsonrpc": "2.0", "id": 3, "method": "tools/list"})).await;
        let all_names = names(&all);
        for t in [
            "list_models",
            "describe_model",
            "run_query",
            "list_dashboards",
            "get_dashboard",
            "save_dashboard",
            "get_members",
        ] {
            assert!(all_names.contains(&t.to_string()), "{t}: {all_names:?}");
        }
        let save = all["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == json!("save_dashboard"))
            .unwrap()
            .clone();
        assert_eq!(save["inputSchema"]["type"], json!("object"));
        assert!(save["description"].as_str().unwrap().len() > 40);
        let (_, mine) = rpc(&app, &ro, json!({"jsonrpc": "2.0", "id": 4, "method": "tools/list"})).await;
        let ro_names = names(&mine);
        assert!(ro_names.contains(&"run_query".to_string()) && ro_names.contains(&"validate_dashboard".to_string()));
        assert!(!ro_names.contains(&"save_dashboard".to_string()), "{ro_names:?}");

        // reading
        let models = tool(&app, &ro, "list_models", json!({})).await;
        assert_eq!(models["isError"], json!(false));
        assert!(models["structuredContent"]["models"].is_array());
        assert!(
            models["content"][0]["text"].as_str().unwrap().contains("sales"),
            "text for clients without structured output"
        );

        // a dashboard: create (dry run first), read with its version, update, conflict
        let id = format!("mcp_{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
        let yaml =
            "title: From an agent\ntiles:\n  - {id: n, type: kpi, query: {model: sales, measures: [transactions]}}\n";
        let checked = tool(&app, &ro, "validate_dashboard", json!({"id": id, "yaml": yaml})).await;
        assert_eq!(checked["isError"], json!(false), "{checked}");
        assert_eq!(checked["structuredContent"]["dry_run"], json!(true));
        let refused = tool(&app, &ro, "save_dashboard", json!({"id": id, "yaml": yaml})).await;
        assert_eq!(refused["isError"], json!(true), "read-only");
        let made = tool(&app, &rw, "save_dashboard", json!({"id": id, "yaml": yaml, "version": "none"})).await;
        assert_eq!(made["isError"], json!(false), "{made}");
        let got = tool(&app, &rw, "get_dashboard", json!({"id": id})).await;
        let version = got["structuredContent"]["version"].as_str().unwrap().to_string();
        assert!(got["structuredContent"]["yaml"].as_str().unwrap().contains("From an agent"));
        let stale = tool(
            &app,
            &rw,
            "save_dashboard",
            json!({"id": id, "yaml": yaml.replace("agent", "x"), "version": "0123456789abcdef"}),
        )
        .await;
        assert_eq!(stale["isError"], json!(true));
        assert!(stale["content"][0]["text"].as_str().unwrap().contains("changed"), "{stale}");
        let updated = tool(
            &app,
            &rw,
            "save_dashboard",
            json!({"id": id, "yaml": yaml.replace("agent", "assistant"), "version": version}),
        )
        .await;
        assert_eq!(updated["isError"], json!(false), "{updated}");
        let history = tool(&app, &rw, "dashboard_history", json!({"id": id})).await;
        assert!(history["structuredContent"]["history"].as_array().unwrap().len() >= 2);
        assert_eq!(tool(&app, &rw, "delete_dashboard", json!({"id": id})).await["isError"], json!(false));

        // themes: list, CSS out and in, the organisation's own (a dry run here: the shared test
        // database's platform workspace keeps its themes)
        let themes = tool(&app, &ro, "list_themes", json!({})).await;
        assert_eq!(themes["structuredContent"]["default_light"], json!("aloha"), "{themes}");
        let css = tool(&app, &ro, "export_theme_css", json!({"id": "aloha"})).await;
        let css_text = css["structuredContent"]["css"].as_str().unwrap().to_string();
        assert!(css_text.contains("--mbi-accent: #c8102e;"), "{css}");
        let imported = tool(
            &app,
            &rw,
            "import_theme_css",
            json!({"css": css_text.replace("#c8102e", "#0055a5"), "base": "aloha"}),
        )
        .await;
        assert_eq!(imported["structuredContent"]["colors"]["accent"], json!("#0055a5"), "{imported}");
        let mut theme = imported["structuredContent"].clone();
        theme["id"] = json!("from-agent");
        theme["label"] = json!("From an agent");
        let mine = tool(&app, &rw, "get_org_themes", json!({})).await;
        assert!(mine["structuredContent"]["version"].is_string(), "{mine}");
        let saved = tool(
            &app,
            &rw,
            "save_org_themes",
            json!({"themes": {"themes": [theme], "default_light": "from-agent"}, "dry_run": true}),
        )
        .await;
        assert_eq!(saved["structuredContent"]["dry_run"], json!(true), "{saved}");
        assert!(
            !ro_names.contains(&"save_org_themes".to_string()) && ro_names.contains(&"export_theme_css".to_string())
        );

        // mistakes come back as tool errors (the agent can read them and retry)
        let bad = tool(&app, &rw, "describe_model", json!({"name": "nope"})).await;
        assert_eq!(bad["isError"], json!(true));
        let bad = tool(&app, &rw, "get_dashboard", json!({})).await;
        assert_eq!(bad["isError"], json!(true), "missing argument: {bad}");
        let (_, unknown) = rpc(
            &app,
            &rw,
            json!({"jsonrpc": "2.0", "id": 9, "method": "tools/call", "params": {"name": "nope", "arguments": {}}}),
        )
        .await;
        assert_eq!(unknown["error"]["code"], json!(-32602), "{unknown}");
        let (_, nomethod) = rpc(&app, &rw, json!({"jsonrpc": "2.0", "id": 10, "method": "resources/subscribe"})).await;
        assert_eq!(nomethod["error"]["code"], json!(-32601));
        let (_, invalid) = rpc(&app, &rw, json!({"id": 11, "method": "ping"})).await;
        assert_eq!(invalid["error"]["code"], json!(-32600));

        // not signed in: refused before anything
        assert_eq!(
            rpc(&app, "mbi_nonsense", json!({"jsonrpc": "2.0", "id": 1, "method": "ping"})).await.0,
            StatusCode::UNAUTHORIZED
        );

        let c = state.store.pool.get().await.unwrap();
        for t in [&rw, &ro] {
            c.execute("DELETE FROM api_tokens WHERE token_hash = $1", &[&crate::store::sha256_hex(t)]).await.unwrap();
        }
    }

    /// A tool called through MCP, in an organisation, on a session.
    async fn org_tool(app: &axum::Router, cookie: &str, org: &str, name: &str, args: Value) -> Value {
        let call =
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": args}});
        let (s, r, _) =
            crate::api::share::tests::send(app, Some(cookie), org, "POST", "/api/mcp", Some(call), None).await;
        assert_eq!(s, StatusCode::OK, "{r}");
        r["result"].clone()
    }

    /// What a tool answered (its structured content), or panics with its error.
    fn ok(r: &Value) -> &Value {
        assert_eq!(r["isError"], json!(false), "{r}");
        &r["structuredContent"]
    }

    #[tokio::test]
    async fn agents_schedule_reports_and_alerts() {
        use crate::api::share::tests::{clean_up, org_with, send};
        let dashboard = "title: Agents\ntiles:\n  - {id: rev, type: kpi, query: {model: sales, measures: [revenue]}}\n";
        let Some((app, state, cookie, org)) = org_with(dashboard).await else { return };
        let (smtp, _mails) = crate::api::delivery::tests::fake_smtp().await;
        let server = json!({"host": "127.0.0.1", "port": smtp, "security": "none", "from": "bi@example.com"});
        send(&app, Some(&cookie), &org, "PUT", "/api/delivery/mail", Some(server), None).await;
        let report = json!({"name": "Monday numbers", "format": "csv", "recipients": [crate::api::tests::OWNER],
                            "when": {"every": "weekly", "day": 1, "at": "08:00", "timezone": "Europe/Rome"}});
        let t = |n, a| org_tool(&app, &cookie, &org, n, a);

        // a dry run says what it would be, and makes nothing
        let r = t("create_report", json!({"dashboard": "shared", "report": report, "dry_run": true})).await;
        assert_eq!(ok(&r)["dry_run"], json!(true));
        assert_eq!(ok(&t("list_reports", json!({"dashboard": "shared"})).await)["schedules"], json!([]));
        let r = t("create_report", json!({"dashboard": "shared", "report": report})).await;
        let id = ok(&r)["id"].as_str().unwrap().to_string();
        // an alert, on a tile's number
        let alert = json!({"name": "Revenue watch", "format": "alert", "recipients": [crate::api::tests::OWNER],
                           "when": {"every": "daily", "at": "07:00", "timezone": "UTC"},
                           "alert": {"tile": "rev", "op": "below", "value": 1000}});
        assert_eq!(
            ok(&t("create_report", json!({"dashboard": "shared", "report": alert})).await)["format"],
            json!("alert")
        );
        // paused, resumed, sent now, deleted
        assert_eq!(ok(&t("pause_report", json!({"id": id})).await)["active"], json!(false));
        assert_eq!(ok(&t("resume_report", json!({"id": id})).await)["active"], json!(true));
        let sent = t("send_report_now", json!({"id": id})).await;
        assert_eq!(sent["isError"], json!(false), "{sent}");
        ok(&t("delete_report", json!({"id": id})).await);
        let left = t("list_reports", json!({"dashboard": "shared"})).await;
        assert_eq!(ok(&left)["schedules"].as_array().unwrap().len(), 1);
        // mistakes come back as the API says them
        let bad = t(
            "create_report",
            json!({"dashboard": "shared", "report": {"name": "x", "format": "pdf",
            "recipients": [], "when": {"every": "daily", "at": "07:00", "timezone": "UTC"}}}),
        )
        .await;
        assert_eq!(bad["isError"], json!(true), "{bad}");
        clean_up(&state, &org).await;
    }

    #[tokio::test]
    async fn agents_use_formulas_and_read_the_catalog() {
        use crate::api::share::tests::{clean_up, org_with};
        let Some((app, state, cookie, org)) = org_with("title: Agents\ntiles: []\n").await else { return };
        let t = |n, a| org_tool(&app, &cookie, &org, n, a);
        // checked as typed
        assert_eq!(
            ok(&t("check_formula", json!({"model": "sales", "expr": "revenue / transactions"})).await)["ok"],
            json!(true)
        );
        let wrong = t("check_formula", json!({"model": "sales", "expr": "revnue / 2"})).await;
        assert!(ok(&wrong)["error"].as_str().unwrap().contains("did you mean revenue"), "{wrong}");
        // shared with the organisation: a dry run, then for real, then gone
        let f =
            json!({"name": "per_sale", "label": "Per sale", "expr": "revenue / transactions", "format": "currency"});
        assert_eq!(
            ok(&t("save_formula", json!({"model": "sales", "formula": f, "dry_run": true})).await)["dry_run"],
            json!(true)
        );
        ok(&t("save_formula", json!({"model": "sales", "formula": f})).await);
        let m = t("describe_model", json!({"name": "sales"})).await;
        assert!(m.to_string().contains("per_sale"), "{m}");
        ok(&t("delete_formula", json!({"model": "sales", "name": "per_sale"})).await);
        // the catalog: who answers for each model, and where it's used
        let c = t("get_catalog", json!({})).await;
        assert!(ok(&c)["models"].as_array().unwrap().iter().any(|m| m["model"] == "sales"), "{c}");
        let l = t("get_lineage", json!({"model": "sales"})).await;
        assert_eq!(ok(&l)["reads"]["connection"], json!("warehouse"), "{l}");
        clean_up(&state, &org).await;
    }
}
