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
pub use netlink::{LinkKind, LocalLink, TransportPreference};
pub use protocol::{CodecKind, DeviceKind, Roles};
pub use queue::{PcmQueue, CAPTURE_DEPTH_FRAMES, DEFAULT_QUEUE_CAPACITY, PLAYBACK_DEPTH_FRAMES};

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use thiserror::Error;

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
        }
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
    pub sessions: Vec<SessionStatus>,
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
    /// Bumped each time a control thread takes over the session (initial
    /// handshake and every successful resume). Host-side grace waits compare
    /// generations to notice a replacement.
    pub control_generation: AtomicU64,
    /// One resume takeover at a time; racing reconnects are rejected while
    /// it is set.
    pub resuming: AtomicBool,
    /// UDP address of the peer's audio socket, learned from its first
    /// datagram. Senders poll this until it is known.
    pub peer_audio_addr: Mutex<Option<SocketAddr>>,
    /// Per-session transmit queue so one capture stream fans out to every
    /// receiving peer without competing consumers.
    pub outgoing: PcmQueue,
}

pub(crate) struct EngineInner {
    config: Mutex<EngineConfig>,
    events: Mutex<VecDeque<RelayEvent>>,
    /// Decoded audio arriving from peers; drained by `pull_playback`.
    pub incoming: PcmQueue,
    sessions: Mutex<BTreeMap<SessionId, Arc<SessionRecord>>>,
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
            incoming: PcmQueue::new(DEFAULT_QUEUE_CAPACITY),
            sessions: Mutex::new(BTreeMap::new()),
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
        if let Ok(mut events) = self.events.lock() {
            events.push_back(event);
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

    fn insert_session(&self, record: Arc<SessionRecord>) {
        if let Ok(mut sessions) = self.sessions.lock() {
            sessions.insert(record.id, record);
        }
    }

    fn session(&self, id: SessionId) -> Option<Arc<SessionRecord>> {
        self.sessions.lock().ok()?.get(&id).cloned()
    }

    fn session_alive(&self, id: SessionId) -> bool {
        self.running.load(Ordering::Relaxed)
            && self
                .sessions
                .lock()
                .map(|sessions| sessions.contains_key(&id))
                .unwrap_or(false)
    }

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
        for record in sessions.values().filter(|record| record.sending) {
            let pushed = if realtime {
                record.outgoing.try_push(samples)
            } else {
                record.outgoing.push(samples);
                true
            };
            accepted &= pushed;
        }
        accepted
    }

    fn remove_session(&self, id: SessionId) -> Option<Arc<SessionRecord>> {
        let record = self.sessions.lock().ok()?.remove(&id);
        if let Some(record) = &record {
            record.stop.store(true, Ordering::Relaxed);
        }
        record
    }

    fn status(&self) -> EngineStatus {
        let host_port = self
            .host
            .lock()
            .ok()
            .and_then(|host| host.as_ref().map(|record| record.port));
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
        if config.pin.trim().is_empty() {
            return Err(RelayError::Engine(
                "a pairing PIN must be configured before hosting".into(),
            ));
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
        *host = Some(record);
        drop(host);
        // Advertise over mDNS so peers can find us (best-effort).
        self.inner.start_advertiser(port);
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

    /// Feed audio to transmit (e.g. the virtual relay sink tap). Oldest
    /// samples are dropped when the queue overflows.
    pub fn push_capture(&self, samples: &[f32]) {
        self.inner.broadcast_capture(samples, false);
    }

    /// Realtime-safe variant of [`Self::push_capture`].
    pub fn try_push_capture(&self, samples: &[f32]) -> bool {
        self.inner.broadcast_capture(samples, true)
    }

    /// Take decoded audio received from peers (e.g. into the virtual relay
    /// microphone). Returns the number of samples written to `out`.
    pub fn pull_playback(&self, out: &mut [f32]) -> usize {
        self.inner.incoming.pull(out)
    }

    /// Realtime-safe variant of [`Self::pull_playback`].
    pub fn try_pull_playback(&self, out: &mut [f32]) -> usize {
        self.inner.incoming.try_pull(out)
    }

    pub fn status(&self) -> EngineStatus {
        self.inner.status()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
