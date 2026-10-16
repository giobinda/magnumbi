//! Sign-in policies: which mechanisms people may use, and what they must add.
//!
//! Three levels, each optional, combine into the effective policy for a person:
//!
//! 1. the platform (system owners): mechanisms any organisation may use, minimum requirements;
//! 2. the platform's rules for one organisation (system owners);
//! 3. the organisation's own choice (its Org admins).
//!
//! Mechanisms allowed = those the server offers ∩ every level's list; a requirement set at any
//! level holds. So a lower level can only tighten what a higher one allows, never loosen it.
//! System owners, and people outside any organisation, follow the platform level alone.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    /// email + password (optionally with an authenticator code)
    Password,
    /// the platform's Google sign-in
    Google,
    /// the organisation's own OpenID Connect provider (Entra ID, Okta, Keycloak, ...)
    Oidc,
    /// WebAuthn: Touch ID, Windows Hello, security keys
    Passkey,
    /// API tokens: agents and scripts acting for the person who created them
    Token,
}

pub const METHODS: [Method; 5] = [Method::Password, Method::Google, Method::Oidc, Method::Passkey, Method::Token];

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Password => "password",
            Method::Google => "google",
            Method::Oidc => "oidc",
            Method::Passkey => "passkey",
            Method::Token => "token",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Method::Password => "password",
            Method::Google => "Google",
            Method::Oidc => "the organisation's single sign-on",
            Method::Passkey => "a passkey",
            Method::Token => "an API token",
        }
    }

    /// The mechanism a session was started with ("password+totp" -> Password).
    pub fn of_session(method: &str) -> Option<Method> {
        let base = method.split('+').next().unwrap_or_default();
        METHODS.into_iter().find(|m| m.as_str() == base)
    }
}

/// One level's rules.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SignInPolicy {
    /// mechanisms allowed at this level; None: no restriction here
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub methods: Option<Vec<Method>>,
    /// passwords need a second factor: a code from an authenticator app
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub require_two_factor: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Effective {
    pub methods: BTreeSet<Method>,
    pub require_two_factor: bool,
}

/// The policy that holds: `levels` from the platform down, `offered` by this server.
pub fn effective(levels: &[Option<&SignInPolicy>], offered: &BTreeSet<Method>) -> Effective {
    let mut methods = offered.clone();
    let mut require_two_factor = false;
    for p in levels.iter().flatten() {
        if let Some(list) = &p.methods {
            methods.retain(|m| list.contains(m));
        }
        require_two_factor |= p.require_two_factor;
    }
    Effective { methods, require_two_factor }
}

/// What an existing session may do under the policy now in force.
#[derive(Debug, PartialEq)]
pub enum SessionCheck {
    Ok,
    /// the way it signed in isn't allowed any more (or no longer enough)
    SignInAgain(String),
    /// allowed in only to add the second factor the policy now requires
    SetUpTwoFactor,
}

impl Effective {
    pub fn allows(&self, m: Method) -> bool {
        self.methods.contains(&m)
    }

    /// Can someone still be signed in with `session_method`? (`has_two_factor`: they have an
    /// authenticator set up.)
    pub fn check_session(&self, session_method: &str, has_two_factor: bool) -> SessionCheck {
        let Some(m) = Method::of_session(session_method) else {
            return SessionCheck::SignInAgain("please sign in again".into());
        };
        if !self.allows(m) {
            return SessionCheck::SignInAgain(format!(
                "{} sign-in isn't allowed any more; please sign in again",
                m.as_str()
            ));
        }
        if m == Method::Password && self.require_two_factor && session_method != "password+totp" {
            return if has_two_factor {
                SessionCheck::SignInAgain("an authenticator code is now required; please sign in again".into())
            } else {
                SessionCheck::SetUpTwoFactor
            };
        }
        SessionCheck::Ok
    }

    /// Ways a person can actually sign in (an organisation's provider only once it's set up;
    /// API tokens aren't one: they're made by someone already signed in).
    pub fn usable(&self, oidc_configured: bool) -> Vec<Method> {
        self.methods
            .iter()
            .copied()
            .filter(|m| *m != Method::Token && (*m != Method::Oidc || oidc_configured))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(methods: Option<&[Method]>, two_factor: bool) -> SignInPolicy {
        SignInPolicy { methods: methods.map(|m| m.to_vec()), require_two_factor: two_factor }
    }

    fn all() -> BTreeSet<Method> {
        METHODS.into_iter().collect()
    }

    #[test]
    fn lower_levels_only_tighten() {
        use Method::*;
        let platform = p(Some(&[Password, Oidc, Passkey]), false);
        let for_org = p(None, true);
        let org = p(Some(&[Password, Google, Oidc]), false); // Google isn't allowed above: stays out
        let e = effective(&[Some(&platform), Some(&for_org), Some(&org)], &all());
        assert_eq!(e.methods, BTreeSet::from([Password, Oidc]));
        assert!(e.require_two_factor, "a requirement set above holds below");

        // nothing set: whatever the server offers
        let offered = BTreeSet::from([Password, Passkey]);
        assert_eq!(effective(&[None, None, None], &offered).methods, offered);
    }

    #[test]
    fn sessions_follow_the_policy_in_force() {
        use Method::*;
        let e = Effective { methods: BTreeSet::from([Password, Passkey]), require_two_factor: true };
        assert_eq!(e.check_session("password+totp", true), SessionCheck::Ok);
        assert_eq!(e.check_session("passkey", false), SessionCheck::Ok);
        assert_eq!(e.check_session("password", false), SessionCheck::SetUpTwoFactor);
        assert!(matches!(e.check_session("password", true), SessionCheck::SignInAgain(_)));
        assert!(matches!(e.check_session("google", false), SessionCheck::SignInAgain(m) if m.contains("google")));
        assert!(matches!(e.check_session("magic", false), SessionCheck::SignInAgain(_)));
        let lax = Effective { methods: BTreeSet::from([Password]), require_two_factor: false };
        assert_eq!(lax.check_session("password", false), SessionCheck::Ok);
    }

    #[test]
    fn single_sign_on_counts_once_set_up() {
        let e = Effective { methods: BTreeSet::from([Method::Oidc]), require_two_factor: false };
        assert!(e.usable(false).is_empty());
        assert_eq!(e.usable(true), [Method::Oidc]);
    }

    #[test]
    fn api_tokens_are_allowed_like_any_method_but_sign_nobody_in() {
        let e = Effective { methods: BTreeSet::from([Method::Token]), require_two_factor: true };
        assert!(e.usable(true).is_empty(), "a token is made by someone signed in another way");
        assert_eq!(e.check_session("token", false), SessionCheck::Ok, "the second factor is for passwords");
        let none = Effective { methods: BTreeSet::from([Method::Password]), require_two_factor: false };
        assert!(matches!(none.check_session("token", false), SessionCheck::SignInAgain(_)));
    }

    #[test]
    fn policies_read_from_settings() {
        let v: SignInPolicy = serde_json::from_value(serde_json::json!({"methods": ["oidc", "passkey"]})).unwrap();
        assert_eq!(v.methods.as_deref(), Some(&[Method::Oidc, Method::Passkey][..]));
        assert!(serde_json::from_value::<SignInPolicy>(serde_json::json!({"methods": ["ldap"]})).is_err());
        assert_eq!(serde_json::to_value(SignInPolicy::default()).unwrap(), serde_json::json!({}));
    }
}
