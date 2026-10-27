//! Hostile input in every field: injection, traversal, bombs, odd characters, absurd numbers.
//! The server must refuse cleanly (4xx), never fail (5xx), never run what it was sent, and never
//! show a restricted organisation more than it may see.

use serde_json::{Value, json};

use super::{As, Who, World};
use crate::api::tests::{OWNER, scramdb_config};

/// Strings aimed at SQL built from values.
const VALUES: &[&str] = &[
    "' OR '1'='1",
    "EMEA') OR ('1'='1",
    "EMEA'; DROP TABLE sample_sales; --",
    "\\' OR 1=1 --",
    "EMEA\u{0}",
    "%",
    "_",
    "\u{202e}AEME",
    "😀",
    "$$ OR 1=1 $$",
    "EMEA' UNION SELECT password_hash FROM users --",
];

/// Strings aimed at SQL built from names (fields, sorts, models).
const NAMES: &[&str] = &[
    "region\" FROM sample_sales --",
    "region; DROP TABLE x",
    "sum(amount) FROM sample_sales; --",
    "count_distinct(store) OVER ()",
    "sum(amount)) FROM users --",
    "sum((SELECT 1))",
    "sum(amount) + (SELECT count(*) FROM users)",
    "../../etc/passwd",
    "region:'; --",
    "sold_at:month'); DROP TABLE x; --",
    "sold_at:century",
    "",
    " ",
    "region\u{0}",
    "users.password_hash",
    "pg_catalog.pg_user",
];

/// Formulas aimed at the SQL they're compiled into.
const FORMULAS: &[&str] = &[
    "revenue; DROP TABLE sample_sales",
    "revenue) FROM users --",
    "revenue /* */ + 1",
    "(SELECT count(*) FROM users)",
    "sum(amount) OVER ()",
    "sum(password_hash)",
    "sum((amount))",
    "current_user",
    "pg_sleep(10)",
    "users.password_hash",
    "'x' || revenue",
    "1e400",
    "if(1, 2, 3)",
    "revenue\u{0}",
    "😀",
    "running_sum(revenue) OVER (PARTITION BY password_hash)",
    "moving_avg(revenue, 99999999999)",
    "moving_avg(revenue, 1e3)",
    "rank(revenue); DROP TABLE x",
    "running_sum(running_sum(revenue))",
    "percent_of_total(region)",
    "previous(users.password_hash)",
];

#[derive(Clone, Copy, PartialEq)]
enum Expect {
    /// refused (4xx)
    Refused,
    /// anything but a server error
    NoCrash,
}

struct Case {
    what: String,
    method: &'static str,
    path: String,
    body: Option<Value>,
    yaml: Option<String>,
    /// JSON sent as is (shapes a Value can't hold safely)
    raw: Option<String>,
    expect: Expect,
}

fn case(what: impl Into<String>, method: &'static str, path: impl Into<String>, body: Value, expect: Expect) -> Case {
    Case { what: what.into(), method, path: path.into(), body: Some(body), yaml: None, raw: None, expect }
}

fn yaml(what: impl Into<String>, path: impl Into<String>, text: String, expect: Expect) -> Case {
    Case { what: what.into(), method: "PUT", path: path.into(), body: None, yaml: Some(text), raw: None, expect }
}

fn query_cases() -> Vec<Case> {
    use Expect::*;
    let mut out = vec![];
    let q = |patch: Value| {
        let mut base = json!({"model": "sales", "dimensions": ["region"], "measures": ["transactions"]});
        for (k, v) in patch.as_object().unwrap() {
            base[k] = v.clone();
        }
        base
    };
    for n in NAMES {
        out.push(case(format!("dimension {n:?}"), "POST", "/api/query", q(json!({"dimensions": [n]})), Refused));
        out.push(case(format!("measure {n:?}"), "POST", "/api/query", q(json!({"measures": [n]})), Refused));
        out.push(case(format!("sort {n:?}"), "POST", "/api/query", q(json!({"sort": [n]})), Refused));
        out.push(case(format!("model {n:?}"), "POST", "/api/query", q(json!({"model": n})), Refused));
        let f = json!([{"field": n, "op": "equals", "values": ["x"]}]);
        out.push(case(format!("filter field {n:?}"), "POST", "/api/query", q(json!({"filters": f})), Refused));
        let top = json!({"dimension": n, "n": 3, "by": "transactions"});
        out.push(case(format!("top N on {n:?}"), "POST", "/api/query", q(json!({"top": top})), Refused));
    }
    for op in ["equals; DROP", "LIKE", "= '' OR 1=1 --", "between"] {
        let f = json!([{"field": "region", "op": op, "values": ["x"]}]);
        out.push(case(format!("filter op {op:?}"), "POST", "/api/query", q(json!({"filters": f})), Refused));
    }
    for f in FORMULAS.iter().map(|f| f.to_string()).chain([
        format!("{}1{}", "(".repeat(5000), ")".repeat(5000)),
        format!("{}revenue", "-".repeat(5000)),
        "revenue + ".repeat(5000) + "1",
    ]) {
        let shown: String = f.chars().take(40).collect();
        let formulas = json!([{"name": "f", "expr": f}]);
        out.push(case(format!("formula {shown:?}"), "POST", "/api/query", q(json!({"formulas": formulas})), Refused));
        let row = json!([{"name": "f", "expr": f, "per_row": true}]);
        let q_row = q(json!({"dimensions": ["f"], "formulas": row}));
        out.push(case(format!("per-row formula {shown:?}"), "POST", "/api/query", q_row, Refused));
        let saved = json!({"name": "f", "expr": f});
        out.push(case(format!("shared formula {shown:?}"), "PUT", "/api/formulas/sales/f", saved, Refused));
        // the editor's check answers, whatever it's sent
        let check = json!({"model": "sales", "expr": f});
        out.push(case(format!("formula check {shown:?}"), "POST", "/api/formulas/check", check, NoCrash));
    }
    // text in a formula is a value, quoted for the engine: it runs, and runs as text
    for text in ["x' OR '1'='1", "x'' OR ''1''=''1", "\\'; DROP TABLE x; --", "$$ OR 1=1 $$"] {
        let expr = format!("if(region = '{}', 1, 0)", text.replace('\'', "''"));
        let formulas = json!([{"name": "f", "expr": expr, "per_row": true}]);
        let query = q(json!({"dimensions": ["f"], "formulas": formulas}));
        out.push(case(format!("text {text:?} in a formula"), "POST", "/api/query", query, NoCrash));
    }
    for name in ["f; DROP", "revenue", "sold_at", "x\u{0}", ""] {
        let formulas = json!([{"name": name, "expr": "1"}]);
        out.push(case(
            format!("formula named {name:?}"),
            "POST",
            "/api/query",
            q(json!({"formulas": formulas})),
            Refused,
        ));
    }
    for limit in [json!(-1), json!(0), json!(1e12), json!("10; DROP TABLE x"), json!(1.5)] {
        out.push(case(format!("limit {limit}"), "POST", "/api/query", q(json!({"limit": limit})), Refused));
    }
    for n in [json!(0), json!(1_000_000_000), json!(-3)] {
        let top = json!({"dimension": "region", "n": n, "by": "transactions"});
        out.push(case(format!("top n {n}"), "POST", "/api/query", q(json!({"top": top})), Refused));
    }
    for days in [json!(-5), json!(1e9), json!("7; DROP")] {
        let p = json!({"field": "sold_at", "current": {"last_n_days": days}});
        out.push(case(format!("period of {days} days"), "POST", "/api/query", q(json!({"period": p})), NoCrash));
    }
    for v in [json!("1e400"), json!([[[[1]]]]), json!({"$gt": ""}), json!(null), json!(true)] {
        let f = json!([{"field": "amount", "op": "gt", "values": [v]}]);
        out.push(case(format!("number filter {f}"), "POST", "/api/query", q(json!({"filters": f})), NoCrash));
    }
    out
}

fn document_cases(w: &World) -> Vec<Case> {
    use Expect::*;
    let mut out = vec![];
    let fine = "title: ok\n".to_string();
    for id in ["..", "../escape", "a/b", "%2e%2e%2fescape", &"x".repeat(300), "id with space", "id\u{0}"] {
        let path = format!("/api/dashboards/{}/yaml", id.replace(' ', "%20").replace('\u{0}', "%00"));
        out.push(yaml(format!("dashboard id {id:?}"), path, fine.clone(), Refused));
    }
    let bomb = "a: &a [x, x, x, x, x, x, x, x, x, x]\nb: &b [*a, *a, *a, *a, *a, *a, *a, *a, *a, *a]\nc: &c [*b, *b, *b, *b, *b, *b, *b, *b, *b, *b]\nd: &d [*c, *c, *c, *c, *c, *c, *c, *c, *c, *c]\ne: &e [*d, *d, *d, *d, *d, *d, *d, *d, *d, *d]\nf: &f [*e, *e, *e, *e, *e, *e, *e, *e, *e, *e]\ng: &g [*f, *f, *f, *f, *f, *f, *f, *f, *f, *f]\ntitle: boom\n";
    out.push(yaml("a YAML bomb", "/api/dashboards/bomb/yaml", bomb.into(), Refused));
    out.push(yaml(
        "deep YAML",
        "/api/dashboards/deep/yaml",
        format!("title: {}{}", "[".repeat(20_000), "]".repeat(20_000)),
        Refused,
    ));
    // quadratic in the YAML parser without the depth guard (40 s of CPU)
    let deep = format!("a: it's x\ntitle: {}{}", "[".repeat(100_000), "]".repeat(100_000));
    out.push(yaml("YAML nested 100,000 deep", "/api/dashboards/deeper/yaml", deep, Refused));
    out.push(yaml("YAML tags", "/api/dashboards/tags/yaml", "title: !!python/object:os.system ls\n".into(), NoCrash));
    out.push(yaml("a NUL in a title", "/api/dashboards/nul/yaml", "title: \"a\\0b\"\n".into(), NoCrash));
    out.push(yaml("a huge title", "/api/dashboards/huge/yaml", format!("title: {}\n", "a".repeat(900_000)), NoCrash));
    let deep = format!("{{\"title\": \"x\", \"tiles\": {}1{}}}", "[".repeat(5000), "]".repeat(5000));
    out.push(Case {
        what: "deep JSON".into(),
        method: "PUT",
        path: "/api/dashboards/deepjson".into(),
        body: None,
        yaml: None,
        raw: Some(deep),
        expect: Refused,
    });
    let tile = |q: Value| json!({"title": "x", "tiles": [{"id": "t", "type": "bar", "query": q}]});
    for n in NAMES.iter().take(6) {
        let q = json!({"model": "sales", "dimensions": [n], "measures": ["transactions"]});
        // saved with a warning at most; but never run as SQL
        out.push(case(format!("a saved tile on {n:?}"), "PUT", "/api/dashboards/injected", tile(q), NoCrash));
    }
    // names and settings
    let duck = |path: &str| json!({"config": {"type": "duckdb", "tables": {"t": path}}});
    for p in [
        "../../../../etc/passwd",
        "/etc/passwd",
        "file:///etc/passwd",
        "a/../../x.parquet",
        "~/x.parquet",
        "\\\\server\\share\\x",
    ] {
        out.push(case(format!("DuckDB table at {p:?}"), "PUT", "/api/connections/files", duck(p), Refused));
    }
    for name in ["../x", "a b", "x;DROP", &"n".repeat(200)] {
        let path = format!("/api/connections/{}", name.replace(' ', "%20"));
        out.push(case(format!("connection named {name:?}"), "PUT", path, json!({"config": scramdb_config()}), Refused));
        let path = format!("/api/delivery/webhooks/{}", name.replace(' ', "%20"));
        out.push(case(
            format!("webhook named {name:?}"),
            "PUT",
            path,
            json!({"kind": "slack", "url": "https://hooks.example.com/x"}),
            Refused,
        ));
    }
    for from in ["a@b.example\r\nBcc: evil@evil.example", "a@b.example\nSubject: hi", "\"x\r\n\" <a@b.example>", "<>"] {
        let mail = json!({"host": "smtp.example.com", "port": 587, "security": "starttls", "from": from});
        out.push(case(format!("mail from {from:?}"), "PUT", "/api/delivery/mail", mail, Refused));
    }
    for host in ["smtp.example.com\r\nRCPT TO:<evil@evil.example>", "", "a b"] {
        let mail = json!({"host": host, "port": 587, "security": "starttls", "from": "a@b.example"});
        out.push(case(format!("mail host {host:?}"), "PUT", "/api/delivery/mail", mail, Refused));
    }
    out.push(case(
        "a webhook of an unknown kind",
        "PUT",
        "/api/delivery/webhooks/x",
        json!({"kind": "irc", "url": "https://x.example"}),
        Refused,
    ));
    for table in ["sample_sales; DROP TABLE x", "sample_sales\" --", "a.b.c.d", "users", "pg_catalog.pg_authid"] {
        let m = json!({"name": "hostile", "connection": "warehouse", "table": table, "dimensions": [{"name": "region"}],
                       "measures": [{"name": "n", "type": "count"}]});
        out.push(case(format!("organisation model on {table:?}"), "PUT", "/api/org-models/hostile", m, Refused));
    }
    for column in ["region\"; DROP TABLE x; --", "(SELECT 1)", "password_hash"] {
        let m = json!({"name": "hostile", "connection": "warehouse", "table": "sample_sales",
                       "dimensions": [{"name": column}], "measures": [{"name": "n", "type": "count"}]});
        out.push(case(format!("organisation model column {column:?}"), "PUT", "/api/org-models/hostile", m, Refused));
        let m = json!({"name": "hostile", "connection": "warehouse", "table": "sample_sales",
                       "dimensions": [{"name": "region"}], "measures": [{"name": "s", "type": "sum", "sql": column}]});
        out.push(case(
            format!("organisation model measure on {column:?}"),
            "PUT",
            "/api/org-models/hostile",
            m,
            Refused,
        ));
    }
    for on in ["region = borough; DROP TABLE x", "region = (SELECT 1)", "1 = 1"] {
        let b = json!({"name": "hostile", "base": "sales", "joins": [{"model": "weather", "on": [on]}]});
        out.push(case(format!("blend joined on {on:?}"), "PUT", "/api/blends/hostile", b, Refused));
    }
    out.push(case(
        "members with a made-up role",
        "PUT",
        "/api/members",
        json!({"users": {"x@x.example": "god"}}),
        Refused,
    ));
    for t in [
        json!({"name": "x", "scope": "admin"}),
        json!({"name": "x", "scope": "read", "expires_in_days": -1}),
        json!({"name": "x", "scope": "read", "expires_in_days": 1_000_000}),
        json!({"name": "x", "scope": "read", "role": "god"}),
    ] {
        out.push(case(format!("token {t}"), "POST", "/api/tokens", t, Refused));
    }
    // a long name is cut to 60 characters
    let long = json!({"name": "x".repeat(10_000), "scope": "read"});
    out.push(case("a token with a huge name", "POST", "/api/tokens", long, NoCrash));
    let links = format!("/api/dashboards/{}/share-links", w.dashboard);
    for c in [json!(-1), json!(1e12), json!(7)] {
        out.push(case(format!("public link cached {c}s"), "POST", links.clone(), json!({"cache_seconds": c}), Refused));
    }
    let guests = format!("/api/dashboards/{}/guest-links", w.dashboard);
    for g in [
        json!({"expires_in_hours": 1, "password": "x".repeat(100_000)}),
        json!({"expires_in_hours": 1e9, "password": "Long-Password-1"}),
        json!({"expires_in_hours": 1, "password": "Long-Password-1", "role": "system_owner"}),
    ] {
        out.push(case("a hostile guest link", "POST", guests.clone(), g, Refused));
    }
    out.push(case(
        "CSS that closes its style tag",
        "POST",
        "/api/themes/import",
        json!({"css": "</style><script>alert(1)</script>"}),
        NoCrash,
    ));
    out.push(case("huge CSS", "POST", "/api/themes/import", json!({"css": "a{}".repeat(300_000)}), NoCrash));
    // documentation and the glossary: text, kept as text
    let docs = "/api/governance/sales/docs";
    out.push(case("a huge description", "PUT", docs, json!({"description": "x".repeat(100_000)}), Refused));
    out.push(case("a tag with markup", "PUT", docs, json!({"tags": ["<img src=x onerror=alert(1)>"]}), Refused));
    out.push(case("docs of no field", "PUT", docs, json!({"fields": {"users.password_hash": {}}}), Refused));
    out.push(case("docs with NUL", "PUT", docs, json!({"description": "a\u{0}b"}), NoCrash));
    out.push(case("HTML in docs", "PUT", docs, json!({"description": "<script>alert(1)</script>"}), NoCrash));
    // a term's name is text ("P/E ratio"), never a path
    for (name, expect) in [
        ("..", NoCrash),
        ("a/b", NoCrash),
        (&"t".repeat(200), Refused),
        (" ", Refused),
        (" x", Refused),
        ("a\u{7}b", Refused),
    ] {
        let path = format!("/api/glossary/{}", name.replace('/', "%2F").replace(' ', "%20").replace('\u{7}', "%07"));
        out.push(case(format!("term {name:?}"), "PUT", path, json!({"name": name, "definition": "d"}), expect));
    }
    out.push(case("a term of no name", "PUT", "/api/glossary/x", json!({"name": "y", "definition": "d"}), Refused));
    out
}

async fn send(w: &World, who: &As, c: &Case) -> super::Reply {
    match (&c.yaml, &c.raw) {
        (Some(text), _) => w.yaml(who, &c.path, text).await,
        (_, Some(text)) => w.raw(who, c.method, &c.path, Some("application/json"), text.clone()).await,
        _ => w.send(who, c.method, &c.path, c.body.clone()).await,
    }
}

/// Whether hostile input made the server work hard: quadratic parsing took 40 s (release) and
/// minutes here. A busy shared test database slows every request alike (over 30 s under the whole
/// suite's load), so it counts only when it is also three times an ordinary request at that moment.
fn too_slow(took: std::time::Duration, ordinary: std::time::Duration) -> bool {
    took > std::time::Duration::from_secs(30) && took > ordinary * 3
}

async fn timed<T>(f: impl std::future::Future<Output = T>) -> std::time::Duration {
    let started = std::time::Instant::now();
    f.await;
    started.elapsed()
}

async fn run(w: &World, who: &As, cases: Vec<Case>) -> Vec<String> {
    let mut wrong = vec![];
    let ordinary = json!({"model": "sales", "dimensions": ["region"], "measures": ["transactions"]});
    for c in cases {
        let started = std::time::Instant::now();
        let r = send(w, who, &c).await;
        let took = started.elapsed();
        let body: String = r.body.chars().take(200).collect();
        if r.status.is_server_error() {
            wrong.push(format!("{}: server error {} {body}", c.what, r.status));
        } else if c.expect == Expect::Refused && !r.status.is_client_error() {
            wrong.push(format!("{}: accepted ({}) {body}", c.what, r.status));
        }
        if too_slow(took, std::time::Duration::ZERO) {
            // slow, or the database was busy: again, beside an ordinary query at the same moment
            let (again, ordinary) =
                tokio::join!(timed(send(w, who, &c)), timed(w.send(who, "POST", "/api/query", Some(ordinary.clone()))));
            if too_slow(again, ordinary) {
                wrong.push(format!("{}: took {took:?}, then {again:?} beside an ordinary {ordinary:?}", c.what));
            }
        }
    }
    wrong
}

#[test]
fn slow_means_slower_than_an_ordinary_request_at_the_same_moment() {
    let s = std::time::Duration::from_secs;
    // a busy test database slows everything alike: not a finding
    assert!(!too_slow(s(31), s(20)));
    assert!(!too_slow(s(45), s(16)));
    // hostile input that makes the server work hard: a finding, however busy the database
    assert!(too_slow(s(40), s(1)));
    assert!(too_slow(s(300), s(60)));
    // quick enough, whatever the ordinary request took
    assert!(!too_slow(s(29), s(0)));
}

#[tokio::test]
async fn hostile_queries_are_refused() {
    let Some(w) = World::new().await else { return };
    let owner = As { who: Who::Person(OWNER.into()), org: w.a.clone(), view_as: None };
    let mut wrong = run(&w, &owner, query_cases()).await;
    // injected values never widen a restriction: B sees EMEA, or nothing
    let b = As { who: Who::Person(w.admin_b.clone()), org: w.b.clone(), view_as: None };
    for v in VALUES {
        for op in ["equals", "not_equals", "contains", "gt"] {
            let q = json!({"model": "sales", "dimensions": ["region"], "measures": ["transactions"],
                           "filters": [{"field": "region", "op": op, "values": [v]}]});
            let r = w.send(&b, "POST", "/api/query", Some(q)).await;
            if r.status.is_server_error() {
                wrong.push(format!("value {v:?} with {op}: server error {} {}", r.status, r.body));
            }
            for row in r.json()["rows"].as_array().into_iter().flatten() {
                if row[0] != json!("EMEA") {
                    wrong.push(format!("value {v:?} with {op} showed B the region {}", row[0]));
                }
            }
        }
    }
    let n = w
        .send(&owner, "POST", "/api/query", Some(json!({"model": "sales", "measures": ["transactions"]})))
        .await
        .json();
    assert!(n["rows"][0][0].as_i64().unwrap_or(0) > 0, "the sample data is gone: {n}");
    assert!(wrong.is_empty(), "hostile queries that weren't refused cleanly:\n{}", wrong.join("\n"));
    w.clean_up().await;
}

#[tokio::test]
async fn hostile_documents_and_settings_are_refused() {
    let Some(w) = World::new().await else { return };
    let owner = As { who: Who::Person(OWNER.into()), org: w.a.clone(), view_as: None };
    let cases = document_cases(&w);
    let wrong = run(&w, &owner, cases).await;
    // anything saved with injected fields is never run as SQL
    let r = w.send(&owner, "POST", "/api/dashboards/injected/tiles/t/query", Some(json!({}))).await;
    let mut wrong = wrong;
    if r.status.is_success() || r.status.is_server_error() {
        wrong.push(format!("a saved tile with an injected field ran: {} {}", r.status, r.body));
    }
    assert!(wrong.is_empty(), "hostile input that wasn't refused cleanly:\n{}", wrong.join("\n"));
    w.clean_up().await;
}
