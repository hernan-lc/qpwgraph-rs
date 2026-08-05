//! Relay control-channel protocol (version 1).
//!
//! The control channel is a TCP byte stream of length-prefixed JSON frames:
//!
//! ```text
//! magic "QPR1" (4 bytes) | version u8 | payload length u32 LE | JSON payload
//! ```
//!
//! JSON keeps third-party implementations trivial. Unknown message types are
//! preserved as `Unknown` so newer peers can extend the protocol without
//! breaking older ones. The full wire specification lives in
//! `docs/relay-protocol.md`.

use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};

pub const CONTROL_MAGIC: &[u8; 4] = b"QPR1";
pub const PROTOCOL_VERSION: u8 = 1;
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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlMessage {
    /// C→H first message: who is calling and what they can do.
    Hello {
        protocol: u32,
        device_name: String,
        device_kind: DeviceKind,
        roles: Roles,
        sample_rate: u32,
        channels: u16,
    },
    /// H→C pairing challenge. `salt` is random hex, fresh per attempt.
    Challenge {
        protocol: u32,
        salt: String,
        host_name: String,
    },
    /// C→H proof of the shared PIN: `HMAC-SHA256(key = PIN, msg = salt)`, hex.
    Pair { digest: String },
    /// H→C pairing accepted; audio runs on `audio_port` of the host address.
    PairOk { audio_port: u16, session_id: u64 },
    /// H→C pairing rejected.
    PairFail { reason: String },
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
    /// Bidirectional liveness ping, every ~2 s.
    Keepalive {},
    /// C→H: request to resume an established session after the control link
    /// dropped (e.g. the Wi-Fi link roamed). The host replies with a fresh
    /// [`ControlMessage::Challenge`]; the client answers with
    /// [`ControlMessage::Pair`]; the host then accepts with
    /// [`ControlMessage::ResumeOk`] or rejects with
    /// [`ControlMessage::PairFail`]. Audio keeps flowing on UDP meanwhile.
    Resume { session_id: u64 },
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
    Bye { reason: String },
    /// Any message this implementation does not know. Kept so newer protocol
    /// versions do not kill the connection.
    #[serde(other)]
    Unknown,
}

/// Serialize one control frame (header + JSON payload).
pub fn encode_frame(message: &ControlMessage) -> Result<Vec<u8>, io::Error> {
    let payload = serde_json::to_vec(message)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let mut frame = Vec::with_capacity(9 + payload.len());
    frame.extend_from_slice(CONTROL_MAGIC);
    frame.push(PROTOCOL_VERSION);
    frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
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
    let mut header = [0u8; 9];
    stream.read_exact(&mut header)?;
    if &header[0..4] != CONTROL_MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "control frame is missing the QPR1 magic",
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
    serde_json::from_slice(&payload).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("control frame is not valid protocol JSON: {error}"),
        )
    })
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
                device_name: "pixel".into(),
                device_kind: DeviceKind::Android,
                roles: Roles::both(),
                sample_rate: 48_000,
                channels: 1,
            },
            ControlMessage::Challenge {
                protocol: PROTOCOL_VERSION as u32,
                salt: "00ff".into(),
                host_name: "pc".into(),
            },
            ControlMessage::Pair {
                digest: "abcd".into(),
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
            ControlMessage::Resume { session_id: 42 },
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
        let mut cursor = Cursor::new(b"XXXX\x01\x00\x00\x00\x00".to_vec());
        assert!(read_frame(&mut cursor).is_err());
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
