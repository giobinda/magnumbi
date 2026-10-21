//! Data governance: who answers for a model, and how far it can be trusted.
//!
//! Each model has, per organisation, an owner and stewards, and a status (certified, draft or
//! deprecated) with a note. Permissions nest: system owners and Org admins (in their
//! organisation) above the owner, the owner above the stewards, everyone else reads. Ownership
//! isn't a role: any member may own a model. Saved in each organisation's own database
//! (api/governance.rs).

use serde::{Deserialize, Serialize};

pub const STATUSES: [&str; 3] = ["certified", "draft", "deprecated"];
pub const MAX_STEWARDS: usize = 20;
pub const MAX_NOTE: usize = 500;

/// A model's governance in one organisation (no row: nobody owns it, no status).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Governance {
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub stewards: Vec<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

/// Where someone stands on a model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Standing {
    Reader,
    Steward,
    Owner,
    /// an Org admin in their organisation, or a system owner
    Admin,
}

impl Standing {
    pub fn as_str(self) -> &'static str {
        match self {
            Standing::Reader => "reader",
            Standing::Steward => "steward",
            Standing::Owner => "owner",
            Standing::Admin => "admin",
        }
    }
}

/// `admin`: the person administers the organisation (or the platform).
pub fn standing(g: &Governance, email: &str, admin: bool) -> Standing {
    let same = |x: &str| x.eq_ignore_ascii_case(email);
    if admin {
        Standing::Admin
    } else if g.owner.as_deref().is_some_and(same) {
        Standing::Owner
    } else if g.stewards.iter().any(|s| same(s)) {
        Standing::Steward
    } else {
        Standing::Reader
    }
}

impl Governance {
    /// The values themselves; `members`: who may own or steward a model here.
    pub fn check(&self, members: &[String]) -> Result<(), String> {
        let member = |e: &str| members.iter().any(|m| m.eq_ignore_ascii_case(e));
        if let Some(o) = &self.owner
            && !member(o)
        {
            return Err(format!("{o} isn't a member of this organisation: only members own models"));
        }
        if self.stewards.len() > MAX_STEWARDS {
            return Err(format!("at most {MAX_STEWARDS} stewards"));
        }
        for (i, s) in self.stewards.iter().enumerate() {
            if !member(s) {
                return Err(format!("{s} isn't a member of this organisation: only members are stewards"));
            }
            if self.stewards[..i].iter().any(|x| x.eq_ignore_ascii_case(s)) {
                return Err(format!("{s} is listed twice"));
            }
            if self.owner.as_deref().is_some_and(|o| o.eq_ignore_ascii_case(s)) {
                return Err(format!("{s} owns the model already"));
            }
        }
        if let Some(st) = &self.status
            && !STATUSES.contains(&st.as_str())
        {
            return Err(format!("unknown status {st:?}: use certified, draft or deprecated"));
        }
        if self.note.as_ref().is_some_and(|n| n.chars().count() > MAX_NOTE) {
            return Err(format!("the note is too long (at most {MAX_NOTE} characters)"));
        }
        Ok(())
    }

    /// Whether someone at `standing` may turn `self` into `next`.
    pub fn may_become(&self, next: &Governance, standing: Standing) -> Result<(), String> {
        match standing {
            Standing::Admin => Ok(()),
            Standing::Owner if !same_email(&self.owner, &next.owner) => {
                Err("only an Org admin gives a model a new owner".into())
            }
            Standing::Owner => Ok(()),
            Standing::Steward => Err("stewards look after a model's docs; its owner decides the rest".into()),
            Standing::Reader => Err("only the model's owner or an Org admin can change this".into()),
        }
    }
}

/// The longest description or definition, and the most synonyms, examples or tags on one thing.
pub const MAX_TEXT: usize = 2000;
pub const MAX_LIST: usize = 20;
/// The longest synonym, example or term name.
const MAX_ITEM: usize = 80;
static TAG: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"^[a-z0-9][a-z0-9_-]{0,39}$").unwrap());

/// What an organisation says about a model and its fields, on top of what the model says itself
/// (stewards and above write it). Shown as text, never as HTML.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModelDocs {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub fields: std::collections::BTreeMap<String, FieldDocs>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FieldDocs {
    #[serde(default)]
    pub description: Option<String>,
    /// other words people use for it ("turnover" for revenue): found by search, given to agents
    #[serde(default)]
    pub synonyms: Vec<String>,
    #[serde(default)]
    pub examples: Vec<String>,
    /// classification (pii, confidential, finance...): information only, no access rule
    #[serde(default)]
    pub tags: Vec<String>,
    /// glossary terms this field implements
    #[serde(default)]
    pub terms: Vec<String>,
}

/// A business term in the organisation's glossary.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Term {
    pub name: String,
    pub definition: String,
    #[serde(default)]
    pub synonyms: Vec<String>,
}

fn text(what: &str, t: &Option<String>) -> Result<(), String> {
    match t {
        Some(t) if t.chars().count() > MAX_TEXT => Err(format!("{what} is too long (at most {MAX_TEXT} characters)")),
        _ => Ok(()),
    }
}

fn items(what: &str, list: &[String], max_len: usize) -> Result<(), String> {
    if list.len() > MAX_LIST {
        return Err(format!("{what}: at most {MAX_LIST}"));
    }
    for i in list {
        if i.trim().is_empty() {
            return Err(format!("{what}: an empty one"));
        }
        if i.chars().count() > max_len {
            return Err(format!("{what}: {i:?} is too long (at most {max_len} characters)"));
        }
    }
    Ok(())
}

pub fn check_synonyms(what: &str, list: &[String]) -> Result<(), String> {
    items(what, list, MAX_ITEM)
}

pub fn check_tags(what: &str, list: &[String]) -> Result<(), String> {
    tags(what, list)
}

fn tags(what: &str, list: &[String]) -> Result<(), String> {
    items(what, list, 40)?;
    match list.iter().find(|t| !TAG.is_match(t)) {
        Some(t) => Err(format!("{what}: {t:?} isn't a tag (small letters, digits, - and _, such as pii)")),
        None => Ok(()),
    }
}

impl ModelDocs {
    /// `fields`: the model's fields; `terms`: the glossary's terms.
    pub fn check(&self, fields: &[&str], terms: &[String]) -> Result<(), String> {
        text("the description", &self.description)?;
        tags("the model's tags", &self.tags)?;
        for (name, f) in &self.fields {
            if !fields.contains(&name.as_str()) {
                return Err(format!("the model has no field {name:?}"));
            }
            text(&format!("{name}'s description"), &f.description)?;
            items(&format!("{name}'s synonyms"), &f.synonyms, MAX_ITEM)?;
            items(&format!("{name}'s examples"), &f.examples, MAX_ITEM * 3)?;
            tags(&format!("{name}'s tags"), &f.tags)?;
            if let Some(t) = f.terms.iter().find(|t| !terms.iter().any(|x| x.eq_ignore_ascii_case(t))) {
                return Err(format!("{name}: {t:?} isn't in the glossary"));
            }
        }
        Ok(())
    }
}

impl Term {
    pub fn check(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("a term needs a name".into());
        }
        if self.name.trim() != self.name || self.name.chars().any(char::is_control) {
            return Err("a term's name is plain text, without spaces around it".into());
        }
        if self.name.chars().count() > MAX_ITEM {
            return Err(format!("the name is too long (at most {MAX_ITEM} characters)"));
        }
        if self.definition.trim().is_empty() {
            return Err("a term needs a definition".into());
        }
        text("the definition", &Some(self.definition.clone()))?;
        items("synonyms", &self.synonyms, MAX_ITEM)
    }
}

/// A model as the API shows it (models::model_json), with the organisation's docs on top: their
/// descriptions win, synonyms and tags add up (once each), examples and terms are theirs.
pub fn document(model: &mut serde_json::Value, docs: &ModelDocs) {
    use serde_json::{Value, json};
    let add = |v: &mut Value, extra: &[String]| {
        let mut list: Vec<String> =
            v.as_array().map_or(vec![], |a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect());
        for e in extra {
            if !list.iter().any(|x| x.eq_ignore_ascii_case(e)) {
                list.push(e.clone());
            }
        }
        *v = json!(list);
    };
    if let Some(d) = &docs.description {
        model["description"] = json!(d);
    }
    if !docs.tags.is_empty() {
        add(&mut model["tags"], &docs.tags);
    }
    for kind in ["dimensions", "measures"] {
        let Some(fields) = model[kind].as_array_mut() else { continue };
        for f in fields {
            let Some(d) = f["name"].as_str().and_then(|n| docs.fields.get(n)) else { continue };
            if let Some(text) = &d.description {
                f["description"] = json!(text);
            }
            if !d.synonyms.is_empty() {
                add(&mut f["synonyms"], &d.synonyms);
            }
            if !d.tags.is_empty() {
                add(&mut f["tags"], &d.tags);
            }
            if !d.examples.is_empty() {
                f["examples"] = json!(d.examples);
            }
            if !d.terms.is_empty() {
                f["terms"] = json!(d.terms);
            }
        }
    }
}

fn same_email(a: &Option<String>, b: &Option<String>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
        (None, None) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gov(owner: Option<&str>, stewards: &[&str]) -> Governance {
        Governance {
            owner: owner.map(String::from),
            stewards: stewards.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn standing_nests_admins_owners_stewards_readers() {
        let g = gov(Some("Ana@acme.example"), &["ben@acme.example"]);
        assert_eq!(standing(&g, "ana@acme.example", false), Standing::Owner);
        assert_eq!(standing(&g, "ben@acme.example", false), Standing::Steward);
        assert_eq!(standing(&g, "eve@acme.example", false), Standing::Reader);
        assert_eq!(standing(&g, "eve@acme.example", true), Standing::Admin);
        // nobody owns it: only admins decide
        assert_eq!(standing(&Governance::default(), "ana@acme.example", false), Standing::Reader);
        assert!(Standing::Admin > Standing::Owner && Standing::Owner > Standing::Steward);
    }

    #[test]
    fn owners_decide_but_hand_over_only_through_an_admin() {
        let now = gov(Some("ana@acme.example"), &[]);
        let certified =
            Governance { status: Some("certified".into()), stewards: vec!["ben@acme.example".into()], ..now.clone() };
        assert!(now.may_become(&certified, Standing::Owner).is_ok());
        let handed = gov(Some("ben@acme.example"), &[]);
        assert!(now.may_become(&handed, Standing::Owner).unwrap_err().contains("Org admin"));
        assert!(now.may_become(&handed, Standing::Admin).is_ok());
        // the same owner, written differently, isn't a hand-over
        assert!(now.may_become(&gov(Some("ANA@acme.example"), &[]), Standing::Owner).is_ok());
        assert!(now.may_become(&certified, Standing::Steward).is_err());
        assert!(now.may_become(&certified, Standing::Reader).is_err());
    }

    #[test]
    fn owners_and_stewards_are_members_and_the_rest_is_checked() {
        let members = ["ana@acme.example".to_string(), "ben@acme.example".to_string()];
        assert!(gov(Some("ana@acme.example"), &["ben@acme.example"]).check(&members).is_ok());
        assert!(gov(Some("eve@evil.example"), &[]).check(&members).unwrap_err().contains("member"));
        assert!(gov(None, &["eve@evil.example"]).check(&members).unwrap_err().contains("member"));
        assert!(gov(None, &["ben@acme.example", "BEN@acme.example"]).check(&members).unwrap_err().contains("twice"));
        assert!(gov(Some("ana@acme.example"), &["ana@acme.example"]).check(&members).unwrap_err().contains("owns"));
        let bad = Governance { status: Some("gold".into()), ..Default::default() };
        assert!(bad.check(&members).unwrap_err().contains("status"));
        let long = Governance { note: Some("x".repeat(MAX_NOTE + 1)), ..Default::default() };
        assert!(long.check(&members).unwrap_err().contains("long"));
        let many: Vec<String> = (0..=MAX_STEWARDS).map(|i| format!("p{i}@acme.example")).collect();
        let crowd = Governance { stewards: many.clone(), ..Default::default() };
        assert!(crowd.check(&many).unwrap_err().contains("at most"));
    }

    fn docs(v: serde_json::Value) -> ModelDocs {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn docs_name_real_fields_and_stay_within_bounds() {
        let fields = ["region", "revenue"];
        let terms = ["net revenue".to_string()];
        let ok = docs(serde_json::json!({"description": "Sales from the tills", "tags": ["finance"],
            "fields": {"revenue": {"description": "After discounts", "synonyms": ["turnover"], "examples": ["1,234.50"],
                                   "tags": ["finance"], "terms": ["Net revenue"]}}}));
        assert!(ok.check(&fields, &terms).is_ok());
        let e = |v| docs(v).check(&fields, &terms).unwrap_err();
        assert!(e(serde_json::json!({"fields": {"password_hash": {}}})).contains("no field"));
        assert!(e(serde_json::json!({"tags": ["Not A Tag!"]})).contains("tag"));
        assert!(e(serde_json::json!({"fields": {"region": {"terms": ["churn"]}}})).contains("glossary"));
        assert!(e(serde_json::json!({"description": "x".repeat(MAX_TEXT + 1)})).contains("long"));
        assert!(
            e(serde_json::json!({"fields": {"region": {"synonyms": vec!["s"; MAX_LIST + 1]}}})).contains("at most")
        );
        assert!(e(serde_json::json!({"fields": {"region": {"synonyms": [" "]}}})).contains("empty"));
    }

    #[test]
    fn docs_add_to_what_the_model_says() {
        let mut model = serde_json::json!({"description": "From YAML",
            "dimensions": [{"name": "region", "description": null, "synonyms": ["area"], "tags": []}],
            "measures": [{"name": "revenue", "description": "YAML says", "synonyms": [], "tags": ["finance"]}]});
        let d = docs(serde_json::json!({"description": "Sales from the tills",
            "fields": {"region": {"synonyms": ["territory", "Area"], "tags": ["geo"]},
                       "revenue": {"description": "After discounts", "tags": ["finance", "audited"], "terms": ["net revenue"]}}}));
        document(&mut model, &d);
        assert_eq!(model["description"], "Sales from the tills");
        let region = &model["dimensions"][0];
        // the model's own synonyms first, then the organisation's, once each (whatever the case)
        assert_eq!(region["synonyms"], serde_json::json!(["area", "territory"]));
        assert_eq!(region["tags"], serde_json::json!(["geo"]));
        let revenue = &model["measures"][0];
        assert_eq!(revenue["description"], "After discounts");
        assert_eq!(revenue["tags"], serde_json::json!(["finance", "audited"]));
        assert_eq!(revenue["terms"], serde_json::json!(["net revenue"]));
        // nothing documented: as it was
        let before = model.clone();
        document(&mut model, &ModelDocs::default());
        assert_eq!(model, before);
    }

    #[test]
    fn glossary_terms_are_checked() {
        let t = |v: serde_json::Value| serde_json::from_value::<Term>(v).unwrap();
        assert!(
            t(serde_json::json!({"name": "Net revenue", "definition": "Sales after discounts and returns",
            "synonyms": ["net sales"]}))
            .check()
            .is_ok()
        );
        assert!(t(serde_json::json!({"name": " ", "definition": "x"})).check().unwrap_err().contains("name"));
        assert!(t(serde_json::json!({"name": " x", "definition": "y"})).check().unwrap_err().contains("plain"));
        assert!(t(serde_json::json!({"name": "a\u{7}b", "definition": "y"})).check().unwrap_err().contains("plain"));
        assert!(t(serde_json::json!({"name": "P/E ratio", "definition": "y"})).check().is_ok());
        assert!(t(serde_json::json!({"name": "x", "definition": ""})).check().unwrap_err().contains("definition"));
        assert!(
            t(serde_json::json!({"name": "x".repeat(81), "definition": "y"})).check().unwrap_err().contains("long")
        );
    }
}
