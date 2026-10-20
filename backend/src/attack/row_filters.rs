//! Row filters can't be escaped. Organisation B sees only EMEA's sales. For random queries, on
//! every path to data, B's answer must equal what the system owner gets in A (unrestricted)
//! for the same query with `region = EMEA` written into it.

use rand::rngs::StdRng;
use rand::seq::SliceRandom;
use rand::{Rng, SeedableRng};
use serde_json::{Value, json};

use super::{As, Who, World};
use crate::api::tests::OWNER;

const DIMS: &[&str] = &["region", "store", "category", "sold_at:month", "sold_at:year", "sold_at:day", "amount"];
const MEASURES: &[&str] = &[
    "revenue",
    "units",
    "transactions",
    "avg_ticket",
    "revenue_per_unit",
    "count_distinct(store)",
    "count_distinct(region)",
    "sum(qty)",
    "max(amount)",
];

fn restriction() -> Value {
    json!({"field": "region", "op": "equals", "values": ["EMEA"]})
}

/// A filter someone might try, including ones aimed at other regions.
fn filter(rng: &mut StdRng) -> Value {
    let pick = |rng: &mut StdRng, xs: &[&str]| xs.choose(rng).unwrap().to_string();
    match rng.gen_range(0..8) {
        0 => json!({"field": "region", "op": "equals", "values": [pick(rng, &["APAC", "AMER", "EMEA"])]}),
        1 => json!({"field": "region", "op": "not_equals", "values": ["EMEA"]}),
        2 => json!({"field": "region", "op": "equals", "values": ["APAC", "AMER", "EMEA"]}),
        3 => json!({"field": "category", "op": "equals", "values": [pick(rng, &["Apparel", "Footwear", "Outerwear"])]}),
        4 => json!({"field": "amount", "op": "gt", "values": [rng.gen_range(10..900)]}),
        5 => json!({"field": "store", "op": "contains", "values": [rng.gen_range(0..3).to_string()]}),
        6 => json!({"field": "region", "op": "is_null", "values": []}),
        _ => json!({"field": "sold_at", "op": "last_n_days", "values": [rng.gen_range(1..400)]}),
    }
}

/// A random semantic query on `sales`.
fn query(rng: &mut StdRng) -> Value {
    let (n_dims, n_measures) = (rng.gen_range(0..3), rng.gen_range(1..4));
    let mut dims: Vec<&str> = DIMS.choose_multiple(rng, n_dims).copied().collect();
    let measures: Vec<&str> = MEASURES.choose_multiple(rng, n_measures).copied().collect();
    let filters: Vec<Value> = (0..rng.gen_range(0..3)).map(|_| filter(rng)).collect();
    let mut q = json!({"model": "sales", "dimensions": dims, "measures": measures, "filters": filters, "limit": 50000});
    if rng.gen_bool(0.2) {
        let compare = ["previous_period", "previous_year"].choose(rng).unwrap();
        q["period"] =
            json!({"field": "sold_at", "current": {"last_n_days": rng.gen_range(7..200)}, "compare": compare});
    }
    if rng.gen_bool(0.15) && !dims.is_empty() {
        dims.truncate(1);
        q["dimensions"] = json!(dims);
        q["top"] =
            json!({"dimension": dims[0].split(':').next().unwrap(), "n": rng.gen_range(1..4), "by": measures[0]});
    }
    q
}

/// The query with the restriction written into it.
fn restricted(q: &Value) -> Value {
    let mut q = q.clone();
    q["filters"].as_array_mut().unwrap().push(restriction());
    q
}

/// Rows as comparable text: numbers rounded (sums may add up in another order), sorted.
fn canonical(rows: &Value) -> Vec<String> {
    let cell = |v: &Value| match v.as_f64() {
        Some(f) if v.is_number() => format!("{:.6e}", f),
        _ => v.to_string(),
    };
    let mut out: Vec<String> = rows
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .map(|r| r.as_array().unwrap_or(&vec![]).iter().map(cell).collect::<Vec<_>>().join("|"))
        .collect();
    out.sort();
    out
}

fn differs(what: &str, got: &Value, expected: &Value) -> Option<String> {
    if got["rows"].is_null() && expected["rows"].is_null() && got["detail"] == expected["detail"] {
        return None; // refused alike (a drill on a field the tile doesn't show, ...)
    }
    if got["rows"].is_null() || expected["rows"].is_null() {
        return Some(format!("{what}: no rows\n  B: {got}\n  expected: {expected}"));
    }
    let (g, e) = (canonical(&got["rows"]), canonical(&expected["rows"]));
    (g != e).then(|| {
        let extra: Vec<_> = g.iter().filter(|r| !e.contains(r)).take(3).collect();
        format!("{what}: {} rows, expected {}; rows B shouldn't see, e.g. {extra:?}", g.len(), e.len())
    })
}

fn people(w: &World) -> (As, As) {
    let b = As { who: Who::Person(w.admin_b.clone()), org: w.b.clone(), view_as: None };
    let owner = As { who: Who::Person(OWNER.into()), org: w.a.clone(), view_as: None };
    (b, owner)
}

#[tokio::test]
async fn explore_and_agents_never_see_past_a_restriction() {
    let Some(w) = World::new().await else { return };
    let (b, owner) = people(&w);
    let token = As { who: Who::Token(w.token_b.clone()), org: w.b.clone(), view_as: None };
    let mut rng = StdRng::seed_from_u64(20261019);
    let mut leaks = vec![];
    for i in 0..120 {
        let q = query(&mut rng);
        let expected = w.send(&owner, "POST", "/api/query", Some(restricted(&q))).await;
        if !expected.status.is_success() {
            continue; // a query the model refuses (both would)
        }
        let got = w.send(&b, "POST", "/api/query", Some(q.clone())).await.json();
        if let Some(d) = differs(&format!("query {i} {q}"), &got, &expected.json()) {
            leaks.push(d);
        }
        if i % 4 == 0 {
            // the same through MCP, with B's API token
            let call = json!({"jsonrpc": "2.0", "id": i, "method": "tools/call",
                              "params": {"name": "run_query", "arguments": {"query": q}}});
            let r = w.send(&token, "POST", "/api/mcp", Some(call)).await.json();
            if let Some(d) = differs(&format!("MCP query {i} {q}"), &r["result"]["structuredContent"], &expected.json())
            {
                leaks.push(d);
            }
        }
    }
    assert!(leaks.is_empty(), "answers past the restriction:\n{}", leaks.join("\n"));
    w.clean_up().await;
}

/// A dashboard of random tiles, in B as is and in A with the restriction in every tile.
fn dashboards(rng: &mut StdRng) -> (String, String, Vec<String>) {
    let mut tiles = vec![];
    let mut ids = vec![];
    for i in 0..10 {
        let mut q = query(rng);
        q.as_object_mut().unwrap().remove("limit");
        let id = format!("t{i}");
        tiles.push((id.clone(), q));
        ids.push(id);
    }
    let yaml = |restrict: bool| {
        let mut d = json!({"title": "Random", "filters": [{"model": "sales", "field": "region"}, {"model": "sales", "field": "store"}],
                           "tiles": []});
        for (id, q) in &tiles {
            let q = if restrict { restricted(q) } else { q.clone() };
            d["tiles"]
                .as_array_mut()
                .unwrap()
                .push(json!({"id": id, "type": "table", "drill": "location", "query": q}));
        }
        serde_yaml::to_string(&d).unwrap()
    };
    (yaml(false), yaml(true), ids)
}

/// What a viewer might ask a saved tile: dashboard filters, a drill-down.
fn request(rng: &mut StdRng) -> Value {
    let mut r = json!({"filters": {}, "drill": []});
    if rng.gen_bool(0.4) {
        let region = *["APAC", "AMER", "EMEA"].choose(rng).unwrap();
        r["filters"]["sales.region"] = json!([region]);
    }
    if rng.gen_bool(0.3) {
        r["filters"]["sales.store"] = json!([format!("Store {:02}", rng.gen_range(1..25))]);
    }
    if rng.gen_bool(0.3) {
        let region = *["APAC", "AMER", "EMEA"].choose(rng).unwrap();
        r["drill"] = json!([{"field": "region", "value": region}]);
    }
    r
}

#[tokio::test]
async fn dashboards_links_and_downloads_never_see_past_a_restriction() {
    let Some(w) = World::new().await else { return };
    let (b, owner) = people(&w);
    let mut rng = StdRng::seed_from_u64(1019);
    let (plain, with_restriction, tiles) = dashboards(&mut rng);
    assert!(w.yaml(&b, "/api/dashboards/random/yaml", &plain).await.status.is_success());
    assert!(w.yaml(&owner, "/api/dashboards/random/yaml", &with_restriction).await.status.is_success());

    // B's own links to it: public (from the cache) and guest (as a User, who may drill)
    let r = w.send(&b, "POST", "/api/dashboards/random/share-links", Some(json!({"cache_seconds": 300}))).await.json();
    let public = format!("/api/public/{}/{}", w.b, r["url"].as_str().unwrap().rsplit('/').next().unwrap());
    let body = json!({"expires_in_hours": 1, "password": "Guest-Password-1", "role": "user"});
    let r = w.send(&b, "POST", "/api/dashboards/random/guest-links", Some(body)).await.json();
    let guest_api = format!("/api/guest/{}/{}", w.b, r["url"].as_str().unwrap().rsplit('/').next().unwrap());
    let anyone = As { who: Who::Anonymous, org: String::new(), view_as: None };
    let r =
        w.send(&anyone, "POST", &format!("{guest_api}/unlock"), Some(json!({"password": "Guest-Password-1"}))).await;
    let guest = As {
        who: Who::Cookie(r.headers["set-cookie"].to_str().unwrap().split(';').next().unwrap().into()),
        ..anyone.clone()
    };

    let mut leaks = vec![];
    for t in &tiles {
        let path = |base: &str| format!("{base}/tiles/{t}/query");
        let expected_default = w.send(&owner, "POST", &path("/api/dashboards/random"), Some(json!({}))).await.json();
        let from_public = w.send(&anyone, "GET", &format!("{public}/tiles/{t}"), None).await.json();
        leaks.extend(differs(&format!("public link, tile {t}"), &from_public, &expected_default));
        for _ in 0..3 {
            let req = request(&mut rng);
            let expected = w.send(&owner, "POST", &path("/api/dashboards/random"), Some(req.clone())).await.json();
            let got = w.send(&b, "POST", &path("/api/dashboards/random"), Some(req.clone())).await.json();
            leaks.extend(differs(&format!("tile {t} with {req}"), &got, &expected));
            let as_guest = w.send(&guest, "POST", &path(&guest_api), Some(req.clone())).await.json();
            leaks.extend(differs(&format!("guest link, tile {t} with {req}"), &as_guest, &expected));
            // the CSV holds what the tile shows them
            let csv = w.send(&b, "POST", &format!("/api/dashboards/random/tiles/{t}/export"), Some(req.clone())).await;
            let shown = w.send(&b, "POST", &path("/api/dashboards/random"), Some(req.clone())).await.json();
            let lines = csv.body.trim_end().lines().count().saturating_sub(1);
            let rows = shown["rows"].as_array().map_or(0, |r| r.len());
            if lines != rows {
                let body: String = csv.body.chars().take(300).collect();
                leaks.push(format!(
                    "CSV of tile {t} with {req}: {lines} rows, the tile shows {rows} ({}: {body})",
                    csv.status
                ));
            }
        }
    }
    // pick lists offer only what B may see
    let r = w.send(&b, "GET", "/api/dashboards/random/filters/values?key=sales.region", None).await.json();
    let regions: Vec<String> =
        r["rows"].as_array().unwrap().iter().map(|x| x[0].as_str().unwrap().to_string()).collect();
    if regions != ["EMEA"] {
        leaks.push(format!("the region pick list offers {regions:?}"));
    }
    let r = w.send(&b, "GET", "/api/dashboards/random/filters/values?key=sales.store", None).await.json();
    let emea = json!({"model": "sales", "dimensions": ["store"], "filters": [restriction()], "limit": 1000});
    let allowed = canonical(&w.send(&owner, "POST", "/api/query", Some(emea)).await.json()["rows"]);
    for store in canonical(&r["rows"]) {
        if !allowed.contains(&store) {
            leaks.push(format!("the store pick list offers {store}, outside EMEA"));
        }
    }
    assert!(leaks.is_empty(), "answers past the restriction:\n{}", leaks.join("\n"));
    w.clean_up().await;
}

#[tokio::test]
async fn an_organisations_own_model_on_the_same_table_is_restricted_too() {
    let Some(w) = World::new().await else { return };
    let (b, owner) = people(&w);
    let mut leaks = vec![];
    // the same table, however its name is written
    for (name, table) in
        [("my_sales", "sample_sales"), ("my_sales_q", "public.sample_sales"), ("my_sales_c", "SAMPLE_SALES")]
    {
        let model = json!({"name": name, "connection": "warehouse", "table": table,
                           "dimensions": [{"name": "region"}, {"name": "category"}], "measures": [{"name": "n", "type": "count"}]});
        let r = w.send(&b, "PUT", &format!("/api/org-models/{name}"), Some(model)).await;
        if !r.status.is_success() {
            continue; // a name the database doesn't know: nothing to read either
        }
        for dims in [vec!["region"], vec!["category"], vec![]] {
            let q = json!({"model": name, "dimensions": dims, "measures": ["n"]});
            let got = w.send(&b, "POST", "/api/query", Some(q)).await.json();
            let oracle =
                json!({"model": "sales", "dimensions": dims, "measures": ["transactions"], "filters": [restriction()]});
            let expected = w.send(&owner, "POST", "/api/query", Some(oracle)).await.json();
            leaks.extend(differs(&format!("{name} on {table} by {dims:?}"), &got, &expected));
        }
    }
    assert!(leaks.is_empty(), "an organisation's own model escaped the restriction:\n{}", leaks.join("\n"));
    w.clean_up().await;
}
