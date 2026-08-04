//! `pw-graph-relay-sdk` — the stable public surface of the qpwgraph-rs
//! audio relay.
//!
//! This crate is what third-party applications depend on. It wraps the
//! [`pw_graph_relay`] engine with role-oriented builders:
//!
//! - [`RelayHost`] — run on a PC: accepts phone/peer connections, exposes
//!   incoming peer audio via [`RelayHost::pull_playback`] and broadcasts
//!   audio fed through [`RelayHost::push_capture`].
//! - [`RelayClient`] — run anywhere (Linux desktop, Android via JNI): emit
//!   captured microphone audio to a host and/or receive host audio for
//!   playback.
//!
//! The SDK is audio-IO agnostic: you push captured PCM and pull playback PCM.
//! That keeps it portable — PipeWire on Linux, AAudio/OpenSL ES on Android,
//! WASAPI/CoreAudio later.
//!
//! ## Host example
//!
//! ```no_run
//! use pw_graph_relay_sdk::RelayHostBuilder;
//!
//! let host = RelayHostBuilder::new()
//!     .device_name("studio-pc")
//!     .pin("123456")
//!     .build()
//!     .expect("builder")
//!     .start()
//!     .expect("host starts");
//! println!("listening on port {}", host.port());
//! // In your audio loop:
//! //   let mut buffer = [0.0f32; 960];
//! //   let n = host.pull_playback(&mut buffer); // peer mic audio
//! //   host.push_capture(&pc_audio);            // sent to listening peers
//! ```
//!
//! ## Client example (phone-as-microphone)
//!
//! ```no_run
//! use pw_graph_relay_sdk::{RelayClientBuilder, Role};
//!
//! let client = RelayClientBuilder::new()
//!     .role(Role::Emit)
//!     .build()
//!     .expect("builder")
//!     .connect("192.168.1.20:48123", "123456")
//!     .expect("connect");
//! // In your capture loop: client.send_capture(&mic_pcm);
//! // In your render loop: client.pull_playback(&mut buffer);
//! ```
//!
//! Wire protocol: see `docs/relay-protocol.md` in the qpwgraph-rs
//! repository.

pub use pw_graph_relay::{
    CodecKind, DeviceKind, EngineConfig, EngineStatus, LinkKind, LocalLink, PeerInfo, RelayError,
    RelayEvent, RelayResult, Roles, SessionId, SessionStatus, TransportPreference,
};

use pw_graph_relay::{RelayEngine, RelayHandle};
use std::net::{SocketAddr, ToSocketAddrs};
use std::time::{Duration, Instant};

/// The role a client takes in a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// Send captured audio to the host (phone-as-microphone).
    Emit,
    /// Receive host audio for playback (phone-as-speaker).
    Receive,
    /// Carry both directions at once.
    Both,
}

impl Role {
    fn to_roles(self) -> Roles {
        match self {
            Self::Emit => Roles::emit_only(),
            Self::Receive => Roles::receive_only(),
            Self::Both => Roles::both(),
        }
    }
}

/// Builder for [`RelayHost`].
#[derive(Clone, Debug)]
pub struct RelayHostBuilder {
    config: EngineConfig,
}

impl RelayHostBuilder {
    pub fn new() -> Self {
        Self {
            config: EngineConfig::default(),
        }
    }

    pub fn device_name(mut self, name: impl Into<String>) -> Self {
        self.config.device_name = name.into();
        self
    }

    /// Pairing PIN clients must present. Required before [`Self::start`].
    pub fn pin(mut self, pin: impl Into<String>) -> Self {
        self.config.pin = pin.into();
        self
    }

    /// TCP control port; 0 picks an ephemeral port (default).
    pub fn port(mut self, port: u16) -> Self {
        self.config.port = port;
        self
    }

    pub fn codec(mut self, codec: CodecKind) -> Self {
        self.config.codec = codec;
        self
    }

    /// Preferred transport link (default: auto-select the best available).
    pub fn transport(mut self, transport: TransportPreference) -> Self {
        self.config.transport = transport;
        self
    }

    pub fn build(self) -> RelayResult<RelayHostPrepared> {
        Ok(RelayHostPrepared {
            config: self.config,
        })
    }
}

impl Default for RelayHostBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// A validated host that has not started listening yet.
pub struct RelayHostPrepared {
    config: EngineConfig,
}

impl RelayHostPrepared {
    /// Start listening. Returns the running host.
    pub fn start(self) -> RelayResult<RelayHost> {
        let engine = RelayEngine::start(self.config)?;
        let handle = engine.handle();
        let port = handle.host_start()?;
        Ok(RelayHost {
            _engine: engine,
            handle,
            port,
        })
    }
}

/// A running relay host.
pub struct RelayHost {
    _engine: RelayEngine,
    handle: RelayHandle,
    port: u16,
}

impl RelayHost {
    /// The TCP control port peers connect to.
    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn handle(&self) -> RelayHandle {
        self.handle.clone()
    }

    /// Audio received from emitting peers (e.g. phone microphones).
    pub fn pull_playback(&self, out: &mut [f32]) -> usize {
        self.handle.pull_playback(out)
    }

    /// Realtime-safe variant of [`Self::pull_playback`].
    pub fn try_pull_playback(&self, out: &mut [f32]) -> usize {
        self.handle.try_pull_playback(out)
    }

    /// Audio to broadcast to receiving peers (e.g. the PC's relay sink tap).
    pub fn push_capture(&self, samples: &[f32]) {
        self.handle.push_capture(samples);
    }

    /// Realtime-safe variant of [`Self::push_capture`].
    pub fn try_push_capture(&self, samples: &[f32]) -> bool {
        self.handle.try_push_capture(samples)
    }

    /// Drain pending events (session established/lost, levels, errors).
    pub fn events(&self) -> Vec<RelayEvent> {
        self.handle.events()
    }

    pub fn status(&self) -> EngineStatus {
        self.handle.status()
    }

    /// End one session.
    pub fn disconnect(&self, session: SessionId) -> RelayResult<()> {
        self.handle.disconnect(session)
    }
}

/// Builder for [`RelayClient`].
#[derive(Clone, Debug)]
pub struct RelayClientBuilder {
    config: EngineConfig,
    role: Role,
}

impl RelayClientBuilder {
    pub fn new() -> Self {
        Self {
            config: EngineConfig::default(),
            role: Role::Emit,
        }
    }

    pub fn device_name(mut self, name: impl Into<String>) -> Self {
        self.config.device_name = name.into();
        self
    }

    pub fn device_kind(mut self, kind: DeviceKind) -> Self {
        self.config.device_kind = kind;
        self
    }

    pub fn role(mut self, role: Role) -> Self {
        self.role = role;
        self
    }

    pub fn codec(mut self, codec: CodecKind) -> Self {
        self.config.codec = codec;
        self
    }

    /// Preferred transport link (default: auto-select the best available).
    pub fn transport(mut self, transport: TransportPreference) -> Self {
        self.config.transport = transport;
        self
    }

    /// Audio format used for capture/playback PCM passed to the client.
    pub fn audio(mut self, sample_rate: u32, channels: u16, frame_ms: u16) -> Self {
        self.config.sample_rate = sample_rate;
        self.config.channels = channels;
        self.config.frame_ms = frame_ms;
        self
    }

    pub fn build(self) -> RelayResult<RelayClientPrepared> {
        Ok(RelayClientPrepared {
            config: self.config,
            role: self.role,
        })
    }
}

impl Default for RelayClientBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// A validated client that has not connected yet.
pub struct RelayClientPrepared {
    config: EngineConfig,
    role: Role,
}

impl RelayClientPrepared {
    /// Connect to a host. Blocks until the handshake completes or fails.
    pub fn connect(self, target: &str, pin: &str) -> RelayResult<RelayClient> {
        let addr = resolve(target)?;
        let engine = RelayEngine::start(self.config)?;
        let handle = engine.handle();
        let session = handle.connect(addr, pin, self.role.to_roles());

        // Wait for the handshake outcome so callers get synchronous errors.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            for event in handle.events() {
                match event {
                    RelayEvent::SessionEstablished { id, peer, .. } if id == session => {
                        return Ok(RelayClient {
                            _engine: engine,
                            handle,
                            session,
                            host_name: peer.name,
                        });
                    }
                    RelayEvent::SessionLost { id, reason } if id == session => {
                        return Err(RelayError::Engine(format!(
                            "connection to {addr} failed: {reason}"
                        )));
                    }
                    _ => {}
                }
            }
            if std::time::Instant::now() > deadline {
                return Err(RelayError::Engine("connection timed out".into()));
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

/// A connected relay client.
pub struct RelayClient {
    _engine: RelayEngine,
    handle: RelayHandle,
    session: SessionId,
    host_name: String,
}

impl RelayClient {
    pub fn session(&self) -> SessionId {
        self.session
    }

    pub fn host_name(&self) -> &str {
        &self.host_name
    }

    pub fn handle(&self) -> RelayHandle {
        self.handle.clone()
    }

    /// Send captured microphone audio to the host (emit role).
    pub fn send_capture(&self, samples: &[f32]) {
        self.handle.push_capture(samples);
    }

    /// Realtime-safe variant of [`Self::send_capture`].
    pub fn try_send_capture(&self, samples: &[f32]) -> bool {
        self.handle.try_push_capture(samples)
    }

    /// Take host audio for playback (receive role).
    pub fn pull_playback(&self, out: &mut [f32]) -> usize {
        self.handle.pull_playback(out)
    }

    /// Realtime-safe variant of [`Self::pull_playback`].
    pub fn try_pull_playback(&self, out: &mut [f32]) -> usize {
        self.handle.try_pull_playback(out)
    }

    pub fn events(&self) -> Vec<RelayEvent> {
        self.handle.events()
    }

    /// Disconnect from the host.
    pub fn disconnect(self) -> RelayResult<()> {
        self.handle.disconnect(self.session)
    }
}

/// Browse the local network for relay hosts for up to `timeout`, returning
/// every host seen. Uses mDNS/DNS-SD (`_qpw-relay._udp`); hosts that do not
/// advertise (or networks that block multicast) simply yield an empty list,
/// so callers should keep manual `host:port` entry as a fallback.
pub fn discover_hosts(timeout: Duration) -> RelayResult<Vec<PeerInfo>> {
    let engine = RelayEngine::start(EngineConfig::default())?;
    let handle = engine.handle();
    handle.discovery_start()?;
    let deadline = Instant::now() + timeout;
    loop {
        // Drain events so PeerDiscovered entries land in the peer snapshot.
        for event in handle.events() {
            if let RelayEvent::Error { message } = event {
                eprintln!("relay discovery: {message}");
            }
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let peers = handle.discovered_peers();
    handle.discovery_stop();
    engine.shutdown();
    Ok(peers)
}

fn resolve(target: &str) -> RelayResult<SocketAddr> {
    let target = target.trim();
    if target.is_empty() {
        return Err(RelayError::Engine("relay target cannot be empty".into()));
    }
    if let Ok(address) = target.parse::<SocketAddr>() {
        if address.port() == 0 {
            return Err(RelayError::Engine(format!(
                "relay target has an invalid control port: {target:?}"
            )));
        }
        return Ok(address);
    }
    if target.starts_with('[') {
        let end = target.find(']').ok_or_else(|| {
            RelayError::Engine(format!(
                "relay target must use [ipv6]:port syntax: {target:?}"
            ))
        })?;
        if target.get(end + 1..end + 2) != Some(":") {
            return Err(RelayError::Engine(format!(
                "relay target is missing a control port: {target:?}"
            )));
        }
    } else if target.matches(':').count() != 1 {
        return Err(RelayError::Engine(format!(
            "relay target must be host:port: {target:?}"
        )));
    }
    target
        .rsplit_once(':')
        .and_then(|(_, port)| port.parse::<u16>().ok())
        .filter(|port| *port != 0)
        .ok_or_else(|| {
            RelayError::Engine(format!(
                "relay target has an invalid control port: {target:?}"
            ))
        })?;
    target
        .to_socket_addrs()
        .map_err(RelayError::Io)?
        .next()
        .ok_or_else(|| RelayError::Engine(format!("could not resolve host address {target:?}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_missing_or_zero_control_ports() {
        assert!(resolve("127.0.0.1").is_err());
        assert!(resolve("127.0.0.1:0").is_err());
        assert!(resolve("[::1]").is_err());
        assert!(resolve("[::1]:0").is_err());
        assert!(resolve("").is_err());
    }

    #[test]
    fn accepts_ipv4_and_ipv6_control_targets() {
        assert_eq!(resolve("127.0.0.1:48123").unwrap().port(), 48123);
        assert_eq!(resolve("[::1]:48123").unwrap().port(), 48123);
    }
}
