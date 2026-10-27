//! GET /api/setup: the "Get started" checklist for this person (setup.rs).

use axum::Json;
use axum::extract::State;
use serde_json::{Value, json};

use super::available_models;
use crate::access::Role;
use crate::auth::Auth;
use crate::error::ApiResult;
use crate::setup::{OrgFacts, PlatformFacts, org_steps, platform_steps};
use crate::state::Shared;

/// System owners in the platform workspace: the platform's steps. Org admins (and system owners
/// inside an organisation): that organisation's. Everyone else: nothing to set up.
pub async fn checklist(State(state): State<Shared>, Auth(caller): Auth) -> ApiResult<Json<Value>> {
    caller.require("view")?;
    let org = caller.org().to_string();
    // lite: one person and their data; no organisations, people or sign-in to set up
    if state.config.lite {
        let steps = workspace_steps(&state, &caller, &org).await?;
        let steps: Vec<Value> =
            steps.into_iter().filter(|s| !matches!(s["id"].as_str(), Some("members" | "sign_in"))).collect();
        return Ok(Json(json!({"for": "lite", "steps": steps})));
    }
    if org.is_empty() && caller.role == Some(Role::SystemOwner) {
        let orgs = &caller.access.platform.organisations;
        let databases = state.store.org_databases().await?;
        let with_admin = orgs
            .keys()
            .filter(|o| caller.access.members_of(o).users.values().any(|u| u.role >= Role::OrgAdmin))
            .count();
        let facts = PlatformFacts {
            secret_key: state.config.secret_key.is_some(),
            https: state.config.is_https(),
            organisations: orgs.len(),
            with_database: orgs.keys().filter(|o| databases.contains_key(*o)).count(),
            with_admin,
            delivery: has_delivery(&state.store, "").await?,
        };
        return Ok(Json(json!({"for": "platform", "steps": platform_steps(&facts)})));
    }
    if org.is_empty() || !caller.can("manage_members") {
        return Ok(Json(json!({"for": null, "steps": []})));
    }
    Ok(Json(json!({"for": "organisation", "steps": workspace_steps(&state, &caller, &org).await?})))
}

/// A workspace's steps: an organisation's (or, in lite, the platform workspace's).
async fn workspace_steps(
    state: &crate::state::AppState,
    caller: &crate::auth::Caller,
    org: &str,
) -> ApiResult<Vec<Value>> {
    let org = org.to_string();
    let store = state.orgs.store(&org).await?;
    let connections: Vec<String> = state.connectors.list(&store, &org).await?.into_iter().map(|c| c.name).collect();
    let models = available_models(caller)
        .values()
        .filter(|m| !caller.project.blends.contains_key(&m.name) && connections.contains(&m.connection))
        .count();
    let members = caller.access.members_of(&org);
    let domains = caller.access.platform.organisations.get(&org).is_some_and(|o| !o.domains.is_empty());
    let themes = state.org_themes(&org).await?;
    let facts = OrgFacts {
        connections: connections.len(),
        models,
        dashboards: store.all_dashboards(&org).await?.len(),
        members: members.users.len() + usize::from(domains),
        themed: !themes.themes.is_empty() || themes.default_light.is_some() || themes.default_dark.is_some(),
        sign_in: members.sign_in.is_some(),
        delivery: has_delivery(&store, &org).await?,
    };
    Ok(org_steps(&facts).iter().map(|s| json!(s)).collect())
}

/// The tenant has a mail server or a webhook of its own.
async fn has_delivery(store: &crate::store::Store, org: &str) -> ApiResult<bool> {
    Ok(store.mail_server(org).await?.is_some() || !store.webhooks(org).await?.is_empty())
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use serde_json::{Value, json};

    use crate::api::share::tests::{clean_up, org_with, send};

    fn done(r: &Value) -> Vec<(String, bool)> {
        r["steps"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| (s["id"].as_str().unwrap().to_string(), s["done"] == json!(true)))
            .collect()
    }

    #[tokio::test]
    async fn the_checklist_is_the_platforms_or_the_organisations() {
        let Some((app, state, cookie, org)) = org_with("title: Setup test\ntiles: []\n").await else { return };
        let c = Some(cookie.as_str());
        // the platform workspace: the platform's steps (an organisation exists, with a database)
        let (s, r, _) = send(&app, c, "", "GET", "/api/setup", None, None).await;
        assert_eq!(s, StatusCode::OK, "{r}");
        assert_eq!(r["for"], json!("platform"));
        let d = done(&r);
        assert!(d.contains(&("secret_key".into(), true)) && d.contains(&("organisation".into(), true)), "{d:?}");

        // inside the organisation: its own (a connection and a dashboard, models on it, no team yet)
        let (_, r, _) = send(&app, c, &org, "GET", "/api/setup", None, None).await;
        assert_eq!(r["for"], json!("organisation"));
        let d = done(&r);
        for (id, is_done) in [("connection", true), ("model", true), ("dashboard", true), ("members", false)] {
            assert!(d.contains(&(id.into(), is_done)), "{id}: {d:?}");
        }
        // nothing to set up for others
        let (_, r, _) = send(&app, c, &org, "GET", "/api/setup", None, Some("power_user")).await;
        assert_eq!(r["steps"], json!([]));
        clean_up(&state, &org).await;
    }
}
