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
}
