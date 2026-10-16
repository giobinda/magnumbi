//! Authenticator codes (TOTP, RFC 6238): the second factor after a password.
//!
//! A 160-bit secret is shared once with the person's authenticator app (a QR code of an
//! otpauth:// URI) and kept sealed in ScramDB. Codes are 6 digits, HMAC-SHA1 over 30-second
//! steps (what every authenticator app does by default); a code from the step before or after
//! is accepted for clock drift, and a step is never accepted twice (replay). Recovery codes are
//! random, shown once, and stored only as SHA-256.

use hmac::{Hmac, KeyInit, Mac};
use sha1::Sha1;

const STEP: i64 = 30;
const DIGITS: u32 = 6;
pub const RECOVERY_CODES: usize = 10;

/// A new secret (20 random bytes).
pub fn new_secret() -> Vec<u8> {
    (0..20).map(|_| rand::random::<u8>()).collect()
}

/// RFC 4648 base32 without padding: how authenticator apps take secrets.
pub fn base32(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut out = String::new();
    let (mut buffer, mut bits) = (0u32, 0u32);
    for b in bytes {
        buffer = (buffer << 8) | u32::from(*b);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((buffer >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}

/// The code for a 30-second step (RFC 4226 dynamic truncation, `DIGITS` digits).
pub fn code(secret: &[u8], step: i64) -> String {
    let mut mac = <Hmac<Sha1> as KeyInit>::new_from_slice(secret).expect("any key length");
    mac.update(&step.to_be_bytes());
    let h = mac.finalize().into_bytes();
    let offset = (h[h.len() - 1] & 0x0f) as usize;
    let n = u32::from_be_bytes([h[offset] & 0x7f, h[offset + 1], h[offset + 2], h[offset + 3]]);
    format!("{:0width$}", n % 10u32.pow(DIGITS), width = DIGITS as usize)
}

pub fn step_at(unix_seconds: i64) -> i64 {
    unix_seconds.div_euclid(STEP)
}

/// The step `input` is the code of (now, or one step either side), if it's newer than
/// `last_used`. Spaces in the input are ignored.
pub fn verify(secret: &[u8], input: &str, unix_seconds: i64, last_used: Option<i64>) -> Option<i64> {
    let input: String = input.chars().filter(|c| !c.is_whitespace()).collect();
    if input.len() != DIGITS as usize || !input.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let now = step_at(unix_seconds);
    [now - 1, now, now + 1]
        .into_iter()
        .filter(|s| last_used.is_none_or(|l| *s > l))
        .find(|s| constant_eq(&code(secret, *s), &input))
}

fn constant_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// What authenticator apps scan: otpauth://totp/Magnum%20BI:email?secret=...&issuer=Magnum%20BI
pub fn uri(secret: &[u8], email: &str) -> String {
    let enc = |s: &str| -> String {
        url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>().replace('+', "%20")
    };
    format!(
        "otpauth://totp/{}:{}?secret={}&issuer={}&algorithm=SHA1&digits={DIGITS}&period={STEP}",
        enc("Magnum BI"),
        enc(email),
        base32(secret),
        enc("Magnum BI")
    )
}

/// The URI as a QR code (SVG), for the setup screen.
pub fn qr_svg(uri: &str) -> String {
    qrcode::QrCode::new(uri.as_bytes())
        .map(|q| q.render::<qrcode::render::svg::Color>().min_dimensions(200, 200).quiet_zone(true).build())
        .unwrap_or_default()
}

/// New recovery codes ("abcd-efgh-jkmn", unambiguous letters) and their hashes.
pub fn recovery_codes() -> (Vec<String>, Vec<String>) {
    const LETTERS: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
    let codes: Vec<String> = (0..RECOVERY_CODES)
        .map(|_| {
            let part = || {
                (0..4)
                    .map(|_| LETTERS[rand::Rng::gen_range(&mut rand::thread_rng(), 0..LETTERS.len())] as char)
                    .collect::<String>()
            };
            format!("{}-{}-{}", part(), part(), part())
        })
        .collect();
    let hashes = codes.iter().map(|c| recovery_hash(c)).collect();
    (codes, hashes)
}

/// How a recovery code is stored and looked up (case, spaces and dashes don't matter).
pub fn recovery_hash(code: &str) -> String {
    let norm: String = code.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_lowercase();
    crate::store::sha256_hex(&norm)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RFC_SECRET: &[u8] = b"12345678901234567890";

    #[test]
    fn rfc_6238_vectors() {
        // RFC 6238 appendix B (SHA-1), last 6 of the 8 digits
        for (t, expected) in [
            (59, "287082"),
            (1111111109, "081804"),
            (1111111111, "050471"),
            (1234567890, "005924"),
            (2000000000, "279037"),
        ] {
            assert_eq!(code(RFC_SECRET, step_at(t)), expected, "t={t}");
        }
        assert_eq!(base32(RFC_SECRET), "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ");
        assert_eq!(base32(b"f"), "MY");
    }

    #[test]
    fn codes_are_accepted_once_with_a_little_drift() {
        let t = 1111111111;
        let now = code(RFC_SECRET, step_at(t));
        assert_eq!(verify(RFC_SECRET, &now, t, None), Some(step_at(t)));
        assert_eq!(verify(RFC_SECRET, &format!("{} {}", &now[..3], &now[3..]), t, None), Some(step_at(t)));
        assert_eq!(verify(RFC_SECRET, &now, t, Some(step_at(t))), None, "replayed");
        assert!(verify(RFC_SECRET, &code(RFC_SECRET, step_at(t) - 1), t, None).is_some(), "30 s slow");
        assert!(verify(RFC_SECRET, &code(RFC_SECRET, step_at(t) - 2), t, None).is_none(), "too old");
        for bad in ["", "12345", "1234567", "abcdef"] {
            assert_eq!(verify(RFC_SECRET, bad, t, None), None, "{bad:?}");
        }
    }

    #[test]
    fn setup_material() {
        let u = uri(RFC_SECRET, "boss@acme.example");
        assert!(u.starts_with("otpauth://totp/Magnum%20BI:boss%40acme.example?secret=GEZDGNBVGY3TQOJQ"), "{u}");
        assert!(qr_svg(&u).starts_with("<?xml"));
        let (codes, hashes) = recovery_codes();
        assert_eq!((codes.len(), hashes.len()), (RECOVERY_CODES, RECOVERY_CODES));
        assert_eq!(recovery_hash(&codes[0].to_uppercase().replace('-', " ")), hashes[0]);
        assert_ne!(codes[0], codes[1]);
    }
}
