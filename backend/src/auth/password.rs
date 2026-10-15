//! Password sign-in: Argon2id hashes (PHC strings) in ScramDB, a minimal policy, and a lockout
//! after repeated failures.
//!
//! Accounts get a password in three ways:
//! - `magnum-bi user set-password <email>` on the server (system owners bootstrapping, recovery);
//! - an Org admin (or system owner) resets a member's password in the UI: Magnum BI generates a
//!   one-time password and the person must choose a new one at the next sign-in. Such accounts
//!   are bound to that organisation (users.owner_org, see auth::restrict_password_accounts);
//! - a signed-in person changes their own password.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use argon2::Argon2;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use rand::Rng;
use rand::rngs::OsRng;

pub const MIN_LENGTH: usize = 10;
pub const MAX_LENGTH: usize = 256;
const MAX_FAILURES: u32 = 5;
const LOCKOUT: Duration = Duration::from_secs(15 * 60);

pub fn check_policy(password: &str, email: &str) -> Result<(), String> {
    let n = password.chars().count();
    if n < MIN_LENGTH {
        return Err(format!("use at least {MIN_LENGTH} characters"));
    }
    if n > MAX_LENGTH {
        return Err(format!("use at most {MAX_LENGTH} characters"));
    }
    let lower = password.to_lowercase();
    let local = email.split('@').next().unwrap_or_default().to_lowercase();
    if lower == email.to_lowercase() || (local.len() >= 4 && lower.contains(&local)) {
        return Err("don't use your email address in your password".into());
    }
    if password.chars().all(|c| c == password.chars().next().unwrap_or(' ')) {
        return Err("that password is too easy to guess".into());
    }
    Ok(())
}

pub fn hash(password: &str) -> anyhow::Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| anyhow::anyhow!("hashing: {e}"))
}

/// Verifies against `stored`, or against a fixed dummy hash when there is no account (so the
/// answer takes as long either way and doesn't reveal which emails exist).
pub fn verify(password: &str, stored: Option<&str>) -> bool {
    static DUMMY: LazyLock<String> = LazyLock::new(|| hash("not a real password, only spends time").expect("argon2"));
    let (target, real) = match stored {
        Some(s) => (s, true),
        None => (DUMMY.as_str(), false),
    };
    let ok =
        PasswordHash::new(target).is_ok_and(|h| Argon2::default().verify_password(password.as_bytes(), &h).is_ok());
    ok && real
}

/// A one-time password for an admin to hand over: 16 characters without look-alikes.
pub fn generate() -> String {
    const ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut rng = OsRng;
    let raw: String = (0..16).map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char).collect();
    format!("{}-{}-{}-{}", &raw[0..4], &raw[4..8], &raw[8..12], &raw[12..16])
}

/// Failed sign-ins per email: after 5, that email is locked for 15 minutes (on this replica).
#[derive(Default)]
pub struct LoginLimiter {
    failures: Mutex<HashMap<String, (u32, Instant)>>,
}

impl LoginLimiter {
    /// Some(seconds left) while locked.
    pub fn locked(&self, email: &str) -> Option<u64> {
        let f = self.failures.lock().expect("limiter lock");
        match f.get(email) {
            Some((n, at)) if *n >= MAX_FAILURES && at.elapsed() < LOCKOUT => {
                Some((LOCKOUT - at.elapsed()).as_secs() + 1)
            }
            _ => None,
        }
    }

    pub fn failed(&self, email: &str) {
        let mut f = self.failures.lock().expect("limiter lock");
        if f.len() > 100_000 {
            f.retain(|_, (_, at)| at.elapsed() < LOCKOUT); // bounded memory
        }
        let e = f.entry(email.to_string()).or_insert((0, Instant::now()));
        if e.1.elapsed() >= LOCKOUT {
            *e = (0, Instant::now());
        }
        e.0 += 1;
        e.1 = Instant::now();
    }

    pub fn succeeded(&self, email: &str) {
        self.failures.lock().expect("limiter lock").remove(email);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_verify() {
        let h = hash("correct horse battery").unwrap();
        assert!(h.starts_with("$argon2id$"));
        assert!(verify("correct horse battery", Some(&h)));
        assert!(!verify("wrong horse battery", Some(&h)));
        assert!(!verify("correct horse battery", None));
        assert!(!verify("anything", Some("not a hash")));
    }

    #[test]
    fn policy() {
        assert!(check_policy("short", "a@b.example").is_err());
        assert!(check_policy("aaaaaaaaaaaa", "a@b.example").is_err());
        assert!(check_policy("maria.rossi-2026", "maria.rossi@b.example").is_err());
        assert!(check_policy("a long passphrase", "maria@b.example").is_ok());
        let g = generate();
        assert_eq!(g.len(), 19);
        assert!(check_policy(&g, "x@y.example").is_ok());
    }

    #[test]
    fn lockout() {
        let l = LoginLimiter::default();
        for _ in 0..4 {
            l.failed("a@b.example");
        }
        assert!(l.locked("a@b.example").is_none());
        l.failed("a@b.example");
        assert!(l.locked("a@b.example").is_some());
        assert!(l.locked("other@b.example").is_none());
        l.succeeded("a@b.example");
        assert!(l.locked("a@b.example").is_none());
    }
}
