//! Hex encoding/decoding utilities.

/// Encode bytes as lowercase hex.
pub fn hex_encode(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write;
        let _ = write!(text, "{byte:02x}");
    }
    text
}

/// Decode a lowercase hex string into bytes.
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
    fn encode_decode_round_trip() {
        let bytes = [0x00u8, 0x7f, 0x80, 0xff];
        assert_eq!(hex_decode(&hex_encode(&bytes)).unwrap(), bytes);
    }

    #[test]
    fn decode_rejects_odd_length() {
        assert!(hex_decode("abc").is_err());
    }

    #[test]
    fn decode_rejects_invalid_chars() {
        assert!(hex_decode("zz").is_err());
    }
}
