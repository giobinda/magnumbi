//! Roles, organisations and permissions.
//!
//! Two levels of authority:
//!
//! * **System owner**: runs the platform. Owns the *platform* settings: who the system owners
//!   are, and every organisation's definition (name, email domains, which models it may use,
//!   organisation-wide row filters). Sees every organisation.
//! * **Organisation**: an independent company using the platform. Its members, their roles and
//!   their personal row filters are managed by its Org admins (the *members* settings).
//!   Organisations never see each other's members, dashboards, models or data.
//!
//! | role         | can                                                                     |
//! |--------------|-------------------------------------------------------------------------|
//! | viewer       | view dashboards, use filters and the period picker                      |
//! | user         | + drill down, share (embed code)                                        |
//! | power_user   | + create / edit / delete dashboards, Explore, see SQL and YAML          |
//! | org_admin    | + manage the organisation's members, roles and personal filters         |
//! | system_owner | everything, in every organisation                                       |
//!
//! A person belongs to exactly one organisation: listed by email in its members, or else by
//! email domain with the organisation's default role. Signed in but in no organisation: no
//! access. Row filters (organisation-wide, then personal) only ever narrow what someone sees,
//! also cover every model reading the same data, and fail closed when misconfigured.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};

use crate::project::ProjectState;
use crate::schema::Filter;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Viewer,
    User,
    PowerUser,
    OrgAdmin,
    SystemOwner,
}

pub const ROLES: [Role; 5] = [Role::Viewer, Role::User, Role::PowerUser, Role::OrgAdmin, Role::SystemOwner];

impl Role {
    pub fn parse(s: &str) -> Option<Role> {
        serde_json::from_value(Value::String(s.trim().to_lowercase())).ok()
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Role::Viewer => "viewer",
            Role::User => "user",
            Role::PowerUser => "power_user",
            Role::OrgAdmin => "org_admin",
            Role::SystemOwner => "system_owner",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Role::Viewer => "Viewer",
            Role::User => "User",
            Role::PowerUser => "Power user",
            Role::OrgAdmin => "Org admin",
            Role::SystemOwner => "System owner",
        }
    }

    pub fn permissions(self) -> Vec<&'static str> {
        let mut p = vec!["view", "filter"];
        if self >= Role::User {
            p.extend(["drill", "share"]);
        }
        if self >= Role::PowerUser {
            p.extend(["explore", "create", "edit", "view_sql"]);
        }
        if self >= Role::OrgAdmin {
            p.push("manage_members");
        }
        if self >= Role::SystemOwner {
            p.extend(["manage_orgs", "manage_system"]);
        }
        p.sort_unstable();
        p
    }

    pub fn can(self, perm: &str) -> bool {
        self.permissions().contains(&perm)
    }
}

pub static ORG_ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-z0-9][a-z0-9_-]{0,62}$").unwrap());

/// Only rows of `model` whose `dimension` is one of `values` (an empty list: no rows).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RowRule {
    pub model: String,
    pub dimension: String,
    #[serde(default)]
    pub values: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UserAccess {
    #[serde(default = "viewer", deserialize_with = "member_role")]
    pub role: Role,
    #[serde(default)]
    pub filters: Vec<RowRule>,
}

fn viewer() -> Role {
    Role::Viewer
}

fn member_role<'de, D: Deserializer<'de>>(d: D) -> Result<Role, D::Error> {
    let r = Role::deserialize(d)?;
    if r == Role::SystemOwner {
        return Err(serde::de::Error::custom("system owners are set on the platform, not as organisation members"));
    }
    Ok(r)
}

fn domain_role<'de, D: Deserializer<'de>>(d: D) -> Result<Role, D::Error> {
    let r = Role::deserialize(d)?;
    if r > Role::PowerUser {
        return Err(serde::de::Error::custom("joining by email domain never makes anyone an admin"));
    }
    Ok(r)
}

/// "email: role" or "email: {role, filters}"; emails lowercased.
fn people<'de, D: Deserializer<'de>>(d: D) -> Result<BTreeMap<String, UserAccess>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Entry {
        Role(String),
        Access(UserAccess),
    }
    let raw: BTreeMap<String, Entry> = BTreeMap::deserialize(d)?;
    let mut out = BTreeMap::new();
    for (k, v) in raw {
        let k = k.trim().to_lowercase();
        if k.is_empty() {
            continue;
        }
        let access = match v {
            Entry::Access(a) => a,
            Entry::Role(r) => {
                let role = Role::parse(&r).filter(|r| *r != Role::SystemOwner);
                UserAccess {
                    role: role.ok_or_else(|| serde::de::Error::custom(format!("{k}: unknown role {r:?}")))?,
                    filters: vec![],
                }
            }
        };
        out.insert(k, access);
    }
    Ok(out)
}

/// An organisation's people: managed by its Org admins (and system owners).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Members {
    /// for people who join by email domain
    #[serde(default = "viewer", deserialize_with = "domain_role")]
    pub default_role: Role,
    #[serde(default)]
    pub default_filters: Vec<RowRule>,
    #[serde(default, deserialize_with = "people")]
    pub users: BTreeMap<String, UserAccess>,
}

impl Default for Members {
    fn default() -> Self {
        Members { default_role: Role::Viewer, default_filters: vec![], users: BTreeMap::new() }
    }
}

fn lower_set<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    let v: Vec<String> = Vec::deserialize(d)?;
    let set: BTreeSet<String> =
        v.iter().map(|s| s.trim().trim_start_matches('@').to_lowercase()).filter(|s| !s.is_empty()).collect();
    Ok(set.into_iter().collect())
}

/// An organisation's definition: system owners only.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Organisation {
    pub name: String,
    #[serde(default, deserialize_with = "lower_set")]
    pub domains: Vec<String>,
    /// ["*"] = every model
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub filters: Vec<RowRule>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Platform {
    #[serde(default, deserialize_with = "lower_set")]
    pub system_owners: Vec<String>,
    #[serde(default)]
    pub organisations: BTreeMap<String, Organisation>,
}

impl Platform {
    pub fn validate(&self) -> Result<(), String> {
        let mut seen: BTreeMap<&str, &str> = BTreeMap::new();
        for (oid, org) in &self.organisations {
            if !ORG_ID.is_match(oid) {
                return Err(format!("organisation id {oid:?}: lowercase letters, digits, - and _ only"));
            }
            for d in &org.domains {
                if let Some(other) = seen.insert(d, oid) {
                    return Err(format!("domain {d:?} belongs to both {other:?} and {oid:?}"));
                }
            }
        }
        Ok(())
    }
}

/// Everything, as loaded: the platform part plus each organisation's members.
#[derive(Debug, Clone, Default)]
pub struct AccessConfig {
    pub platform: Platform,
    pub members: BTreeMap<String, Members>,
    /// set when the settings couldn't be read: nobody gets data until fixed
    pub broken: bool,
}

impl AccessConfig {
    pub fn members_of(&self, org: &str) -> Members {
        self.members.get(org).cloned().unwrap_or_default()
    }

    /// The access.yaml shape: organisations with their members inline.
    pub fn from_document(raw: Value) -> Result<AccessConfig, String> {
        let mut raw = match raw {
            Value::Object(m) => m,
            Value::Null => serde_json::Map::new(),
            _ => return Err("access settings: expected a mapping".into()),
        };
        let mut members = BTreeMap::new();
        if let Some(Value::Object(orgs)) = raw.get_mut("organisations") {
            for (oid, org) in orgs.iter_mut() {
                if let Value::Object(o) = org {
                    let m = o.remove("members").unwrap_or(Value::Null);
                    let m: Members = if m.is_null() {
                        Members::default()
                    } else {
                        serde_json::from_value(m).map_err(|e| format!("organisations.{oid}.members: {e}"))?
                    };
                    members.insert(oid.clone(), m);
                }
            }
        }
        let platform: Platform = serde_json::from_value(Value::Object(raw)).map_err(|e| e.to_string())?;
        platform.validate()?;
        Ok(AccessConfig { platform, members, broken: false })
    }
}

// --------------------------------------------------------------------------- scope

/// Who is asking, as what, inside which organisation. `role` None: not a member anywhere.
#[derive(Debug, Clone, Default)]
pub struct Scope {
    pub role: Option<Role>,
    /// None: the platform workspace (system owners) or no organisation
    pub org: Option<String>,
    pub org_filters: Vec<RowRule>,
    pub personal_filters: Vec<RowRule>,
    /// None: every model
    pub models: Option<BTreeSet<String>>,
    pub broken: bool,
}

impl Scope {
    pub fn none() -> Scope {
        Scope::default()
    }

    pub fn filters(&self) -> impl Iterator<Item = &RowRule> {
        self.org_filters.iter().chain(self.personal_filters.iter())
    }
}

fn org_scope(cfg: &AccessConfig, role: Role, oid: &str, personal: Vec<RowRule>) -> Scope {
    let org = &cfg.platform.organisations[oid];
    let models = if org.models.iter().any(|m| m == "*") { None } else { Some(org.models.iter().cloned().collect()) };
    Scope {
        role: Some(role),
        org: Some(oid.into()),
        org_filters: org.filters.clone(),
        personal_filters: personal,
        models,
        broken: false,
    }
}

/// (organisation, role, personal filters) for a signed-in person, or None.
pub fn membership(cfg: &AccessConfig, email: &str) -> Option<(String, Role, Vec<RowRule>)> {
    let email = email.to_lowercase();
    for oid in cfg.platform.organisations.keys() {
        // explicit membership first
        if let Some(ua) = cfg.members.get(oid).and_then(|m| m.users.get(&email)) {
            return Some((oid.clone(), ua.role, ua.filters.clone()));
        }
    }
    let domain = email.rsplit_once('@').map(|(_, d)| d)?;
    for (oid, org) in &cfg.platform.organisations {
        if org.domains.iter().any(|d| d == domain) {
            let m = cfg.members_of(oid);
            return Some((oid.clone(), m.default_role, m.default_filters));
        }
    }
    None
}

pub fn is_system_owner(cfg: &AccessConfig, email: &str) -> bool {
    cfg.platform.system_owners.iter().any(|o| o.eq_ignore_ascii_case(email))
}

/// The scope of a signed-in person. System owners pick the organisation with the org header
/// (none: the platform workspace, unrestricted).
pub fn scope_of(cfg: &AccessConfig, email: &str, org_choice: Option<&str>) -> Scope {
    if cfg.broken {
        return Scope { role: Some(Role::Viewer), broken: true, ..Scope::default() };
    }
    if is_system_owner(cfg, email) {
        return match org_choice {
            Some(oid) if cfg.platform.organisations.contains_key(oid) => org_scope(cfg, Role::SystemOwner, oid, vec![]),
            _ => Scope { role: Some(Role::SystemOwner), ..Scope::default() },
        };
    }
    match membership(cfg, email) {
        Some((oid, role, personal)) => org_scope(cfg, role, &oid, personal),
        None => Scope::none(),
    }
}

/// Anyone may preview Magnum BI as a lower role ("view as"); never a higher one.
pub fn effective_role(actual: Option<Role>, view_as: Option<&str>) -> Option<Role> {
    let actual = actual?;
    match view_as.and_then(Role::parse) {
        Some(w) if w < actual => Some(w),
        _ => Some(actual),
    }
}

pub fn model_available(scope: &Scope, project: &ProjectState, name: &str) -> bool {
    scope.role.is_some() && project.models.contains_key(name) && scope.models.as_ref().is_none_or(|m| m.contains(name))
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct AccessDenied(pub String);

/// Mandatory filters for a query on `model_name` (empty when nothing applies). Fails closed.
pub fn row_filters_for(scope: &Scope, project: &ProjectState, model_name: &str) -> Result<Vec<Filter>, AccessDenied> {
    if scope.broken {
        return Err(AccessDenied("the access settings are broken; a system owner must fix them".into()));
    }
    if scope.role.is_none() {
        return Err(AccessDenied("you're not a member of any organisation".into()));
    }
    if !model_available(scope, project, model_name) {
        return Err(AccessDenied(format!("model {model_name:?} isn't available")));
    }
    let model = &project.models[model_name];
    let mut out = vec![];
    for rule in scope.filters() {
        if !same_data(project, &rule.model, model_name) {
            continue; // filters only apply to models reading the filtered data
        }
        let Some(dim) = model.dimension(&rule.dimension) else {
            return Err(AccessDenied(format!(
                "an access rule uses {:?}, which {model_name:?} doesn't have; ask your administrator",
                rule.dimension
            )));
        };
        if rule.values.is_empty() {
            return Err(AccessDenied(format!(
                "you don't have access to any {} in {}",
                dim.label.as_deref().unwrap_or(&dim.name),
                model.label.as_deref().unwrap_or(&model.name)
            )));
        }
        out.push(Filter::equals(&rule.dimension, rule.values.clone()));
    }
    Ok(out)
}

fn root<'a>(project: &'a ProjectState, mut name: &'a str) -> &'a str {
    let mut seen = BTreeSet::new();
    while let Some(base) = project.bases.get(name) {
        if !seen.insert(name) {
            break;
        }
        name = base;
    }
    name
}

fn source(project: &ProjectState, name: &str) -> Option<(String, String)> {
    let m = project.models.get(name)?;
    let body = match (&m.table, &m.sql) {
        (Some(t), _) => t.clone(),
        (None, Some(s)) => s.split_whitespace().collect::<Vec<_>>().join(" "),
        _ => return None,
    };
    let body = body.to_lowercase().trim_matches(|c| c == '`' || c == '"' || c == ' ').to_string();
    (!body.is_empty()).then(|| (m.connection.clone(), body))
}

/// Does a rule on `rule_model` cover queries on `model_name`? Same model, same extends family,
/// or the same connection + table/SQL. Case-insensitive; an unknown rule model covers nothing.
pub fn same_data(project: &ProjectState, rule_model: &str, model_name: &str) -> bool {
    let (a, b) = (rule_model.to_lowercase(), model_name.to_lowercase());
    if a == b {
        return true;
    }
    let find = |x: &str| project.models.keys().find(|n| n.to_lowercase() == x).cloned();
    let (Some(ra), Some(rb)) = (find(&a), find(&b)) else { return false };
    if root(project, &ra) == root(project, &rb) {
        return true;
    }
    matches!((source(project, &ra), source(project, &rb)), (Some(x), Some(y)) if x == y)
}

pub fn describe_restrictions(scope: &Scope, project: &ProjectState) -> Vec<Value> {
    let mut out = vec![];
    for (src, rules) in [("organisation", &scope.org_filters), ("personal", &scope.personal_filters)] {
        for rule in rules {
            let m = project.models.get(&rule.model);
            let d = m.and_then(|m| m.dimension(&rule.dimension));
            out.push(json!({
                "model": rule.model,
                "model_label": m.and_then(|m| m.label.clone()).unwrap_or_else(|| rule.model.clone()),
                "dimension": rule.dimension,
                "label": d.and_then(|d| d.label.clone()).unwrap_or_else(|| rule.dimension.clone()),
                "values": rule.values,
                "source": src,
            }));
        }
    }
    out
}

/// Mistakes worth showing to admins (queries hitting them fail closed anyway).
pub fn lint(access: &AccessConfig, project: &ProjectState) -> BTreeMap<String, String> {
    let mut errors = BTreeMap::new();
    let check = |errors: &mut BTreeMap<String, String>, at: String, r: &RowRule| match project
        .models
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(&r.model))
    {
        None => {
            errors.insert(at.clone(), format!("{at}: unknown model {:?}", r.model));
        }
        Some((_, m)) if m.dimension(&r.dimension).is_none() => {
            errors.insert(at.clone(), format!("{at}: model {:?} has no dimension {:?}", r.model, r.dimension));
        }
        _ => {}
    };
    for (oid, org) in &access.platform.organisations {
        for r in &org.filters {
            check(&mut errors, format!("organisations/{oid}"), r);
        }
        for m in &org.models {
            if m != "*" && !project.models.contains_key(m) {
                errors
                    .insert(format!("organisations/{oid}#models"), format!("organisations/{oid}: unknown model {m:?}"));
            }
        }
        // a model with no organisation filter shows this organisation every row: almost always a
        // mistake on a platform shared by several companies
        let all = org.models.iter().any(|m| m == "*");
        for n in project.models.keys().filter(|n| all || org.models.contains(n)) {
            if !org.filters.iter().any(|r| same_data(project, &r.model, n)) {
                errors.insert(
                    format!("organisations/{oid}#unfiltered:{n}"),
                    format!("organisations/{oid}: model {n:?} has no organisation filter, so its members see all of its rows"),
                );
            }
        }
        let mem = access.members_of(oid);
        for r in &mem.default_filters {
            check(&mut errors, format!("organisations/{oid}/members/default"), r);
        }
        for (who, ua) in &mem.users {
            for r in &ua.filters {
                check(&mut errors, format!("organisations/{oid}/members/{who}"), r);
            }
        }
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> AccessConfig {
        AccessConfig::from_document(serde_json::from_str(r#"{
            "system_owners": ["Owner@Platform.example"],
            "organisations": {
                "acme": {"name": "ACME", "domains": ["@acme.example"], "models": ["sales"],
                         "filters": [{"model": "sales", "dimension": "region", "values": ["EMEA"]}],
                         "members": {"default_role": "user", "users": {
                             "Boss@acme.example": "org_admin",
                             "berlin@acme.example": {"role": "viewer", "filters": [{"model": "sales", "dimension": "store", "values": ["Store 01"]}]}}}},
                "globex": {"name": "Globex", "domains": ["globex.example"], "models": ["*"]}
            }}"#).unwrap()).unwrap()
    }

    #[test]
    fn roles_only_grow() {
        assert!(Role::Viewer.can("view") && !Role::Viewer.can("drill"));
        assert!(Role::PowerUser.can("explore") && !Role::PowerUser.can("manage_members"));
        assert!(Role::SystemOwner.can("manage_orgs"));
        assert_eq!(effective_role(Some(Role::OrgAdmin), Some("viewer")), Some(Role::Viewer));
        assert_eq!(effective_role(Some(Role::Viewer), Some("system_owner")), Some(Role::Viewer));
        assert_eq!(effective_role(None, Some("viewer")), None);
    }

    #[test]
    fn membership_explicit_then_domain() {
        let c = cfg();
        assert_eq!(membership(&c, "boss@acme.example").unwrap().1, Role::OrgAdmin);
        assert_eq!(membership(&c, "someone@acme.example").unwrap().1, Role::User);
        assert_eq!(membership(&c, "x@globex.example").unwrap().0, "globex");
        assert!(membership(&c, "x@elsewhere.example").is_none());
        let s = scope_of(&c, "berlin@acme.example", Some("globex"));
        assert_eq!((s.role, s.org.as_deref()), (Some(Role::Viewer), Some("acme")), "members can't switch organisation");
        assert_eq!(s.filters().count(), 2);
    }

    #[test]
    fn system_owners_switch_organisation() {
        let c = cfg();
        let s = scope_of(&c, "owner@platform.example", None);
        assert_eq!((s.role, s.org.as_deref()), (Some(Role::SystemOwner), None));
        let s = scope_of(&c, "owner@platform.example", Some("acme"));
        assert_eq!(s.org.as_deref(), Some("acme"));
        assert_eq!(s.org_filters.len(), 1);
    }

    #[test]
    fn invalid_settings_are_refused() {
        let dup = r#"{"organisations": {"a": {"name": "A", "domains": ["x.example"]}, "b": {"name": "B", "domains": ["x.example"]}}}"#;
        assert!(AccessConfig::from_document(serde_json::from_str(dup).unwrap()).is_err());
        let admin_by_domain = r#"{"organisations": {"a": {"name": "A", "members": {"default_role": "org_admin"}}}}"#;
        assert!(AccessConfig::from_document(serde_json::from_str(admin_by_domain).unwrap()).is_err());
        let owner_member =
            r#"{"organisations": {"a": {"name": "A", "members": {"users": {"x@y.example": "system_owner"}}}}}"#;
        assert!(AccessConfig::from_document(serde_json::from_str(owner_member).unwrap()).is_err());
        let bad_id = r#"{"organisations": {"Bad Id": {"name": "A"}}}"#;
        assert!(AccessConfig::from_document(serde_json::from_str(bad_id).unwrap()).is_err());
    }

    #[test]
    fn broken_settings_fail_closed() {
        let c = AccessConfig { broken: true, ..AccessConfig::default() };
        let s = scope_of(&c, "anyone@x.example", None);
        assert!(s.broken);
    }
}
