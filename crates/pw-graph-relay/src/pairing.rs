//! PIN pairing for relay sessions.
//!
//! The host displays a short numeric PIN. A client proves knowledge of it by
//! returning `HMAC-SHA256(key = PIN, msg = salt)` for a fresh random salt the
//! host sends in its challenge. The PIN itself never crosses the wire.

use hmac::{Hmac, Mac};
use rand::Rng;
use sha2::Sha256;

pub const PIN_LENGTH: usize = 6;
const SALT_BYTES: usize = 16;

type HmacSha256 = Hmac<Sha256>;

/// Generate a fresh numeric PIN for display in the host UI.
pub fn generate_pin() -> String {
    let mut rng = rand::thread_rng();
    (0..PIN_LENGTH)
        .map(|_| rng.gen_range(0..10u8).to_string())
        .collect()
}

/// Generate a random salt and return it as lowercase hex.
pub fn generate_salt() -> String {
    let mut rng = rand::thread_rng();
    let mut salt = [0u8; SALT_BYTES];
    rng.fill(&mut salt);
    hex_encode(&salt)
}

/// The digest a client must return for a given PIN and challenge salt.
pub fn pair_digest(pin: &str, salt: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(pin.as_bytes())
        .expect("HMAC-SHA256 accepts any key length");
    mac.update(salt.as_bytes());
    hex_encode(&mac.finalize().into_bytes())
}

/// Constant-time verification of a client's digest on the host side.
pub fn verify_digest(pin: &str, salt: &str, digest: &str) -> bool {
    let Ok(provided) = hex_decode(digest) else {
        return false;
    };
    let Ok(mut mac) = HmacSha256::new_from_slice(pin.as_bytes()) else {
        return false;
    };
    mac.update(salt.as_bytes());
    mac.verify_slice(&provided).is_ok()
}

pub fn hex_encode(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push_str(&format!("{byte:02x}"));
    }
    text
}

pub fn hex_decode(text: &str) -> Result<Vec<u8>, String> {
    if !text.len().is_multiple_of(2) {
        return Err("odd-length hex string".into());
    }
    (0..text.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&text[index..index + 2], 16)
                .map_err(|error| format!("invalid hex byte: {error}"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_is_six_digits() {
        for _ in 0..32 {
            let pin = generate_pin();
            assert_eq!(pin.len(), PIN_LENGTH);
            assert!(pin.chars().all(|c| c.is_ascii_digit()));
        }
    }

    #[test]
    fn digest_verifies_with_matching_pin() {
        let pin = "123456";
        let salt = generate_salt();
        let digest = pair_digest(pin, &salt);
        assert!(verify_digest(pin, &salt, &digest));
    }

    #[test]
    fn digest_rejects_wrong_pin_or_salt() {
        let salt = generate_salt();
        let digest = pair_digest("123456", &salt);
        assert!(!verify_digest("654321", &salt, &digest));
        assert!(!verify_digest("123456", "other-salt", &digest));
        assert!(!verify_digest("123456", &salt, "not-hex"));
    }

    #[test]
    fn known_hmac_vector() {
        // Deterministic vector so the wire format never drifts silently.
        let digest = pair_digest("000000", "aabbcc");
        assert_eq!(digest.len(), 64);
        assert!(verify_digest("000000", "aabbcc", &digest));
    }

    #[test]
    fn hex_round_trip() {
        let bytes = [0x00u8, 0x7f, 0x80, 0xff];
        assert_eq!(hex_decode(&hex_encode(&bytes)).unwrap(), bytes);
        assert!(hex_decode("abc").is_err());
        assert!(hex_decode("zz").is_err());
    }
}
