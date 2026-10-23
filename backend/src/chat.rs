//! The in-app chat's provider side: an organisation brings its own model (an OpenAI-compatible or
//! an Anthropic API, with its key), and Magnum BI relays the conversation and runs the tools
//! (api/chat.rs). Here: the settings, the two request formats, and what a tool's result may show
//! the provider (rows capped, sensitive fields left out).

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The most rows of a result the provider may see (an organisation chooses up to this).
pub const MAX_ROWS_TO_MODEL: u32 = 5000;

fn yes() -> bool {
    true
}
fn rows_default() -> u32 {
    200
}
fn questions_default() -> u32 {
    1000
}
fn home_default() -> String {
    "dashboards".into()
}

/// A tenant's chat: its provider (the key is sealed apart, write-only) and its rules.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChatSettings {
    #[serde(default)]
    pub enabled: bool,
    /// openai (any OpenAI-compatible API: OpenAI, Azure, Mistral, OpenRouter, Ollama...) | anthropic
    pub format: String,
    /// the API's base address, e.g. https://api.openai.com/v1 or https://api.anthropic.com
    pub url: String,
    pub model: String,
    /// questions a month, for the whole organisation (0: none)
    #[serde(default = "questions_default")]
    pub monthly_questions: u32,
    /// rows of each result the provider sees (the chart shows them all)
    #[serde(default = "rows_default")]
    pub rows_to_model: u32,
    /// fields tagged pii, personal, confidential or restricted: their values never go to the provider
    #[serde(default = "yes")]
    pub keep_sensitive: bool,
    /// where members start: dashboards | chat (each person may choose otherwise)
    #[serde(default = "home_default")]
    pub home: String,
}

impl ChatSettings {
    pub fn check(&self) -> Result<(), String> {
        if !["openai", "anthropic"].contains(&self.format.as_str()) {
            return Err("the format is openai (OpenAI-compatible APIs) or anthropic".into());
        }
        host(&self.url, true)?;
        if self.model.trim().is_empty() || self.model.len() > 200 {
            return Err("say which model to use, e.g. gpt-4.1 or claude-sonnet-5-5".into());
        }
        if !(1..=MAX_ROWS_TO_MODEL).contains(&self.rows_to_model) {
            return Err(format!("rows sent to the model: between 1 and {MAX_ROWS_TO_MODEL}"));
        }
        if !["dashboards", "chat"].contains(&self.home.as_str()) {
            return Err("the home screen is dashboards or chat".into());
        }
        Ok(())
    }
}

/// The API's host, for the network guard; `allow_http`: plain http allowed (a model on the
/// local network, in development).
pub fn host(url: &str, allow_http: bool) -> Result<String, String> {
    let u = url::Url::parse(url.trim()).map_err(|_| "the API's address isn't a web address".to_string())?;
    match u.scheme() {
        "https" => {}
        "http" if allow_http => {}
        _ => return Err("the API's address starts with https://".into()),
    }
    if !u.username().is_empty() || u.password().is_some() {
        return Err("no user name or password in the API's address: the key goes in its own field".into());
    }
    u.host_str().filter(|h| !h.is_empty()).map(str::to_lowercase).ok_or_else(|| "the API's address needs a host".into())
}

/// A tool the model asked for.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Call {
    pub id: String,
    pub name: String,
    pub args: Value,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct ToolResult {
    pub id: String,
    pub content: String,
}

/// A conversation, in neither provider's format.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum Turn {
    User { text: String },
    Assistant { text: String, calls: Vec<Call> },
    Results { results: Vec<ToolResult> },
}

/// A tool as the model is told about it.
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub schema: Value,
}

/// The address and body of the next request to the provider.
pub fn request(s: &ChatSettings, system: &str, turns: &[Turn], tools: &[ToolSpec]) -> (String, Value) {
    let base = s.url.trim().trim_end_matches('/');
    if s.format == "anthropic" {
        let url = if base.ends_with("/v1") { format!("{base}/messages") } else { format!("{base}/v1/messages") };
        let messages: Vec<Value> = turns
            .iter()
            .map(|t| match t {
                Turn::User { text } => json!({"role": "user", "content": [{"type": "text", "text": text}]}),
                Turn::Assistant { text, calls } => {
                    let mut content = vec![];
                    if !text.is_empty() {
                        content.push(json!({"type": "text", "text": text}));
                    }
                    content.extend(calls.iter().map(|c| json!({"type": "tool_use", "id": c.id, "name": c.name, "input": c.args})));
                    json!({"role": "assistant", "content": content})
                }
                Turn::Results { results } => json!({"role": "user", "content": results.iter()
                    .map(|r| json!({"type": "tool_result", "tool_use_id": r.id, "content": r.content})).collect::<Vec<_>>()}),
            })
            .collect();
        let tools: Vec<Value> = tools
            .iter()
            .map(|t| json!({"name": t.name, "description": t.description, "input_schema": t.schema}))
            .collect();
        return (
            url,
            json!({"model": s.model, "max_tokens": 4096, "system": system, "messages": messages, "tools": tools}),
        );
    }
    let mut messages = vec![json!({"role": "system", "content": system})];
    for t in turns {
        match t {
            Turn::User { text } => messages.push(json!({"role": "user", "content": text})),
            Turn::Assistant { text, calls } => {
                let mut m = json!({"role": "assistant", "content": text});
                if !calls.is_empty() {
                    m["tool_calls"] = json!(
                        calls
                            .iter()
                            .map(|c| json!({"id": c.id, "type": "function",
                        "function": {"name": c.name, "arguments": c.args.to_string()}}))
                            .collect::<Vec<_>>()
                    );
                }
                messages.push(m);
            }
            Turn::Results { results } => {
                messages.extend(
                    results.iter().map(|r| json!({"role": "tool", "tool_call_id": r.id, "content": r.content})),
                );
            }
        }
    }
    let tools: Vec<Value> = tools
        .iter()
        .map(|t| json!({"type": "function", "function": {"name": t.name, "description": t.description, "parameters": t.schema}}))
        .collect();
    (format!("{base}/chat/completions"), json!({"model": s.model, "messages": messages, "tools": tools}))
}

/// The provider's answer as the next assistant turn, or what it said went wrong.
pub fn parse(s: &ChatSettings, body: &Value) -> Result<Turn, String> {
    if let Some(e) = body.get("error") {
        let m = e["message"].as_str().map_or_else(|| e.to_string(), String::from);
        return Err(format!("the model's API said: {m}"));
    }
    if s.format == "anthropic" {
        let blocks = body["content"].as_array().ok_or("the model's API answered without content")?;
        let text = blocks
            .iter()
            .filter(|b| b["type"] == "text")
            .filter_map(|b| b["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let calls = blocks
            .iter()
            .filter(|b| b["type"] == "tool_use")
            .map(|b| Call { id: str_of(&b["id"]), name: str_of(&b["name"]), args: b["input"].clone() })
            .collect();
        return Ok(Turn::Assistant { text, calls });
    }
    let m = &body["choices"][0]["message"];
    if m.is_null() {
        return Err("the model's API answered without a message".into());
    }
    let calls = m["tool_calls"]
        .as_array()
        .map(|cs| {
            cs.iter()
                .map(|c| Call {
                    id: str_of(&c["id"]),
                    name: str_of(&c["function"]["name"]),
                    // arguments are JSON in a string; unreadable ones reach the tool as they are, and fail there
                    args: serde_json::from_str(c["function"]["arguments"].as_str().unwrap_or("{}"))
                        .unwrap_or(Value::Null),
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Turn::Assistant { text: m["content"].as_str().unwrap_or_default().to_string(), calls })
}

fn str_of(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

/// A query's result as the provider may see it: at most `max_rows` rows, no SQL, and no values
/// of `sensitive` fields (counting them is fine: how many, never which).
pub fn for_provider(result: &Value, sensitive: &[String], max_rows: usize) -> Value {
    let columns = result["columns"].as_array().cloned().unwrap_or_default();
    let shown = |c: &Value| {
        let name = c["field"].as_str().or(c["name"].as_str()).unwrap_or_default();
        let name = name.split(':').next().unwrap_or(name);
        let name = name.strip_suffix("__previous").or_else(|| name.strip_suffix("__change")).unwrap_or(name);
        match name.split_once("__") {
            Some((agg, col)) if sensitive.iter().any(|s| s == col) => matches!(agg, "count" | "count_distinct"),
            _ => !sensitive.iter().any(|s| s == name),
        }
    };
    let keep: Vec<bool> = columns.iter().map(shown).collect();
    let rows = result["rows"].as_array().cloned().unwrap_or_default();
    let mut out = result.clone();
    if let Some(o) = out.as_object_mut() {
        o.remove("sql");
    }
    out["columns"] = json!(columns.iter().zip(&keep).filter(|(_, k)| **k).map(|(c, _)| c).collect::<Vec<_>>());
    out["rows"] = json!(
        rows.iter()
            .take(max_rows)
            .map(|r| r.as_array().map_or(vec![], |r| r
                .iter()
                .zip(&keep)
                .filter(|(_, k)| **k)
                .map(|(v, _)| v.clone())
                .collect()))
            .collect::<Vec<_>>()
    );
    out["rows_total"] = json!(rows.len());
    out["rows_shown"] = json!(rows.len().min(max_rows));
    let left_out: Vec<&Value> = columns.iter().zip(&keep).filter(|(_, k)| !**k).map(|(c, _)| &c["name"]).collect();
    if !left_out.is_empty() {
        out["left_out"] = json!(left_out);
    }
    out
}

/// A proposed change, in words, for the card the person confirms.
pub fn summary(tool: &str, args: &Value) -> String {
    let s = |k: &str| args[k].as_str().unwrap_or("?").to_string();
    match tool {
        "save_dashboard" => {
            let title = dashboard_yaml(args).and_then(|d| d["title"].as_str().map(str::to_string));
            match (title, args["version"].as_str()) {
                (Some(t), Some("none")) => format!("Create the dashboard \"{t}\""),
                (Some(t), Some(_)) => format!("Change the dashboard \"{t}\""),
                _ => format!("Save the dashboard {}", s("id")),
            }
        }
        "delete_dashboard" => format!("Delete the dashboard {}", s("id")),
        "save_blend" => format!("Save the blend {}", args["blend"]["name"].as_str().unwrap_or("?")),
        "create_report" => {
            let kind = if args["report"]["format"] == "alert" { "alert" } else { "report" };
            format!(
                "Schedule the {kind} \"{}\" on the dashboard {}",
                args["report"]["name"].as_str().unwrap_or("?"),
                s("dashboard")
            )
        }
        "pause_report" => format!("Pause the report {}", s("id")),
        "resume_report" => format!("Resume the report {}", s("id")),
        "send_report_now" => format!("Send the report {} now", s("id")),
        "delete_report" => format!("Delete the report {}", s("id")),
        "save_formula" => {
            format!("Share the formula {} on {}", args["formula"]["name"].as_str().unwrap_or("?"), s("model"))
        }
        "delete_formula" => format!("Remove the shared formula {} from {}", s("name"), s("model")),
        other => format!("Run {other}"),
    }
}

/// What a proposal holds, a line each, for the card: a dashboard's tiles.
pub fn details(tool: &str, args: &Value) -> Vec<String> {
    let Some(d) = dashboard_yaml(args).filter(|_| tool == "save_dashboard") else { return vec![] };
    let tiles = d["tiles"].as_array().cloned().unwrap_or_default();
    tiles
        .iter()
        .map(|t| {
            let kind = match t["type"].as_str().unwrap_or_default() {
                "text" => return "Text".to_string(),
                "kpi" => "number".to_string(),
                "table" => "table".to_string(),
                "hbar" => "bar chart".to_string(),
                other => format!("{other} chart"),
            };
            match t["title"].as_str().filter(|t| !t.trim().is_empty()) {
                Some(title) => format!("{title} ({kind})"),
                None => format!("A {kind}"),
            }
        })
        .collect()
}

fn dashboard_yaml(args: &Value) -> Option<Value> {
    serde_yaml::from_str(args["yaml"].as_str()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(v: Value) -> ChatSettings {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn settings_are_checked() {
        let ok = settings(
            json!({"enabled": true, "format": "openai", "url": "https://api.openai.com/v1", "model": "gpt-4.1"}),
        );
        assert!(ok.check().is_ok());
        assert_eq!((ok.rows_to_model, ok.keep_sensitive, ok.monthly_questions), (200, true, 1000));
        let e = |v: Value| settings(v).check().unwrap_err();
        let base = |patch: Value| {
            let mut b = json!({"enabled": true, "format": "anthropic", "url": "https://api.anthropic.com", "model": "claude-sonnet-5-5"});
            for (k, v) in patch.as_object().unwrap() {
                b[k] = v.clone();
            }
            b
        };
        assert!(e(base(json!({"format": "gemini"}))).contains("format"));
        assert!(e(base(json!({"url": "ftp://x"}))).contains("address"));
        assert!(e(base(json!({"url": "https://user:pw@api.example.com"}))).contains("address"));
        assert!(e(base(json!({"model": " "}))).contains("model"));
        assert!(e(base(json!({"rows_to_model": 0}))).contains("rows"));
        assert!(e(base(json!({"rows_to_model": 100_000}))).contains("rows"));
    }

    fn conversation() -> Vec<Turn> {
        vec![
            Turn::User { text: "How did revenue go?".into() },
            Turn::Assistant {
                text: "Let me look.".into(),
                calls: vec![Call {
                    id: "c1".into(),
                    name: "run_query".into(),
                    args: json!({"query": {"model": "sales"}}),
                }],
            },
            Turn::Results { results: vec![ToolResult { id: "c1".into(), content: "{\"rows\": []}".into() }] },
        ]
    }

    fn tools() -> Vec<ToolSpec> {
        vec![ToolSpec {
            name: "run_query".into(),
            description: "Run a query".into(),
            schema: json!({"type": "object"}),
        }]
    }

    #[test]
    fn openai_requests_and_answers() {
        let s = settings(json!({"format": "openai", "url": "https://api.openai.com/v1/", "model": "gpt-4.1"}));
        let (url, body) = request(&s, "Be brief.", &conversation(), &tools());
        assert_eq!(url, "https://api.openai.com/v1/chat/completions");
        assert_eq!(body["model"], "gpt-4.1");
        assert_eq!(body["messages"][0], json!({"role": "system", "content": "Be brief."}));
        assert_eq!(body["messages"][1], json!({"role": "user", "content": "How did revenue go?"}));
        assert_eq!(body["messages"][2]["tool_calls"][0]["function"]["name"], "run_query");
        assert_eq!(body["messages"][2]["tool_calls"][0]["function"]["arguments"], "{\"query\":{\"model\":\"sales\"}}");
        assert_eq!(body["messages"][3], json!({"role": "tool", "tool_call_id": "c1", "content": "{\"rows\": []}"}));
        assert_eq!(body["tools"][0]["function"]["parameters"], json!({"type": "object"}));
        let answer = json!({"choices": [{"message": {"role": "assistant", "content": null, "tool_calls": [
            {"id": "c2", "type": "function", "function": {"name": "search_catalog", "arguments": "{\"q\":\"takings\"}"}}]}}]});
        assert_eq!(
            parse(&s, &answer).unwrap(),
            Turn::Assistant {
                text: String::new(),
                calls: vec![Call { id: "c2".into(), name: "search_catalog".into(), args: json!({"q": "takings"}) }]
            }
        );
        let done = json!({"choices": [{"message": {"role": "assistant", "content": "Revenue rose 12%."}}]});
        assert_eq!(parse(&s, &done).unwrap(), Turn::Assistant { text: "Revenue rose 12%.".into(), calls: vec![] });
        assert!(parse(&s, &json!({"error": {"message": "bad key"}})).unwrap_err().contains("bad key"));
    }

    #[test]
    fn anthropic_requests_and_answers() {
        let s =
            settings(json!({"format": "anthropic", "url": "https://api.anthropic.com", "model": "claude-sonnet-5-5"}));
        let (url, body) = request(&s, "Be brief.", &conversation(), &tools());
        assert_eq!(url, "https://api.anthropic.com/v1/messages");
        assert_eq!((&body["system"], &body["model"]), (&json!("Be brief."), &json!("claude-sonnet-5-5")));
        assert!(body["max_tokens"].as_u64().unwrap() > 0);
        assert_eq!(
            body["messages"][0],
            json!({"role": "user", "content": [{"type": "text", "text": "How did revenue go?"}]})
        );
        assert_eq!(
            body["messages"][1]["content"][1],
            json!({"type": "tool_use", "id": "c1", "name": "run_query", "input": {"query": {"model": "sales"}}})
        );
        assert_eq!(
            body["messages"][2],
            json!({"role": "user", "content": [{"type": "tool_result", "tool_use_id": "c1", "content": "{\"rows\": []}"}]})
        );
        assert_eq!(
            body["tools"][0],
            json!({"name": "run_query", "description": "Run a query", "input_schema": {"type": "object"}})
        );
        let answer = json!({"content": [{"type": "text", "text": "Looking."},
            {"type": "tool_use", "id": "t1", "name": "search_catalog", "input": {"q": "takings"}}], "stop_reason": "tool_use"});
        assert_eq!(
            parse(&s, &answer).unwrap(),
            Turn::Assistant {
                text: "Looking.".into(),
                calls: vec![Call { id: "t1".into(), name: "search_catalog".into(), args: json!({"q": "takings"}) }]
            }
        );
        assert!(
            parse(&s, &json!({"type": "error", "error": {"message": "overloaded"}}))
                .unwrap_err()
                .contains("overloaded")
        );
    }

    #[test]
    fn the_provider_sees_capped_rows_and_no_sensitive_values() {
        let result = json!({
            "columns": [{"name": "email", "field": "email", "kind": "dimension"},
                        {"name": "count_distinct__email", "field": "count_distinct__email", "kind": "measure"},
                        {"name": "max__email", "field": "max__email", "kind": "measure"},
                        {"name": "salary", "field": "salary", "kind": "measure"},
                        {"name": "revenue", "field": "revenue", "kind": "measure"}],
            "rows": [["a@x", 1, "a@x", 10, 5], ["b@x", 1, "b@x", 20, 6], ["c@x", 1, "c@x", 30, 7]],
            "sql": "SELECT ...", "stats": {"duration_ms": 3}});
        let shown = for_provider(&result, &["email".into(), "salary".into()], 2);
        // counting a sensitive field is fine; its values, or any other measure of it, aren't
        let names: Vec<&str> =
            shown["columns"].as_array().unwrap().iter().map(|c| c["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["count_distinct__email", "revenue"]);
        assert_eq!(shown["rows"], json!([[1, 5], [1, 6]]));
        assert_eq!((&shown["rows_total"], &shown["rows_shown"]), (&json!(3), &json!(2)));
        assert_eq!(shown["left_out"], json!(["email", "max__email", "salary"]));
        assert!(shown.get("sql").is_none(), "{shown}");
        // nothing sensitive, few rows: as it was, minus the SQL
        let plain = for_provider(&result, &[], 200);
        assert_eq!(plain["rows"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn proposals_say_what_they_change() {
        let report = json!({"dashboard": "sales_overview", "report": {"name": "Monday numbers", "format": "csv"}});
        assert_eq!(
            summary("create_report", &report),
            "Schedule the report \"Monday numbers\" on the dashboard sales_overview"
        );
        let alert = json!({"dashboard": "d", "report": {"name": "Watch", "format": "alert"}});
        assert_eq!(summary("create_report", &alert), "Schedule the alert \"Watch\" on the dashboard d");
        assert_eq!(
            summary("save_formula", &json!({"model": "sales", "formula": {"name": "per_sale"}})),
            "Share the formula per_sale on sales"
        );
        assert_eq!(summary("delete_dashboard", &json!({"id": "old"})), "Delete the dashboard old");
    }

    #[test]
    fn a_dashboard_proposal_shows_what_the_dashboard_will_hold() {
        let yaml = "title: Sales overview\ntiles:\n\
            - {id: a, title: Revenue by month, type: line, query: {model: sales, measures: [revenue]}}\n\
            - {id: b, type: bar, query: {model: sales, measures: [revenue], dimensions: [region]}}\n\
            - {id: c, type: text, text: '# Notes'}\n";
        let new = json!({"id": "sales_overview", "yaml": yaml, "version": "none"});
        assert_eq!(summary("save_dashboard", &new), "Create the dashboard \"Sales overview\"");
        assert_eq!(details("save_dashboard", &new), ["Revenue by month (line chart)", "A bar chart", "Text"]);
        let changed = json!({"id": "sales_overview", "yaml": yaml, "version": "v3"});
        assert_eq!(summary("save_dashboard", &changed), "Change the dashboard \"Sales overview\"");
        // YAML it can't read (the check would have refused it) still says what it is
        let odd = json!({"id": "x", "yaml": ": ["});
        assert_eq!(summary("save_dashboard", &odd), "Save the dashboard x");
        assert!(details("save_dashboard", &odd).is_empty());
        assert!(details("delete_dashboard", &json!({"id": "x"})).is_empty());
    }
}
