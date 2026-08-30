//! PIN generation and the "scan to connect" payload for relay pairing.
//!
//! The host displays a short numeric PIN; the actual proof of knowledge runs
//! as a SPAKE2 exchange in [`crate::crypto`], so the PIN never crosses the
//! wire and a captured transcript cannot be brute-forced offline. That is why
//! six digits remains a defensible length here: guessing is an online-only
//! game, and [`crate::PAIRING_ATTEMPT_LIMIT`] makes each round of it costly.

use rand::Rng;

pub const PIN_LENGTH: usize = 6;

/// Scheme of the "scan to connect" QR payload.
pub const QR_SCHEME: &str = "qpw-relay://";

/// A parsed connection payload: the `host:port` control endpoint plus the
/// optional pairing PIN carried by a QR code or a pasted string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QrPayload {
    pub target: String,
    pub pin: Option<String>,
}

/// Build the `qpw-relay://host:port?pin=...` payload encoded in the host QR.
pub fn build_qr_payload(addr: impl std::fmt::Display, port: u16, pin: &str) -> String {
    let mut payload = format!("{QR_SCHEME}{addr}:{port}");
    let pin = pin.trim();
    if !pin.is_empty() {
        payload.push_str("?pin=");
        payload.push_str(pin);
    }
    payload
}

/// Parse a scanned or pasted connection payload.
///
/// Accepts the app's own `qpw-relay://host:port?pin=123456` URI as well as a
/// plain `host:port` string, so any generic QR carrying the address still
/// works. Returns `None` when the text does not describe a usable endpoint.
pub fn parse_qr_payload(text: &str) -> Option<QrPayload> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if let Some(rest) = text.strip_prefix(QR_SCHEME) {
        let (target, query) = rest.split_once('?').unwrap_or((rest, ""));
        let target = target.trim_end_matches('/');
        split_host_port(target)?;
        let pin = query
            .split('&')
            .find_map(|pair| pair.strip_prefix("pin="))
            .map(str::trim)
            .filter(|pin| !pin.is_empty())
            .map(str::to_owned);
        return Some(QrPayload {
            target: target.to_owned(),
            pin,
        });
    }
    split_host_port(text).map(|target| QrPayload { target, pin: None })
}

/// Validate a bare `host:port` string and return it normalized.
fn split_host_port(text: &str) -> Option<String> {
    let (host, port) = text.rsplit_once(':')?;
    if host.is_empty()
        || port.is_empty()
        || host.chars().any(char::is_whitespace)
        || host.contains('/')
        || !port.bytes().all(|byte| byte.is_ascii_digit())
        || port.parse::<u16>().is_err()
    {
        return None;
    }
    Some(text.to_owned())
}

/// Generate a fresh numeric PIN for display in the host UI.
pub fn generate_pin() -> String {
    let mut rng = rand::thread_rng();
    (0..PIN_LENGTH)
        .map(|_| rng.gen_range(0..10u8).to_string())
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
    fn payload_round_trip_with_pin() {
        let payload = build_qr_payload("192.168.1.20", 48123, "123456");
        assert_eq!(payload, "qpw-relay://192.168.1.20:48123?pin=123456");
        let parsed = parse_qr_payload(&payload).unwrap();
        assert_eq!(parsed.target, "192.168.1.20:48123");
        assert_eq!(parsed.pin.as_deref(), Some("123456"));
    }

    #[test]
    fn payload_without_pin_omits_query() {
        let payload = build_qr_payload("10.0.0.1", 1234, "  ");
        assert_eq!(payload, "qpw-relay://10.0.0.1:1234");
        let parsed = parse_qr_payload(&payload).unwrap();
        assert_eq!(parsed.target, "10.0.0.1:1234");
        assert_eq!(parsed.pin, None);
    }

    #[test]
    fn payload_accepts_plain_host_port_and_whitespace() {
        let parsed = parse_qr_payload("  studio.local:48123  ").unwrap();
        assert_eq!(parsed.target, "studio.local:48123");
        assert_eq!(parsed.pin, None);
    }

    #[test]
    fn payload_rejects_unusable_text() {
        for text in [
            "",
            "   ",
            "qpw-relay://",
            "qpw-relay://?pin=123456",
            "qpw-relay://192.168.1.20?pin=123456",
            "192.168.1.20",
            "192.168.1.20:notaport",
            "192.168.1.20:70000",
            "hello world:1234",
        ] {
            assert!(parse_qr_payload(text).is_none(), "accepted {text:?}");
        }
    }

    #[test]
    fn hex_round_trip() {
        use pw_graph_utils::hex::{hex_decode, hex_encode};
        let bytes = [0x00u8, 0x7f, 0x80, 0xff];
        assert_eq!(hex_decode(&hex_encode(&bytes)).unwrap(), bytes);
        assert!(hex_decode("abc").is_err());
        assert!(hex_decode("zz").is_err());
    }
}
