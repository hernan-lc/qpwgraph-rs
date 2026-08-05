//! Session establishment and audio worker threads.
//!
//! Threading model (deliberately simple for a handful of local peers):
//!
//! - one accept-loop thread while hosting, one thread per connected peer,
//! - one thread per outgoing connection attempt,
//! - per established session: the control thread keeps watching keepalives,
//!   plus one UDP receiver and, when this side transmits, one UDP sender.
//!
//! Every loop checks its session's stop flag and the engine's running flag,
//! so shutdown completes within roughly one socket timeout.

use crate::audio::{announce_packet, AudioPacket, JitterBuffer, JitterPop};
use crate::codec::{make_decoder, make_encoder, AudioFormat};
use crate::netlink;
use crate::pairing::{generate_salt, pair_digest, verify_digest};
use crate::protocol::{
    read_frame, write_frame, CodecKind, ControlMessage, DeviceKind, Roles, PROTOCOL_VERSION,
};
use crate::realtime::{request_realtime_thread, tune_audio_socket};
use crate::{EngineInner, PeerInfo, RelayError, RelayEvent, RelayResult, SessionId, SessionRecord};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(2);
const SESSION_TIMEOUT: Duration = Duration::from_secs(6);
/// How long a host keeps a dropped session alive waiting for the client to
/// re-establish its control channel (link roaming, brief Wi-Fi outages).
const RESUME_GRACE: Duration = Duration::from_secs(15);
/// Client-side reconnect attempts before a session is declared lost.
const RESUME_ATTEMPTS: u32 = 3;
/// Initial buffering before playback starts, in frames. Two is the smallest
/// depth that still lets the sender's keyframe anchor the stream when its
/// first two datagrams arrive out of order; the buffer's reorder tolerance
/// adapts from there, and the receive queue's target depth bounds what the
/// priming delay can turn into. At the default 10 ms frame this is 20 ms.
const JITTER_DEPTH_FRAMES: usize = 2;
const MAX_DATAGRAM: usize = 8192;
/// How long the sender parks waiting for a complete frame. The condvar wakes
/// it as soon as one is ready, so this only bounds how long teardown takes to
/// be noticed.
const FRAME_WAIT_TIMEOUT: Duration = Duration::from_millis(250);

/// Bookkeeping for a running host listener.
pub(crate) struct HostRecord {
    pub port: u16,
    pub stop: Arc<AtomicBool>,
}

pub(crate) fn start_host(inner: &Arc<EngineInner>, port: u16) -> RelayResult<HostRecord> {
    let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, port))?;
    let bound = listener.local_addr()?.port();
    listener.set_nonblocking(true)?;
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    let inner = Arc::clone(inner);
    std::thread::Builder::new()
        .name("relay-host".into())
        .spawn(move || accept_loop(inner, listener, thread_stop))?;
    Ok(HostRecord { port: bound, stop })
}

pub(crate) fn stop_host(inner: &EngineInner) {
    let taken = inner.host.lock().ok().and_then(|mut host| host.take());
    if let Some(record) = taken {
        record.stop.store(true, Ordering::Relaxed);
    }
}

fn accept_loop(inner: Arc<EngineInner>, listener: TcpListener, stop: Arc<AtomicBool>) {
    loop {
        if !inner.running.load(Ordering::Relaxed) || stop.load(Ordering::Relaxed) {
            break;
        }
        match listener.accept() {
            Ok((stream, addr)) => {
                let _ = stream.set_nonblocking(false);
                let inner = Arc::clone(&inner);
                let _ = std::thread::Builder::new()
                    .name(format!("relay-peer-{addr}"))
                    .spawn(move || host_peer_thread(inner, stream, addr));
            }
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

pub(crate) fn connect_peer(
    inner: &Arc<EngineInner>,
    id: SessionId,
    target: SocketAddr,
    pin: String,
    roles: Roles,
) {
    let inner = Arc::clone(inner);
    let _ = std::thread::Builder::new()
        .name(format!("relay-client-{target}"))
        .spawn(move || client_thread(inner, id, target, pin, roles));
}

/// Ask a session's control thread to send `bye` and tear down. Only the
/// bye flag is set: the control thread checks it before its stop condition
/// so the farewell frame actually goes out.
pub(crate) fn request_bye(inner: &EngineInner, id: SessionId) {
    if let Some(record) = inner.session(id) {
        record.bye_requested.store(true, Ordering::Relaxed);
    }
}

/// Remove a session and announce its loss. Idempotent.
pub(crate) fn teardown(inner: &EngineInner, id: SessionId, reason: String) {
    if inner.remove_session(id).is_some() {
        inner.emit(RelayEvent::SessionLost { id, reason });
    }
}

/// Report a connection attempt that failed before any session existed. The
/// caller owns the id (it came from `connect`), so the loss must always be
/// announced even though nothing was registered.
pub(crate) fn fail_attempt(inner: &EngineInner, id: SessionId, reason: String) {
    inner.remove_session(id);
    inner.emit(RelayEvent::SessionLost { id, reason });
}

/// Host side of one peer connection: either a fresh handshake or a resume of
/// an existing session, then the keepalive watcher.
fn host_peer_thread(inner: Arc<EngineInner>, mut stream: TcpStream, peer_addr: SocketAddr) {
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT));

    let first = match read_frame(&mut stream) {
        Ok(message) => message,
        Err(_) => return,
    };
    let (peer_name, peer_kind) = match first {
        ControlMessage::Resume { session_id } => {
            resume_peer_session(&inner, SessionId(session_id), stream);
            return;
        }
        ControlMessage::Hello {
            protocol,
            device_name,
            device_kind,
            ..
        } if protocol == PROTOCOL_VERSION as u32 => (device_name, device_kind),
        _ => return,
    };

    let salt = generate_salt();
    let host_name = inner.config().device_name;
    if write_frame(
        &mut stream,
        &ControlMessage::Challenge {
            protocol: PROTOCOL_VERSION as u32,
            salt: salt.clone(),
            host_name,
        },
    )
    .is_err()
    {
        return;
    }

    let digest = match read_frame(&mut stream) {
        Ok(ControlMessage::Pair { digest }) => digest,
        _ => return,
    };
    let pin = inner.config().pin;
    if !verify_digest(&pin, &salt, &digest) {
        let _ = write_frame(
            &mut stream,
            &ControlMessage::PairFail {
                reason: "PIN did not match".into(),
            },
        );
        return;
    }

    let socket = match UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)) {
        Ok(socket) => socket,
        Err(_) => return,
    };
    tune_audio_socket(&socket);
    let Ok(audio_port) = socket.local_addr().map(|addr| addr.port()) else {
        return;
    };
    let id = inner.next_session_id();
    if write_frame(
        &mut stream,
        &ControlMessage::PairOk {
            audio_port,
            session_id: id.0,
        },
    )
    .is_err()
    {
        return;
    }

    let start = match read_frame(&mut stream) {
        Ok(ControlMessage::SessionStart {
            roles,
            codec,
            frame_ms,
            sample_rate,
            channels,
        }) => (roles, codec, frame_ms, sample_rate, channels),
        Ok(_) | Err(_) => return,
    };
    let (roles, codec, frame_ms, sample_rate, channels) = start;
    if let Err(error) = validate_negotiation(codec, frame_ms, sample_rate, channels) {
        let _ = write_frame(
            &mut stream,
            &ControlMessage::PairFail {
                reason: error.to_string(),
            },
        );
        return;
    }
    if roles.is_empty() {
        let _ = write_frame(
            &mut stream,
            &ControlMessage::PairFail {
                reason: "no audio direction requested".into(),
            },
        );
        return;
    }

    let format = AudioFormat::new(sample_rate, channels, frame_ms);
    if let Err(error) =
        make_encoder(codec, format).and_then(|_| make_decoder(codec, format).map(|_| ()))
    {
        let _ = write_frame(
            &mut stream,
            &ControlMessage::PairFail {
                reason: error.to_string(),
            },
        );
        return;
    }
    if write_frame(&mut stream, &ControlMessage::SessionReady {}).is_err() {
        return;
    }
    let record = Arc::new(SessionRecord {
        id,
        wire_id: id.0,
        peer: PeerInfo {
            name: peer_name,
            kind: peer_kind,
            addr: peer_addr,
        },
        roles,
        codec,
        format,
        // Host perspective: it receives when the client emits, and sends
        // when the client wants playback.
        sending: roles.receive,
        receiving: roles.emit,
        stop: Arc::new(AtomicBool::new(false)),
        bye_requested: AtomicBool::new(false),
        control_generation: AtomicU64::new(0),
        resuming: AtomicBool::new(false),
        peer_audio_addr: Mutex::new(None),
        outgoing: crate::PcmQueue::new(crate::DEFAULT_QUEUE_CAPACITY),
    });
    inner.insert_session(Arc::clone(&record));
    inner.emit(RelayEvent::SessionEstablished {
        id,
        peer: record.peer.clone(),
        roles,
        codec,
    });

    let socket = Arc::new(socket);
    {
        let inner = Arc::clone(&inner);
        let record = Arc::clone(&record);
        let socket = Arc::clone(&socket);
        let _ = std::thread::Builder::new()
            .name(format!("relay-rx-{id}"))
            .spawn(move || run_rx(inner, record, socket, true));
    }
    if record.sending {
        let inner = Arc::clone(&inner);
        let record = Arc::clone(&record);
        let socket = Arc::clone(&socket);
        let _ = std::thread::Builder::new()
            .name(format!("relay-tx-{id}"))
            .spawn(move || run_tx(inner, record, socket));
    }

    record.control_generation.fetch_add(1, Ordering::Relaxed);
    host_control_loop(inner, record, stream);
}

/// Host-side resume of a session whose control link dropped. Re-authenticates
/// the client with a fresh challenge, then takes over the control watch. The
/// UDP audio workers never stopped, so audio resumes without renegotiation.
fn resume_peer_session(inner: &Arc<EngineInner>, id: SessionId, mut stream: TcpStream) {
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT));

    let Some(record) = inner.session(id) else {
        let _ = write_frame(
            &mut stream,
            &ControlMessage::PairFail {
                reason: "unknown or expired session".into(),
            },
        );
        return;
    };

    let salt = generate_salt();
    let host_name = inner.config().device_name;
    if write_frame(
        &mut stream,
        &ControlMessage::Challenge {
            protocol: PROTOCOL_VERSION as u32,
            salt: salt.clone(),
            host_name,
        },
    )
    .is_err()
    {
        return;
    }
    let digest = match read_frame(&mut stream) {
        Ok(ControlMessage::Pair { digest }) => digest,
        _ => return,
    };
    let pin = inner.config().pin;
    if !verify_digest(&pin, &salt, &digest) {
        let _ = write_frame(
            &mut stream,
            &ControlMessage::PairFail {
                reason: "PIN did not match".into(),
            },
        );
        return;
    }
    // One takeover at a time: a racing reconnect loses cleanly.
    if record
        .resuming
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        let _ = write_frame(
            &mut stream,
            &ControlMessage::PairFail {
                reason: "a resume is already in progress".into(),
            },
        );
        return;
    }
    if write_frame(&mut stream, &ControlMessage::ResumeOk {}).is_err() {
        record.resuming.store(false, Ordering::Relaxed);
        return;
    }

    record.control_generation.fetch_add(1, Ordering::Relaxed);
    record.resuming.store(false, Ordering::Relaxed);
    let inner = Arc::clone(inner);
    host_control_loop(inner, record, stream);
}

/// Watch the control channel, waiting out link drops for [`RESUME_GRACE`]
/// so a reconnecting client can take over without losing the session. The
/// resuming thread runs its own watch, so every outcome ends this one.
fn host_control_loop(inner: Arc<EngineInner>, record: Arc<SessionRecord>, stream: TcpStream) {
    match watch_control(Arc::clone(&inner), Arc::clone(&record), stream) {
        ControlExit::Stopped => {
            teardown(&inner, record.id, "session stopped".into());
        }
        ControlExit::PeerBye(reason) => {
            teardown(&inner, record.id, format!("peer left: {reason}"));
        }
        ControlExit::Dropped(reason) => {
            if await_resume_grace(&inner, &record) {
                // A resume thread took over (or the session is already gone);
                // this watch must not touch the session further.
                return;
            }
            teardown(&inner, record.id, reason);
        }
    }
}

/// Wait for a client resume to replace this control watch. Returns `true`
/// when somebody else now owns the session (no teardown by the caller).
fn await_resume_grace(inner: &Arc<EngineInner>, record: &Arc<SessionRecord>) -> bool {
    let generation = record.control_generation.load(Ordering::Relaxed);
    let deadline = Instant::now() + RESUME_GRACE;
    loop {
        if record.stop.load(Ordering::Relaxed) || !inner.session_alive(record.id) {
            return true;
        }
        if record.control_generation.load(Ordering::Relaxed) != generation {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Client side: connect, pair, negotiate, then keepalive watcher.
fn client_thread(
    inner: Arc<EngineInner>,
    id: SessionId,
    target: SocketAddr,
    pin: String,
    roles: Roles,
) {
    if roles.is_empty() {
        fail_attempt(&inner, id, "no audio direction requested".into());
        return;
    }
    let config = inner.config();

    // Bind outgoing sockets to the best local link for this target, honouring
    // the configured transport preference.
    let links = netlink::local_links();
    let bind = netlink::outbound_bind_addr(&links, target, config.transport);

    let mut stream = match netlink::connect_tcp(target, bind, CONNECT_TIMEOUT) {
        Ok(stream) => stream,
        Err(error) => {
            fail_attempt(&inner, id, format!("connection failed: {error}"));
            return;
        }
    };
    let _ = stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT));

    let hello = ControlMessage::Hello {
        protocol: PROTOCOL_VERSION as u32,
        device_name: config.device_name.clone(),
        device_kind: config.device_kind,
        roles,
        sample_rate: config.sample_rate,
        channels: config.channels,
    };
    if write_frame(&mut stream, &hello).is_err() {
        fail_attempt(&inner, id, "handshake failed while sending hello".into());
        return;
    }

    let (salt, host_name) = match read_frame(&mut stream) {
        Ok(ControlMessage::Challenge {
            protocol,
            salt,
            host_name,
        }) if protocol == PROTOCOL_VERSION as u32 => (salt, host_name),
        Ok(_) => {
            fail_attempt(&inner, id, "host sent an unexpected message".into());
            return;
        }
        Err(error) => {
            fail_attempt(&inner, id, format!("handshake failed: {error}"));
            return;
        }
    };

    let digest = pair_digest(&pin, &salt);
    if write_frame(&mut stream, &ControlMessage::Pair { digest }).is_err() {
        fail_attempt(&inner, id, "handshake failed while pairing".into());
        return;
    }

    let (audio_port, wire_id) = match read_frame(&mut stream) {
        Ok(ControlMessage::PairOk {
            audio_port,
            session_id,
        }) => (audio_port, session_id),
        Ok(ControlMessage::PairFail { reason }) => {
            fail_attempt(&inner, id, format!("host rejected pairing: {reason}"));
            return;
        }
        Ok(_) | Err(_) => {
            fail_attempt(&inner, id, "pairing response was malformed".into());
            return;
        }
    };

    let start = ControlMessage::SessionStart {
        roles,
        codec: config.codec,
        frame_ms: config.frame_ms,
        sample_rate: config.sample_rate,
        channels: config.channels,
    };
    if write_frame(&mut stream, &start).is_err() {
        fail_attempt(&inner, id, "handshake failed during session setup".into());
        return;
    }
    match read_frame(&mut stream) {
        Ok(ControlMessage::SessionReady {}) => {}
        Ok(ControlMessage::PairFail { reason }) => {
            fail_attempt(&inner, id, format!("host rejected session: {reason}"));
            return;
        }
        Ok(_) => {
            fail_attempt(
                &inner,
                id,
                "host sent an unexpected session response".into(),
            );
            return;
        }
        Err(error) => {
            fail_attempt(&inner, id, format!("session setup failed: {error}"));
            return;
        }
    }

    let socket = match UdpSocket::bind((bind.unwrap_or(Ipv4Addr::UNSPECIFIED), 0)) {
        Ok(socket) => socket,
        Err(error) => {
            fail_attempt(&inner, id, format!("could not open audio socket: {error}"));
            return;
        }
    };
    tune_audio_socket(&socket);
    let host_audio_addr = SocketAddr::new(target.ip(), audio_port);
    // Teach the host our UDP address before real audio flows.
    let _ = socket.send_to(&announce_packet(config.codec), host_audio_addr);

    let format = AudioFormat::new(config.sample_rate, config.channels, config.frame_ms);
    let record = Arc::new(SessionRecord {
        id,
        wire_id,
        peer: PeerInfo {
            name: host_name,
            kind: DeviceKind::Other,
            addr: target,
        },
        roles,
        codec: config.codec,
        format,
        // Client perspective: it sends when emitting, receives when playing.
        sending: roles.emit,
        receiving: roles.receive,
        stop: Arc::new(AtomicBool::new(false)),
        bye_requested: AtomicBool::new(false),
        control_generation: AtomicU64::new(0),
        resuming: AtomicBool::new(false),
        peer_audio_addr: Mutex::new(Some(host_audio_addr)),
        outgoing: crate::PcmQueue::new(crate::DEFAULT_QUEUE_CAPACITY),
    });
    inner.insert_session(Arc::clone(&record));
    inner.emit(RelayEvent::SessionEstablished {
        id,
        peer: record.peer.clone(),
        roles,
        codec: config.codec,
    });

    let socket = Arc::new(socket);
    if record.receiving {
        let inner = Arc::clone(&inner);
        let record = Arc::clone(&record);
        let socket = Arc::clone(&socket);
        let _ = std::thread::Builder::new()
            .name(format!("relay-rx-{id}"))
            .spawn(move || run_rx(inner, record, socket, false));
    }
    if record.sending {
        let inner = Arc::clone(&inner);
        let record = Arc::clone(&record);
        let socket = Arc::clone(&socket);
        let _ = std::thread::Builder::new()
            .name(format!("relay-tx-{id}"))
            .spawn(move || run_tx(inner, record, socket));
    }

    client_control_loop(inner, record, stream, socket, target, pin);
}

/// Client-side control watch: on a link drop the host is re-dialed and the
/// session resumed, so Wi-Fi roaming or brief outages do not end the session.
fn client_control_loop(
    inner: Arc<EngineInner>,
    record: Arc<SessionRecord>,
    stream: TcpStream,
    socket: Arc<UdpSocket>,
    target: SocketAddr,
    pin: String,
) {
    let socket_codec = record.codec;
    let mut stream = stream;
    loop {
        match watch_control(Arc::clone(&inner), Arc::clone(&record), stream) {
            ControlExit::Stopped => {
                teardown(&inner, record.id, "session stopped".into());
                return;
            }
            ControlExit::PeerBye(reason) => {
                teardown(&inner, record.id, format!("peer left: {reason}"));
                return;
            }
            ControlExit::Dropped(reason) => {
                if record.bye_requested.load(Ordering::Relaxed) || !inner.session_alive(record.id) {
                    teardown(&inner, record.id, reason);
                    return;
                }
                inner.emit(RelayEvent::Error {
                    message: format!("control link to host lost ({reason}); attempting to resume"),
                });
                match resume_client_control(&inner, &record, target, &pin) {
                    Some(new_stream) => {
                        // Re-announce our UDP address from the real audio
                        // socket: the route may have changed link (e.g.
                        // Wi-Fi to USB tethering), and the host must learn
                        // the new source address.
                        if let Ok(slot) = record.peer_audio_addr.lock() {
                            if let Some(addr) = *slot {
                                let _ = socket.send_to(&announce_packet(socket_codec), addr);
                            }
                        }
                        stream = new_stream;
                        continue;
                    }
                    None => {
                        teardown(&inner, record.id, reason);
                        return;
                    }
                }
            }
        }
    }
}

/// Re-dial the host and resume an established session. Returns the new
/// control stream on success.
fn resume_client_control(
    inner: &Arc<EngineInner>,
    record: &Arc<SessionRecord>,
    target: SocketAddr,
    pin: &str,
) -> Option<TcpStream> {
    let config = inner.config();
    let mut backoff = Duration::from_millis(500);
    for _ in 0..RESUME_ATTEMPTS {
        if !inner.session_alive(record.id) || record.stop.load(Ordering::Relaxed) {
            return None;
        }
        std::thread::sleep(backoff);
        backoff = (backoff * 2).min(Duration::from_secs(4));

        let links = netlink::local_links();
        let bind = netlink::outbound_bind_addr(&links, target, config.transport);
        let mut stream = match netlink::connect_tcp(target, bind, CONNECT_TIMEOUT) {
            Ok(stream) => stream,
            Err(_) => continue,
        };
        let _ = stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT));
        if write_frame(
            &mut stream,
            &ControlMessage::Resume {
                session_id: record.wire_id,
            },
        )
        .is_err()
        {
            continue;
        }
        let salt = match read_frame(&mut stream) {
            Ok(ControlMessage::Challenge { protocol, salt, .. })
                if protocol == PROTOCOL_VERSION as u32 =>
            {
                salt
            }
            Ok(ControlMessage::PairFail { reason }) => {
                inner.emit(RelayEvent::Error {
                    message: format!("host rejected resume: {reason}"),
                });
                return None;
            }
            _ => continue,
        };
        let digest = pair_digest(pin, &salt);
        if write_frame(&mut stream, &ControlMessage::Pair { digest }).is_err() {
            continue;
        }
        match read_frame(&mut stream) {
            Ok(ControlMessage::ResumeOk {}) => {
                let _ = stream.set_read_timeout(Some(Duration::from_millis(250)));
                return Some(stream);
            }
            Ok(ControlMessage::PairFail { reason }) => {
                inner.emit(RelayEvent::Error {
                    message: format!("host rejected resume: {reason}"),
                });
                return None;
            }
            _ => continue,
        }
    }
    None
}

fn validate_negotiation(
    codec: CodecKind,
    frame_ms: u16,
    sample_rate: u32,
    channels: u16,
) -> RelayResult<()> {
    if !matches!(frame_ms, 5 | 10 | 20 | 40 | 60) {
        return Err(RelayError::Protocol(format!(
            "unsupported frame duration {frame_ms} ms"
        )));
    }
    if !matches!(sample_rate, 16_000 | 24_000 | 48_000) {
        return Err(RelayError::Protocol(format!(
            "unsupported sample rate {sample_rate} Hz"
        )));
    }
    if !matches!(channels, 1 | 2) {
        return Err(RelayError::Protocol(format!(
            "unsupported channel count {channels}"
        )));
    }
    let _ = codec; // both Pcm and Opus are supported
    Ok(())
}

fn is_timeout(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    )
}

/// Receive loop: datagrams → jitter buffer → decoder → incoming queue.
fn run_rx(
    inner: Arc<EngineInner>,
    record: Arc<SessionRecord>,
    socket: Arc<UdpSocket>,
    host_side: bool,
) {
    request_realtime_thread();
    let _ = socket.set_read_timeout(Some(Duration::from_millis(500)));
    // Bound what a stalled playback consumer can turn into standing delay:
    // decoded audio waiting here is latency, not safety.
    inner
        .incoming
        .set_target_depth(record.format.frame_samples() * crate::PLAYBACK_DEPTH_FRAMES);
    let mut decoder = match make_decoder(record.codec, record.format) {
        Ok(decoder) => decoder,
        Err(error) => {
            inner.emit(RelayEvent::Error {
                message: format!("decoder init failed: {error}"),
            });
            teardown(&inner, record.id, format!("decoder init failed: {error}"));
            return;
        }
    };

    let mut jitter = JitterBuffer::new(JITTER_DEPTH_FRAMES);
    let mut datagram = vec![0u8; MAX_DATAGRAM];
    let mut frame_buf = vec![0.0f32; record.format.frame_samples()];
    let mut sumsq = 0.0f64;
    let mut level_samples = 0usize;
    let mut frames_since_level = 0u32;

    loop {
        if !inner.session_alive(record.id) {
            break;
        }
        let (len, addr) = match socket.recv_from(&mut datagram) {
            Ok(result) => result,
            Err(error) if is_timeout(&error) => continue,
            Err(_) => break,
        };
        let Some(packet) = AudioPacket::parse(&datagram[..len]) else {
            continue;
        };

        if host_side {
            // Keep the peer's audio address current: after a link roam the
            // client may return from a different source address. The socket
            // stays unconnected so the new source is accepted.
            if let Ok(mut slot) = record.peer_audio_addr.lock() {
                if *slot != Some(addr) {
                    *slot = Some(addr);
                }
            }
        }
        if packet.is_announce() {
            continue;
        }
        if packet.keyframe {
            jitter.set_anchor(packet.sequence);
        }
        if !jitter.push(packet.sequence, packet.payload.to_vec()) {
            continue;
        }

        loop {
            match jitter.pop() {
                JitterPop::Buffering => break,
                JitterPop::Frame(payload) => match decoder.decode(&payload, &mut frame_buf) {
                    Ok(samples) => {
                        let frame = &frame_buf[..samples];
                        inner.incoming.push(frame);
                        for sample in frame {
                            sumsq += (*sample as f64) * (*sample as f64);
                        }
                        level_samples += samples;
                    }
                    Err(error) => inner.emit(RelayEvent::Error {
                        message: format!("audio decode failed: {error}"),
                    }),
                },
                JitterPop::Lost => {
                    if let Ok(samples) = decoder.conceal(&mut frame_buf) {
                        inner.incoming.push(&frame_buf[..samples]);
                    }
                }
            }
        }

        frames_since_level += 1;
        if frames_since_level >= 25 {
            let rms = if level_samples == 0 {
                0.0
            } else {
                (sumsq / level_samples as f64).sqrt() as f32
            };
            inner.emit(RelayEvent::AudioLevel {
                id: record.id,
                rms: rms.min(1.0),
            });
            frames_since_level = 0;
            sumsq = 0.0;
            level_samples = 0;
        }
    }
}

/// Send loop: outgoing queue → encoder → datagrams.
///
/// Pacing comes from the capture side filling the queue in real time. The
/// thread parks on the queue's condvar rather than polling, so a completed
/// frame is encoded and sent the moment the capture callback delivers it
/// instead of up to a poll interval later; the wait timeout exists only so
/// teardown is noticed promptly. The peer address is re-read per frame so a
/// roaming client (new source address) is followed without a restart.
fn run_tx(inner: Arc<EngineInner>, record: Arc<SessionRecord>, socket: Arc<UdpSocket>) {
    request_realtime_thread();
    // Same reasoning as the receive side: captured audio that cannot be sent
    // promptly is better dropped than delivered late.
    record
        .outgoing
        .set_target_depth(record.format.frame_samples() * crate::CAPTURE_DEPTH_FRAMES);
    let mut encoder = match make_encoder(record.codec, record.format) {
        Ok(encoder) => encoder,
        Err(error) => {
            inner.emit(RelayEvent::Error {
                message: format!("encoder init failed: {error}"),
            });
            return;
        }
    };

    let frame_samples = record.format.frame_samples();
    let mut sequence = 0u32;
    let mut timestamp_ms = 0u32;
    let mut payload = Vec::with_capacity(4096);

    loop {
        if !inner.session_alive(record.id) {
            break;
        }
        // The host learns the peer address from received datagrams; wait
        // briefly until it is known.
        let peer_addr = record.peer_audio_addr.lock().ok().and_then(|slot| *slot);
        let Some(peer_addr) = peer_addr else {
            std::thread::sleep(Duration::from_millis(10));
            continue;
        };
        let Some(samples) = record
            .outgoing
            .pop_exact_timeout(frame_samples, FRAME_WAIT_TIMEOUT)
        else {
            continue;
        };
        match encoder.encode(&samples, &mut payload) {
            Ok(0) => continue,
            Ok(_) => {
                let packet = AudioPacket {
                    stereo: record.format.is_stereo(),
                    keyframe: sequence == 0,
                    codec: record.codec,
                    sequence,
                    timestamp_ms,
                    payload: &payload,
                };
                if socket.send_to(&packet.to_datagram(), peer_addr).is_err() {
                    break;
                }
                sequence = sequence.wrapping_add(1);
                timestamp_ms = timestamp_ms.wrapping_add(record.format.frame_ms as u32);
            }
            Err(error) => {
                inner.emit(RelayEvent::Error {
                    message: format!("audio encode failed: {error}"),
                });
                break;
            }
        }
    }
}

/// Why a control watch ended. Teardown decisions belong to the caller so
/// host and client loops can attempt a resume first.
enum ControlExit {
    /// Engine shutdown or local bye request (farewell already sent).
    Stopped,
    /// The peer said goodbye.
    PeerBye(String),
    /// The link broke (closed or keepalive timeout); may be resumable.
    Dropped(String),
}

/// Keepalive watcher; runs on the session's control thread until the watch
/// ends. Returns the reason so the caller can decide about resuming.
fn watch_control(
    inner: Arc<EngineInner>,
    record: Arc<SessionRecord>,
    mut stream: TcpStream,
) -> ControlExit {
    let _ = stream.set_read_timeout(Some(Duration::from_millis(250)));
    let mut last_seen = Instant::now();
    let mut last_keepalive = Instant::now();

    loop {
        if !inner.running.load(Ordering::Relaxed) || record.stop.load(Ordering::Relaxed) {
            return ControlExit::Stopped;
        }
        if record.bye_requested.load(Ordering::Relaxed) {
            let _ = write_frame(
                &mut stream,
                &ControlMessage::Bye {
                    reason: "user disconnected".into(),
                },
            );
            return ControlExit::Stopped;
        }
        match read_frame(&mut stream) {
            Ok(ControlMessage::Bye { reason }) => return ControlExit::PeerBye(reason),
            Ok(_) => last_seen = Instant::now(),
            Err(error) if is_timeout(&error) => {}
            Err(_) => return ControlExit::Dropped("control channel closed".into()),
        }
        let now = Instant::now();
        if now.duration_since(last_seen) > SESSION_TIMEOUT {
            return ControlExit::Dropped("keepalive timeout".into());
        }
        if now.duration_since(last_keepalive) >= KEEPALIVE_INTERVAL {
            let _ = write_frame(&mut stream, &ControlMessage::Keepalive {});
            last_keepalive = now;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
