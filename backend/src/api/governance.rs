//! Who answers for each model, and its status (governance.rs): read by everyone, changed by the
//! model's owner or an Org admin, in the organisation's own database.

use std::collections::BTreeMap;

use axum::Json;
use axum::extract::{Path, Query as UrlQuery, State};
use serde_json::{Value, json};

use super::Body;
use crate::auth::{Auth, Caller};
use crate::error::{ApiError, ApiResult};
use crate::governance::{Governance, standing};
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
    let out: Vec<Value> = super::available_models(&caller)
        .keys()
        .map(|name| {
            let row = saved.get(name);
            let mut v = shown(&caller, row.map(|(g, ..)| g));
            v["model"] = json!(name);
            v["updated_at"] = json!(row.map(|(_, at, _)| at));
            v["updated_by"] = json!(row.and_then(|(_, _, by)| by.clone()));
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
}
