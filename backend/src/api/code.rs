//! An organisation's setup as code: export it as YAML files to keep in git, see what applying
//! files would change (a plan, `?dry_run=1`), and apply them, like terraform. Applying goes
//! through the API's own handlers, so the same permissions and checks hold. Secrets are never in
//! the files: they stay where they're kept, write-only.
//!
//! The files, one per thing (the stem is its name):
//!   models/<name>.yaml             the organisation's own models
//!   formulas/<model>/<name>.yaml   shared formulas
//!   blends/<name>.yaml             the organisation's own blends
//!   themes.yaml                    its themes and defaults
//!   glossary/<name>.yaml           business terms
//!   catalog/<model>.yaml           a model's owner, stewards, status and documentation
//!   dashboards/<id>.yaml           dashboards, as their YAML view shows them

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query as UrlQuery, State};
use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::dashboards::{WriteParams, flag};
use super::{Body, blends, dashboards, formulas, governance, org_models, themes};
use crate::auth::{Auth, Caller};
use crate::error::{ApiError, ApiResult};
use crate::governance::{Governance, ModelDocs, Term};
use crate::state::{AppState, Shared};

/// Kinds of files, in the order they're applied (each may use the ones before it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Model,
    Formula,
    Blend,
    Themes,
    Term,
    Catalog,
    Dashboard,
}

/// A model's catalog entry: who looks after it, and what the organisation says about it.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Catalog {
    #[serde(default)]
    governance: Governance,
    #[serde(default)]
    docs: ModelDocs,
}

/// One file: what it is, and its content (`value` to compare and send, `text` as written).
#[derive(Debug, Clone)]
struct Item {
    kind: Kind,
    path: String,
    /// the names in its path: [name], or [model, name] for a formula
    names: Vec<String>,
    value: Value,
    text: String,
}

impl Item {
    fn new<T: Serialize>(kind: Kind, path: String, names: Vec<String>, t: &T) -> ApiResult<Item> {
        let text = serde_yaml::to_string(t).map_err(|e| ApiError::Internal(e.into()))?;
        Ok(Item { kind, path, names, value: json!(t), text })
    }

    fn name(&self) -> String {
        self.names.last().cloned().unwrap_or_default()
    }
}

/// What a path names, or why it isn't one of the files.
fn kind_of(path: &str) -> Result<(Kind, Vec<String>), String> {
    let unknown = || format!("{path}: not a file Magnum BI knows (see the export for the layout)");
    if path == "themes.yaml" {
        return Ok((Kind::Themes, vec![]));
    }
    let stem = path.strip_suffix(".yaml").ok_or_else(unknown)?;
    let parts: Vec<&str> = stem.split('/').collect();
    let kind = match parts[0] {
        "models" => Kind::Model,
        "formulas" => Kind::Formula,
        "blends" => Kind::Blend,
        "glossary" => Kind::Term,
        "catalog" => Kind::Catalog,
        "dashboards" => Kind::Dashboard,
        _ => return Err(unknown()),
    };
    let names: Vec<String> = parts[1..].iter().map(|s| s.to_string()).collect();
    let expected = if kind == Kind::Formula { 2 } else { 1 };
    if names.len() != expected || names.iter().any(|n| n.is_empty()) {
        return Err(unknown());
    }
    Ok((kind, names))
}

/// A file read as what its path says it is.
fn parse(path: &str, text: &str) -> Result<Item, String> {
    let (kind, names) = kind_of(path)?;
    let name = names.last().cloned().unwrap_or_default();
    let bad = |e: serde_yaml::Error| format!("{path}: {e}");
    let named = |found: &str| {
        if found == name { Ok(()) } else { Err(format!("{path}: its name is {found:?}: name the file after it")) }
    };
    let value = match kind {
        Kind::Model => {
            let m: crate::schema::Model = serde_yaml::from_str(text).map_err(bad)?;
            named(&m.name)?;
            json!(m)
        }
        Kind::Formula => {
            let f: crate::schema::Formula = serde_yaml::from_str(text).map_err(bad)?;
            named(&f.name)?;
            json!(f)
        }
        Kind::Blend => {
            let b: crate::blend::Blend = serde_yaml::from_str(text).map_err(bad)?;
            named(&b.name)?;
            json!(b)
        }
        Kind::Themes => json!(serde_yaml::from_str::<crate::themes::OrgThemes>(text).map_err(bad)?),
        Kind::Term => {
            let t: Term = serde_yaml::from_str(text).map_err(bad)?;
            named(&t.name)?;
            json!(t)
        }
        Kind::Catalog => json!(serde_yaml::from_str::<Catalog>(text).map_err(bad)?),
        Kind::Dashboard => json!(crate::project::parse_dashboard(text).map_err(|e| format!("{path}: {e}"))?),
    };
    Ok(Item { kind, path: path.to_string(), names, value, text: text.to_string() })
}

/// Everything the organisation has, as files.
async fn current(state: &AppState, caller: &Caller) -> ApiResult<BTreeMap<String, Item>> {
    let org = caller.org();
    let store = state.orgs.store(org).await?;
    let mut items = vec![];
    for (m, ..) in store.org_models(org).await? {
        items.push(Item::new(Kind::Model, format!("models/{}.yaml", m.name), vec![m.name.clone()], &m)?);
    }
    for (model, f, ..) in store.formulas(org).await? {
        let path = format!("formulas/{model}/{}.yaml", f.name);
        items.push(Item::new(Kind::Formula, path, vec![model, f.name.clone()], &f)?);
    }
    for (b, ..) in store.blends(org).await? {
        items.push(Item::new(Kind::Blend, format!("blends/{}.yaml", b.name), vec![b.name.clone()], &b)?);
    }
    let t = store.org_themes(org).await?;
    if json!(t) != json!(crate::themes::OrgThemes::default()) {
        items.push(Item::new(Kind::Themes, "themes.yaml".into(), vec![], &t)?);
    }
    for (t, ..) in store.glossary(org).await? {
        items.push(Item::new(Kind::Term, format!("glossary/{}.yaml", t.name), vec![t.name.clone()], &t)?);
    }
    let mut catalog: BTreeMap<String, Catalog> = BTreeMap::new();
    for (model, g, ..) in store.governance(org).await? {
        catalog.entry(model).or_default().governance = g;
    }
    for (model, d) in store.model_docs(org).await? {
        catalog.entry(model).or_default().docs = d;
    }
    for (model, c) in catalog.into_iter().filter(|(_, c)| *c != Catalog::default()) {
        items.push(Item::new(Kind::Catalog, format!("catalog/{model}.yaml"), vec![model], &c)?);
    }
    for (id, text) in store.all_dashboards(org).await? {
        let path = format!("dashboards/{id}.yaml");
        // as written (comments and all); one that no longer reads is compared as text
        let value = crate::project::parse_dashboard(&text).map(|d| json!(d)).unwrap_or_else(|_| json!(text));
        items.push(Item { kind: Kind::Dashboard, path, names: vec![id], value, text });
    }
    Ok(items.into_iter().map(|i| (i.path.clone(), i)).collect())
}

/// `GET /api/code`: the organisation's setup as files (Org admins).
pub async fn export(State(state): State<Shared>, Auth(caller): Auth) -> ApiResult<Json<Value>> {
    caller.require("manage_members")?;
    let files: BTreeMap<String, String> =
        current(&state, &caller).await?.into_values().map(|i| (i.path, i.text)).collect();
    Ok(Json(json!({"files": files})))
}

#[derive(Deserialize)]
pub struct Bundle {
    files: BTreeMap<String, String>,
}

#[derive(Deserialize, Default)]
pub struct ApplyParams {
    #[serde(default, deserialize_with = "flag")]
    dry_run: bool,
    /// also delete what the files don't have (otherwise it's kept)
    #[serde(default, deserialize_with = "flag")]
    prune: bool,
}

/// One change a plan makes: a file to create, update or delete, and what's wrong with it.
#[derive(Serialize)]
struct Change {
    path: String,
    action: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// `POST /api/code`: apply files (`?dry_run=1`: only the plan; `?prune=1`: also delete what they
/// don't have). Each change is checked first; with any problem nothing is changed.
pub async fn apply(
    State(state): State<Shared>,
    Auth(caller): Auth,
    UrlQuery(p): UrlQuery<ApplyParams>,
    Body(bundle): Body<Bundle>,
) -> ApiResult<Json<Value>> {
    caller.require("manage_members")?;
    let now = current(&state, &caller).await?;
    let mut changes = vec![];
    let mut wanted: BTreeMap<String, Item> = BTreeMap::new();
    for (path, text) in &bundle.files {
        match parse(path, text) {
            Ok(item) => {
                wanted.insert(path.clone(), item);
            }
            Err(e) => changes.push(Change { path: path.clone(), action: "error", error: Some(e) }),
        }
    }
    let writes: Vec<&Item> = wanted.values().filter(|i| now.get(&i.path).is_none_or(|n| n.value != i.value)).collect();
    let deletes: Vec<&Item> =
        if p.prune { now.values().filter(|i| !wanted.contains_key(&i.path)).collect() } else { vec![] };

    // checked against the organisation as it will be: a formula on a model these files add fits
    let after = planned(&state, &caller, &now, &wanted, p.prune);
    let mut ordered = writes.clone();
    ordered.sort_by_key(|i| i.kind);
    for item in &ordered {
        let error = write(&state, &after, item, true).await.err().map(|e| detail(e).1);
        let action = if now.contains_key(&item.path) { "update" } else { "create" };
        changes.push(Change { path: item.path.clone(), action, error });
    }
    for item in &deletes {
        changes.push(Change { path: item.path.clone(), action: "delete", error: None });
    }
    let problems = changes.iter().any(|c| c.error.is_some());
    let unchanged = wanted.len() - writes.len();
    if p.dry_run || problems {
        let plan = json!({"ok": !problems, "dry_run": true, "changes": changes, "unchanged": unchanged});
        return if problems && !p.dry_run {
            Err(ApiError::Body(axum::http::StatusCode::BAD_REQUEST, plan))
        } else {
            Ok(Json(plan))
        };
    }

    for item in &ordered {
        write(&state, &after, item, false).await.map_err(|e| stopped(e, &item.path))?;
    }
    let mut deletes = deletes;
    deletes.sort_by_key(|i| std::cmp::Reverse(i.kind));
    let caller = Arc::new(caller);
    for item in &deletes {
        remove(&state, &caller, item).await.map_err(|e| stopped(e, &item.path))?;
    }
    state.usage.record(crate::usage::Event {
        detail: Some(json!({"changes": changes.len()})),
        ..caller.event("code_applied")
    });
    Ok(Json(json!({"ok": true, "changes": changes, "unchanged": unchanged})))
}

/// An error's status and message.
fn detail(e: ApiError) -> (axum::http::StatusCode, String) {
    let (status, body) = e.into_parts();
    (status, body["detail"].as_str().unwrap_or("failed").to_string())
}

/// A change that failed while applying: which, and that the ones before it were made.
fn stopped(e: ApiError, path: &str) -> ApiError {
    let (status, why) = detail(e);
    ApiError::Status(status, format!("{path}: {why}; the changes before it were made, the rest weren't"))
}

/// The caller, seeing the organisation as the files will leave it: its models, formulas, blends,
/// themes and glossary terms.
fn planned(
    state: &AppState,
    caller: &Caller,
    now: &BTreeMap<String, Item>,
    wanted: &BTreeMap<String, Item>,
    prune: bool,
) -> Arc<Caller> {
    let mut after: BTreeMap<&str, &Item> =
        if prune { BTreeMap::new() } else { now.iter().map(|(k, v)| (k.as_str(), v)).collect() };
    after.extend(wanted.iter().map(|(k, v)| (k.as_str(), v)));
    let of = |kind: Kind| after.values().filter(move |i| i.kind == kind);
    let models: Vec<crate::schema::Model> =
        of(Kind::Model).filter_map(|i| serde_json::from_value(i.value.clone()).ok()).collect();
    let formulas: Vec<(String, crate::schema::Formula)> = of(Kind::Formula)
        .filter_map(|i| Some((i.names[0].clone(), serde_json::from_value(i.value.clone()).ok()?)))
        .collect();
    let blends: Vec<crate::blend::Blend> =
        of(Kind::Blend).filter_map(|i| serde_json::from_value(i.value.clone()).ok()).collect();
    let mut project =
        state.project.current().with_org_models(&models).with_formulas(&formulas).with_org_blends(&blends);
    if let Some(t) = of(Kind::Themes).find_map(|i| serde_json::from_value(i.value.clone()).ok()) {
        project = project.with_org_themes(&t);
    }
    let terms = of(Kind::Term).map(|i| i.name()).collect::<BTreeSet<_>>().into_iter().collect();
    Arc::new(Caller { project: Arc::new(project), planned_terms: terms, ..caller.clone() })
}

/// Save one file's content through its handler (`dry_run`: check only).
async fn write(state: &Shared, caller: &Arc<Caller>, item: &Item, dry_run: bool) -> ApiResult<()> {
    let (s, a) = (|| State(state.clone()), || Auth(caller.clone()));
    let q = || UrlQuery(WriteParams { dry_run });
    let _ = match item.kind {
        Kind::Model => org_models::save(s(), a(), Path(item.name()), q(), Body(from(item)?)).await?,
        Kind::Formula => {
            formulas::save(s(), a(), Path((item.names[0].clone(), item.name())), q(), Body(from(item)?)).await?
        }
        Kind::Blend => blends::save(s(), a(), Path(item.name()), q(), Body(from(item)?)).await?,
        Kind::Themes => themes::put_org(s(), a(), q(), HeaderMap::new(), Body(from(item)?)).await?,
        Kind::Term => governance::save_term(s(), a(), Path(item.name()), q(), Body(from(item)?)).await?,
        Kind::Catalog => {
            let c: Catalog = from(item)?;
            let _ = governance::save(s(), a(), Path(item.name()), q(), Body(c.governance)).await?;
            governance::save_docs(s(), a(), Path(item.name()), q(), Body(c.docs)).await?
        }
        Kind::Dashboard => {
            dashboards::save_yaml(s(), a(), Path(item.name()), q(), HeaderMap::new(), item.text.clone()).await?
        }
    };
    Ok(())
}

/// Delete what one file was (a model's catalog entry and the themes go back to none).
async fn remove(state: &Shared, caller: &Arc<Caller>, item: &Item) -> ApiResult<()> {
    let (s, a) = (|| State(state.clone()), || Auth(caller.clone()));
    let q = || UrlQuery(WriteParams { dry_run: false });
    let _ = match item.kind {
        Kind::Model => org_models::delete(s(), a(), Path(item.name())).await?,
        Kind::Formula => formulas::delete(s(), a(), Path((item.names[0].clone(), item.name()))).await?,
        Kind::Blend => blends::delete(s(), a(), Path(item.name())).await?,
        Kind::Themes => themes::put_org(s(), a(), q(), HeaderMap::new(), Body(Default::default())).await?,
        Kind::Term => governance::delete_term(s(), a(), Path(item.name())).await?,
        Kind::Catalog => {
            let _ = governance::save(s(), a(), Path(item.name()), q(), Body(Governance::default())).await?;
            governance::save_docs(s(), a(), Path(item.name()), q(), Body(ModelDocs::default())).await?
        }
        Kind::Dashboard => dashboards::delete(s(), a(), Path(item.name()), HeaderMap::new()).await?,
    };
    Ok(())
}

fn from<T: serde::de::DeserializeOwned>(item: &Item) -> ApiResult<T> {
    serde_json::from_value(item.value.clone()).map_err(|e| ApiError::bad_request(format!("{}: {e}", item.path)))
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use serde_json::{Value, json};

    use crate::api::share::tests::{clean_up, send};
    use crate::api::tests::{OWNER, app, scramdb_config};

    #[test]
    fn files_are_named_after_what_they_hold() {
        assert!(super::kind_of("models/sales.yaml").is_ok());
        assert!(super::kind_of("formulas/sales/per_sale.yaml").is_ok());
        assert!(super::kind_of("themes.yaml").is_ok());
        for bad in ["models/sales.yml", "formulas/per_sale.yaml", "models/a/b.yaml", "secrets/key.yaml", "models/.yaml"]
        {
            assert!(super::kind_of(bad).is_err(), "{bad}");
        }
        let e = super::parse("models/other.yaml", "name: sales\nconnection: w\ntable: t\n").unwrap_err();
        assert!(e.contains("name the file after it"), "{e}");
    }

    #[tokio::test]
    async fn an_organisation_goes_to_code_and_back() {
        let Some((app, state, cookie)) = app().await else { return };
        let id = || uuid::Uuid::new_v4().simple().to_string()[..10].to_string();
        let (a, b) = (format!("codea{}", id()), format!("codeb{}", id()));
        let orgs = json!({a.clone(): {"name": "From", "models": ["*"]}, b.clone(): {"name": "To", "models": ["*"]}});
        state.access.pin(
            crate::access::AccessConfig::from_document(json!({"system_owners": [OWNER], "organisations": orgs}))
                .unwrap(),
        );
        let c = Some(cookie.as_str());
        for org in [&a, &b] {
            state.orgs.pin(org, state.store.clone());
            let (s, r, _) = send(
                &app,
                c,
                org,
                "PUT",
                "/api/connections/warehouse",
                Some(json!({"config": scramdb_config()})),
                None,
            )
            .await;
            assert_eq!(s, StatusCode::OK, "{r}");
        }

        // A: a model of its own, a formula on it, a blend, a term, its catalog entry, a dashboard
        let name = format!("my_sales_{}", id());
        let proposal = format!("/api/connections/warehouse/tables/sample_sales/model?name={name}");
        let (_, p, _) = send(&app, c, &a, "GET", &proposal, None, None).await;
        let model = p["model"].clone();
        let measure = model["measures"][0]["name"].as_str().unwrap().to_string();
        let term = format!("Sale{}", id());
        let blend = format!("{name}_blend");
        for (method, path, body) in [
            ("PUT", format!("/api/org-models/{name}"), model.clone()),
            ("PUT", format!("/api/formulas/{name}/twice"), json!({"name": "twice", "expr": format!("{measure} * 2")})),
            (
                "PUT",
                format!("/api/blends/{blend}"),
                json!({"name": blend, "base": "sales", "joins": [{"model": name, "on": ["region = region"]}]}),
            ),
            ("PUT", format!("/api/glossary/{term}"), json!({"name": term, "definition": "One receipt."})),
            ("PUT", format!("/api/governance/{name}"), json!({"status": "certified", "note": "Checked"})),
            (
                "PUT",
                format!("/api/governance/{name}/docs"),
                json!({"description": "Our sales", "fields": {"twice": {"terms": [term]}}}),
            ),
        ] {
            let (s, r, _) = send(&app, c, &a, method, &path, Some(body), None).await;
            assert_eq!(s, StatusCode::OK, "{path}: {r}");
        }
        let dashboard = format!(
            "title: Code test\ntiles:\n  - {{id: t1, type: kpi, query: {{model: {name}, measures: [twice]}}}}\n"
        );
        put_yaml(&app, &cookie, &a, "code_test", &dashboard).await;

        // its setup as files, for Org admins only
        let (s, _, _) = send(&app, c, &a, "GET", "/api/code", None, Some("power_user")).await;
        assert_eq!(s, StatusCode::FORBIDDEN);
        let (s, code, _) = send(&app, c, &a, "GET", "/api/code", None, None).await;
        assert_eq!(s, StatusCode::OK, "{code}");
        let files = code["files"].clone();
        for path in [
            format!("models/{name}.yaml"),
            format!("formulas/{name}/twice.yaml"),
            format!("blends/{blend}.yaml"),
            format!("glossary/{term}.yaml"),
            format!("catalog/{name}.yaml"),
            "dashboards/code_test.yaml".to_string(),
        ] {
            assert!(files.get(&path).is_some(), "{path} in {files}");
        }
        assert_eq!(files["dashboards/code_test.yaml"], json!(dashboard), "dashboards as written");

        // B: the plan creates everything (in an order that works) and changes nothing
        let (s, plan, _) = send(&app, c, &b, "POST", "/api/code?dry_run=1", Some(json!({"files": files})), None).await;
        assert_eq!((s, &plan["ok"]), (StatusCode::OK, &json!(true)), "{plan}");
        let actions: Vec<&str> =
            plan["changes"].as_array().unwrap().iter().map(|c| c["action"].as_str().unwrap()).collect();
        assert_eq!(actions, ["create"; 6], "{plan}");
        assert_eq!(plan["changes"][0]["path"], json!(format!("models/{name}.yaml")), "models first: {plan}");
        assert_eq!(export(&app, &cookie, &b).await, json!({}), "a plan changes nothing");

        // applied, B has what A has; applying again changes nothing
        let (s, r, _) = send(&app, c, &b, "POST", "/api/code", Some(json!({"files": files})), None).await;
        assert_eq!(s, StatusCode::OK, "{r}");
        assert_eq!(export(&app, &cookie, &b).await, files);
        let (_, again, _) = send(&app, c, &b, "POST", "/api/code?dry_run=1", Some(json!({"files": files})), None).await;
        assert_eq!((&again["changes"], &again["unchanged"]), (&json!([]), &json!(6)), "{again}");

        // a change is an update; a mistake is shown, and stops everything
        let mut changed = files.clone();
        changed[format!("formulas/{name}/twice.yaml")] = json!(format!("name: twice\nexpr: {measure} * 3\n"));
        let (_, plan, _) =
            send(&app, c, &b, "POST", "/api/code?dry_run=1", Some(json!({"files": changed})), None).await;
        assert_eq!(plan["changes"], json!([{"path": format!("formulas/{name}/twice.yaml"), "action": "update"}]));
        let mut broken = changed.clone();
        broken["glossary/new.yaml"] = json!("name: new\ndefinition: New\n");
        broken[format!("formulas/{name}/twice.yaml")] = json!("name: twice\nexpr: nope * 3\n");
        broken["secrets/x.yaml"] = json!("a: 1\n");
        let (s, r, _) = send(&app, c, &b, "POST", "/api/code", Some(json!({"files": broken})), None).await;
        assert_eq!((s, &r["ok"]), (StatusCode::BAD_REQUEST, &json!(false)), "{r}");
        let errors: Vec<&str> = r["changes"].as_array().unwrap().iter().filter_map(|c| c["error"].as_str()).collect();
        assert_eq!(errors.len(), 2, "{r}");
        assert_eq!(export(&app, &cookie, &b).await, files, "nothing changed");

        // a file left out is kept, unless pruning
        let mut fewer = files.clone();
        fewer.as_object_mut().unwrap().remove(&format!("blends/{blend}.yaml"));
        let (_, plan, _) = send(&app, c, &b, "POST", "/api/code?dry_run=1", Some(json!({"files": fewer})), None).await;
        assert_eq!(plan["changes"], json!([]));
        let (s, r, _) = send(&app, c, &b, "POST", "/api/code?prune=1", Some(json!({"files": fewer})), None).await;
        assert_eq!(r["changes"], json!([{"path": format!("blends/{blend}.yaml"), "action": "delete"}]), "{s} {r}");
        assert_eq!(export(&app, &cookie, &b).await, fewer);

        // power users can't apply
        let (s, _, _) =
            send(&app, c, &b, "POST", "/api/code?dry_run=1", Some(json!({"files": files})), Some("power_user")).await;
        assert_eq!(s, StatusCode::FORBIDDEN);
        for org in [&a, &b] {
            clean_up(&state, org).await;
        }
    }

    async fn export(app: &axum::Router, cookie: &str, org: &str) -> Value {
        send(app, Some(cookie), org, "GET", "/api/code", None, None).await.1["files"].clone()
    }

    async fn put_yaml(app: &axum::Router, cookie: &str, org: &str, id: &str, yaml: &str) {
        let req = axum::http::Request::builder()
            .method("PUT")
            .uri(format!("/api/dashboards/{id}/yaml"))
            .header("cookie", cookie)
            .header("x-magnum-org", org)
            .header("content-type", "text/plain")
            .body(axum::body::Body::from(yaml.to_string()))
            .unwrap();
        let resp = tower::ServiceExt::oneshot(app.clone(), req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
