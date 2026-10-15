//! Secrets at rest: each organisation's ScramDB connection string (it carries a password) is
//! sealed with AES-256-GCM under MAGNUM_SECRET_KEY before it reaches the primary database.
//!
//! The key is 32 random bytes, base64-encoded (`magnum-bi gen-key`). Every replica needs the
//! same key; losing it means re-entering the organisations' connection strings.

use aes_gcm::aead::{Aead, KeyInit, OsRng, rand_core::RngCore};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;

const PREFIX: &str = "v1:";

#[derive(Clone)]
pub struct SecretKey(Key<Aes256Gcm>);

impl std::fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretKey(..)") // never print the key
    }
}

impl SecretKey {
    pub fn parse(b64: &str) -> Result<SecretKey> {
        let bytes = STANDARD
            .decode(b64.trim())
            .context("MAGNUM_SECRET_KEY isn't base64 (create one with `magnum-bi gen-key`)")?;
        if bytes.len() != 32 {
            bail!("MAGNUM_SECRET_KEY must be 32 bytes, base64-encoded (create one with `magnum-bi gen-key`)");
        }
        Ok(SecretKey(*Key::<Aes256Gcm>::from_slice(&bytes)))
    }

    pub fn generate() -> String {
        let mut k = [0u8; 32];
        OsRng.fill_bytes(&mut k);
        STANDARD.encode(k)
    }

    /// "v1:" + base64(nonce || ciphertext+tag). `context` (e.g. the organisation id) is bound
    /// as associated data, so a sealed value can't be moved to another organisation's row.
    pub fn seal(&self, plain: &str, context: &str) -> Result<String> {
        let mut nonce = [0u8; 12];
        OsRng.fill_bytes(&mut nonce);
        let ct = Aes256Gcm::new(&self.0)
            .encrypt(
                Nonce::from_slice(&nonce),
                aes_gcm::aead::Payload { msg: plain.as_bytes(), aad: context.as_bytes() },
            )
            .map_err(|_| anyhow!("encryption failed"))?;
        let mut out = nonce.to_vec();
        out.extend(ct);
        Ok(format!("{PREFIX}{}", STANDARD.encode(out)))
    }

    pub fn open(&self, sealed: &str, context: &str) -> Result<String> {
        let raw = sealed.strip_prefix(PREFIX).ok_or_else(|| anyhow!("unknown secret format"))?;
        let bytes = STANDARD.decode(raw)?;
        if bytes.len() < 12 + 16 {
            bail!("sealed value too short");
        }
        let (nonce, ct) = bytes.split_at(12);
        let plain = Aes256Gcm::new(&self.0)
            .decrypt(Nonce::from_slice(nonce), aes_gcm::aead::Payload { msg: ct, aad: context.as_bytes() })
            .map_err(|_| anyhow!("can't decrypt: wrong MAGNUM_SECRET_KEY, or the value was tampered with"))?;
        Ok(String::from_utf8(plain)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_and_open() {
        let k = SecretKey::parse(&SecretKey::generate()).unwrap();
        let s = k.seal("postgresql://u:secret@h/db", "acme").unwrap();
        assert!(s.starts_with("v1:") && !s.contains("secret"));
        assert_eq!(k.open(&s, "acme").unwrap(), "postgresql://u:secret@h/db");
        assert!(k.open(&s, "globex").is_err(), "bound to its organisation");
        let other = SecretKey::parse(&SecretKey::generate()).unwrap();
        assert!(other.open(&s, "acme").is_err());
        assert!(SecretKey::parse("dG9vIHNob3J0").is_err());
        assert_ne!(k.seal("x", "a").unwrap(), k.seal("x", "a").unwrap(), "fresh nonce each time");
    }
}
