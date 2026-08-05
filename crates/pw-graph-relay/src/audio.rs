//! UDP audio-channel framing and the receive-side jitter buffer.
//!
//! Audio packet layout (little-endian, 12-byte header):
//!
//! ```text
//! offset 0  u16  magic 0xA1E5
//! offset 2  u8   version (low nibble) | flags (high nibble)
//!                flag 0x10 = stereo, flag 0x20 = keyframe
//! offset 3  u8   codec id (0 = f32 LE PCM, 1 = Opus)
//! offset 4  u32  sequence number, one per frame
//! offset 8  u32  sender timestamp in milliseconds
//! offset 12 ..  one encoded frame (Opus packet or interleaved f32 PCM)
//! ```
//!
//! An empty payload marks an *announce* packet: a client sends one right
//! after pairing so the host learns its UDP address before any real audio
//! flows. Receivers record the address and ignore the payload.

use crate::protocol::CodecKind;
use std::collections::BTreeMap;

pub const AUDIO_MAGIC: u16 = 0xA1E5;
pub const AUDIO_HEADER_LEN: usize = 12;
pub const AUDIO_VERSION: u8 = 1;
const FLAG_STEREO: u8 = 0x10;
const FLAG_KEYFRAME: u8 = 0x20;

/// A parsed audio packet header plus its payload.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioPacket<'a> {
    pub stereo: bool,
    pub keyframe: bool,
    pub codec: CodecKind,
    pub sequence: u32,
    pub timestamp_ms: u32,
    pub payload: &'a [u8],
}

impl<'a> AudioPacket<'a> {
    /// Parse a datagram. Returns `None` when the magic, version, or codec id
    /// is unknown — such datagrams are dropped silently.
    pub fn parse(datagram: &'a [u8]) -> Option<Self> {
        if datagram.len() < AUDIO_HEADER_LEN {
            return None;
        }
        let magic = u16::from_le_bytes(datagram[0..2].try_into().ok()?);
        if magic != AUDIO_MAGIC {
            return None;
        }
        let version_flags = datagram[2];
        if version_flags & 0x0F != AUDIO_VERSION {
            return None;
        }
        let codec = CodecKind::from_id(datagram[3])?;
        Some(Self {
            stereo: version_flags & FLAG_STEREO != 0,
            keyframe: version_flags & FLAG_KEYFRAME != 0,
            codec,
            sequence: u32::from_le_bytes(datagram[4..8].try_into().ok()?),
            timestamp_ms: u32::from_le_bytes(datagram[8..12].try_into().ok()?),
            payload: &datagram[AUDIO_HEADER_LEN..],
        })
    }

    pub fn is_announce(&self) -> bool {
        self.payload.is_empty()
    }

    /// Serialize header + payload into a datagram.
    pub fn to_datagram(&self) -> Vec<u8> {
        let mut flags = AUDIO_VERSION;
        if self.stereo {
            flags |= FLAG_STEREO;
        }
        if self.keyframe {
            flags |= FLAG_KEYFRAME;
        }
        let mut datagram = Vec::with_capacity(AUDIO_HEADER_LEN + self.payload.len());
        datagram.extend_from_slice(&AUDIO_MAGIC.to_le_bytes());
        datagram.push(flags);
        datagram.push(self.codec.id());
        datagram.extend_from_slice(&self.sequence.to_le_bytes());
        datagram.extend_from_slice(&self.timestamp_ms.to_le_bytes());
        datagram.extend_from_slice(self.payload);
        datagram
    }
}

/// Build an announce packet used to teach the peer our UDP address.
pub fn announce_packet(codec: CodecKind) -> Vec<u8> {
    AudioPacket {
        stereo: false,
        keyframe: true,
        codec,
        sequence: u32::MAX,
        timestamp_ms: 0,
        payload: &[],
    }
    .to_datagram()
}

/// Result of asking the jitter buffer for the next frame.
#[derive(Debug, PartialEq)]
pub enum JitterPop {
    /// Still collecting the initial buffering depth; nothing to play yet.
    Buffering,
    /// The next in-order frame.
    Frame(Vec<u8>),
    /// The next frame never arrived (or arrived too late); conceal it.
    Lost,
}

/// Smallest reorder tolerance: one frame must be queued past a gap before
/// the gap counts as a loss.
const MIN_LOOKAHEAD: usize = 1;
/// Largest reorder tolerance. Each step costs one frame of concealment delay
/// on a genuinely lost packet, so the ceiling stays low.
const MAX_LOOKAHEAD: usize = 4;
/// In-order frames that must pass before the tolerance relaxes by one step.
/// At a 10 ms frame this is about two and a half seconds of clean audio.
const LOOKAHEAD_DECAY_POPS: u32 = 250;

/// A small reordering/loss buffer keyed by sequence number.
///
/// Playback starts once `depth_frames` packets are queued, then advances one
/// sequence number per pop. Late packets are dropped; missing ones surface as
/// [`JitterPop::Lost`] so the decoder can run packet-loss concealment.
///
/// Sequence numbers wrap, so "earliest" is defined relative to an anchor:
/// the first packet that arrived while the buffer was empty, refined by
/// [`JitterBuffer::set_anchor`] when the sender marks a keyframe (its first
/// frame). This keeps priming correct across the u32 wraparound.
///
/// # Adaptive reorder tolerance
///
/// A gap is only called lost once `lookahead` later frames are queued behind
/// it. That tolerance is not fixed: it grows when the network actually
/// delivers packets late, and decays back to [`MIN_LOOKAHEAD`] after a long
/// clean run. A clean link therefore pays no reordering delay at all, while a
/// jittery one buys tolerance only for as long as it needs it — the reason to
/// adapt rather than pick one conservative constant for every network.
pub struct JitterBuffer {
    queued: BTreeMap<u32, Vec<u8>>,
    next_sequence: Option<u32>,
    primed: bool,
    anchor: Option<u32>,
    depth_frames: usize,
    lookahead: usize,
    clean_pops: u32,
    pub frames_received: u64,
    pub frames_dropped_late: u64,
    pub frames_lost: u64,
}

impl JitterBuffer {
    pub fn new(depth_frames: usize) -> Self {
        Self {
            queued: BTreeMap::new(),
            next_sequence: None,
            primed: false,
            anchor: None,
            depth_frames: depth_frames.max(1),
            lookahead: MIN_LOOKAHEAD,
            clean_pops: 0,
            frames_received: 0,
            frames_dropped_late: 0,
            frames_lost: 0,
        }
    }

    /// Current reorder tolerance in frames.
    pub fn lookahead(&self) -> usize {
        self.lookahead
    }

    pub fn queue_len(&self) -> usize {
        self.queued.len()
    }

    /// Tell the buffer which queued sequence starts the stream. Senders mark
    /// their very first frame, so this is the most reliable ordering hint.
    /// Ignored once playback has started.
    pub fn set_anchor(&mut self, sequence: u32) {
        if !self.primed {
            self.anchor = Some(sequence);
        }
    }

    /// Insert a frame. Returns `false` when it was dropped as late or
    /// duplicate.
    pub fn push(&mut self, sequence: u32, payload: Vec<u8>) -> bool {
        self.frames_received += 1;
        if let Some(next) = self.next_sequence {
            // Signed distance handles the u32 wraparound for any realistic
            // session length.
            let distance = sequence.wrapping_sub(next) as i32;
            if distance < 0 {
                self.frames_dropped_late += 1;
                // A frame that arrived after its slot passed is direct
                // evidence the current tolerance is too tight for this link.
                self.lookahead = (self.lookahead + 1).min(MAX_LOOKAHEAD);
                self.clean_pops = 0;
                return false;
            }
        } else if self.queued.is_empty() && self.anchor.is_none() {
            self.anchor = Some(sequence);
        }
        self.queued.insert(sequence, payload).is_none()
    }

    /// Take the next playback frame in sequence order.
    pub fn pop(&mut self) -> JitterPop {
        if self.queued.is_empty() {
            return JitterPop::Buffering;
        }

        if !self.primed {
            if self.queued.len() < self.depth_frames {
                return JitterPop::Buffering;
            }
            self.primed = true;
            let anchor = self
                .anchor
                .unwrap_or_else(|| *self.queued.keys().next().expect("queue is not empty"));
            // Earliest queued frame measured forward from the anchor; this
            // stays correct when sequences wrap around u32::MAX.
            let start = *self
                .queued
                .keys()
                .min_by_key(|sequence| u32::wrapping_sub(**sequence, anchor))
                .expect("queue is not empty");
            // Prune anything queued behind the start point or implausibly
            // far ahead; it cannot be played in order anymore.
            self.queued.retain(|sequence, _| {
                let distance = sequence.wrapping_sub(start) as i32;
                (0..1_000_000).contains(&distance)
            });
            self.next_sequence = Some(start);
        }

        let next = self.next_sequence.expect("primed buffers track a sequence");
        if let Some(payload) = self.queued.remove(&next) {
            self.next_sequence = Some(next.wrapping_add(1));
            self.note_clean_pop();
            return JitterPop::Frame(payload);
        }

        // The expected frame is missing. Only declare it lost once enough
        // later frames are queued; until then it may simply still be in
        // flight, and the caller should try again on the next datagram.
        let queued_ahead = self
            .queued
            .keys()
            .filter(|sequence| (sequence.wrapping_sub(next) as i32) > 0)
            .count();
        if queued_ahead >= self.lookahead {
            self.next_sequence = Some(next.wrapping_add(1));
            self.frames_lost += 1;
            self.clean_pops = 0;
            JitterPop::Lost
        } else {
            JitterPop::Buffering
        }
    }

    /// Relax the reorder tolerance one step after a long clean run, so a
    /// brief burst of jitter does not leave the stream permanently delayed.
    fn note_clean_pop(&mut self) {
        if self.lookahead <= MIN_LOOKAHEAD {
            return;
        }
        self.clean_pops += 1;
        if self.clean_pops >= LOOKAHEAD_DECAY_POPS {
            self.lookahead -= 1;
            self.clean_pops = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(sequence: u32, payload: &[u8]) -> Vec<u8> {
        AudioPacket {
            stereo: false,
            keyframe: sequence == 0,
            codec: CodecKind::Opus,
            sequence,
            timestamp_ms: sequence * 20,
            payload,
        }
        .to_datagram()
    }

    #[test]
    fn packet_round_trip() {
        let datagram = packet(7, &[9, 9]);
        let parsed = AudioPacket::parse(&datagram).expect("parses");
        assert_eq!(parsed.sequence, 7);
        assert_eq!(parsed.timestamp_ms, 140);
        assert_eq!(parsed.payload, &[9, 9]);
        assert_eq!(parsed.codec, CodecKind::Opus);
        assert!(!parsed.stereo);
    }

    #[test]
    fn rejects_bad_magic_version_and_codec() {
        let datagram = packet(1, &[0]);
        assert!(AudioPacket::parse(&datagram).is_some());
        let mut bad = datagram.clone();
        bad[0] = 0x00;
        assert!(AudioPacket::parse(&bad).is_none());
        let mut bad = datagram.clone();
        bad[2] = 2; // unknown version nibble
        assert!(AudioPacket::parse(&bad).is_none());
        let mut bad = datagram.clone();
        bad[3] = 42; // unknown codec id
        assert!(AudioPacket::parse(&bad).is_none());
        assert!(AudioPacket::parse(&datagram[..6]).is_none());
    }

    #[test]
    fn announce_packet_has_empty_payload() {
        let datagram = announce_packet(CodecKind::Opus);
        let parsed = AudioPacket::parse(&datagram).expect("parses");
        assert!(parsed.is_announce());
        assert!(parsed.keyframe);
    }

    #[test]
    fn jitter_buffer_buffers_then_streams_in_order() {
        let mut buffer = JitterBuffer::new(2);
        assert!(buffer.push(0, vec![0]));
        assert_eq!(buffer.pop(), JitterPop::Buffering);
        assert!(buffer.push(1, vec![1]));
        assert_eq!(buffer.pop(), JitterPop::Frame(vec![0]));
        assert_eq!(buffer.pop(), JitterPop::Frame(vec![1]));
    }

    #[test]
    fn jitter_buffer_tolerates_reordering() {
        let mut buffer = JitterBuffer::new(3);
        assert!(buffer.push(2, vec![2]));
        assert!(buffer.push(0, vec![0]));
        // Senders mark their first frame; the receiver uses it as the start
        // hint even when it arrives out of order.
        buffer.set_anchor(0);
        assert!(buffer.push(1, vec![1]));
        assert_eq!(buffer.pop(), JitterPop::Frame(vec![0]));
        assert_eq!(buffer.pop(), JitterPop::Frame(vec![1]));
        assert_eq!(buffer.pop(), JitterPop::Frame(vec![2]));
    }

    #[test]
    fn jitter_buffer_reports_loss_and_drops_late() {
        let mut buffer = JitterBuffer::new(1);
        assert!(buffer.push(0, vec![0]));
        assert!(buffer.push(2, vec![2]));
        assert_eq!(buffer.pop(), JitterPop::Frame(vec![0]));
        assert_eq!(buffer.pop(), JitterPop::Lost);
        assert_eq!(buffer.pop(), JitterPop::Frame(vec![2]));
        // A late duplicate of frame 1 is dropped, not replayed.
        assert!(!buffer.push(1, vec![1]));
        assert_eq!(buffer.frames_dropped_late, 1);
    }

    #[test]
    fn lookahead_starts_minimal_and_grows_on_late_arrivals() {
        let mut buffer = JitterBuffer::new(1);
        assert_eq!(buffer.lookahead(), MIN_LOOKAHEAD);
        assert!(buffer.push(0, vec![0]));
        assert!(buffer.push(2, vec![2]));
        assert_eq!(buffer.pop(), JitterPop::Frame(vec![0]));
        // One frame queued past the gap is enough at the minimum tolerance.
        assert_eq!(buffer.pop(), JitterPop::Lost);
        assert_eq!(buffer.pop(), JitterPop::Frame(vec![2]));
        // Frame 1 finally shows up too late; the link has proven it reorders.
        assert!(!buffer.push(1, vec![1]));
        assert_eq!(buffer.lookahead(), MIN_LOOKAHEAD + 1);
    }

    #[test]
    fn a_grown_lookahead_waits_for_more_evidence_before_declaring_loss() {
        let mut buffer = JitterBuffer::new(1);
        assert!(buffer.push(0, vec![0]));
        assert!(buffer.push(2, vec![2]));
        assert_eq!(buffer.pop(), JitterPop::Frame(vec![0]));
        assert_eq!(buffer.pop(), JitterPop::Lost);
        assert_eq!(buffer.pop(), JitterPop::Frame(vec![2]));
        assert!(!buffer.push(1, vec![1]));
        assert_eq!(buffer.lookahead(), 2);

        // With tolerance 2, a single frame past the gap no longer triggers
        // concealment: frame 4 alone leaves frame 3 a chance to arrive.
        assert!(buffer.push(4, vec![4]));
        assert_eq!(buffer.pop(), JitterPop::Buffering);
        assert!(buffer.push(3, vec![3]));
        assert_eq!(buffer.pop(), JitterPop::Frame(vec![3]));
        assert_eq!(buffer.pop(), JitterPop::Frame(vec![4]));
        assert_eq!(
            buffer.frames_lost, 1,
            "frame 3 was recovered, not concealed"
        );
    }

    #[test]
    fn lookahead_decays_after_a_clean_run() {
        let mut buffer = JitterBuffer::new(1);
        assert!(buffer.push(0, vec![0]));
        assert!(buffer.push(2, vec![2]));
        assert_eq!(buffer.pop(), JitterPop::Frame(vec![0]));
        assert_eq!(buffer.pop(), JitterPop::Lost);
        assert_eq!(buffer.pop(), JitterPop::Frame(vec![2]));
        assert!(!buffer.push(1, vec![1]));
        assert_eq!(buffer.lookahead(), 2);

        for sequence in 3..(3 + LOOKAHEAD_DECAY_POPS) {
            assert!(buffer.push(sequence, vec![0]));
            assert!(matches!(buffer.pop(), JitterPop::Frame(_)));
        }
        assert_eq!(
            buffer.lookahead(),
            MIN_LOOKAHEAD,
            "a clean link must not stay penalised forever"
        );
    }

    #[test]
    fn sequence_wraparound_keeps_ordering() {
        let mut buffer = JitterBuffer::new(1);
        let near_max = u32::MAX - 1;
        assert!(buffer.push(near_max, vec![1]));
        assert!(buffer.push(near_max.wrapping_add(1), vec![2]));
        assert!(buffer.push(near_max.wrapping_add(2), vec![3]));
        assert_eq!(buffer.pop(), JitterPop::Frame(vec![1]));
        assert_eq!(buffer.pop(), JitterPop::Frame(vec![2]));
        assert_eq!(buffer.pop(), JitterPop::Frame(vec![3]));
    }
}
