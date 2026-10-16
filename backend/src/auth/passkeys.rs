//! Passkeys (WebAuthn): Touch ID, Windows Hello, phones, security keys.
//!
//! webauthn-rs does the protocol: challenges, origin and relying-party checks, signatures,
//! counters. Here: who the relying party is. With MAGNUM_PUBLIC_URL, its host and origin; without
//! (development), localhost on the server's port and on the Vite dev server's.
//!
//! Sign-in starts from the email (the person's passkeys are offered), the flow webauthn-rs
//! recommends. The ceremonies' state is kept in ScramDB between the two steps, so any replica
//! can finish what another started.

use url::Url;
use webauthn_rs::{Webauthn, WebauthnBuilder};

use crate::config::Config;

pub fn relying_party(config: &Config) -> Option<Webauthn> {
    let (rp_id, origins) = match &config.public_url {
        Some(u) => {
            let url = Url::parse(u).ok()?;
            (url.host_str()?.to_string(), vec![url])
        }
        None => (
            "localhost".to_string(),
            [format!("http://localhost:{}", config.port), "http://localhost:5173".to_string()]
                .iter()
                .filter_map(|o| Url::parse(o).ok())
                .collect(),
        ),
    };
    let mut builder = WebauthnBuilder::new(&rp_id, origins.first()?).ok()?.rp_name("Magnum BI");
    for o in origins.iter().skip(1) {
        builder = builder.append_allowed_origin(o);
    }
    match builder.build() {
        Ok(w) => Some(w),
        Err(e) => {
            tracing::warn!("passkeys unavailable: {e}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relying_party_from_the_public_url() {
        let mut c = Config::from_lookup(|_| None).unwrap();
        assert!(relying_party(&c).is_some(), "development: localhost");
        c.public_url = Some("https://bi.acme.example".into());
        assert!(relying_party(&c).is_some());
        c.public_url = Some("not a url".into());
        assert!(relying_party(&c).is_none());
    }
}
