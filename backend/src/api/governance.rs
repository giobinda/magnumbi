//! Who answers for each model, and its status (governance.rs): read by everyone, changed by the
//! model's owner or an Org admin, in the organisation's own database.

use std::collections::BTreeMap;

use axum::Json;
use axum::extract::{Path, Query as UrlQuery, State};
use serde_json::{Value, json};

use super::Body;
use crate::auth::{Auth, Caller};
use crate::error::{ApiError, ApiResult};
use crate::governance::{Governance, ModelDocs, Standing, Term, standing};
use crate::state::{AppState, Shared};

/// Who may own or steward a model here: the organisation's members and the system owners (the
/// platform workspace: its system owners).
fn members(caller: &Caller) -> Vec<String> {
    let owners = caller.access.platform.system_owners.clone();
    match caller.org() {
        "" => owners,
        org => caller.access.members_of(org).users.keys().cloned().chain(owners).collect(),
    }
}

/// Each model's governance in the caller's organisation (cached).
pub async fn of_org(state: &AppState, caller: &Caller) -> ApiResult<BTreeMap<String, Governance>> {
    Ok(state.org_governance(caller.org()).await?.iter().cloned().collect())
}

/// A model's governance as the caller sees it: with where they stand.
pub fn shown(caller: &Caller, g: Option<&Governance>) -> Value {
    let g = g.cloned().unwrap_or_default();
    let standing = standing(&g, caller.email(), caller.can("manage_members"));
    json!({"owner": g.owner, "stewards": g.stewards, "status": g.status, "note": g.note, "standing": standing.as_str()})
}

/// Every model the caller may see, with its governance.
pub async fn list(State(state): State<Shared>, Auth(caller): Auth) -> ApiResult<Json<Value>> {
    caller.require("view")?;
    let store = state.orgs.store(caller.org()).await?;
    let saved: BTreeMap<String, (Governance, String, Option<String>)> =
        store.governance(caller.org()).await?.into_iter().map(|(m, g, at, by)| (m, (g, at, by))).collect();
    let docs = docs_of(&state, &caller).await?;
    let out: Vec<Value> = super::available_models(&caller)
        .keys()
        .map(|name| {
            let row = saved.get(name);
            let mut v = shown(&caller, row.map(|(g, ..)| g));
            v["model"] = json!(name);
            v["updated_at"] = json!(row.map(|(_, at, _)| at));
            v["updated_by"] = json!(row.and_then(|(_, _, by)| by.clone()));
            // the organisation's own docs, as written (the models show them merged)
            v["docs"] = json!(docs.get(name).cloned().unwrap_or_default());
            v
        })
        .collect();
    Ok(Json(json!({"models": out})))
}

/// Change a model's governance (`?dry_run=1`: check only): its owner or an Org admin; only an
/// Org admin gives it a new owner.
pub async fn save(
    State(state): State<Shared>,
    Auth(caller): Auth,
    Path(model): Path<String>,
    UrlQuery(p): UrlQuery<super::dashboards::WriteParams>,
    Body(next): Body<Governance>,
) -> ApiResult<Json<Value>> {
    caller.require("view")?;
    super::model_of(&caller, &model)?;
    let store = state.orgs.store(caller.org()).await?;
    let now = store.governance(caller.org()).await?.into_iter().find(|(m, ..)| *m == model).map(|(_, g, ..)| g);
    let now = now.unwrap_or_default();
    let at = standing(&now, caller.email(), caller.can("manage_members"));
    now.may_become(&next, at).map_err(ApiError::forbidden)?;
    next.check(&members(&caller)).map_err(ApiError::bad_request)?;
    if p.dry_run {
        return Ok(Json(json!({"ok": true, "dry_run": true})));
    }
    store.put_governance(caller.org(), &model, &next, Some(caller.email())).await?;
    state.governance.forget(caller.org());
    state.usage.record(crate::usage::Event { model: Some(model), ..caller.event("governance_saved") });
    Ok(Json(json!({"ok": true})))
}

/// The caller's standing on one model of their organisation.
async fn standing_on(state: &AppState, caller: &Caller, model: &str) -> ApiResult<Standing> {
    let g = of_org(state, caller).await?.remove(model).unwrap_or_default();
    Ok(standing(&g, caller.email(), caller.can("manage_members")))
}

/// Change what the organisation says about a model and its fields (`?dry_run=1`: check only):
/// its stewards, its owner, Org admins.
pub async fn save_docs(
    State(state): State<Shared>,
    Auth(caller): Auth,
    Path(model): Path<String>,
    UrlQuery(p): UrlQuery<super::dashboards::WriteParams>,
    Body(docs): Body<ModelDocs>,
) -> ApiResult<Json<Value>> {
    caller.require("view")?;
    let m = super::model_of(&caller, &model)?;
    if standing_on(&state, &caller, &model).await? < Standing::Steward {
        return Err(ApiError::forbidden("only the model's stewards, its owner or an Org admin document it"));
    }
    let fields: Vec<&str> =
        m.dimensions.iter().map(|d| d.name.as_str()).chain(m.measures.iter().map(|x| x.name.as_str())).collect();
    let store = state.orgs.store(caller.org()).await?;
    let terms: Vec<String> = store.glossary(caller.org()).await?.into_iter().map(|(t, ..)| t.name).collect();
    docs.check(&fields, &terms).map_err(ApiError::bad_request)?;
    if p.dry_run {
        return Ok(Json(json!({"ok": true, "dry_run": true})));
    }
    store.put_model_docs(caller.org(), &model, &docs, Some(caller.email())).await?;
    state.docs.forget(caller.org());
    state.usage.record(crate::usage::Event { model: Some(model), ..caller.event("docs_saved") });
    Ok(Json(json!({"ok": true})))
}

/// The model as the API shows it, with what the organisation says about it.
pub fn documented(v: &mut Value, docs: &BTreeMap<String, ModelDocs>, model: &str) {
    if let Some(d) = docs.get(model) {
        crate::governance::document(v, d);
    }
}

pub async fn docs_of(state: &AppState, caller: &Caller) -> ApiResult<BTreeMap<String, ModelDocs>> {
    Ok(state.org_docs(caller.org()).await?.iter().cloned().collect())
}

/// Whether the caller may add terms: Org admins, and whoever looks after a model.
async fn may_add_terms(state: &AppState, caller: &Caller) -> ApiResult<bool> {
    let admin = caller.can("manage_members");
    Ok(admin || of_org(state, caller).await?.values().any(|g| standing(g, caller.email(), false) >= Standing::Steward))
}

/// The organisation's glossary, each term with the fields that implement it.
pub async fn glossary(State(state): State<Shared>, Auth(caller): Auth) -> ApiResult<Json<Value>> {
    caller.require("view")?;
    let store = state.orgs.store(caller.org()).await?;
    let docs = docs_of(&state, &caller).await?;
    let visible = super::available_models(&caller);
    let admin = caller.can("manage_members");
    let terms: Vec<Value> = store
        .glossary(caller.org())
        .await?
        .into_iter()
        .map(|(t, owner, at, by)| {
            let fields: Vec<String> = docs
                .iter()
                .filter(|(m, _)| visible.contains_key(*m))
                .flat_map(|(m, d)| {
                    d.fields
                        .iter()
                        .filter(|(_, f)| f.terms.iter().any(|x| x.eq_ignore_ascii_case(&t.name)))
                        .map(move |(f, _)| format!("{m}.{f}"))
                })
                .collect();
            let mine = owner.as_deref().is_some_and(|o| o.eq_ignore_ascii_case(caller.email()));
            json!({"name": t.name, "definition": t.definition, "synonyms": t.synonyms, "owner": owner,
                   "fields": fields, "updated_at": at, "updated_by": by, "may_edit": admin || mine})
        })
        .collect();
    Ok(Json(json!({"terms": terms, "may_add": may_add_terms(&state, &caller).await?})))
}

/// Add or change a term (`?dry_run=1`: check only). A new term belongs to its maker; it's changed
/// by its owner or an Org admin.
pub async fn save_term(
    State(state): State<Shared>,
    Auth(caller): Auth,
    Path(name): Path<String>,
    UrlQuery(p): UrlQuery<super::dashboards::WriteParams>,
    Body(t): Body<Term>,
) -> ApiResult<Json<Value>> {
    caller.require("view")?;
    if !t.name.eq_ignore_ascii_case(&name) {
        return Err(ApiError::bad_request("the term's name must match the address it's saved to"));
    }
    t.check().map_err(ApiError::bad_request)?;
    let store = state.orgs.store(caller.org()).await?;
    let existing = store.glossary(caller.org()).await?.into_iter().find(|(x, ..)| x.name.eq_ignore_ascii_case(&name));
    let owner = match existing {
        Some((_, owner, ..)) => {
            let mine = owner.as_deref().is_some_and(|o| o.eq_ignore_ascii_case(caller.email()));
            if !mine && !caller.can("manage_members") {
                return Err(ApiError::forbidden("only the term's owner or an Org admin changes it"));
            }
            owner.unwrap_or_else(|| caller.email().to_string())
        }
        None if may_add_terms(&state, &caller).await? => caller.email().to_string(),
        None => return Err(ApiError::forbidden("terms are added by Org admins and whoever looks after a model")),
    };
    if p.dry_run {
        return Ok(Json(json!({"ok": true, "dry_run": true})));
    }
    store.put_term(caller.org(), &t, &owner, caller.email()).await?;
    state.usage.record(caller.event("term_saved"));
    Ok(Json(json!({"ok": true})))
}

pub async fn delete_term(
    State(state): State<Shared>,
    Auth(caller): Auth,
    Path(name): Path<String>,
) -> ApiResult<Json<Value>> {
    caller.require("view")?;
    let store = state.orgs.store(caller.org()).await?;
    let Some((_, owner, ..)) =
        store.glossary(caller.org()).await?.into_iter().find(|(x, ..)| x.name.eq_ignore_ascii_case(&name))
    else {
        return Err(ApiError::not_found(format!("no term {name:?} in the glossary")));
    };
    let mine = owner.as_deref().is_some_and(|o| o.eq_ignore_ascii_case(caller.email()));
    if !mine && !caller.can("manage_members") {
        return Err(ApiError::forbidden("only the term's owner or an Org admin removes it"));
    }
    store.delete_term(caller.org(), &name).await?;
    state.usage.record(caller.event("term_deleted"));
    Ok(Json(json!({"ok": true})))
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use serde_json::{Value, json};

    use crate::api::share::tests::{clean_up, org_with, send};
    use crate::api::tests::OWNER;

    const DASHBOARD: &str = "title: Governance test\ntiles: []\n";

    /// A member of `org` with a session: (email, cookie).
    async fn member(state: &crate::state::AppState, name: &str, org: &str) -> (String, String) {
        let email = format!("{name}-{}@gov.example", &org[5..]);
        let hash = crate::auth::password::hash("governance-password-1").unwrap();
        state.store.set_password(&email, None, &hash, None, false).await.unwrap();
        let s = state.store.create_session_as(&email, "password", chrono::Duration::hours(1), None).await.unwrap();
        (email, format!("magnum_session={s}"))
    }

    fn entry<'a>(list: &'a Value, model: &str) -> &'a Value {
        list["models"].as_array().unwrap().iter().find(|m| m["model"] == json!(model)).unwrap()
    }

    #[tokio::test]
    async fn owners_and_stewards_answer_for_their_models() {
        let Some((app, state, cookie, org)) = org_with(DASHBOARD).await else { return };
        let admin = Some(cookie.as_str());
        let (ana, ana_cookie) = member(&state, "ana", &org).await;
        let (ben, ben_cookie) = member(&state, "ben", &org).await;
        let access = json!({"system_owners": [OWNER], "organisations": {org.clone(): {"name": "G", "models": ["*"],
            "members": {"users": {ana.clone(): {"role": "viewer"}, ben.clone(): {"role": "user"}}}}}});
        state.access.pin(crate::access::AccessConfig::from_document(access).unwrap());
        let (ana_c, ben_c) = (Some(ana_cookie.as_str()), Some(ben_cookie.as_str()));
        let put = |owner: &str, stewards: Value, status: &str| json!({"owner": owner, "stewards": stewards, "status": status, "note": "checked against finance"});

        // nobody owns it yet: everyone reads, only an Org admin decides
        let (s, list, _) = send(&app, ana_c, &org, "GET", "/api/governance", None, None).await;
        assert_eq!(s, StatusCode::OK, "{list}");
        assert_eq!(
            (&entry(&list, "sales")["owner"], &entry(&list, "sales")["standing"]),
            (&Value::Null, &json!("reader"))
        );
        let (s, _, _) =
            send(&app, ana_c, &org, "PUT", "/api/governance/sales", Some(put(&ana, json!([]), "draft")), None).await;
        assert_eq!(s, StatusCode::FORBIDDEN);

        // an Org admin hands it to Ana (a dry run first)
        let (s, b, _) = send(
            &app,
            admin,
            &org,
            "PUT",
            "/api/governance/sales?dry_run=1",
            Some(put(&ana, json!([]), "draft")),
            None,
        )
        .await;
        assert_eq!((s, &b["dry_run"]), (StatusCode::OK, &json!(true)), "{b}");
        let (_, list, _) = send(&app, ana_c, &org, "GET", "/api/governance", None, None).await;
        assert_eq!(entry(&list, "sales")["owner"], Value::Null);
        let (s, b, _) =
            send(&app, admin, &org, "PUT", "/api/governance/sales", Some(put(&ana, json!([]), "draft")), None).await;
        assert_eq!(s, StatusCode::OK, "{b}");

        // Ana decides now (she's a viewer: ownership isn't a role), but doesn't hand it over
        let certified = put(&ana, json!([ben]), "certified");
        let (s, b, _) = send(&app, ana_c, &org, "PUT", "/api/governance/sales", Some(certified.clone()), None).await;
        assert_eq!(s, StatusCode::OK, "{b}");
        let (s, b, _) =
            send(&app, ana_c, &org, "PUT", "/api/governance/sales", Some(put(&ben, json!([]), "certified")), None)
                .await;
        assert_eq!(s, StatusCode::FORBIDDEN);
        assert!(b["detail"].as_str().unwrap().contains("Org admin"), "{b}");
        // Ben, her steward, doesn't decide the status
        let (s, _, _) =
            send(&app, ben_c, &org, "PUT", "/api/governance/sales", Some(put(&ana, json!([ben]), "deprecated")), None)
                .await;
        assert_eq!(s, StatusCode::FORBIDDEN);
        // only members, known statuses, known models
        for (body, says) in [
            (put("eve@evil.example", json!([]), "draft"), "member"),
            (put(&ana, json!(["eve@evil.example"]), "draft"), "member"),
            (put(&ana, json!([]), "gold"), "status"),
        ] {
            let (s, b, _) = send(&app, admin, &org, "PUT", "/api/governance/sales", Some(body), None).await;
            assert_eq!(s, StatusCode::BAD_REQUEST, "{b}");
            assert!(b["detail"].as_str().unwrap().contains(says), "{b}");
        }
        let (s, _, _) = send(&app, admin, &org, "PUT", "/api/governance/nope", Some(certified), None).await;
        assert!(s.is_client_error());

        // everyone sees it with the model, and where they stand
        let (_, models, _) = send(&app, ben_c, &org, "GET", "/api/models", None, None).await;
        let sales = models["models"].as_array().unwrap().iter().find(|m| m["name"] == "sales").unwrap();
        let g = &sales["governance"];
        assert_eq!(
            (&g["owner"], &g["status"], &g["standing"]),
            (&json!(ana), &json!("certified"), &json!("steward")),
            "{g}"
        );
        assert_eq!(g["note"], json!("checked against finance"));
        let (_, list, _) = send(&app, admin, &org, "GET", "/api/governance", None, None).await;
        assert_eq!(
            (&entry(&list, "sales")["standing"], &entry(&list, "sales")["updated_by"]),
            (&json!("admin"), &json!(ana))
        );

        // another organisation's sales is its own: nothing of A's is in B's rows
        let other = format!("other{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
        assert!(state.store.governance(&other).await.unwrap().is_empty());
        clean_up(&state, &org).await;
    }

    #[tokio::test]
    async fn an_organisations_model_belongs_to_its_maker() {
        let Some((app, state, cookie, org)) = org_with(DASHBOARD).await else { return };
        let c = Some(cookie.as_str());
        let name = format!("gov_{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
        let model = json!({"name": name, "connection": "warehouse", "table": "sample_sales",
                           "dimensions": [{"name": "region"}], "measures": [{"name": "n", "type": "count"}]});
        let (s, b, _) = send(&app, c, &org, "PUT", &format!("/api/org-models/{name}"), Some(model), None).await;
        assert_eq!(s, StatusCode::OK, "{b}");
        let (_, list, _) = send(&app, c, &org, "GET", "/api/governance", None, None).await;
        assert_eq!(entry(&list, &name)["owner"], json!(OWNER));
        // gone with its model
        send(&app, c, &org, "DELETE", &format!("/api/org-models/{name}"), None, None).await;
        let rows = state.store.governance(&org).await.unwrap();
        assert!(!rows.iter().any(|(m, ..)| *m == name));
        clean_up(&state, &org).await;
    }

    /// An organisation with Ana (a viewer) and Ben (a user) as members, Ben owning sales and Ana
    /// its steward; Chloe is a member with no part in it.
    async fn stewarded()
    -> Option<(axum::Router, std::sync::Arc<crate::state::AppState>, String, String, [(String, String); 3])> {
        let (app, state, cookie, org) = org_with(DASHBOARD).await?;
        let people =
            [member(&state, "ana", &org).await, member(&state, "ben", &org).await, member(&state, "chloe", &org).await];
        let users: serde_json::Map<String, Value> =
            people.iter().map(|(e, _)| (e.clone(), json!({"role": "viewer"}))).collect();
        let access = json!({"system_owners": [OWNER], "organisations": {org.clone(): {"name": "G", "models": ["*"],
            "members": {"users": users}}}});
        state.access.pin(crate::access::AccessConfig::from_document(access).unwrap());
        let g = json!({"owner": people[1].0, "stewards": [people[0].0], "status": "certified"});
        let (s, b, _) = send(&app, Some(&cookie), &org, "PUT", "/api/governance/sales", Some(g), None).await;
        assert_eq!(s, StatusCode::OK, "{b}");
        Some((app, state, cookie, org, people))
    }

    #[tokio::test]
    async fn stewards_document_their_models() {
        let Some((app, state, cookie, org, [(_, ana), (_, ben), (_, chloe)])) = stewarded().await else { return };
        let docs = json!({"description": "Every sale at the tills, after returns", "tags": ["finance"],
            "fields": {"revenue": {"description": "After discounts", "synonyms": ["takings"], "examples": ["1,234.50"],
                                   "tags": ["audited"]},
                       "store": {"tags": ["pii"]}}});
        let path = "/api/governance/sales/docs";
        // a member with no part in it: no
        let (s, _, _) = send(&app, Some(&chloe), &org, "PUT", path, Some(docs.clone()), None).await;
        assert_eq!(s, StatusCode::FORBIDDEN);
        // the steward and the owner: yes (a dry run first)
        let (s, b, _) =
            send(&app, Some(&ana), &org, "PUT", &format!("{path}?dry_run=1"), Some(docs.clone()), None).await;
        assert_eq!((s, &b["dry_run"]), (StatusCode::OK, &json!(true)), "{b}");
        let (s, b, _) = send(&app, Some(&ana), &org, "PUT", path, Some(docs.clone()), None).await;
        assert_eq!(s, StatusCode::OK, "{b}");
        let (s, _, _) = send(&app, Some(&ben), &org, "PUT", path, Some(docs.clone()), None).await;
        assert_eq!(s, StatusCode::OK);
        // the catalog has the organisation's own docs, to edit them
        let (_, list, _) = send(&app, Some(&chloe), &org, "GET", "/api/governance", None, None).await;
        let as_docs = |v: &Value| serde_json::from_value::<crate::governance::ModelDocs>(v.clone()).unwrap();
        assert_eq!(as_docs(&entry(&list, "sales")["docs"]), as_docs(&docs));
        // only real fields, known terms, plain tags
        for (bad, says) in [
            (json!({"fields": {"password_hash": {}}}), "no field"),
            (json!({"fields": {"revenue": {"terms": ["net revenue"]}}}), "glossary"),
            (json!({"tags": ["<script>"]}), "tag"),
        ] {
            let (s, b, _) = send(&app, Some(&ana), &org, "PUT", path, Some(bad), None).await;
            assert_eq!(s, StatusCode::BAD_REQUEST, "{b}");
            assert!(b["detail"].as_str().unwrap().contains(says), "{b}");
        }

        // everyone reads it with the model, on top of what the model says itself
        let (_, models, _) = send(&app, Some(&chloe), &org, "GET", "/api/models", None, None).await;
        let sales = models["models"].as_array().unwrap().iter().find(|m| m["name"] == "sales").unwrap();
        assert_eq!(sales["description"], json!("Every sale at the tills, after returns"));
        assert_eq!(sales["tags"], json!(["finance"]));
        let revenue = sales["measures"].as_array().unwrap().iter().find(|m| m["name"] == "revenue").unwrap();
        assert_eq!(revenue["description"], json!("After discounts"));
        assert_eq!(revenue["synonyms"], json!(["sales", "turnover", "takings"]));
        assert_eq!(revenue["tags"], json!(["finance", "audited"]));
        assert_eq!(revenue["examples"], json!(["1,234.50"]));
        let store = sales["dimensions"].as_array().unwrap().iter().find(|d| d["name"] == "store").unwrap();
        assert_eq!(store["tags"], json!(["pii"]));
        // and so do agents
        let call = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "describe_model", "arguments": {"name": "sales"}}});
        let (_, r, _) = send(&app, Some(&cookie), &org, "POST", "/api/mcp", Some(call), None).await;
        assert!(r.to_string().contains("takings"), "{r}");
        clean_up(&state, &org).await;
    }

    #[tokio::test]
    async fn the_glossary_belongs_to_its_makers() {
        let Some((app, state, cookie, org, [(ana_email, ana), (_, ben), (_, chloe)])) = stewarded().await else {
            return;
        };
        let term = json!({"name": "Net revenue", "definition": "Sales after discounts and returns", "synonyms": ["net sales"]});
        let path = "/api/glossary/Net%20revenue";
        // someone who looks after no model doesn't add terms
        let (s, _, _) = send(&app, Some(&chloe), &org, "PUT", path, Some(term.clone()), None).await;
        assert_eq!(s, StatusCode::FORBIDDEN);
        // a steward does, and owns the term
        let (s, b, _) =
            send(&app, Some(&ana), &org, "PUT", &format!("{path}?dry_run=1"), Some(term.clone()), None).await;
        assert_eq!((s, &b["dry_run"]), (StatusCode::OK, &json!(true)), "{b}");
        let (s, b, _) = send(&app, Some(&ana), &org, "PUT", path, Some(term.clone()), None).await;
        assert_eq!(s, StatusCode::OK, "{b}");
        // Ben owns sales, not the term: he can't change it; an Org admin can
        let changed = json!({"name": "Net revenue", "definition": "Gross sales minus discounts and returns"});
        let (s, _, _) = send(&app, Some(&ben), &org, "PUT", path, Some(changed.clone()), None).await;
        assert_eq!(s, StatusCode::FORBIDDEN);
        let (s, _, _) = send(&app, Some(&cookie), &org, "PUT", path, Some(changed), None).await;
        assert_eq!(s, StatusCode::OK);
        // the name in the address is the term's
        let (s, _, _) = send(&app, Some(&ana), &org, "PUT", "/api/glossary/Other", Some(term.clone()), None).await;
        assert_eq!(s, StatusCode::BAD_REQUEST);

        // fields say which terms they implement; the glossary shows them
        let docs = json!({"fields": {"revenue": {"terms": ["net revenue"]}}});
        let (s, b, _) = send(&app, Some(&ana), &org, "PUT", "/api/governance/sales/docs", Some(docs), None).await;
        assert_eq!(s, StatusCode::OK, "{b}");
        let (s, g, _) = send(&app, Some(&chloe), &org, "GET", "/api/glossary", None, None).await;
        assert_eq!(s, StatusCode::OK, "{g}");
        let t = &g["terms"][0];
        assert_eq!(
            (&t["name"], &t["owner"], &t["fields"]),
            (&json!("Net revenue"), &json!(ana_email), &json!(["sales.revenue"])),
            "{g}"
        );
        assert_eq!((&t["may_edit"], &g["may_add"]), (&json!(false), &json!(false)));
        let (_, g, _) = send(&app, Some(&ana), &org, "GET", "/api/glossary", None, None).await;
        assert_eq!((&g["terms"][0]["may_edit"], &g["may_add"]), (&json!(true), &json!(true)));
        // agents read it too
        let call = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "get_glossary", "arguments": {}}});
        let (_, r, _) = send(&app, Some(&cookie), &org, "POST", "/api/mcp", Some(call), None).await;
        assert!(r.to_string().contains("Gross sales minus discounts"), "{r}");

        let (s, _, _) = send(&app, Some(&ben), &org, "DELETE", path, None, None).await;
        assert_eq!(s, StatusCode::FORBIDDEN);
        let (s, _, _) = send(&app, Some(&ana), &org, "DELETE", path, None, None).await;
        assert_eq!(s, StatusCode::OK);
        let (_, g, _) = send(&app, Some(&ana), &org, "GET", "/api/glossary", None, None).await;
        assert_eq!(g["terms"], json!([]));
        clean_up(&state, &org).await;
    }
}
