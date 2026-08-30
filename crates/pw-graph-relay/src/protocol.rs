//! Relay control-channel protocol (version 3).
//!
//! The control channel is a TCP byte stream of length-prefixed frames:
//!
//! ```text
//! magic "QPR3" (4 bytes) | version u8 | payload length u32 LE | payload
//! ```
//!
//! The original pairing handshake frames — up to and including key
//! confirmation — carry their JSON in the clear, because there is no key yet.
//! A reconnecting peer then uses the explicit cleartext
//! `ResumeHello`/`ResumeChallenge`/`ResumeProof` exchange below; after proof
//! succeeds, `ResumeOk` and subsequent frames are sealed. Every other
//! post-pairing frame is a ChaCha20-Poly1305 sealed JSON document with the
//! 9-byte header authenticated as associated data, so the control channel is
//! confidential and tamper-evident rather than merely well-formed.
//!
//! JSON keeps third-party implementations trivial. Unknown message types are
//! preserved as `Unknown` so newer peers can extend the protocol without
//! breaking older ones. The full wire specification lives in
//! `docs/relay-protocol.md`.

use crate::crypto::{Opener, Sealer};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::io::{self, Read, Write};

pub const CONTROL_MAGIC: &[u8; 4] = b"QPR3";
pub const PROTOCOL_VERSION: u8 = 3;

/// Frame durations the codec layer and the session negotiation both accept.
///
/// One list, used by the UI, the config layer, and the wire check alike: a
/// hand-edited config used to be able to smuggle a value such as 7 ms past a
/// `clamp(5, 60)` and only fail at the far end of a handshake.
pub const FRAME_DURATIONS_MS: [u16; 5] = [5, 10, 20, 40, 60];

/// Snap `frame_ms` to the nearest supported frame duration.
pub fn normalize_frame_ms(frame_ms: u16) -> u16 {
    FRAME_DURATIONS_MS
        .into_iter()
        .min_by_key(|candidate| candidate.abs_diff(frame_ms))
        .expect("the supported frame duration list is never empty")
}

/// Whether `frame_ms` is exactly one of the supported durations.
pub fn is_supported_frame_ms(frame_ms: u16) -> bool {
    FRAME_DURATIONS_MS.contains(&frame_ms)
}

/// Sample rates the codec layer and the session negotiation both accept.
///
/// Opus itself accepts more, but the relay narrows the set so both ends can
/// size their conversion buffers from a known maximum ratio rather than from
/// whatever the peer happened to ask for.
pub const SAMPLE_RATES_HZ: [u32; 3] = [16_000, 24_000, 48_000];

/// The largest negotiable sample rate. Buffer sizing upper bounds come from
/// this, so it must stay the maximum of [`SAMPLE_RATES_HZ`].
pub const MAX_SAMPLE_RATE_HZ: u32 = 48_000;

/// The largest negotiable channel count.
pub const MAX_CHANNELS: u16 = 2;

/// Whether `sample_rate` is exactly one of the negotiable rates.
pub fn is_supported_sample_rate(sample_rate: u32) -> bool {
    SAMPLE_RATES_HZ.contains(&sample_rate)
}

/// Whether `channels` is a negotiable channel count (mono or stereo).
pub fn is_supported_channels(channels: u16) -> bool {
    (1..=MAX_CHANNELS).contains(&channels)
}
/// Refuse control frames larger than this. They are small JSON documents;
/// a bigger frame indicates a broken or hostile peer.
pub const MAX_CONTROL_FRAME: u32 = 64 * 1024;

// Which kind of device a peer runs on. Informational for display only.
pw_graph_utils::enum_str! {
    #[derive(Serialize, Deserialize)]
    #[serde(rename_all = "lowercase")]
    pub enum DeviceKind {
        Android = "android",
        Linux = "linux",
        Other = "other",
    }
}

/// Directions a client wants to carry in a session, from the client's point of
/// view. `emit` sends the client's captured audio to the host; `receive`
/// plays back audio the host sends. A session may carry both.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Roles {
    pub emit: bool,
    pub receive: bool,
}

impl Roles {
    pub fn emit_only() -> Self {
        Self {
            emit: true,
            receive: false,
        }
    }

    pub fn receive_only() -> Self {
        Self {
            emit: false,
            receive: true,
        }
    }

    pub fn both() -> Self {
        Self {
            emit: true,
            receive: true,
        }
    }

    pub fn is_empty(self) -> bool {
        !self.emit && !self.receive
    }
}

// Audio codec carried on the UDP audio channel.
pw_graph_utils::enum_str! {
    #[derive(Serialize, Deserialize)]
    #[serde(rename_all = "lowercase")]
    pub enum CodecKind {
        Pcm = "pcm",
        Opus = "opus",
    }
}

impl CodecKind {
    /// Numeric identifier used in audio packet headers.
    pub fn id(self) -> u8 {
        match self {
            Self::Pcm => 0,
            Self::Opus => 1,
        }
    }

    pub fn from_id(id: u8) -> Option<Self> {
        match id {
            0 => Some(Self::Pcm),
            1 => Some(Self::Opus),
            _ => None,
        }
    }
}

/// Control-channel messages. Tagged by the `type` field; unknown tags fall
/// through to [`ControlMessage::Unknown`].
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlMessage {
    /// C→H first message: who is calling, what they can do, and the client's
    /// SPAKE2 message so pairing costs one round trip rather than two.
    Hello {
        protocol: u32,
        /// Stable installation identity. Older v3 peers may omit it; the
        /// receiver falls back to the advertised name for that case.
        #[serde(default)]
        device_id: String,
        /// `adb` selects the authenticated TCP audio side channel. Empty or
        /// any other value keeps the normal UDP audio transport.
        #[serde(default)]
        transport: String,
        device_name: String,
        device_kind: DeviceKind,
        roles: Roles,
        sample_rate: u32,
        channels: u16,
        pake: String,
    },
    /// H→C the host's SPAKE2 message, hex. Public: it reveals nothing about
    /// the PIN, which is the whole point of using a PAKE here.
    Challenge {
        protocol: u32,
        pake: String,
        host_name: String,
        /// Stable host identity used for trusted reconnects.
        #[serde(default)]
        device_id: String,
    },
    /// C→H the client's SPAKE2 message plus its key-confirmation value, both
    /// hex. The confirmation is what actually proves the PIN matched.
    Pair {
        pake: String,
        confirm: String,
    },
    /// H→C the host's key confirmation. After this frame the channel is
    /// encrypted in both directions.
    PairConfirm {
        confirm: String,
    },
    /// H→C pairing accepted; audio runs on `audio_port` of the host address.
    PairOk {
        audio_port: u16,
        session_id: u64,
    },
    /// H→C pairing rejected.
    PairFail {
        reason: String,
    },
    /// C→H trusted reconnect. This is only accepted when the host has a
    /// credential previously enrolled by this client id.
    TrustedHello {
        protocol: u32,
        device_id: String,
        device_name: String,
        device_kind: DeviceKind,
        host_id: String,
        #[serde(default)]
        transport: String,
        roles: Roles,
        sample_rate: u32,
        channels: u16,
        client_nonce: String,
    },
    /// H→C challenge for a trusted reconnect.
    TrustedChallenge {
        server_nonce: String,
        session_id: u64,
        host_id: String,
        #[serde(default)]
        host_name: String,
    },
    /// C→H proof of the enrolled credential.
    TrustedProof {
        proof: String,
    },
    /// H→C trusted authentication accepted.
    TrustedOk {},
    /// C→H negotiated session parameters. The host adopts them.
    SessionStart {
        roles: Roles,
        codec: CodecKind,
        frame_ms: u16,
        sample_rate: u32,
        channels: u16,
    },
    /// H→C: negotiated session parameters accepted and audio may start.
    SessionReady {},
    /// C→H enrollment of a credential generated after an explicit PIN
    /// pairing. The surrounding control channel is already authenticated.
    TrustEnroll {
        peer_id: String,
        secret: String,
    },
    /// H→C acknowledgement of a credential enrollment.
    TrustAccepted {},
    /// H→C authenticated refusal when this host is configured for PIN-only
    /// operation or the enrollment payload is invalid.
    TrustRejected {
        reason: String,
    },
    /// Cleartext setup for the second TCP connection used by ADB. The
    /// connection is accepted only for an already authenticated session.
    AudioHello {
        session_id: u64,
        client_nonce: String,
    },
    AudioChallenge {
        server_nonce: String,
    },
    AudioProof {
        proof: String,
    },
    AudioReady {
        proof: String,
    },
    /// Bidirectional liveness ping, every ~2 s.
    Keepalive {},
    /// C→H: begin resuming an established session after its control link
    /// dropped. `client_nonce` is fresh for this attempt and is not secret.
    ResumeHello {
        session_id: u64,
        client_nonce: String,
    },
    /// H→C: challenge bound to the session's current resume generation.
    ResumeChallenge {
        server_nonce: String,
        generation: u64,
    },
    /// C→H: proof of possession of the original session's resume secret.
    /// The proof is hex-encoded HMAC-SHA256 over the session and both nonces.
    ResumeProof {
        proof: String,
    },
    /// H→C: resume accepted; keepalives continue on this stream.
    ResumeOk {},
    /// Bidirectional informational volume/mute hint.
    ControlHint {
        #[serde(skip_serializing_if = "Option::is_none")]
        volume: Option<f32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        mute: Option<bool>,
    },
    /// Bidirectional graceful shutdown.
    Bye {
        reason: String,
    },
    /// Any message this implementation does not know. Kept so newer protocol
    /// versions do not kill the connection.
    #[serde(other)]
    Unknown,
}

/// Control frames contain bearer credentials, proofs, and session material in
/// addition to ordinary metadata. Keep their debug representation tag-only so
/// an accidental diagnostic of a parsed frame cannot disclose a secret.
impl fmt::Debug for ControlMessage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Hello { .. } => "Hello",
            Self::Challenge { .. } => "Challenge",
            Self::Pair { .. } => "Pair",
            Self::PairConfirm { .. } => "PairConfirm",
            Self::PairOk { .. } => "PairOk",
            Self::PairFail { .. } => "PairFail",
            Self::TrustedHello { .. } => "TrustedHello",
            Self::TrustedChallenge { .. } => "TrustedChallenge",
            Self::TrustedProof { .. } => "TrustedProof",
            Self::TrustedOk { .. } => "TrustedOk",
            Self::SessionStart { .. } => "SessionStart",
            Self::SessionReady { .. } => "SessionReady",
            Self::TrustEnroll { .. } => "TrustEnroll",
            Self::TrustAccepted { .. } => "TrustAccepted",
            Self::TrustRejected { .. } => "TrustRejected",
            Self::AudioHello { .. } => "AudioHello",
            Self::AudioChallenge { .. } => "AudioChallenge",
            Self::AudioProof { .. } => "AudioProof",
            Self::AudioReady { .. } => "AudioReady",
            Self::Keepalive { .. } => "Keepalive",
            Self::ResumeHello { .. } => "ResumeHello",
            Self::ResumeChallenge { .. } => "ResumeChallenge",
            Self::ResumeProof { .. } => "ResumeProof",
            Self::ResumeOk { .. } => "ResumeOk",
            Self::ControlHint { .. } => "ControlHint",
            Self::Bye { .. } => "Bye",
            Self::Unknown => "Unknown",
        };
        formatter.write_str(name)
    }
}

const HEADER_LEN: usize = 9;

fn frame_header(length: usize) -> [u8; HEADER_LEN] {
    let mut header = [0u8; HEADER_LEN];
    header[0..4].copy_from_slice(CONTROL_MAGIC);
    header[4] = PROTOCOL_VERSION;
    header[5..9].copy_from_slice(&(length as u32).to_le_bytes());
    header
}

/// Serialize one cleartext control frame (header + JSON payload).
///
/// Only the clear pairing and resume exchanges use this; authenticated
/// session traffic after key confirmation or successful resume goes through
/// [`write_sealed_frame`].
pub fn encode_frame(message: &ControlMessage) -> Result<Vec<u8>, io::Error> {
    let payload = serde_json::to_vec(message)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let mut frame = Vec::with_capacity(HEADER_LEN + payload.len());
    frame.extend_from_slice(&frame_header(payload.len()));
    frame.extend_from_slice(&payload);
    Ok(frame)
}

/// Write one control frame to a stream.
pub fn write_frame(stream: &mut impl Write, message: &ControlMessage) -> io::Result<()> {
    let frame = encode_frame(message)?;
    stream.write_all(&frame)?;
    stream.flush()
}

/// Read one control frame from a stream.
///
/// Honours any read timeout configured on the stream: a timeout surfaces as
/// `io::ErrorKind::WouldBlock`/`TimedOut` so keepalive loops can distinguish
/// idle from broken connections.
pub fn read_frame(stream: &mut impl Read) -> io::Result<ControlMessage> {
    let payload = read_body(stream)?;
    decode_message(&payload)
}

/// Read a frame body: validates the header and returns the raw payload.
fn read_body(stream: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut header = [0u8; HEADER_LEN];
    stream.read_exact(&mut header)?;
    if &header[0..4] != CONTROL_MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "control frame is missing the QPR3 magic",
        ));
    }
    let version = header[4];
    if version != PROTOCOL_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unsupported protocol version {version}"),
        ));
    }
    let length = u32::from_le_bytes(header[5..9].try_into().expect("slice is 4 bytes"));
    if length > MAX_CONTROL_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("control frame of {length} bytes exceeds the limit"),
        ));
    }
    let mut payload = vec![0u8; length as usize];
    stream.read_exact(&mut payload)?;
    Ok(payload)
}

fn decode_message(payload: &[u8]) -> io::Result<ControlMessage> {
    serde_json::from_slice(payload).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("control frame is not valid protocol JSON: {error}"),
        )
    })
}

fn crypto_io(error: crate::RelayError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

/// Write one sealed control frame. The header is authenticated as associated
/// data, so a peer cannot rewrite the length or version without detection.
pub fn write_sealed_frame(
    stream: &mut impl Write,
    sealer: &mut Sealer,
    message: &ControlMessage,
) -> io::Result<()> {
    let plaintext = serde_json::to_vec(message)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    // The header depends on the sealed length, and the seal depends on the
    // header, so the length is computed from the known AEAD expansion first.
    let header = frame_header(plaintext.len() + crate::crypto::TAG_LEN);
    let sealed = sealer.seal(&plaintext, &header).map_err(crypto_io)?;
    debug_assert_eq!(sealed.len(), plaintext.len() + crate::crypto::TAG_LEN);
    let mut frame = Vec::with_capacity(HEADER_LEN + sealed.len());
    frame.extend_from_slice(&header);
    frame.extend_from_slice(&sealed);
    stream.write_all(&frame)?;
    stream.flush()
}

/// Read one sealed control frame, rejecting anything that does not
/// authenticate or that arrives out of order.
pub fn read_sealed_frame(
    stream: &mut impl Read,
    opener: &mut Opener,
) -> io::Result<ControlMessage> {
    let mut header = [0u8; HEADER_LEN];
    stream.read_exact(&mut header)?;
    if &header[0..4] != CONTROL_MAGIC || header[4] != PROTOCOL_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "sealed control frame has a bad header",
        ));
    }
    let length = u32::from_le_bytes(header[5..9].try_into().expect("slice is 4 bytes"));
    if length > MAX_CONTROL_FRAME || (length as usize) < crate::crypto::TAG_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("sealed control frame of {length} bytes is out of range"),
        ));
    }
    let mut sealed = vec![0u8; length as usize];
    stream.read_exact(&mut sealed)?;
    let plaintext = opener
        .open_sequential(&sealed, &header)
        .map_err(crypto_io)?;
    decode_message(&plaintext)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn frame_round_trips_every_message() {
        let messages = [
            ControlMessage::Hello {
                protocol: PROTOCOL_VERSION as u32,
                device_id: "phone-id".into(),
                transport: "wifi".into(),
                device_name: "pixel".into(),
                device_kind: DeviceKind::Android,
                roles: Roles::both(),
                sample_rate: 48_000,
                channels: 1,
                pake: "aa55".into(),
            },
            ControlMessage::Challenge {
                protocol: PROTOCOL_VERSION as u32,
                pake: "00ff".into(),
                host_name: "pc".into(),
                device_id: "pc-id".into(),
            },
            ControlMessage::Pair {
                pake: "abcd".into(),
                confirm: "ef01".into(),
            },
            ControlMessage::PairConfirm {
                confirm: "2233".into(),
            },
            ControlMessage::PairOk {
                audio_port: 48123,
                session_id: 7,
            },
            ControlMessage::PairFail {
                reason: "bad pin".into(),
            },
            ControlMessage::SessionStart {
                roles: Roles::emit_only(),
                codec: CodecKind::Opus,
                frame_ms: 20,
                sample_rate: 48_000,
                channels: 1,
            },
            ControlMessage::SessionReady {},
            ControlMessage::Keepalive {},
            ControlMessage::ResumeHello {
                session_id: 42,
                client_nonce: "beef".into(),
            },
            ControlMessage::ResumeChallenge {
                server_nonce: "cafe".into(),
                generation: 2,
            },
            ControlMessage::ResumeProof {
                proof: "deadbeef".into(),
            },
            ControlMessage::ResumeOk {},
            ControlMessage::ControlHint {
                volume: Some(0.5),
                mute: None,
            },
            ControlMessage::Bye {
                reason: "done".into(),
            },
        ];
        for message in &messages {
            let frame = encode_frame(message).expect("frame encodes");
            let mut cursor = Cursor::new(frame);
            let decoded = read_frame(&mut cursor).expect("frame decodes");
            assert_eq!(&decoded, message);
        }
    }

    #[test]
    fn unknown_message_type_is_preserved() {
        let payload = br#"{"type":"future_thing","x":1}"#;
        let mut raw = Vec::new();
        raw.extend_from_slice(CONTROL_MAGIC);
        raw.push(PROTOCOL_VERSION);
        raw.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        raw.extend_from_slice(payload);
        let mut cursor = Cursor::new(raw);
        assert_eq!(read_frame(&mut cursor).unwrap(), ControlMessage::Unknown);
    }

    #[test]
    fn oversized_frames_are_rejected() {
        let mut frame = Vec::new();
        frame.extend_from_slice(CONTROL_MAGIC);
        frame.push(PROTOCOL_VERSION);
        frame.extend_from_slice(&(MAX_CONTROL_FRAME + 1).to_le_bytes());
        let mut cursor = Cursor::new(frame);
        assert!(read_frame(&mut cursor).is_err());
    }

    #[test]
    fn bad_magic_is_rejected() {
        let mut cursor = Cursor::new(b"XXXX\x02\x00\x00\x00\x00".to_vec());
        assert!(read_frame(&mut cursor).is_err());
    }

    #[test]
    fn control_debug_never_contains_bearer_material_or_proofs() {
        let message = ControlMessage::TrustEnroll {
            peer_id: "phone-installation".into(),
            secret: "ab".repeat(32),
        };
        let debug = format!("{message:?}");
        assert_eq!(debug, "TrustEnroll");
        assert!(!debug.contains("phone-installation"));

        let proof = ControlMessage::ResumeProof {
            proof: "cd".repeat(32),
        };
        assert_eq!(format!("{proof:?}"), "ResumeProof");
    }

    fn sealed_pair() -> (crate::crypto::Sealer, crate::crypto::Opener) {
        use crate::crypto::{pake_start, Side};
        let client = pake_start(Side::Client, "123456");
        let host = pake_start(Side::Host, "123456");
        let client_message = client.message.clone();
        let host_message = host.message.clone();
        let client_keys = client.finish(&host_message).expect("client pairs");
        let host_keys = host.finish(&client_message).expect("host pairs");
        let (sealer, _) = client_keys.control_channel().expect("client control keys");
        let (_, opener) = host_keys.control_channel().expect("host control keys");
        (sealer, opener)
    }

    #[test]
    fn sealed_frames_round_trip() {
        let (mut sealer, mut opener) = sealed_pair();
        let mut wire = Vec::new();
        write_sealed_frame(&mut wire, &mut sealer, &ControlMessage::Keepalive {}).unwrap();
        write_sealed_frame(
            &mut wire,
            &mut sealer,
            &ControlMessage::Bye {
                reason: "done".into(),
            },
        )
        .unwrap();
        let mut cursor = Cursor::new(wire);
        assert_eq!(
            read_sealed_frame(&mut cursor, &mut opener).unwrap(),
            ControlMessage::Keepalive {}
        );
        assert_eq!(
            read_sealed_frame(&mut cursor, &mut opener).unwrap(),
            ControlMessage::Bye {
                reason: "done".into()
            }
        );
    }

    #[test]
    fn a_tampered_sealed_frame_is_refused() {
        let (mut sealer, mut opener) = sealed_pair();
        let mut wire = Vec::new();
        write_sealed_frame(&mut wire, &mut sealer, &ControlMessage::Keepalive {}).unwrap();
        wire[HEADER_LEN] ^= 0x01;
        let mut cursor = Cursor::new(wire);
        assert!(read_sealed_frame(&mut cursor, &mut opener).is_err());
    }

    #[test]
    fn frame_durations_normalise_to_the_supported_set() {
        for frame_ms in FRAME_DURATIONS_MS {
            assert_eq!(normalize_frame_ms(frame_ms), frame_ms);
            assert!(is_supported_frame_ms(frame_ms));
        }
        // Values a hand-edited config could hold snap to a real duration
        // instead of failing at the far end of a handshake.
        assert_eq!(normalize_frame_ms(7), 5);
        assert_eq!(normalize_frame_ms(13), 10);
        assert_eq!(normalize_frame_ms(35), 40);
        assert_eq!(normalize_frame_ms(0), 5);
        assert_eq!(normalize_frame_ms(9_000), 60);
        assert!(!is_supported_frame_ms(7));
    }

    #[test]
    fn codec_ids_round_trip() {
        assert_eq!(
            CodecKind::from_id(CodecKind::Pcm.id()),
            Some(CodecKind::Pcm)
        );
        assert_eq!(
            CodecKind::from_id(CodecKind::Opus.id()),
            Some(CodecKind::Opus)
        );
        assert_eq!(CodecKind::from_id(9), None);
    }
}
