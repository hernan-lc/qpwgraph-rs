//! UDP audio-channel framing and the receive-side jitter buffer.
//!
//! Audio packet layout (little-endian, 20-byte header):
//!
//! ```text
//! offset 0  u16  magic 0xA1E5
//! offset 2  u8   version (low nibble) | flags (high nibble)
//!                flag 0x10 = stereo, flag 0x20 = keyframe
//! offset 3  u8   codec id (0 = f32 LE PCM, 1 = Opus)
//! offset 4  u32  sequence number, one per frame
//! offset 8  u32  sender timestamp in milliseconds
//! offset 12 u64  AEAD nonce counter, strictly increasing per sender
//! offset 20 ..   ChaCha20-Poly1305 ciphertext of one encoded frame,
//!                followed by its 16-byte authentication tag
//! ```
//!
//! The header travels in the clear because the receiver needs the nonce
//! counter before it can decrypt, but it is authenticated as associated
//! data: flipping a single header bit makes the packet fail to open. A
//! datagram that does not open is dropped without any side effect at all —
//! in particular it can never move the peer's audio address, which is what
//! made the previous unauthenticated format hijackable by anyone who could
//! reach the port.
//!
//! An *announce* packet carries an empty plaintext (so on the wire it is a
//! bare tag): a client sends one right after pairing so the host learns its
//! UDP address. Because it is sealed with the session key, only the paired
//! peer can teach us an address.

use crate::crypto::{Opener, Sealer, TAG_LEN};
use crate::protocol::CodecKind;
use crate::RelayError;
use std::collections::BTreeMap;

pub const AUDIO_MAGIC: u16 = 0xA1E5;
pub const AUDIO_HEADER_LEN: usize = 20;
pub const AUDIO_VERSION: u8 = 2;
const FLAG_STEREO: u8 = 0x10;
const FLAG_KEYFRAME: u8 = 0x20;

/// A parsed audio packet header plus the sealed body that follows it.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioPacket<'a> {
    pub stereo: bool,
    pub keyframe: bool,
    pub codec: CodecKind,
    pub sequence: u32,
    pub timestamp_ms: u32,
    /// AEAD nonce counter chosen by the sender.
    pub counter: u64,
    /// Ciphertext plus tag; open it with the session's [`Opener`].
    pub sealed: &'a [u8],
    /// The 20 header bytes, authenticated as associated data.
    header: [u8; AUDIO_HEADER_LEN],
}

impl<'a> AudioPacket<'a> {
    /// Parse a datagram. Returns `None` when the magic, version, or codec id
    /// is unknown, or when the body is too short to hold an AEAD tag — such
    /// datagrams are dropped silently.
    pub fn parse(datagram: &'a [u8]) -> Option<Self> {
        if datagram.len() < AUDIO_HEADER_LEN + TAG_LEN {
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
        let mut header = [0u8; AUDIO_HEADER_LEN];
        header.copy_from_slice(&datagram[..AUDIO_HEADER_LEN]);
        Some(Self {
            stereo: version_flags & FLAG_STEREO != 0,
            keyframe: version_flags & FLAG_KEYFRAME != 0,
            codec,
            sequence: u32::from_le_bytes(datagram[4..8].try_into().ok()?),
            timestamp_ms: u32::from_le_bytes(datagram[8..12].try_into().ok()?),
            counter: u64::from_le_bytes(datagram[12..20].try_into().ok()?),
            sealed: &datagram[AUDIO_HEADER_LEN..],
            header,
        })
    }

    /// Authenticate and decrypt the body, rejecting replays.
    ///
    /// Everything downstream — the peer-address update, the jitter buffer,
    /// the decoder — runs only after this succeeds, so an unauthenticated
    /// datagram costs one failed AEAD open and nothing more.
    pub fn open(&self, opener: &mut Opener) -> Result<Vec<u8>, RelayError> {
        opener.open_windowed(self.counter, self.sealed, &self.header)
    }
}

/// Header fields a sender fills in before sealing the frame.
#[derive(Clone, Copy, Debug)]
pub struct AudioHeader {
    pub stereo: bool,
    pub keyframe: bool,
    pub codec: CodecKind,
    pub sequence: u32,
    pub timestamp_ms: u32,
}

fn header_bytes(header: &AudioHeader, counter: u64) -> [u8; AUDIO_HEADER_LEN] {
    let mut flags = AUDIO_VERSION;
    if header.stereo {
        flags |= FLAG_STEREO;
    }
    if header.keyframe {
        flags |= FLAG_KEYFRAME;
    }
    let mut bytes = [0u8; AUDIO_HEADER_LEN];
    bytes[0..2].copy_from_slice(&AUDIO_MAGIC.to_le_bytes());
    bytes[2] = flags;
    bytes[3] = header.codec.id();
    bytes[4..8].copy_from_slice(&header.sequence.to_le_bytes());
    bytes[8..12].copy_from_slice(&header.timestamp_ms.to_le_bytes());
    bytes[12..20].copy_from_slice(&counter.to_le_bytes());
    bytes
}

/// Seal one frame into a datagram: cleartext header, then the encrypted and
/// authenticated payload.
pub fn seal_datagram(
    sealer: &mut Sealer,
    header: &AudioHeader,
    payload: &[u8],
) -> Result<Vec<u8>, RelayError> {
    let counter = sealer.next_counter();
    let bytes = header_bytes(header, counter);
    let sealed = sealer.seal(payload, &bytes)?;
    let mut datagram = Vec::with_capacity(AUDIO_HEADER_LEN + sealed.len());
    datagram.extend_from_slice(&bytes);
    datagram.extend_from_slice(&sealed);
    Ok(datagram)
}

/// Build an announce packet used to teach the peer our UDP address. Its
/// plaintext is empty, so only its authentication tag rides the wire.
pub fn announce_packet(sealer: &mut Sealer, codec: CodecKind) -> Result<Vec<u8>, RelayError> {
    seal_datagram(
        sealer,
        &AudioHeader {
            stereo: false,
            keyframe: true,
            codec,
            sequence: u32::MAX,
            timestamp_ms: 0,
        },
        &[],
    )
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

/// How far ahead of the next expected sequence a frame may claim to be.
///
/// A jitter buffer is a reordering window, not a store: at a 10 ms frame this
/// is 640 ms, which is already far past the point where the audio would be
/// useful. Bounding it matters because the sequence number is attacker-chosen
/// within a session, and an unbounded forward window turns a stream of
/// distinct far-future sequences into unbounded memory growth plus an
/// ever-more-expensive scan on every pop.
pub const MAX_FORWARD_FRAMES: u32 = 64;
/// Hard cap on queued frames, independent of their sequence spread. Reached
/// only when the consumer has stopped draining, in which case the oldest
/// frames are the ones worth discarding.
pub const MAX_QUEUED_FRAMES: usize = 128;

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
    /// Frames refused for claiming a sequence outside the reordering window,
    /// or evicted because the queue was already full.
    pub frames_dropped_far: u64,
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
            frames_dropped_far: 0,
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

    /// Insert a frame. Returns `false` when it was dropped as late, as a
    /// duplicate, or as implausibly far ahead of the stream.
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
            if distance as u32 > MAX_FORWARD_FRAMES {
                // Too far ahead to ever be played in order. Accepting it would
                // let a peer grow the queue without bound.
                self.frames_dropped_far += 1;
                return false;
            }
        } else if self.queued.is_empty() && self.anchor.is_none() {
            self.anchor = Some(sequence);
        }
        let inserted = self.queued.insert(sequence, payload).is_none();
        // Backstop for the not-yet-primed case, where there is no `next` to
        // measure a forward distance against.
        while self.queued.len() > MAX_QUEUED_FRAMES {
            let oldest = *self.queued.keys().next().expect("queue is not empty");
            self.queued.remove(&oldest);
            self.frames_dropped_far += 1;
        }
        inserted
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
                (0..=MAX_FORWARD_FRAMES as i32).contains(&distance)
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

    use crate::crypto::{pake_start, Side};

    /// A sealed sender/receiver pair over a shared PIN, as the session
    /// workers hold.
    fn channel() -> (crate::crypto::Sealer, crate::crypto::Opener) {
        let client = pake_start(Side::Client, "123456");
        let host = pake_start(Side::Host, "123456");
        let client_message = client.message.clone();
        let host_message = host.message.clone();
        let client_keys = client.finish(&host_message).expect("client pairs");
        let host_keys = host.finish(&client_message).expect("host pairs");
        let (sealer, _) = client_keys.audio_channel().expect("client audio keys");
        let (_, opener) = host_keys.audio_channel().expect("host audio keys");
        (sealer, opener)
    }

    fn packet(sealer: &mut crate::crypto::Sealer, sequence: u32, payload: &[u8]) -> Vec<u8> {
        seal_datagram(
            sealer,
            &AudioHeader {
                stereo: false,
                keyframe: sequence == 0,
                codec: CodecKind::Opus,
                sequence,
                timestamp_ms: sequence * 20,
            },
            payload,
        )
        .expect("frame seals")
    }

    #[test]
    fn packet_round_trip() {
        let (mut sealer, mut opener) = channel();
        let datagram = packet(&mut sealer, 7, &[9, 9]);
        let parsed = AudioPacket::parse(&datagram).expect("parses");
        assert_eq!(parsed.sequence, 7);
        assert_eq!(parsed.timestamp_ms, 140);
        assert_eq!(parsed.codec, CodecKind::Opus);
        assert!(!parsed.stereo);
        assert_eq!(parsed.open(&mut opener).expect("opens"), vec![9, 9]);
    }

    #[test]
    fn rejects_bad_magic_version_and_codec() {
        let (mut sealer, _) = channel();
        let datagram = packet(&mut sealer, 1, &[0]);
        assert!(AudioPacket::parse(&datagram).is_some());
        let mut bad = datagram.clone();
        bad[0] = 0x00;
        assert!(AudioPacket::parse(&bad).is_none());
        let mut bad = datagram.clone();
        bad[2] = 3; // unknown version nibble
        assert!(AudioPacket::parse(&bad).is_none());
        let mut bad = datagram.clone();
        bad[3] = 42; // unknown codec id
        assert!(AudioPacket::parse(&bad).is_none());
        // A body too short to even hold a tag is not a packet.
        assert!(AudioPacket::parse(&datagram[..AUDIO_HEADER_LEN + 4]).is_none());
    }

    #[test]
    fn a_packet_from_the_wrong_key_never_opens() {
        // The attack this replaces: anyone who could reach the UDP port used
        // to be able to inject audio and move the peer address.
        let (mut sealer, _) = channel();
        let (_, mut other_opener) = channel();
        let datagram = packet(&mut sealer, 0, &[1, 2, 3]);
        let parsed = AudioPacket::parse(&datagram).expect("parses");
        assert!(parsed.open(&mut other_opener).is_err());
    }

    #[test]
    fn a_tampered_header_fails_authentication() {
        let (mut sealer, mut opener) = channel();
        let mut datagram = packet(&mut sealer, 5, &[7]);
        datagram[4] ^= 0x01; // rewrite the sequence number
        let parsed = AudioPacket::parse(&datagram).expect("still parses");
        assert!(parsed.open(&mut opener).is_err());
    }

    #[test]
    fn announce_packet_carries_an_empty_plaintext() {
        let (mut sealer, mut opener) = channel();
        let datagram = announce_packet(&mut sealer, CodecKind::Opus).expect("announce seals");
        let parsed = AudioPacket::parse(&datagram).expect("parses");
        assert!(parsed.keyframe);
        assert!(parsed.open(&mut opener).expect("opens").is_empty());
    }

    #[test]
    fn far_future_sequences_cannot_grow_the_queue() {
        let mut buffer = JitterBuffer::new(1);
        assert!(buffer.push(0, vec![0]));
        assert!(buffer.push(1, vec![1]));
        assert!(matches!(buffer.pop(), JitterPop::Frame(_)));
        // A peer that sprays distinct far-future sequence numbers must not be
        // able to make the buffer hold them.
        for sequence in 0..10_000u32 {
            buffer.push(1_000_000 + sequence, vec![0; 64]);
        }
        assert!(
            buffer.queue_len() <= MAX_QUEUED_FRAMES,
            "queue grew to {}",
            buffer.queue_len()
        );
        assert!(buffer.frames_dropped_far > 0);
    }

    #[test]
    fn an_unprimed_buffer_is_capped_too() {
        let mut buffer = JitterBuffer::new(4);
        for sequence in 0..10_000u32 {
            buffer.push(sequence, vec![0; 64]);
        }
        assert!(buffer.queue_len() <= MAX_QUEUED_FRAMES);
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
