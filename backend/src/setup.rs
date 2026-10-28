//! "Get started": what's left to set up, for the platform's owners and for an organisation's admins.
//!
//! Each step is worked out from what is actually set up (no "mark as done"), says where in the
//! app to do it, and which part of the documentation explains it.

use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct Step {
    pub id: &'static str,
    pub title: &'static str,
    pub detail: String,
    pub done: bool,
    /// nice to have, not needed to get going
    pub optional: bool,
    /// where in the app
    pub link: &'static str,
    /// the website's documentation, relative to it (page#section)
    pub guide: &'static str,
}

fn step(
    id: &'static str,
    title: &'static str,
    done: bool,
    detail: String,
    link: &'static str,
    guide: &'static str,
) -> Step {
    Step { id, title, detail, done, optional: false, link, guide }
}

/// The sample data (crate::demo): something to look around in before connecting one's own. Its
/// system owners set it up in the platform workspace (in lite: the one workspace).
pub fn demo_step(done: bool) -> Step {
    Step {
        optional: true,
        ..step(
            "demo",
            "Try it with sample data",
            done,
            "New York taxi trips with the weather in each borough: two dashboards and a blend, set up for you \
             (about 60 MB a month of data, downloaded)."
                .into(),
            "/dashboards/nyc_taxi_weather",
            "docs.html#demo",
        )
    }
}

/// What the platform has, for its owners' checklist.
pub struct PlatformFacts {
    pub secret_key: bool,
    pub https: bool,
    pub organisations: usize,
    /// organisations with their own ScramDB
    pub with_database: usize,
    /// organisations with at least one Org admin
    pub with_admin: usize,
    /// the platform workspace's own mail server or webhooks
    pub delivery: bool,
}

pub fn platform_steps(f: &PlatformFacts) -> Vec<Step> {
    vec![
        step(
            "secret_key",
            "Keep the secret key safe",
            f.secret_key,
            if f.secret_key {
                "It encrypts organisations' database addresses and connection secrets: keep a copy somewhere safe."
                    .into()
            } else {
                "Set MAGNUM_SECRET_KEY (or MAGNUM_SECRET_KEY_FILE): organisations' databases and connections need it."
                    .into()
            },
            "/organisations",
            "guides/infrastructure.html#secret-key",
        ),
        Step {
            optional: true,
            ..step(
                "https",
                "Serve it over https",
                f.https,
                "For people other than you: an https address in MAGNUM_PUBLIC_URL (cookies, sign-in, passkeys).".into(),
                "/organisations",
                "guides/infrastructure.html#https",
            )
        },
        step(
            "organisation",
            "Create an organisation",
            f.organisations > 0,
            "Each organisation has its own members, data, dashboards and colours.".into(),
            "/organisations",
            "guides/platform-owner.html#organisations",
        ),
        step(
            "org_database",
            "Give an organisation its own database",
            f.with_database > 0,
            format!(
                "{} of {} organisations have their own ScramDB, where their dashboards and data settings live.",
                f.with_database, f.organisations
            ),
            "/organisations",
            "guides/platform-owner.html#databases",
        ),
        step(
            "org_admin",
            "Name each organisation's admin",
            f.with_admin > 0,
            "Org admins connect their data, make models and invite their team.".into(),
            "/organisations",
            "guides/platform-owner.html#org-admins",
        ),
        Step {
            optional: true,
            ..step(
                "delivery",
                "Email and webhooks for the platform workspace",
                f.delivery,
                "The platform workspace's own mail server or Slack/Teams webhooks. Organisations set up theirs.".into(),
                "/delivery",
                "guides/platform-owner.html#delivery",
            )
        },
    ]
}

/// What an organisation has, for its admins' checklist.
pub struct OrgFacts {
    pub connections: usize,
    /// models it can query (its own, and the platform's on its connections)
    pub models: usize,
    pub dashboards: usize,
    /// listed members (people joining by domain count as one more)
    pub members: usize,
    pub themed: bool,
    /// its own sign-in rules or single sign-on
    pub sign_in: bool,
    /// its own mail server or webhooks
    pub delivery: bool,
    /// an AI agent has used it over MCP (usage: mcp_tool)
    pub agent: bool,
}

pub fn org_steps(f: &OrgFacts) -> Vec<Step> {
    let optional = |s: Step| Step { optional: true, ..s };
    vec![
        step(
            "connection",
            "Connect your data",
            f.connections > 0,
            "A database or files (PostgreSQL, BigQuery, ScramDB, Parquet/CSV with DuckDB), read-only.".into(),
            "/connections",
            "guides/org-admin.html#connect",
        ),
        step(
            "model",
            "Describe your data",
            f.models > 0,
            "A model says which table, what people group by and what they add up: Models › New model proposes one."
                .into(),
            "/models",
            "guides/org-admin.html#models",
        ),
        step(
            "dashboard",
            "Make a first dashboard",
            f.dashboards > 0,
            "From a model in one click (Models › Starter dashboard), or chart by chart in Explore.".into(),
            "/models",
            "guides/org-admin.html#dashboards",
        ),
        step(
            "members",
            "Invite your team",
            f.members > 1,
            "Add people with a role (and data restrictions if needed), or let your email domain in.".into(),
            "/members",
            "guides/org-admin.html#members",
        ),
        optional(step(
            "theme",
            "Use your colours",
            f.themed,
            "A theme in your company's colours, for everyone in the organisation.".into(),
            "/themes",
            "guides/org-admin.html#themes",
        )),
        optional(step(
            "sign_in",
            "Sign-in rules and single sign-on",
            f.sign_in,
            "Require authenticator codes or passkeys, or sign in with your company's provider.".into(),
            "/members",
            "guides/org-admin.html#sign-in",
        )),
        optional(step(
            "delivery",
            "Email and webhooks",
            f.delivery,
            "Your own mail server, or Slack and Teams webhooks, for reports and alerts.".into(),
            "/delivery",
            "guides/org-admin.html#delivery",
        )),
        optional(step(
            "agent",
            "Connect your AI agent",
            f.agent,
            "Claude, Codex or Cursor explore your data and build dashboards for you, through MCP: pick yours for \
             the settings to paste."
                .into(),
            "/security",
            "docs.html#agents",
        )),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(steps: &[Step]) -> Vec<(&str, bool)> {
        steps.iter().map(|s| (s.id, s.done)).collect()
    }

    #[test]
    fn the_sample_data_is_optional_and_done_once_its_dashboard_is_there() {
        let s = demo_step(false);
        assert_eq!((s.id, s.done, s.optional), ("demo", false, true));
        assert!(s.detail.contains("taxi") && s.detail.contains("weather"), "{}", s.detail);
        assert!(demo_step(true).done);
    }

    #[test]
    fn the_platforms_steps_follow_what_is_set_up() {
        let fresh = PlatformFacts {
            secret_key: false,
            https: false,
            organisations: 0,
            with_database: 0,
            with_admin: 0,
            delivery: false,
        };
        assert_eq!(
            ids(&platform_steps(&fresh)),
            [
                ("secret_key", false),
                ("https", false),
                ("organisation", false),
                ("org_database", false),
                ("org_admin", false),
                ("delivery", false)
            ]
        );
        let ready = PlatformFacts {
            secret_key: true,
            https: true,
            organisations: 2,
            with_database: 1,
            with_admin: 2,
            delivery: true,
        };
        assert!(platform_steps(&ready).iter().all(|s| s.done));
        assert!(platform_steps(&fresh).iter().all(|s| s.link.starts_with('/') && s.guide.contains('#')));
    }

    #[test]
    fn an_organisations_steps_follow_what_it_has() {
        let fresh = OrgFacts {
            connections: 0,
            models: 0,
            dashboards: 0,
            members: 1,
            themed: false,
            sign_in: false,
            delivery: false,
            agent: false,
        };
        let steps = org_steps(&fresh);
        assert_eq!(
            ids(&steps),
            [
                ("connection", false),
                ("model", false),
                ("dashboard", false),
                ("members", false),
                ("theme", false),
                ("sign_in", false),
                ("delivery", false),
                ("agent", false)
            ]
        );
        assert_eq!(
            steps.iter().filter(|s| s.optional).map(|s| s.id).collect::<Vec<_>>(),
            ["theme", "sign_in", "delivery", "agent"]
        );
        let busy = OrgFacts {
            connections: 1,
            models: 3,
            dashboards: 2,
            members: 4,
            themed: true,
            sign_in: true,
            delivery: true,
            agent: true,
        };
        assert!(org_steps(&busy).iter().all(|s| s.done));
    }
}
