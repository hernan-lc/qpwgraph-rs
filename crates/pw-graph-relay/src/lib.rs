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

/// Maximum number of host-side enrollment transactions waiting for an
/// embedding to durably commit them. Keeping this bounded prevents a peer
/// from turning the application callback into an allocation DoS.
pub const MAX_PENDING_TRUST_ENROLLMENTS: usize = 64;
/// A host embedding has this long to persist an enrollment and accept it.
pub const TRUST_ENROLLMENT_TIMEOUT: Duration = Duration::from_secs(10);
/// Maximum discovered addresses retained for one stable peer identity.
pub const MAX_TRUSTED_CANDIDATE_ADDRESSES: usize = 16;
/// Maximum `(peer, address)` failure records retained for candidate backoff.
pub const MAX_TRUSTED_CANDIDATE_FAILURES: usize = 1024;
/// Maximum discovered addresses retained across all stable peer identities.
/// Discovery is untrusted input and must not be allowed to grow metadata maps.
pub const MAX_DISCOVERED_PEER_ADDRESSES: usize = 4096;
/// Maximum stable identities retained in the last-success preference cache.
pub const MAX_TRUSTED_SUCCESSFUL_ADDRESSES: usize = 1024;
/// Maximum trusted credentials accepted from an embedding or persistence
/// layer. Trusted-device management is user-controlled, but its backing table
/// still needs a hard bound against malformed or stale configuration.
pub const MAX_TRUSTED_PEERS: usize = 256;

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

enum EnrollmentDecision {
    Pending,
    Accepted,
    Rejected(String),
}

struct PendingEnrollment {
    session_id: SessionId,
    peer_id: String,
    secret: [u8; 32],
    created: Instant,
    decision: EnrollmentDecision,
}

struct EnrollmentResolution {
    peer_id: String,
    secret: [u8; 32],
    accepted: bool,
    reason: Option<String>,
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
    /// Stable identity advertised by the peer. Socket addresses are
    /// deliberately not identity: a tethered peer may have a Wi-Fi address
    /// and a USB address at different times.
    pub id: String,
    pub name: String,
    pub kind: DeviceKind,
    pub addr: SocketAddr,
}

/// A persistent bearer credential for one authenticated peer.
///
/// The engine never derives this from a PIN. It is generated after an
/// explicit PIN pairing and can therefore be used for later cable/network
/// discovery without weakening the one-time pairing exchange.
#[derive(Clone, PartialEq, Eq)]
pub struct TrustedPeer {
    pub peer_id: String,
    pub secret: [u8; 32],
}

impl fmt::Debug for TrustedPeer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TrustedPeer")
            .field("peer_id", &self.peer_id)
            .field("secret", &"<redacted>")
            .finish()
    }
}

/// Events drained by the host application (typically once per UI frame).
#[derive(Clone, PartialEq)]
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
    /// A fresh PIN pairing produced a persistent credential. Embeddings may
    /// store it in owner-only application storage and use `connect_trusted`
    /// when the same peer is discovered again.
    TrustedPeerAvailable {
        peer_id: String,
        peer: PeerInfo,
        secret: [u8; 32],
    },
    /// A host embedding must durably persist the credential obtained through
    /// [`RelayHandle::trusted_enrollment_secret`] and then call
    /// [`RelayHandle::accept_trusted_enrollment`]. No credential is imported
    /// into the live engine and no TrustAccepted is sent before that call.
    TrustedPeerEnrollmentRequested {
        transaction_id: u64,
        peer_id: String,
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

impl fmt::Debug for RelayEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TrustedPeerAvailable { peer_id, peer, .. } => formatter
                .debug_struct("TrustedPeerAvailable")
                .field("peer_id", peer_id)
                .field("peer", peer)
                .field("secret", &"<redacted>")
                .finish(),
            Self::TrustedPeerEnrollmentRequested {
                transaction_id,
                peer_id,
                peer,
            } => formatter
                .debug_struct("TrustedPeerEnrollmentRequested")
                .field("transaction_id", transaction_id)
                .field("peer_id", peer_id)
                .field("peer", peer)
                .finish(),
            Self::HostStarted { port } => formatter
                .debug_struct("HostStarted")
                .field("port", port)
                .finish(),
            Self::HostStopped => formatter.write_str("HostStopped"),
            Self::PeerDiscovered { peer } => formatter
                .debug_struct("PeerDiscovered")
                .field("peer", peer)
                .finish(),
            Self::PeerLost { peer } => formatter
                .debug_struct("PeerLost")
                .field("peer", peer)
                .finish(),
            Self::SessionEstablished {
                id,
                peer,
                roles,
                codec,
            } => formatter
                .debug_struct("SessionEstablished")
                .field("id", id)
                .field("peer", peer)
                .field("roles", roles)
                .field("codec", codec)
                .finish(),
            Self::SessionLost { id, reason } => formatter
                .debug_struct("SessionLost")
                .field("id", id)
                .field("reason", reason)
                .finish(),
            Self::AudioLevel { id, rms } => formatter
                .debug_struct("AudioLevel")
                .field("id", id)
                .field("rms", rms)
                .finish(),
            Self::Error { message } => formatter
                .debug_struct("Error")
                .field("message", message)
                .finish(),
        }
    }
}

/// Engine-wide configuration. Apply with [`RelayHandle::update_config`]
/// before starting a host; `connect` reads the audio parameters live.
#[derive(Clone, PartialEq)]
pub struct EngineConfig {
    /// Stable identity for this installation. It is advertised in discovery
    /// records and bound into trusted handshakes.
    pub device_id: String,
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
    /// Trusted peer credentials imported by the embedding application.
    pub trusted_peers: Vec<TrustedPeer>,
    /// Generate a trusted credential after an explicit PIN pairing. This is
    /// enabled by default so a user who pairs once gets real cable
    /// auto-connect; embedders that want PIN-only operation can disable it.
    pub trust_new_peers: bool,
}

impl fmt::Debug for EngineConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EngineConfig")
            .field("device_id", &self.device_id)
            .field("device_name", &self.device_name)
            .field("device_kind", &self.device_kind)
            .field("pin", &"<redacted>")
            .field("port", &self.port)
            .field("codec", &self.codec)
            .field("frame_ms", &self.frame_ms)
            .field("sample_rate", &self.sample_rate)
            .field("channels", &self.channels)
            .field("client_roles", &self.client_roles)
            .field("transport", &self.transport)
            .field("local_sample_rate", &self.local_sample_rate)
            .field("local_channels", &self.local_channels)
            .field("bind_addr", &self.bind_addr)
            .field("max_pending_handshakes", &self.max_pending_handshakes)
            .field("max_sessions", &self.max_sessions)
            .field("trusted_peers", &self.trusted_peers)
            .field("trust_new_peers", &self.trust_new_peers)
            .finish()
    }
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            device_id: generate_device_id(),
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
            trusted_peers: Vec::new(),
            trust_new_peers: true,
        }
    }
}

/// Generate a durable-format installation identity for an embedding that
/// wants to persist it outside the relay config.
pub fn generate_device_id() -> String {
    let mut bytes = [0u8; 16];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut bytes);
    pw_graph_utils::hex::hex_encode(&bytes)
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
    /// Carrier used by the session: `udp` for normal links or `adb-tcp` for
    /// ADB forwarding. This is diagnostic metadata, never an authorization
    /// signal.
    pub transport: String,
    /// Classified link used by the current peer address, when known.
    pub link: String,
    /// Local endpoint is not exposed until the transport has one to report.
    pub local_addr: Option<SocketAddr>,
    pub remote_addr: SocketAddr,
    pub control_state: String,
    pub audio_channel_state: String,
    pub trusted: bool,
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
    /// this into an apparently active session with no control owner. The
    /// generation is the in-progress generation, not the stale generation
    /// owned by the control watcher that entered the grace period.
    InProgress { generation: u64 },
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
    /// Credential generated by the client for the peer, or learned by the
    /// host from an authenticated enrollment message.
    pub trust_secret: Mutex<Option<[u8; 32]>>,
    /// Authenticated replacement stream for ADB-only operation. `None`
    /// means the session uses UDP audio.
    pub tcp_audio: Option<Arc<session::TcpAudioSlot>>,
    /// Interface-scoped UDP socket, replaceable after authenticated resume.
    pub udp_audio: Option<Arc<session::UdpAudioSlot>>,
    /// Current authenticated control peer address. The stable discovery
    /// address in `peer` remains the identity label; this field reports the
    /// path actually carrying the live control session.
    pub control_peer_addr: Mutex<SocketAddr>,
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
            return ResumeGraceResult::InProgress {
                generation: self.control_generation.load(Ordering::Acquire),
            };
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
            ControlState::Resuming { generation } => ResumeGraceResult::InProgress { generation },
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
    /// Imported persistent credentials, keyed by the remote stable identity.
    trusted_peers: Mutex<BTreeMap<String, [u8; 32]>>,
    /// Connections currently inside the pre-authentication handshake.
    pending_handshakes: AtomicU64,
    /// Durable-enrollment transactions awaiting an embedding decision.
    /// Secrets remain private to this map and are never included in the
    /// request event or diagnostics.
    pending_enrollments: Mutex<BTreeMap<u64, PendingEnrollment>>,
    next_enrollment: AtomicU64,
    host: Mutex<Option<session::HostRecord>>,
    /// Discovered (not necessarily connected) relay hosts, keyed by address.
    peers: Mutex<BTreeMap<SocketAddr, PeerInfo>>,
    /// Resolved addresses grouped by mDNS service identity.
    peer_services: Mutex<BTreeMap<String, BTreeMap<SocketAddr, PeerInfo>>>,
    /// Discovery metadata used only to rank candidate addresses. Identity is
    /// still proved by the trusted/resume handshake, never by this metadata.
    peer_links: Mutex<BTreeMap<SocketAddr, LinkKind>>,
    candidate_failures: Mutex<BTreeMap<(String, SocketAddr), FailureRecord>>,
    last_successful_addresses: Mutex<BTreeMap<String, SocketAddr>>,
    advertiser: Mutex<Option<discovery::Advertiser>>,
    browser: Mutex<Option<discovery::Browser>>,
    usb_scanner: Mutex<Option<usb_probe::UsbScanner>>,
    next_session: AtomicU64,
    running: AtomicBool,
}

impl EngineInner {
    fn new(config: EngineConfig) -> Arc<Self> {
        let trusted_peers = config
            .trusted_peers
            .iter()
            .take(MAX_TRUSTED_PEERS)
            .map(|peer| (peer.peer_id.clone(), peer.secret))
            .collect();
        Arc::new(Self {
            config: Mutex::new(config),
            events: Mutex::new(VecDeque::new()),
            // Allocated once, at the largest quantum the realtime callback
            // will ever present, so `mix_playback` never grows it. 64 KiB.
            mix_scratch: Mutex::new(Vec::with_capacity(MAX_REALTIME_QUANTUM_SAMPLES)),
            sessions: Mutex::new(BTreeMap::new()),
            pairing_failures: Mutex::new(BTreeMap::new()),
            trusted_peers: Mutex::new(trusted_peers),
            pending_handshakes: AtomicU64::new(0),
            pending_enrollments: Mutex::new(BTreeMap::new()),
            next_enrollment: AtomicU64::new(1),
            host: Mutex::new(None),
            peers: Mutex::new(BTreeMap::new()),
            peer_services: Mutex::new(BTreeMap::new()),
            peer_links: Mutex::new(BTreeMap::new()),
            candidate_failures: Mutex::new(BTreeMap::new()),
            last_successful_addresses: Mutex::new(BTreeMap::new()),
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

    pub(crate) fn candidate_allowed(&self, peer_id: &str, addr: SocketAddr) -> bool {
        let Ok(mut failures) = self.candidate_failures.lock() else {
            return true;
        };
        let key = (peer_id.to_owned(), addr);
        let Some(record) = failures.get(&key) else {
            return true;
        };
        if record.locked_until <= Instant::now() {
            failures.remove(&key);
            true
        } else {
            false
        }
    }

    pub(crate) fn note_candidate_failure(&self, peer_id: &str, addr: SocketAddr) {
        let Ok(mut failures) = self.candidate_failures.lock() else {
            return;
        };
        let now = Instant::now();
        let key = (peer_id.to_owned(), addr);
        if !failures.contains_key(&key) && failures.len() >= MAX_TRUSTED_CANDIDATE_FAILURES {
            failures.retain(|_, record| record.locked_until > now);
            if failures.len() >= MAX_TRUSTED_CANDIDATE_FAILURES {
                if let Some(oldest) = failures
                    .iter()
                    .min_by_key(|(_, record)| record.last_seen)
                    .map(|(key, _)| key.clone())
                {
                    failures.remove(&oldest);
                }
            }
        }
        let record = failures.entry(key).or_insert(FailureRecord {
            count: 0,
            locked_until: now,
            last_seen: now,
        });
        record.count = record.count.saturating_add(1);
        record.last_seen = now;
        let exponent = record.count.saturating_sub(1).min(6);
        let delay = Duration::from_millis(500u64.saturating_mul(1u64 << exponent));
        record.locked_until = now + delay.min(Duration::from_secs(30));
    }

    pub(crate) fn note_candidate_success(&self, peer_id: &str, addr: SocketAddr) {
        if let Ok(mut failures) = self.candidate_failures.lock() {
            failures.remove(&(peer_id.to_owned(), addr));
        }
        if let Ok(mut addresses) = self.last_successful_addresses.lock() {
            if !addresses.contains_key(peer_id)
                && addresses.len() >= MAX_TRUSTED_SUCCESSFUL_ADDRESSES
            {
                if let Some(oldest) = addresses.keys().next().cloned() {
                    addresses.remove(&oldest);
                }
            }
            addresses.insert(peer_id.to_owned(), addr);
        }
    }

    pub(crate) fn last_successful_address(&self, peer_id: &str) -> Option<SocketAddr> {
        self.last_successful_addresses
            .lock()
            .ok()
            .and_then(|addresses| addresses.get(peer_id).copied())
    }

    pub(crate) fn discovered_link(&self, addr: SocketAddr) -> Option<LinkKind> {
        self.peer_links.lock().ok()?.get(&addr).copied()
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

    pub(crate) fn trusted_secret(&self, peer_id: &str) -> Option<[u8; 32]> {
        self.trusted_peers.lock().ok()?.get(peer_id).copied()
    }

    pub(crate) fn remember_trusted_peer(&self, peer_id: String, secret: [u8; 32]) {
        if let Ok(mut peers) = self.trusted_peers.lock() {
            peers.insert(peer_id, secret);
        }
    }

    /// Start a host-side durable enrollment transaction. The secret is kept
    /// only in the bounded transaction table until the embedding confirms
    /// that its own durable store has committed it.
    pub(crate) fn begin_trusted_enrollment(
        &self,
        session_id: SessionId,
        peer_id: String,
        peer: PeerInfo,
        secret: [u8; 32],
    ) -> Result<u64, String> {
        if peer_id.trim().is_empty() || peer_id != peer.id {
            return Err("trusted peer identity did not match the session".into());
        }
        if secret.iter().all(|byte| *byte == 0) {
            return Err("trusted credential was malformed".into());
        }
        if self.trusted_secret(&peer_id).is_none()
            && self
                .trusted_peers
                .lock()
                .map(|peers| peers.len() >= MAX_TRUSTED_PEERS)
                .unwrap_or(true)
        {
            return Err("trusted credential capacity has been reached".into());
        }
        let now = Instant::now();
        let mut pending = self
            .pending_enrollments
            .lock()
            .map_err(|_| "trusted enrollment state is locked".to_string())?;
        pending.retain(|_, enrollment| {
            now.duration_since(enrollment.created) < TRUST_ENROLLMENT_TIMEOUT
        });
        if pending.len() >= MAX_PENDING_TRUST_ENROLLMENTS {
            return Err("too many trusted enrollments are pending".into());
        }
        if pending
            .values()
            .any(|enrollment| enrollment.peer_id == peer_id || enrollment.session_id == session_id)
        {
            return Err("a trusted enrollment for this peer is already pending".into());
        }
        let transaction_id = self.next_enrollment.fetch_add(1, Ordering::Relaxed);
        pending.insert(
            transaction_id,
            PendingEnrollment {
                session_id,
                peer_id,
                secret,
                created: now,
                decision: EnrollmentDecision::Pending,
            },
        );
        Ok(transaction_id)
    }

    /// Return a transaction's secret to the embedding that owns durable
    /// storage. Callers should copy it only for the persistence operation and
    /// then immediately accept or reject the transaction.
    pub(crate) fn trusted_enrollment_secret(&self, transaction_id: u64) -> Option<[u8; 32]> {
        let pending = self.pending_enrollments.lock().ok()?;
        let enrollment = pending.get(&transaction_id)?;
        (enrollment.created.elapsed() < TRUST_ENROLLMENT_TIMEOUT).then_some(enrollment.secret)
    }

    pub(crate) fn accept_trusted_enrollment(&self, transaction_id: u64) -> RelayResult<()> {
        let mut pending = self
            .pending_enrollments
            .lock()
            .map_err(|_| RelayError::Engine("trusted enrollment state is locked".into()))?;
        let enrollment = pending
            .get_mut(&transaction_id)
            .ok_or_else(|| RelayError::Engine("trusted enrollment expired or is unknown".into()))?;
        if enrollment.created.elapsed() >= TRUST_ENROLLMENT_TIMEOUT {
            pending.remove(&transaction_id);
            return Err(RelayError::Engine("trusted enrollment expired".into()));
        }
        match &enrollment.decision {
            EnrollmentDecision::Pending => enrollment.decision = EnrollmentDecision::Accepted,
            EnrollmentDecision::Accepted => {}
            EnrollmentDecision::Rejected(_) => {
                return Err(RelayError::Engine("trusted enrollment was rejected".into()))
            }
        }
        Ok(())
    }

    pub(crate) fn reject_trusted_enrollment(
        &self,
        transaction_id: u64,
        reason: String,
    ) -> RelayResult<()> {
        let mut pending = self
            .pending_enrollments
            .lock()
            .map_err(|_| RelayError::Engine("trusted enrollment state is locked".into()))?;
        let enrollment = pending
            .get_mut(&transaction_id)
            .ok_or_else(|| RelayError::Engine("trusted enrollment expired or is unknown".into()))?;
        if enrollment.created.elapsed() >= TRUST_ENROLLMENT_TIMEOUT {
            pending.remove(&transaction_id);
            return Err(RelayError::Engine("trusted enrollment expired".into()));
        }
        if matches!(&enrollment.decision, EnrollmentDecision::Accepted) {
            return Err(RelayError::Engine(
                "trusted enrollment was already accepted".into(),
            ));
        }
        enrollment.decision = EnrollmentDecision::Rejected(if reason.trim().is_empty() {
            "trusted enrollment rejected".into()
        } else {
            reason
        });
        Ok(())
    }

    /// Resolve one transaction belonging to a session. Expiry is resolved as
    /// a rejection, so the client never waits forever for a host callback.
    pub(crate) fn take_trusted_enrollment(
        &self,
        session_id: SessionId,
    ) -> Option<EnrollmentResolution> {
        let mut pending = self.pending_enrollments.lock().ok()?;
        let transaction_id = pending
            .iter()
            .filter(|(_, enrollment)| enrollment.session_id == session_id)
            .min_by_key(|(_, enrollment)| enrollment.created)
            .map(|(id, _)| *id)?;
        let enrollment = pending.get(&transaction_id)?;
        let expired = enrollment.created.elapsed() >= TRUST_ENROLLMENT_TIMEOUT;
        let accepted = matches!(&enrollment.decision, EnrollmentDecision::Accepted);
        let reason = match &enrollment.decision {
            EnrollmentDecision::Rejected(reason) => Some(reason.clone()),
            EnrollmentDecision::Pending if expired => Some("trusted enrollment timed out".into()),
            _ => None,
        };
        if !accepted && reason.is_none() {
            return None;
        }
        let enrollment = pending.remove(&transaction_id)?;
        Some(EnrollmentResolution {
            peer_id: enrollment.peer_id,
            secret: enrollment.secret,
            accepted,
            reason,
        })
    }

    pub(crate) fn remove_trusted_peer(&self, peer_id: &str) -> bool {
        let removed = self
            .trusted_peers
            .lock()
            .ok()
            .and_then(|mut peers| peers.remove(peer_id))
            .is_some();
        let ids = self
            .sessions
            .lock()
            .map(|sessions| {
                sessions
                    .values()
                    .filter(|record| record.peer.id == peer_id)
                    .map(|record| record.id)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for id in &ids {
            session::teardown(self, *id, "trusted peer was revoked".into());
        }
        if let Ok(mut pending) = self.pending_enrollments.lock() {
            pending.retain(|_, enrollment| enrollment.peer_id != peer_id);
        }
        removed || !ids.is_empty()
    }

    pub(crate) fn trusted_peers(&self) -> Vec<TrustedPeer> {
        self.trusted_peers
            .lock()
            .map(|peers| {
                peers
                    .iter()
                    .map(|(peer_id, secret)| TrustedPeer {
                        peer_id: peer_id.clone(),
                        secret: *secret,
                    })
                    .collect()
            })
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
            if realtime {
                if !converter.try_convert_prepared(samples, buffer) {
                    accepted = false;
                    continue;
                }
            } else {
                converter.convert(samples, buffer);
            }
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
        let host = self.host.lock().ok().and_then(|host| {
            host.as_ref()
                .map(|record| (record.port, record.bind_addr()))
        });
        let (host_port, host_addr) = host
            .map(|(port, addr)| (Some(port), addr))
            .unwrap_or((None, None));
        let config = self.config();
        let sessions = self
            .sessions
            .lock()
            .map(|sessions| {
                sessions
                    .values()
                    .map(|record| {
                        let control_state = record
                            .control_state
                            .lock()
                            .map(|state| match *state {
                                ControlState::Active => "active",
                                ControlState::ResumeEligible { .. } => "resume-eligible",
                                ControlState::Resuming { .. } => "resuming",
                            })
                            .unwrap_or("unknown")
                            .to_string();
                        let audio_over_tcp = record.tcp_audio.is_some();
                        let audio_channel_state = if let Some(audio) = &record.tcp_audio {
                            if audio.is_active() {
                                "active"
                            } else {
                                "reconnecting"
                            }
                        } else {
                            "active"
                        };
                        let remote_addr = record
                            .control_peer_addr
                            .lock()
                            .ok()
                            .map(|addr| *addr)
                            .unwrap_or(record.peer.addr);
                        let link = if config.transport == TransportPreference::Adb {
                            "loopback"
                        } else {
                            self.discovered_link(remote_addr)
                                .map(LinkKind::as_str)
                                .unwrap_or("unknown")
                        };
                        let local_addr = record
                            .udp_audio
                            .as_ref()
                            .and_then(|socket| socket.local_addr());
                        let trusted = record
                            .trust_secret
                            .lock()
                            .ok()
                            .and_then(|secret| *secret)
                            .is_some()
                            || self.trusted_secret(&record.peer.id).is_some();
                        SessionStatus {
                            id: record.id,
                            peer: record.peer.clone(),
                            roles: record.roles,
                            codec: record.codec,
                            sending: record.sending,
                            receiving: record.receiving,
                            transport: if audio_over_tcp { "adb-tcp" } else { "udp" }.into(),
                            link: link.into(),
                            local_addr,
                            remote_addr,
                            control_state,
                            audio_channel_state: audio_channel_state.into(),
                            trusted,
                        }
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DiscoveryStartOutcome {
    MdnsAndUsb,
    MdnsOnly,
    UsbOnly,
}

/// Decide whether discovery can remain usable when one of its independent
/// mechanisms fails. Keeping this decision separate from worker startup makes
/// the partial-failure contract explicit and testable.
fn discovery_start_outcome(
    mdns: &RelayResult<()>,
    usb: &RelayResult<()>,
) -> RelayResult<DiscoveryStartOutcome> {
    match (mdns.is_ok(), usb.is_ok()) {
        (true, true) => Ok(DiscoveryStartOutcome::MdnsAndUsb),
        (true, false) => Ok(DiscoveryStartOutcome::MdnsOnly),
        (false, true) => Ok(DiscoveryStartOutcome::UsbOnly),
        (false, false) => Err(RelayError::Engine(format!(
            "all relay discovery mechanisms failed (mDNS: {}; USB: {})",
            mdns.as_ref().unwrap_err(),
            usb.as_ref().unwrap_err()
        ))),
    }
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
        if let Ok(mut peers) = self.inner.trusted_peers.lock() {
            peers.clear();
            for peer in self
                .inner
                .config()
                .trusted_peers
                .into_iter()
                .take(MAX_TRUSTED_PEERS)
            {
                peers.insert(peer.peer_id, peer.secret);
            }
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
        let bind_addr = record.bind_addr();
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
            record.stop();
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

    /// Connect using a credential created by a previous explicit PIN pairing.
    /// The stable peer id is part of the authenticated transcript, so a
    /// credential cannot be replayed against an unrelated discovered host.
    pub fn connect_trusted(
        &self,
        target: SocketAddr,
        peer_id: &str,
        secret: [u8; 32],
        roles: Roles,
    ) -> SessionId {
        let id = self.inner.next_session_id();
        session::connect_trusted_peer(&self.inner, id, target, peer_id.to_owned(), secret, roles);
        id
    }

    /// Return the secret held by a pending host enrollment transaction. The
    /// embedding must durably commit this value before accepting the
    /// transaction. It is intentionally not carried in the request event.
    pub fn trusted_enrollment_secret(&self, transaction_id: u64) -> Option<[u8; 32]> {
        self.inner.trusted_enrollment_secret(transaction_id)
    }

    /// Commit a pending trusted enrollment after durable application
    /// persistence has succeeded. Only after the control thread observes this
    /// decision does it import the credential and send TrustAccepted.
    pub fn accept_trusted_enrollment(&self, transaction_id: u64) -> RelayResult<()> {
        self.inner.accept_trusted_enrollment(transaction_id)
    }

    /// Reject a pending trusted enrollment. The client is not told to retain
    /// the credential and the live trusted map is unchanged.
    pub fn reject_trusted_enrollment(
        &self,
        transaction_id: u64,
        reason: impl Into<String>,
    ) -> RelayResult<()> {
        self.inner
            .reject_trusted_enrollment(transaction_id, reason.into())
    }

    /// Remove a trusted identity immediately from the live engine. An
    /// embedding should remove the same record from its durable store too.
    pub fn remove_trusted_peer(&self, peer_id: &str) -> RelayResult<()> {
        if peer_id.trim().is_empty() {
            return Err(RelayError::Config(
                "trusted peer id must not be empty".into(),
            ));
        }
        self.inner.remove_trusted_peer(peer_id);
        Ok(())
    }

    /// Snapshot of live trusted identities. Secrets are returned only because
    /// this API is used to rebuild an engine's authenticated configuration;
    /// status and event JSON never expose them.
    pub fn trusted_peers(&self) -> Vec<TrustedPeer> {
        self.inner.trusted_peers()
    }

    /// Begin browsing for relay hosts on the local network. Discovered peers
    /// arrive as [`RelayEvent::PeerDiscovered`]. Runs mDNS alongside a direct
    /// probe of USB tether subnets, because mDNS often does not cross a USB
    /// tether. Idempotent.
    pub fn discovery_start(&self) -> RelayResult<()> {
        // mDNS and direct USB probing are independent mechanisms. A multicast
        // failure is expected on some tethered networks and must not prevent
        // the mechanism designed for exactly that case from starting.
        let mdns = self.inner.start_browser();
        let usb = self.inner.start_usb_scanner();
        match discovery_start_outcome(&mdns, &usb)? {
            DiscoveryStartOutcome::MdnsAndUsb => Ok(()),
            DiscoveryStartOutcome::MdnsOnly => {
                let error = usb.expect_err("mDNS-only discovery must have a USB error");
                self.inner.emit(RelayEvent::Error {
                    message: format!("USB relay discovery unavailable: {error}"),
                });
                Ok(())
            }
            DiscoveryStartOutcome::UsbOnly => {
                let error = mdns.expect_err("USB-only discovery must have an mDNS error");
                self.inner.emit(RelayEvent::Error {
                    message: format!("mDNS relay discovery unavailable: {error}"),
                });
                Ok(())
            }
        }
    }

    /// Stop browsing for relay hosts. Idempotent.
    pub fn discovery_stop(&self) {
        self.inner.stop_browser();
        self.inner.stop_usb_scanner();
        self.inner.clear_discovered();
    }

    /// Forget direct USB-probe results while leaving mDNS browsing active.
    /// Platform link watchers call this as soon as a tether disappears; the
    /// scanner also performs the same refresh when its next loop observes it.
    pub fn discovery_usb_link_lost(&self) {
        self.inner.lost_peer(usb_probe::USB_PROBE_SERVICE);
    }

    /// Snapshot of relay hosts discovered so far.
    pub fn discovered_peers(&self) -> Vec<PeerInfo> {
        self.inner.discovered_peers()
    }

    /// Discovery snapshot with non-authoritative link classification for
    /// candidate ranking and diagnostics. The link is public metadata; only a
    /// successful authenticated handshake establishes peer identity.
    pub fn discovered_peer_candidates(&self) -> Vec<(PeerInfo, Option<LinkKind>)> {
        let peers = self.inner.discovered_peers();
        let links = self.inner.peer_links.lock().ok();
        peers
            .into_iter()
            .map(|peer| {
                let link = links
                    .as_ref()
                    .and_then(|links| links.get(&peer.addr).copied());
                (peer, link)
            })
            .collect()
    }

    /// Supply peer addresses discovered by an embedding-owned browser.
    ///
    /// Some platform adapters keep discovery in a separate engine (for
    /// example, an Android browser handle that must outlive client handles).
    /// Feeding its identity-tagged snapshot into the client engine lets an
    /// in-progress session resume over a newly visible address without
    /// allowing an unrelated host with the same port to become a target.
    pub fn update_discovered_peers(&self, peers: Vec<PeerInfo>) {
        self.inner
            .refresh_service("embedding-discovery._qpw-relay._udp.local.", peers);
    }

    /// Supply an embedding-owned discovery snapshot with link hints for
    /// candidate ranking. The hints are public routing metadata only; resume
    /// and trusted authentication still prove the peer identity.
    pub fn update_discovered_peer_candidates(&self, peers: Vec<(PeerInfo, Option<LinkKind>)>) {
        self.inner.refresh_embedding_candidates(peers);
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
    use std::net::{Ipv4Addr, Ipv6Addr, TcpListener};

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
    fn discovery_start_accepts_each_working_mechanism_and_rejects_both_failures() {
        let ok = Ok(());
        let mdns_error = Err(RelayError::Engine("mDNS unavailable".into()));
        let usb_error = Err(RelayError::Engine("USB scanner unavailable".into()));

        assert!(matches!(
            discovery_start_outcome(&ok, &ok),
            Ok(DiscoveryStartOutcome::MdnsAndUsb)
        ));
        assert!(matches!(
            discovery_start_outcome(&ok, &usb_error),
            Ok(DiscoveryStartOutcome::MdnsOnly)
        ));
        assert!(matches!(
            discovery_start_outcome(&mdns_error, &ok),
            Ok(DiscoveryStartOutcome::UsbOnly)
        ));
        assert!(matches!(
            discovery_start_outcome(&mdns_error, &usb_error),
            Err(RelayError::Engine(message))
                if message.contains("mDNS unavailable")
                    && message.contains("USB scanner unavailable")
        ));
    }

    #[test]
    fn host_start_requires_a_pin() {
        let engine = RelayEngine::start(EngineConfig::default()).unwrap();
        let handle = engine.handle();
        assert!(handle.host_start().is_err());
    }

    #[test]
    fn host_start_reports_a_port_conflict_without_falling_back_to_another_port() {
        let occupied = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = occupied.local_addr().unwrap().port();
        let engine = RelayEngine::start(EngineConfig {
            pin: "123456".into(),
            port,
            bind_addr: Some(Ipv4Addr::LOCALHOST),
            ..EngineConfig::default()
        })
        .unwrap();
        let handle = engine.handle();
        let error = handle
            .host_start()
            .expect_err("the occupied port must fail");
        assert!(error.to_string().contains("control port"));
        assert!(error.to_string().contains(&port.to_string()));
        assert!(!handle.status().host_active);
        assert_eq!(handle.status().host_port, None);
        engine.shutdown();
    }

    #[test]
    fn host_stop_releases_an_explicit_port_before_returning() {
        let reservation = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = reservation.local_addr().unwrap().port();
        drop(reservation);
        let engine = RelayEngine::start(EngineConfig {
            pin: "123456".into(),
            port,
            bind_addr: Some(Ipv4Addr::LOCALHOST),
            ..EngineConfig::default()
        })
        .unwrap();
        let handle = engine.handle();
        assert_eq!(handle.host_start().unwrap(), port);
        handle.host_stop().unwrap();
        assert_eq!(handle.host_start().unwrap(), port);
        handle.host_stop().unwrap();
        engine.shutdown();
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
        assert_eq!(
            record.expire_resume_grace(1),
            ResumeGraceResult::InProgress { generation }
        );
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
    fn resume_expiry_aborts_the_in_progress_generation_not_the_stale_owner() {
        let record = mixing_session(4, false);
        assert!(record.mark_control_dropped());
        let generation = record.begin_resume().expect("resume is eligible");
        assert_eq!(generation, 2);

        // The old control watcher still owns generation 1. Once the grace
        // deadline is reached, the result names generation 2 so the stalled
        // challenge can be cancelled precisely.
        assert_eq!(
            record.expire_resume_grace(1),
            ResumeGraceResult::InProgress { generation: 2 }
        );
        assert!(record.abort_resume(2));
        assert_eq!(*record.control_state.lock().unwrap(), ControlState::Active);
        assert!(!record.abort_resume(1));
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

    fn enrollment_peer(id: &str) -> PeerInfo {
        PeerInfo {
            id: id.into(),
            name: format!("{id}-name"),
            kind: DeviceKind::Other,
            addr: "192.168.42.2:48123".parse().unwrap(),
        }
    }

    #[test]
    fn trusted_enrollment_is_not_imported_until_the_embedding_commits() {
        let inner = EngineInner::new(EngineConfig::default());
        let peer = enrollment_peer("phone");
        let secret = [7u8; 32];
        let transaction = inner
            .begin_trusted_enrollment(SessionId(9), peer.id.clone(), peer.clone(), secret)
            .unwrap();

        assert_eq!(inner.trusted_secret(&peer.id), None);
        assert_eq!(inner.trusted_enrollment_secret(transaction), Some(secret));
        inner.accept_trusted_enrollment(transaction).unwrap();
        let resolution = inner.take_trusted_enrollment(SessionId(9)).unwrap();
        assert!(resolution.accepted);
        inner.remember_trusted_peer(resolution.peer_id, resolution.secret);
        assert_eq!(inner.trusted_secret(&peer.id), Some(secret));
    }

    #[test]
    fn failed_or_rejected_enrollment_preserves_the_previous_credential() {
        let inner = EngineInner::new(EngineConfig::default());
        let peer = enrollment_peer("phone");
        let old = [3u8; 32];
        let new = [4u8; 32];
        inner.remember_trusted_peer(peer.id.clone(), old);
        let transaction = inner
            .begin_trusted_enrollment(SessionId(10), peer.id.clone(), peer, new)
            .unwrap();
        inner
            .reject_trusted_enrollment(transaction, "simulated persistence failure".into())
            .unwrap();
        let resolution = inner.take_trusted_enrollment(SessionId(10)).unwrap();
        assert!(!resolution.accepted);
        assert_eq!(inner.trusted_secret("phone"), Some(old));
    }

    #[test]
    fn enrollment_rejects_malformed_duplicate_and_mismatched_requests() {
        let inner = EngineInner::new(EngineConfig::default());
        let peer = enrollment_peer("phone");
        assert!(inner
            .begin_trusted_enrollment(SessionId(1), peer.id.clone(), peer.clone(), [0u8; 32],)
            .is_err());
        assert!(inner
            .begin_trusted_enrollment(SessionId(1), "other".into(), peer.clone(), [1u8; 32],)
            .is_err());
        let transaction = inner
            .begin_trusted_enrollment(SessionId(1), peer.id.clone(), peer.clone(), [1u8; 32])
            .unwrap();
        assert!(inner
            .begin_trusted_enrollment(SessionId(1), peer.id.clone(), peer, [2u8; 32])
            .is_err());
        assert!(inner
            .begin_trusted_enrollment(
                SessionId(2),
                "another".into(),
                enrollment_peer("another"),
                [2u8; 32],
            )
            .is_ok());
        inner
            .reject_trusted_enrollment(transaction, "duplicate".into())
            .unwrap();
    }

    #[test]
    fn enrollment_expiry_is_bounded_and_does_not_expose_the_secret() {
        let inner = EngineInner::new(EngineConfig::default());
        let peer = enrollment_peer("phone");
        let peer_id = peer.id.clone();
        let transaction = inner
            .begin_trusted_enrollment(SessionId(3), peer_id, peer, [9u8; 32])
            .unwrap();
        inner
            .pending_enrollments
            .lock()
            .unwrap()
            .get_mut(&transaction)
            .unwrap()
            .created = Instant::now() - TRUST_ENROLLMENT_TIMEOUT - Duration::from_secs(1);
        assert_eq!(inner.trusted_enrollment_secret(transaction), None);
        assert!(inner.accept_trusted_enrollment(transaction).is_err());
    }

    #[test]
    fn revocation_removes_the_live_credential_without_affecting_other_peers() {
        let inner = EngineInner::new(EngineConfig::default());
        inner.remember_trusted_peer("one".into(), [1u8; 32]);
        inner.remember_trusted_peer("two".into(), [2u8; 32]);
        assert!(inner.remove_trusted_peer("one"));
        assert_eq!(inner.trusted_secret("one"), None);
        assert_eq!(inner.trusted_secret("two"), Some([2u8; 32]));
    }

    #[test]
    fn candidate_backoff_is_scoped_to_one_peer_and_address() {
        let inner = EngineInner::new(EngineConfig::default());
        let fake: SocketAddr = "192.168.1.66:48123".parse().unwrap();
        let real: SocketAddr = "192.168.42.1:48123".parse().unwrap();
        inner.note_candidate_failure("host", fake);
        assert!(!inner.candidate_allowed("host", fake));
        assert!(inner.candidate_allowed("host", real));
        assert!(inner.candidate_allowed("other", fake));
        inner.note_candidate_success("host", real);
        assert_eq!(inner.last_successful_address("host"), Some(real));
        assert!(!inner
            .candidate_failures
            .lock()
            .unwrap()
            .contains_key(&("host".into(), real)));
    }

    #[test]
    fn trusted_peer_debug_redacts_the_secret() {
        let secret = [0xabu8; 32];
        let text = format!(
            "{:?}",
            TrustedPeer {
                peer_id: "phone".into(),
                secret
            }
        );
        assert!(!text.contains("ab".repeat(32).as_str()));
        assert!(text.contains("redacted"));
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
                id: format!("peer-{id}"),
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
            trust_secret: Mutex::new(None),
            tcp_audio: None,
            udp_audio: None,
            control_peer_addr: Mutex::new("127.0.0.1:1".parse().unwrap()),
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
