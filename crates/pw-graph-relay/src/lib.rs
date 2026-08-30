//! `pw-graph-relay` — networked audio relay engine.
//!
//! This crate implements the PC side of an AudioRelay-style feature: audio
//! captured on a phone (or any peer) can be *emitted* to this machine, and
//! audio played here can be *received* and rendered by a peer. Transport runs
//! over the local network (Wi-Fi LAN, USB tethering, or Bluetooth PAN) with
//! Opus-compressed UDP audio and a JSON-over-TCP control channel.
//!
//! The crate is UI-free and PipeWire-free so it also compiles for Android.
//! [`RelayEngine`] owns all sockets and worker threads; a cheap, cloneable
//! [`RelayHandle`] exposes commands and audio push/pull endpoints.
//!
//! Wire protocol: `docs/relay-protocol.md`.

pub mod audio;
pub mod codec;
pub mod convert;
pub mod crypto;
pub mod discovery;
pub mod netlink;
pub mod pairing;
pub mod protocol;
pub mod qr;
pub mod usb_probe;

mod queue;
mod realtime;
mod session;

pub use codec::AudioFormat;
pub use convert::Converter;
pub use crypto::{Opener, Sealer};
pub use netlink::{LinkKind, LocalLink, TransportPreference};
pub use protocol::{
    is_supported_channels, is_supported_frame_ms, is_supported_sample_rate, normalize_frame_ms,
    CodecKind, DeviceKind, Roles, FRAME_DURATIONS_MS, MAX_CHANNELS, MAX_SAMPLE_RATE_HZ,
    SAMPLE_RATES_HZ,
};
pub use queue::{PcmQueue, CAPTURE_DEPTH_FRAMES, DEFAULT_QUEUE_CAPACITY, PLAYBACK_DEPTH_FRAMES};

/// The largest buffer a realtime audio callback may hand to
/// [`RelayHandle::try_pull_playback`] or [`RelayHandle::try_push_capture`],
/// in samples.
///
/// Everything the realtime path might otherwise have to grow — the mixing
/// scratch, each session's conversion buffers — is sized from this at setup
/// time. A callback presenting more than this gets served only this much
/// rather than triggering an allocation on the audio thread.
pub const MAX_REALTIME_QUANTUM_SAMPLES: usize = 16_384;

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use thiserror::Error;

/// Upper bound on queued events.
///
/// The queue used to be unbounded, which made it a memory-growth path: a peer
/// sending malformed audio produced one error event per datagram, and a UI
/// that drains once per frame could never keep up with a flood. Dropping the
/// oldest events is right — a consumer that has fallen this far behind wants
/// the recent state of the world, not a backlog.
pub const MAX_QUEUED_EVENTS: usize = 256;

/// Failed pairings one source address may make before it is locked out.
pub const PAIRING_ATTEMPT_LIMIT: u32 = 5;
/// How long a source stays locked out. With a PAKE, guessing a six-digit PIN
/// is an online-only game; at five tries per lockout it would take centuries.
pub const PAIRING_LOCKOUT: Duration = Duration::from_secs(60);
/// Maximum number of source addresses retained in the pairing rate limiter.
///
/// This is deliberately a hard cap, rather than a cleanup threshold: a flood
/// of distinct source addresses must not turn the limiter itself into an
/// unbounded allocation.
const MAX_PAIRING_FAILURE_RECORDS: usize = 1024;

/// Pairing failures recorded against one source address.
struct FailureRecord {
    count: u32,
    locked_until: Instant,
    last_seen: Instant,
}

#[derive(Debug, Error)]
pub enum RelayError {
    #[error("relay I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("relay protocol error: {0}")]
    Protocol(String),
    #[error("relay codec error: {0}")]
    Codec(String),
    #[error("relay engine error: {0}")]
    Engine(String),
    /// A caller-supplied configuration that could never work — an audio
    /// geometry outside the negotiable set, say. Distinguished from
    /// [`Self::Protocol`] because nothing was ever put on the wire: the
    /// mistake is local and the caller can fix it directly.
    #[error("relay configuration error: {0}")]
    Config(String),
}

pub type RelayResult<T> = Result<T, RelayError>;

/// Identifier for one live relay session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionId(pub u64);

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "session-{}", self.0)
    }
}

/// A remote device we know about, discovered or connected.
#[derive(Clone, Debug, PartialEq)]
pub struct PeerInfo {
    pub name: String,
    pub kind: DeviceKind,
    pub addr: SocketAddr,
}

/// Events drained by the host application (typically once per UI frame).
#[derive(Clone, Debug, PartialEq)]
pub enum RelayEvent {
    HostStarted {
        port: u16,
    },
    HostStopped,
    /// A relay host appeared on the local network (mDNS browse).
    PeerDiscovered {
        peer: PeerInfo,
    },
    /// A previously discovered host went away.
    PeerLost {
        peer: PeerInfo,
    },
    SessionEstablished {
        id: SessionId,
        peer: PeerInfo,
        roles: Roles,
        codec: CodecKind,
    },
    SessionLost {
        id: SessionId,
        reason: String,
    },
    /// Rough incoming level for a session, for metering. `rms` is 0..=1.
    AudioLevel {
        id: SessionId,
        rms: f32,
    },
    /// Non-fatal background error worth surfacing in the UI.
    Error {
        message: String,
    },
}

/// Engine-wide configuration. Apply with [`RelayHandle::update_config`]
/// before starting a host; `connect` reads the audio parameters live.
#[derive(Clone, Debug, PartialEq)]
pub struct EngineConfig {
    /// Advertised device name.
    pub device_name: String,
    pub device_kind: DeviceKind,
    /// Pairing PIN. Hosts must set one before [`RelayHandle::host_start`].
    pub pin: String,
    /// TCP control port when hosting; 0 picks an ephemeral port.
    pub port: u16,
    pub codec: CodecKind,
    pub frame_ms: u16,
    pub sample_rate: u32,
    /// 1 (mono microphone) or 2 (stereo playback).
    pub channels: u16,
    /// Roles used when this engine connects to a host as a client.
    pub client_roles: Roles,
    /// Preferred transport link (`auto` picks the best available).
    pub transport: TransportPreference,
    /// Sample rate of this machine's own audio endpoints. Sessions are
    /// converted to and from this rate, so a peer negotiating 16 kHz does not
    /// play back at three times the pitch.
    pub local_sample_rate: u32,
    /// Channel count of this machine's own audio endpoints.
    pub local_channels: u16,
    /// Local address the host listens on. When `None`, the best active
    /// relay-capable link selected by [`TransportPreference::Auto`] is used;
    /// only a machine with no usable link information falls back to every
    /// IPv4 interface.
    pub bind_addr: Option<Ipv4Addr>,
    /// Concurrent connections allowed to sit in the pairing handshake. Each
    /// costs a thread and a five-second read timeout before it has proven
    /// anything, so an unbounded count is a trivial resource-exhaustion path.
    pub max_pending_handshakes: usize,
    /// Established sessions a host will hold at once.
    pub max_sessions: usize,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            device_name: "qpwgraph-rs".into(),
            device_kind: DeviceKind::Linux,
            pin: String::new(),
            port: 0,
            codec: CodecKind::Opus,
            frame_ms: 10,
            sample_rate: 48_000,
            channels: 1,
            client_roles: Roles::emit_only(),
            transport: TransportPreference::Auto,
            local_sample_rate: 48_000,
            local_channels: 1,
            bind_addr: None,
            max_pending_handshakes: 8,
            max_sessions: 16,
        }
    }
}

impl EngineConfig {
    /// This machine's own audio geometry, as a frame-less format.
    pub fn local_format(&self) -> AudioFormat {
        AudioFormat::new(self.local_sample_rate, self.local_channels, self.frame_ms)
    }
}

/// Status snapshot for UI display.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionStatus {
    pub id: SessionId,
    pub peer: PeerInfo,
    pub roles: Roles,
    pub codec: CodecKind,
    /// True when this side sends audio in the session.
    pub sending: bool,
    /// True when this side receives audio in the session.
    pub receiving: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct EngineStatus {
    pub host_active: bool,
    pub host_port: Option<u16>,
    /// The exact IPv4 address selected for the active listener. `None` means
    /// the documented no-link fallback is listening on all IPv4 interfaces.
    pub host_addr: Option<Ipv4Addr>,
    pub sessions: Vec<SessionStatus>,
}

/// State of the control connection relevant to session resumption.
///
/// This is deliberately separate from the generation counter: the counter
/// identifies one set of control keys, while this state prevents a second
/// connection from taking over while the original control owner is still
/// active.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ControlState {
    Active,
    ResumeEligible { generation: u64 },
    Resuming { generation: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResumeGraceResult {
    /// The grace period expired without an in-flight resume.
    Expired,
    /// A different control generation already owns the session, or the
    /// session is otherwise no longer waiting for this grace period.
    Resumed,
    /// A resume challenge is being authenticated. The watcher must not turn
    /// this into an apparently active session with no control owner.
    InProgress,
}

/// Internal session bookkeeping shared with worker threads.
pub(crate) struct SessionRecord {
    pub id: SessionId,
    /// Session identifier assigned by the host and used on resume.
    pub wire_id: u64,
    pub peer: PeerInfo,
    pub roles: Roles,
    pub codec: CodecKind,
    pub format: AudioFormat,
    /// This side sends audio (peer receives).
    pub sending: bool,
    /// This side receives audio (peer sends).
    pub receiving: bool,
    pub stop: Arc<AtomicBool>,
    /// Set by `disconnect`; the control thread sends `bye` and tears down.
    pub bye_requested: AtomicBool,
    /// Identifies the control-key generation currently in use. Host-side
    /// grace waits compare generations to notice a replacement.
    pub control_generation: AtomicU64,
    /// Derived from the original PAKE shared secret. It is never transmitted
    /// and is used only for challenge-response resumption.
    pub resume_secret: [u8; 32],
    /// Explicitly gates resume takeover and serializes racing attempts.
    pub control_state: Mutex<ControlState>,
    /// UDP address of the peer's audio socket.
    ///
    /// Only ever updated from a datagram that authenticated against this
    /// session's audio key. Learning it from any syntactically valid packet,
    /// as an earlier version did, let anyone who could reach the port
    /// redirect our outbound audio to themselves.
    pub peer_audio_addr: Mutex<Option<SocketAddr>>,
    /// Per-session transmit queue so one capture stream fans out to every
    /// receiving peer without competing consumers. Holds audio already
    /// converted into *this session's* negotiated format.
    pub outgoing: PcmQueue,
    /// Per-session receive queue holding audio converted into the engine's
    /// local format, ready to be mixed with the other sessions'.
    pub incoming: PcmQueue,
    /// Local-format-to-session-format conversion for the transmit path, with
    /// its reusable output buffer. One converter per session: they have
    /// independent geometries and independent interpolation state.
    pub capture_convert: Mutex<(Converter, Vec<f32>)>,
    /// Seals this session's outgoing datagrams. Shared between the transmit
    /// worker and the announce path, because a single nonce counter per key
    /// is what keeps the AEAD safe.
    pub audio_sealer: Mutex<Sealer>,
    /// Opens this session's incoming datagrams and tracks its replay window.
    pub audio_opener: Mutex<Opener>,
}

impl SessionRecord {
    /// Mark the current control owner as gone. Calling this more than once is
    /// harmless while the grace period is in progress.
    pub(crate) fn mark_control_dropped(&self) -> bool {
        let Ok(mut state) = self.control_state.lock() else {
            return false;
        };
        match *state {
            ControlState::Active => {
                let generation = self.control_generation.load(Ordering::Acquire);
                *state = ControlState::ResumeEligible { generation };
                true
            }
            ControlState::ResumeEligible { .. } => true,
            ControlState::Resuming { .. } => false,
        }
    }

    /// Claim one eligible resume generation. A session whose old control
    /// channel is active cannot be claimed, and only one challenge may be in
    /// flight at once.
    pub(crate) fn begin_resume(&self) -> Option<u64> {
        let Ok(mut state) = self.control_state.lock() else {
            return None;
        };
        let ControlState::ResumeEligible { generation } = *state else {
            return None;
        };
        let next = generation.checked_add(1)?;
        *state = ControlState::Resuming { generation: next };
        Some(next)
    }

    /// Return a failed resume attempt to the eligible state without rotating
    /// the live control generation.
    pub(crate) fn cancel_resume(&self, generation: u64) {
        if let Ok(mut state) = self.control_state.lock() {
            if *state == (ControlState::Resuming { generation }) {
                let current = self.control_generation.load(Ordering::Acquire);
                *state = ControlState::ResumeEligible {
                    generation: current,
                };
            }
        }
    }

    /// Commit a successful resume and rotate the control-key generation.
    pub(crate) fn finish_resume(&self, generation: u64) -> bool {
        let Ok(mut state) = self.control_state.lock() else {
            return false;
        };
        if *state != (ControlState::Resuming { generation }) {
            return false;
        }
        self.control_generation.store(generation, Ordering::Release);
        *state = ControlState::Active;
        true
    }

    /// End a grace period without allowing its old control watcher to tear
    /// down a session that has already been resumed. The state transition is
    /// serialized with `finish_resume`, so the watcher and the new owner
    /// cannot both decide the session's fate. An in-flight challenge remains
    /// in progress until it succeeds or cancels itself.
    pub(crate) fn expire_resume_grace(&self, generation: u64) -> ResumeGraceResult {
        let Ok(mut state) = self.control_state.lock() else {
            return ResumeGraceResult::InProgress;
        };
        if self.control_generation.load(Ordering::Acquire) != generation {
            return ResumeGraceResult::Resumed;
        }
        match *state {
            ControlState::ResumeEligible {
                generation: current,
            } if current == generation => {
                *state = ControlState::Active;
                ResumeGraceResult::Expired
            }
            ControlState::Resuming { .. } => ResumeGraceResult::InProgress,
            _ => ResumeGraceResult::Resumed,
        }
    }

    /// Abort a challenge that has remained in flight beyond the bounded
    /// handshake timeout. The caller will tear down the session; a successful
    /// finisher racing this method wins or loses under the same state lock.
    pub(crate) fn abort_resume(&self, generation: u64) -> bool {
        let Ok(mut state) = self.control_state.lock() else {
            return false;
        };
        if *state == (ControlState::Resuming { generation }) {
            *state = ControlState::Active;
            true
        } else {
            false
        }
    }
}

pub(crate) struct EngineInner {
    config: Mutex<EngineConfig>,
    events: Mutex<VecDeque<RelayEvent>>,
    /// Scratch used while summing the per-session receive queues.
    mix_scratch: Mutex<Vec<f32>>,
    sessions: Mutex<BTreeMap<SessionId, Arc<SessionRecord>>>,
    /// Recent failed pairing attempts per source address. A PAKE makes
    /// guessing an online-only game; this is what makes that game slow.
    pairing_failures: Mutex<BTreeMap<IpAddr, FailureRecord>>,
    /// Connections currently inside the pre-authentication handshake.
    pending_handshakes: AtomicU64,
    host: Mutex<Option<session::HostRecord>>,
    /// Discovered (not necessarily connected) relay hosts, keyed by address.
    peers: Mutex<BTreeMap<SocketAddr, PeerInfo>>,
    /// Resolved addresses grouped by mDNS service identity.
    peer_services: Mutex<BTreeMap<String, BTreeMap<SocketAddr, PeerInfo>>>,
    advertiser: Mutex<Option<discovery::Advertiser>>,
    browser: Mutex<Option<discovery::Browser>>,
    usb_scanner: Mutex<Option<usb_probe::UsbScanner>>,
    next_session: AtomicU64,
    running: AtomicBool,
}

impl EngineInner {
    fn new(config: EngineConfig) -> Arc<Self> {
        Arc::new(Self {
            config: Mutex::new(config),
            events: Mutex::new(VecDeque::new()),
            // Allocated once, at the largest quantum the realtime callback
            // will ever present, so `mix_playback` never grows it. 64 KiB.
            mix_scratch: Mutex::new(Vec::with_capacity(MAX_REALTIME_QUANTUM_SAMPLES)),
            sessions: Mutex::new(BTreeMap::new()),
            pairing_failures: Mutex::new(BTreeMap::new()),
            pending_handshakes: AtomicU64::new(0),
            host: Mutex::new(None),
            peers: Mutex::new(BTreeMap::new()),
            peer_services: Mutex::new(BTreeMap::new()),
            advertiser: Mutex::new(None),
            browser: Mutex::new(None),
            usb_scanner: Mutex::new(None),
            next_session: AtomicU64::new(1),
            running: AtomicBool::new(true),
        })
    }

    fn emit(&self, event: RelayEvent) {
        let Ok(mut events) = self.events.lock() else {
            return;
        };
        // Meter updates and repeated identical errors are replaceable rather
        // than cumulative: a consumer only ever wants the latest. Coalescing
        // them keeps a noisy session from pushing everything else out of a
        // bounded queue.
        let replaceable = match &event {
            RelayEvent::AudioLevel { id, .. } => {
                let id = *id;
                events.iter_mut().find(|queued| {
                    matches!(queued, RelayEvent::AudioLevel { id: queued, .. } if *queued == id)
                })
            }
            RelayEvent::Error { message } => {
                // Only a recent identical error is folded away; a genuinely
                // new message always gets through.
                let recent = events.len().saturating_sub(8);
                events.iter_mut().skip(recent).find(|queued| {
                    matches!(queued, RelayEvent::Error { message: queued } if queued.as_str() == message.as_str())
                })
            }
            _ => None,
        };
        if let Some(slot) = replaceable {
            *slot = event;
            return;
        }
        events.push_back(event);
        while events.len() > MAX_QUEUED_EVENTS {
            events.pop_front();
        }
    }

    /// Whether `addr` may attempt a pairing right now.
    fn pairing_allowed(&self, addr: IpAddr) -> bool {
        let Ok(mut failures) = self.pairing_failures.lock() else {
            return true;
        };
        match failures.get(&addr) {
            Some(record) if record.locked_until > Instant::now() => false,
            Some(record)
                if record.locked_until <= Instant::now()
                    && record.count >= PAIRING_ATTEMPT_LIMIT =>
            {
                // The lockout expired; give the peer a fresh budget.
                failures.remove(&addr);
                true
            }
            _ => true,
        }
    }

    /// Record a failed pairing, locking the source out once it has burned
    /// through its budget.
    fn note_pairing_failure(&self, addr: IpAddr) {
        let Ok(mut failures) = self.pairing_failures.lock() else {
            return;
        };
        let now = Instant::now();
        // Keep the table bounded if many addresses probe. Expired records are
        // discarded first; if a hostile burst has filled the table with
        // active lockouts, evict the least recently seen source so a new
        // address cannot grow the map past the limit.
        if !failures.contains_key(&addr) && failures.len() >= MAX_PAIRING_FAILURE_RECORDS {
            failures.retain(|_, record| record.locked_until > now);
            if failures.len() >= MAX_PAIRING_FAILURE_RECORDS {
                let oldest = failures
                    .iter()
                    .min_by_key(|(_, record)| record.last_seen)
                    .map(|(address, _)| *address);
                if let Some(oldest) = oldest {
                    failures.remove(&oldest);
                }
            }
        }
        let record = failures.entry(addr).or_insert(FailureRecord {
            count: 0,
            locked_until: now,
            last_seen: now,
        });
        record.count += 1;
        record.last_seen = now;
        if record.count >= PAIRING_ATTEMPT_LIMIT {
            record.locked_until = now + PAIRING_LOCKOUT;
        }
    }

    /// Forget a source's failures after it pairs successfully.
    fn clear_pairing_failures(&self, addr: IpAddr) {
        if let Ok(mut failures) = self.pairing_failures.lock() {
            failures.remove(&addr);
        }
    }

    fn drain_events(&self) -> Vec<RelayEvent> {
        self.events
            .lock()
            .map(|mut events| events.drain(..).collect())
            .unwrap_or_default()
    }

    fn config(&self) -> EngineConfig {
        self.config
            .lock()
            .map(|config| config.clone())
            .unwrap_or_default()
    }

    fn next_session_id(&self) -> SessionId {
        SessionId(self.next_session.fetch_add(1, Ordering::Relaxed))
    }

    fn insert_session(&self, record: Arc<SessionRecord>) -> bool {
        let limit = self.config().max_sessions;
        let Ok(mut sessions) = self.sessions.lock() else {
            return false;
        };
        // Session IDs are allocated monotonically, but rejecting a duplicate
        // here also prevents a test/backend mistake from replacing a live
        // record and bypassing the bound.
        if sessions.contains_key(&record.id) || sessions.len() >= limit {
            return false;
        }
        sessions.insert(record.id, record);
        true
    }

    fn session(&self, id: SessionId) -> Option<Arc<SessionRecord>> {
        self.sessions.lock().ok()?.get(&id).cloned()
    }

    fn session_count(&self) -> usize {
        self.sessions
            .lock()
            .map(|sessions| sessions.len())
            .unwrap_or(0)
    }

    /// Claim a pre-authentication handshake slot, or `None` when the host is
    /// already handling as many as it allows.
    fn claim_handshake(self: &Arc<Self>) -> Option<HandshakeSlot> {
        let limit = self.config().max_pending_handshakes.max(1) as u64;
        let mut current = self.pending_handshakes.load(Ordering::Relaxed);
        loop {
            if current >= limit {
                return None;
            }
            match self.pending_handshakes.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    return Some(HandshakeSlot {
                        inner: Arc::clone(self),
                    })
                }
                Err(observed) => current = observed,
            }
        }
    }

    fn session_alive(&self, id: SessionId) -> bool {
        self.running.load(Ordering::Relaxed)
            && self
                .sessions
                .lock()
                .map(|sessions| sessions.contains_key(&id))
                .unwrap_or(false)
    }

    /// Fan captured audio out to every transmitting session, converting the
    /// engine's local geometry into each session's negotiated one.
    ///
    /// `samples` are interleaved in the local format
    /// ([`EngineConfig::local_sample_rate`] / `local_channels`). Sessions may
    /// each have negotiated something different, so the conversion is
    /// per-session and stateful; its buffers are reused, so the realtime path
    /// does not allocate after warm-up.
    fn broadcast_capture(&self, samples: &[f32], realtime: bool) -> bool {
        let sessions = if realtime {
            let Ok(sessions) = self.sessions.try_lock() else {
                return false;
            };
            sessions
        } else {
            let Ok(sessions) = self.sessions.lock() else {
                return false;
            };
            sessions
        };
        let mut accepted = true;
        let mut found = false;
        for record in sessions.values().filter(|record| record.sending) {
            found = true;
            let converted = if realtime {
                record.capture_convert.try_lock().ok()
            } else {
                record.capture_convert.lock().ok()
            };
            let Some(mut converted) = converted else {
                accepted = false;
                continue;
            };
            let (converter, buffer) = &mut *converted;
            if converter.is_identity() {
                // Avoid the copy on the common matched-geometry path.
                accepted &= if realtime {
                    record.outgoing.try_push(samples)
                } else {
                    record.outgoing.push(samples);
                    true
                };
                continue;
            }
            converter.convert(samples, buffer);
            accepted &= if realtime {
                record.outgoing.try_push(buffer)
            } else {
                record.outgoing.push(buffer);
                true
            };
        }
        found && accepted
    }

    /// Sum every receiving session's decoded audio into `out`.
    ///
    /// Each session decodes into its own queue in the engine's local format,
    /// so mixing is a plain sum. Sharing one queue — as an earlier version
    /// did — concatenated two peers' audio into one stream rather than
    /// mixing it, and let either peer resize the other's playback buffer.
    fn mix_playback(&self, out: &mut [f32], realtime: bool) -> usize {
        if out.is_empty() {
            return 0;
        }
        let sessions = if realtime {
            let Ok(sessions) = self.sessions.try_lock() else {
                return 0;
            };
            sessions
        } else {
            let Ok(sessions) = self.sessions.lock() else {
                return 0;
            };
            sessions
        };
        // Iterate the map directly. Collecting the receiving sessions into a
        // `Vec` first — as this used to — allocated on the PipeWire process
        // callback, on a path whose entire contract is that it does not.
        let mut receiving = sessions.values().filter(|record| record.receiving);
        let Some(first) = receiving.next() else {
            return 0;
        };
        let Some(second) = receiving.next() else {
            // One session is the overwhelmingly common case; skip the scratch
            // buffer and the summing loop entirely.
            return if realtime {
                first.incoming.try_pull(out)
            } else {
                first.incoming.pull(out)
            };
        };

        let scratch = if realtime {
            self.mix_scratch.try_lock().ok()
        } else {
            self.mix_scratch.lock().ok()
        };
        let Some(mut scratch) = scratch else {
            return 0;
        };
        // `mix_scratch` is allocated at [`MAX_REALTIME_QUANTUM_SAMPLES`] when
        // the engine is built, so for any realtime caller this resize is a
        // length change inside existing capacity. A caller that ignores that
        // bound and hands over something longer would otherwise reallocate
        // here, so instead it mixes into the part that is already backed and
        // reports only that much.
        let usable = if realtime {
            if scratch.capacity() < out.len() {
                scratch.capacity()
            } else {
                out.len()
            }
        } else {
            out.len()
        };
        if usable == 0 {
            return 0;
        }
        scratch.clear();
        scratch.resize(usable, 0.0);
        out[..usable].fill(0.0);

        let mut produced = 0;
        // `first` and `second` are already pulled off the iterator; chaining
        // them back on keeps one loop body without building a collection.
        for record in [first, second].into_iter().chain(receiving) {
            let count = if realtime {
                record.incoming.try_pull(&mut scratch[..])
            } else {
                record.incoming.pull(&mut scratch[..])
            };
            for (slot, sample) in out.iter_mut().zip(scratch.iter()).take(count) {
                *slot += *sample;
            }
            produced = produced.max(count);
        }
        // Summed peers can exceed full scale; clamping is far less
        // objectionable than the wraparound a raw sum would hand to an
        // integer conversion downstream.
        for sample in out.iter_mut().take(produced) {
            *sample = sample.clamp(-1.0, 1.0);
        }
        produced
    }

    fn remove_session(&self, id: SessionId) -> Option<Arc<SessionRecord>> {
        let record = self.sessions.lock().ok()?.remove(&id);
        if let Some(record) = &record {
            record.stop.store(true, Ordering::Relaxed);
        }
        record
    }

    fn status(&self) -> EngineStatus {
        let host = self
            .host
            .lock()
            .ok()
            .and_then(|host| host.as_ref().map(|record| (record.port, record.bind_addr)));
        let (host_port, host_addr) = host
            .map(|(port, addr)| (Some(port), addr))
            .unwrap_or((None, None));
        let sessions = self
            .sessions
            .lock()
            .map(|sessions| {
                sessions
                    .values()
                    .map(|record| SessionStatus {
                        id: record.id,
                        peer: record.peer.clone(),
                        roles: record.roles,
                        codec: record.codec,
                        sending: record.sending,
                        receiving: record.receiving,
                    })
                    .collect()
            })
            .unwrap_or_default();
        EngineStatus {
            host_active: host_port.is_some(),
            host_port,
            host_addr,
            sessions,
        }
    }

    fn stop_all(&self) {
        self.running.store(false, Ordering::Relaxed);
        self.stop_advertiser();
        self.stop_browser();
        self.stop_usb_scanner();
        session::stop_host(self);
        let ids: Vec<SessionId> = self
            .sessions
            .lock()
            .map(|sessions| sessions.keys().copied().collect())
            .unwrap_or_default();
        for id in ids {
            session::teardown(self, id, "engine stopped".into());
        }
    }
}

/// Releases a pre-authentication handshake slot when the handshake thread
/// ends, however it ends.
pub(crate) struct HandshakeSlot {
    inner: Arc<EngineInner>,
}

impl Drop for HandshakeSlot {
    fn drop(&mut self) {
        self.inner.pending_handshakes.fetch_sub(1, Ordering::AcqRel);
    }
}

/// The relay engine. Owns background threads until [`RelayEngine::shutdown`].
pub struct RelayEngine {
    inner: Arc<EngineInner>,
}

impl RelayEngine {
    /// Create the engine. No sockets open until `host_start`/`connect`.
    pub fn start(config: EngineConfig) -> RelayResult<Self> {
        Ok(Self {
            inner: EngineInner::new(config),
        })
    }

    pub fn handle(&self) -> RelayHandle {
        RelayHandle {
            inner: Arc::clone(&self.inner),
        }
    }

    /// Stop the host, end all sessions, and mark the engine stopped.
    /// Background threads observe the stop within about a second.
    pub fn shutdown(&self) {
        self.inner.stop_all();
    }
}

impl Drop for RelayEngine {
    fn drop(&mut self) {
        self.inner.stop_all();
    }
}

/// Cheap cloneable handle used by the embedding application.
#[derive(Clone)]
pub struct RelayHandle {
    inner: Arc<EngineInner>,
}

impl RelayHandle {
    /// Replace the engine configuration. Safe to call while idle; hosts
    /// re-read the PIN per pairing attempt.
    pub fn update_config(&self, config: EngineConfig) {
        if let Ok(mut slot) = self.inner.config.lock() {
            *slot = config;
        }
    }

    pub fn config(&self) -> EngineConfig {
        self.inner.config()
    }

    /// Start listening for clients. Returns the bound TCP control port.
    pub fn host_start(&self) -> RelayResult<u16> {
        let config = self.inner.config();
        let pin = config.pin.trim();
        if pin.is_empty() {
            return Err(RelayError::Engine(
                "a pairing PIN must be configured before hosting".into(),
            ));
        }
        if pin.len() < pairing::PIN_LENGTH {
            return Err(RelayError::Engine(format!(
                "the pairing PIN must be at least {} characters",
                pairing::PIN_LENGTH
            )));
        }
        let mut host = self
            .inner
            .host
            .lock()
            .map_err(|_| RelayError::Engine("host state is locked".into()))?;
        if host.is_some() {
            return Err(RelayError::Engine(
                "the relay host is already running".into(),
            ));
        }
        let record = session::start_host(&self.inner, config.port)?;
        let port = record.port;
        let bind_addr = record.bind_addr;
        *host = Some(record);
        drop(host);
        // Advertise over mDNS so peers can find us (best-effort).
        self.inner.start_advertiser(port, bind_addr);
        self.inner.emit(RelayEvent::HostStarted { port });
        Ok(port)
    }

    pub fn host_stop(&self) -> RelayResult<()> {
        let removed = {
            let mut host = self
                .inner
                .host
                .lock()
                .map_err(|_| RelayError::Engine("host state is locked".into()))?;
            host.take()
        };
        if let Some(record) = removed {
            record.stop.store(true, Ordering::Relaxed);
            self.inner.stop_advertiser();
            self.inner.emit(RelayEvent::HostStopped);
        }
        Ok(())
    }

    /// Connect to a host as a client. The handshake runs on a background
    /// thread; success or failure arrives as a [`RelayEvent`]. The returned
    /// id is valid immediately.
    pub fn connect(&self, target: SocketAddr, pin: &str, roles: Roles) -> SessionId {
        let id = self.inner.next_session_id();
        session::connect_peer(&self.inner, id, target, pin.to_owned(), roles);
        id
    }

    /// Begin browsing for relay hosts on the local network. Discovered peers
    /// arrive as [`RelayEvent::PeerDiscovered`]. Runs mDNS alongside a direct
    /// probe of USB tether subnets, because mDNS often does not cross a USB
    /// tether. Idempotent.
    pub fn discovery_start(&self) -> RelayResult<()> {
        self.inner.start_browser()?;
        // Best-effort: a missing USB scanner must not fail mDNS browsing.
        let _ = self.inner.start_usb_scanner();
        Ok(())
    }

    /// Stop browsing for relay hosts. Idempotent.
    pub fn discovery_stop(&self) {
        self.inner.stop_browser();
        self.inner.stop_usb_scanner();
    }

    /// Snapshot of relay hosts discovered so far.
    pub fn discovered_peers(&self) -> Vec<PeerInfo> {
        self.inner.discovered_peers()
    }

    /// End a session gracefully.
    pub fn disconnect(&self, id: SessionId) -> RelayResult<()> {
        session::request_bye(&self.inner, id);
        Ok(())
    }

    /// Drain pending events. Call once per update tick.
    pub fn events(&self) -> Vec<RelayEvent> {
        self.inner.drain_events()
    }

    /// Publish a non-fatal error from an embedding audio endpoint or worker.
    ///
    /// Relay transports already surface their background failures through the
    /// event queue; platform adapters use the same path so a stopped capture
    /// or render thread cannot look like a healthy, silent session.
    pub fn report_error(&self, message: impl Into<String>) {
        self.inner.emit(RelayEvent::Error {
            message: message.into(),
        });
    }

    /// Feed audio to transmit (e.g. the virtual relay sink tap). Oldest
    /// samples are dropped when the queue overflows.
    pub fn push_capture(&self, samples: &[f32]) {
        self.inner.broadcast_capture(samples, false);
    }

    /// Realtime-safe variant of [`Self::push_capture`].
    ///
    /// Returns `false` without touching the input when `samples` exceeds
    /// [`MAX_REALTIME_QUANTUM_SAMPLES`], when a realtime lock is busy, or
    /// when no session accepts capture. A successful call enqueues the whole
    /// quantum for each available session; bounded queues may drop their
    /// oldest samples when full.
    pub fn try_push_capture(&self, samples: &[f32]) -> bool {
        if samples.len() > MAX_REALTIME_QUANTUM_SAMPLES {
            return false;
        }
        self.inner.broadcast_capture(samples, true)
    }

    /// Take decoded audio received from peers (e.g. into the virtual relay
    /// microphone), mixed across sessions and converted into the engine's
    /// local format. Returns the number of samples written to `out`.
    pub fn pull_playback(&self, out: &mut [f32]) -> usize {
        self.inner.mix_playback(out, false)
    }

    /// Realtime-safe variant of [`Self::pull_playback`].
    ///
    /// Returns zero when a realtime lock is busy or no audio is available.
    /// At most [`MAX_REALTIME_QUANTUM_SAMPLES`] samples are produced; an
    /// oversized output slice is short-served and its tail is untouched.
    pub fn try_pull_playback(&self, out: &mut [f32]) -> usize {
        let usable = out.len().min(MAX_REALTIME_QUANTUM_SAMPLES);
        self.inner.mix_playback(&mut out[..usable], true)
    }

    pub fn status(&self) -> EngineStatus {
        self.inner.status()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv6Addr;

    #[test]
    fn default_config_is_usable() {
        let config = EngineConfig::default();
        assert_eq!(config.frame_ms, 10);
        assert_eq!(config.sample_rate, 48_000);
        assert!(config.client_roles.emit);
        let format = AudioFormat::new(config.sample_rate, config.channels, config.frame_ms);
        assert_eq!(format.frame_samples(), 480);
    }

    #[test]
    fn host_start_requires_a_pin() {
        let engine = RelayEngine::start(EngineConfig::default()).unwrap();
        let handle = engine.handle();
        assert!(handle.host_start().is_err());
    }

    #[test]
    fn resume_is_only_eligible_after_control_drop_and_consumes_generations() {
        let record = mixing_session(1, false);

        // A live control owner cannot be replaced by a connection that merely
        // knows the session id.
        assert_eq!(record.begin_resume(), None);
        assert_eq!(*record.control_state.lock().unwrap(), ControlState::Active);

        assert!(record.mark_control_dropped());
        let first = record.begin_resume().expect("dropped control is resumable");
        assert_eq!(first, 2);
        // One challenge can be in flight at a time.
        assert_eq!(record.begin_resume(), None);

        // A failed challenge returns to eligibility. The next challenge uses
        // a fresh server nonce, so an old proof cannot be replayed even though
        // the live control-key generation has not changed yet.
        record.cancel_resume(first);
        let second = record.begin_resume().expect("retry remains eligible");
        assert_eq!(second, 2);
        assert!(record.finish_resume(second));
        assert_eq!(*record.control_state.lock().unwrap(), ControlState::Active);
        assert_eq!(record.begin_resume(), None);
    }

    #[test]
    fn resume_grace_expiry_cannot_race_a_successful_resume() {
        let record = mixing_session(2, false);
        assert!(record.mark_control_dropped());
        let generation = record.begin_resume().expect("resume is eligible");
        assert_eq!(record.expire_resume_grace(1), ResumeGraceResult::InProgress);
        assert_eq!(
            *record.control_state.lock().unwrap(),
            ControlState::Resuming { generation }
        );
        assert!(record.finish_resume(generation));
        // The old watcher's generation is stale after the successful resume.
        assert_eq!(record.expire_resume_grace(1), ResumeGraceResult::Resumed);

        let record = mixing_session(3, false);
        assert!(record.mark_control_dropped());
        let generation = record.begin_resume().expect("resume is eligible");
        record.cancel_resume(generation);
        assert_eq!(record.expire_resume_grace(1), ResumeGraceResult::Expired);
        // Expiry wins only while the old generation is still current.
        assert!(!record.finish_resume(generation));
    }

    #[test]
    fn realtime_push_reports_no_acceptor_when_no_session_exists() {
        let handle = RelayHandle {
            inner: mixing_engine(Vec::new()),
        };
        assert!(!handle.try_push_capture(&[]));
    }

    #[test]
    fn realtime_push_rejects_oversized_quantum_before_converter_work() {
        let session = mixing_session(1, false);
        let inner = mixing_engine(vec![Arc::clone(&session)]);
        let before = session.capture_convert.lock().unwrap().1.capacity();
        let oversized = vec![0.0f32; MAX_REALTIME_QUANTUM_SAMPLES + 1];

        let handle = RelayHandle { inner };
        assert!(!handle.try_push_capture(&oversized));
        assert_eq!(session.capture_convert.lock().unwrap().1.capacity(), before);
    }

    #[test]
    fn established_session_admission_is_atomically_bounded() {
        let inner = EngineInner::new(EngineConfig {
            max_sessions: 1,
            ..EngineConfig::default()
        });
        assert!(inner.insert_session(mixing_session(1, false)));
        assert!(!inner.insert_session(mixing_session(2, false)));
        assert_eq!(inner.session_count(), 1);
    }

    #[test]
    fn pairing_failure_table_has_a_hard_bound() {
        let inner = EngineInner::new(EngineConfig::default());
        let now = Instant::now();
        {
            let mut failures = inner.pairing_failures.lock().unwrap();
            for index in 0..MAX_PAIRING_FAILURE_RECORDS {
                failures.insert(
                    IpAddr::V6(Ipv6Addr::from(index as u128)),
                    FailureRecord {
                        count: PAIRING_ATTEMPT_LIMIT,
                        locked_until: now + PAIRING_LOCKOUT,
                        last_seen: now,
                    },
                );
            }
        }

        inner.note_pairing_failure(IpAddr::V6(Ipv6Addr::from(99_999u128)));
        assert_eq!(
            inner.pairing_failures.lock().unwrap().len(),
            MAX_PAIRING_FAILURE_RECORDS
        );
    }

    /// A session record with just enough filled in to exercise mixing. The
    /// crypto halves are real, because `SessionRecord` has nowhere to put a
    /// placeholder, but nothing in the mix path touches them.
    fn mixing_session(id: u64, receiving: bool) -> Arc<SessionRecord> {
        use crate::crypto::{pake_start, Side};
        let client = pake_start(Side::Client, "123456");
        let host = pake_start(Side::Host, "123456");
        let client_message = client.message.clone();
        let host_message = host.message.clone();
        let keys = client.finish(&host_message).expect("client pairs");
        let peer_keys = host.finish(&client_message).expect("host pairs");
        let (sealer, _) = keys.audio_channel().expect("audio keys");
        let (_, opener) = peer_keys.audio_channel().expect("peer audio keys");
        let format = AudioFormat::new(48_000, 1, 10);
        let capture_converter =
            Converter::with_capacity(48_000, 1, 48_000, 1, MAX_REALTIME_QUANTUM_SAMPLES);
        let capture_destination =
            Vec::with_capacity(capture_converter.output_capacity_for(MAX_REALTIME_QUANTUM_SAMPLES));
        Arc::new(SessionRecord {
            id: SessionId(id),
            wire_id: id,
            peer: PeerInfo {
                name: format!("peer-{id}"),
                kind: DeviceKind::Other,
                addr: "127.0.0.1:1".parse().unwrap(),
            },
            roles: Roles::both(),
            codec: CodecKind::Pcm,
            format,
            sending: true,
            receiving,
            stop: Arc::new(AtomicBool::new(false)),
            bye_requested: AtomicBool::new(false),
            control_generation: AtomicU64::new(1),
            resume_secret: keys.resume_auth_key(),
            control_state: Mutex::new(ControlState::Active),
            peer_audio_addr: Mutex::new(None),
            outgoing: PcmQueue::new(DEFAULT_QUEUE_CAPACITY),
            incoming: PcmQueue::new(DEFAULT_QUEUE_CAPACITY),
            capture_convert: Mutex::new((capture_converter, capture_destination)),
            audio_sealer: Mutex::new(sealer),
            audio_opener: Mutex::new(opener),
        })
    }

    fn mixing_engine(sessions: Vec<Arc<SessionRecord>>) -> Arc<EngineInner> {
        let inner = EngineInner::new(EngineConfig::default());
        for record in sessions {
            inner.insert_session(record);
        }
        inner
    }

    #[test]
    fn mixing_with_no_receiving_sessions_produces_nothing() {
        let inner = mixing_engine(vec![mixing_session(1, false)]);
        let mut out = [9.0f32; 4];
        assert_eq!(inner.mix_playback(&mut out, true), 0);
        // Producing nothing must also mean touching nothing: the caller fills
        // the untouched tail with silence itself.
        assert_eq!(out, [9.0; 4]);
        assert_eq!(mixing_engine(vec![]).mix_playback(&mut out, true), 0);
    }

    #[test]
    fn mixing_one_receiving_session_passes_its_audio_through() {
        let session = mixing_session(1, true);
        session.incoming.push(&[0.1, 0.2, 0.3]);
        let inner = mixing_engine(vec![session, mixing_session(2, false)]);
        let mut out = [0.0f32; 4];
        assert_eq!(inner.mix_playback(&mut out, true), 3);
        assert_eq!(&out[..3], &[0.1, 0.2, 0.3]);
    }

    #[test]
    fn mixing_several_receiving_sessions_sums_them() {
        let first = mixing_session(1, true);
        let second = mixing_session(2, true);
        let third = mixing_session(3, true);
        first.incoming.push(&[0.1, 0.1, 0.1, 0.1]);
        second.incoming.push(&[0.2, 0.2]);
        third.incoming.push(&[0.3, 0.3, 0.3]);
        let inner = mixing_engine(vec![first, second, third]);
        let mut out = [0.0f32; 4];
        // The count is the longest contributor, not the sum of the lengths:
        // peers are mixed, not concatenated.
        assert_eq!(inner.mix_playback(&mut out, true), 4);
        assert!((out[0] - 0.6).abs() < 1e-6, "{out:?}");
        assert!((out[1] - 0.6).abs() < 1e-6, "{out:?}");
        assert!((out[2] - 0.4).abs() < 1e-6, "{out:?}");
        assert!((out[3] - 0.1).abs() < 1e-6, "{out:?}");
    }

    #[test]
    fn a_summed_mix_is_clamped_to_full_scale() {
        let first = mixing_session(1, true);
        let second = mixing_session(2, true);
        first.incoming.push(&[0.9, -0.9]);
        second.incoming.push(&[0.9, -0.9]);
        let inner = mixing_engine(vec![first, second]);
        let mut out = [0.0f32; 2];
        assert_eq!(inner.mix_playback(&mut out, true), 2);
        assert_eq!(out, [1.0, -1.0]);
    }

    #[test]
    fn realtime_mixing_does_not_grow_the_scratch_buffer() {
        // This is the regression the `Vec` collect used to cause: every
        // realtime callback allocated, on the one thread that must not.
        let first = mixing_session(1, true);
        let second = mixing_session(2, true);
        let inner = mixing_engine(vec![Arc::clone(&first), Arc::clone(&second)]);
        let capacity = inner.mix_scratch.lock().unwrap().capacity();
        assert!(capacity >= MAX_REALTIME_QUANTUM_SAMPLES);
        let block = vec![0.25f32; 1_024];
        let mut out = vec![0.0f32; 1_024];
        for _ in 0..64 {
            first.incoming.push(&block);
            second.incoming.push(&block);
            inner.mix_playback(&mut out, true);
            assert_eq!(inner.mix_scratch.lock().unwrap().capacity(), capacity);
        }
    }

    #[test]
    fn realtime_mixing_of_an_oversized_buffer_serves_what_it_can_without_growing() {
        // A caller ignoring `MAX_REALTIME_QUANTUM_SAMPLES` gets a short read
        // rather than an allocation on the audio thread.
        let first = mixing_session(1, true);
        let second = mixing_session(2, true);
        let oversized = MAX_REALTIME_QUANTUM_SAMPLES + 512;
        first.incoming.push(&vec![0.5f32; oversized]);
        second.incoming.push(&vec![0.5f32; oversized]);
        let inner = mixing_engine(vec![first, second]);
        let capacity = inner.mix_scratch.lock().unwrap().capacity();
        let mut out = vec![0.0f32; oversized];
        let produced = inner.mix_playback(&mut out, true);
        assert!(produced <= MAX_REALTIME_QUANTUM_SAMPLES);
        assert_eq!(inner.mix_scratch.lock().unwrap().capacity(), capacity);
    }
}
