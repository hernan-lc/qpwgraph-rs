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

use crate::audio::{
    announce_packet, seal_datagram, AudioHeader, AudioPacket, JitterBuffer, JitterPop,
};
use crate::codec::{make_decoder, make_encoder, AudioFormat};
use crate::convert::Converter;
use crate::crypto::{pake_start, Opener, Sealer, SessionKeys, Side};
use crate::netlink;
use crate::protocol::{
    is_supported_frame_ms, read_frame, read_sealed_frame, write_frame, write_sealed_frame,
    CodecKind, ControlMessage, DeviceKind, Roles, PROTOCOL_VERSION,
};
use crate::realtime::{request_realtime_thread, tune_audio_socket};
use crate::{EngineInner, PeerInfo, RelayError, RelayEvent, RelayResult, SessionId, SessionRecord};
use pw_graph_utils::hex::{hex_decode, hex_encode};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The two ends of an encrypted control channel, owned by one control thread.
struct ControlCipher {
    sealer: Sealer,
    opener: Opener,
}

impl ControlCipher {
    fn send(&mut self, stream: &mut TcpStream, message: &ControlMessage) -> std::io::Result<()> {
        write_sealed_frame(stream, &mut self.sealer, message)
    }

    fn receive(&mut self, stream: &mut TcpStream) -> std::io::Result<ControlMessage> {
        read_sealed_frame(stream, &mut self.opener)
    }
}

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
    // Binding every interface exposes pairing on the LAN, on any VPN, and on
    // whatever else happens to be up. Honour the configured address, or the
    // transport preference when no address is pinned, so the relay is offered
    // on the link it is meant to serve.
    let bind_ip = host_bind_addr(&inner.config());
    let listener = TcpListener::bind((bind_ip.unwrap_or(Ipv4Addr::UNSPECIFIED), port))?;
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

/// The address a host listens on: an explicitly configured one wins, then the
/// transport preference, and finally every interface.
fn host_bind_addr(config: &crate::EngineConfig) -> Option<Ipv4Addr> {
    config
        .bind_addr
        .or_else(|| netlink::listen_bind_addr(&netlink::local_links(), config.transport))
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
            Ok((mut stream, addr)) => {
                let _ = stream.set_nonblocking(false);
                // Every accepted connection costs a thread that can sit in a
                // handshake read timeout before proving anything, so refuse
                // rather than let an unauthenticated peer spawn without bound.
                let Some(slot) = inner.claim_handshake() else {
                    let _ = write_frame(
                        &mut stream,
                        &ControlMessage::PairFail {
                            reason: "the host is busy; try again shortly".into(),
                        },
                    );
                    continue;
                };
                let inner = Arc::clone(&inner);
                if std::thread::Builder::new()
                    .name(format!("relay-peer-{addr}"))
                    .spawn(move || host_peer_thread(inner, stream, addr, slot))
                    .is_err()
                {
                    // The slot guard was moved into the closure only on
                    // success; a failed spawn drops it here.
                    continue;
                }
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
///
/// `_slot` is the pre-authentication admission ticket; holding it for the
/// whole thread is what bounds concurrent handshakes.
fn host_peer_thread(
    inner: Arc<EngineInner>,
    mut stream: TcpStream,
    peer_addr: SocketAddr,
    _slot: crate::HandshakeSlot,
) {
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT));

    // Guessing a short PIN is now an online-only game, so make each round of
    // that game cost the guesser a lockout.
    if !inner.pairing_allowed(peer_addr.ip()) {
        let _ = write_frame(
            &mut stream,
            &ControlMessage::PairFail {
                reason: "too many failed pairing attempts; wait and retry".into(),
            },
        );
        return;
    }

    let first = match read_frame(&mut stream) {
        Ok(message) => message,
        Err(_) => return,
    };
    let (peer_name, peer_kind, client_pake) = match first {
        ControlMessage::Resume { session_id, pake } => {
            resume_peer_session(&inner, SessionId(session_id), stream, peer_addr, &pake);
            return;
        }
        ControlMessage::Hello {
            protocol,
            device_name,
            device_kind,
            pake,
            ..
        } if protocol == PROTOCOL_VERSION as u32 => (device_name, device_kind, pake),
        _ => return,
    };

    if inner.session_count() >= inner.config().max_sessions {
        let _ = write_frame(
            &mut stream,
            &ControlMessage::PairFail {
                reason: "the host is already at its session limit".into(),
            },
        );
        return;
    }

    let host_name = inner.config().device_name;
    let Some(keys) = host_pake_exchange(&inner, &mut stream, peer_addr, &client_pake, host_name)
    else {
        return;
    };
    inner.clear_pairing_failures(peer_addr.ip());
    let Ok((control_sealer, control_opener)) = keys.control_channel() else {
        return;
    };
    let mut cipher = ControlCipher {
        sealer: control_sealer,
        opener: control_opener,
    };

    // The audio socket follows the listener onto the same link.
    let bind_ip = host_bind_addr(&inner.config()).unwrap_or(Ipv4Addr::UNSPECIFIED);
    let socket = match UdpSocket::bind((bind_ip, 0)) {
        Ok(socket) => socket,
        Err(_) => return,
    };
    tune_audio_socket(&socket);
    let Ok(audio_port) = socket.local_addr().map(|addr| addr.port()) else {
        return;
    };
    let id = inner.next_session_id();
    if cipher
        .send(
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

    let start = match cipher.receive(&mut stream) {
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
        let _ = cipher.send(
            &mut stream,
            &ControlMessage::PairFail {
                reason: error.to_string(),
            },
        );
        return;
    }
    if roles.is_empty() {
        let _ = cipher.send(
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
        let _ = cipher.send(
            &mut stream,
            &ControlMessage::PairFail {
                reason: error.to_string(),
            },
        );
        return;
    }
    let Ok((audio_sealer, audio_opener)) = keys.audio_channel() else {
        return;
    };
    if cipher
        .send(&mut stream, &ControlMessage::SessionReady {})
        .is_err()
    {
        return;
    }
    let local = inner.config().local_format();
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
        incoming: crate::PcmQueue::new(crate::DEFAULT_QUEUE_CAPACITY),
        capture_convert: Mutex::new(prepared_capture_converter(local, format)),
        audio_sealer: Mutex::new(audio_sealer),
        audio_opener: Mutex::new(audio_opener),
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
    host_control_loop(inner, record, stream, cipher);
}

/// Run the host's half of the SPAKE2 exchange and both key confirmations.
///
/// Returns the derived keys only when the client proved it holds the same
/// PIN. A mismatch is recorded against the source address so repeated
/// guessing runs into the lockout.
fn host_pake_exchange(
    inner: &Arc<EngineInner>,
    stream: &mut TcpStream,
    peer_addr: SocketAddr,
    client_pake: &str,
    host_name: String,
) -> Option<SessionKeys> {
    let Ok(client_message) = hex_decode(client_pake) else {
        return None;
    };
    let pin = inner.config().pin;
    let host = pake_start(Side::Host, &pin);
    let host_message = host.message.clone();
    if write_frame(
        stream,
        &ControlMessage::Challenge {
            protocol: PROTOCOL_VERSION as u32,
            pake: hex_encode(&host_message),
            host_name,
        },
    )
    .is_err()
    {
        return None;
    }
    let keys = match host.finish(&client_message) {
        Ok(keys) => keys,
        Err(_) => {
            // A malformed SPAKE2 message is as much a failed attempt as a
            // wrong PIN, and must count against the same budget.
            reject_pairing(inner, stream, peer_addr.ip(), "pairing exchange failed");
            return None;
        }
    };
    let confirm = match read_frame(stream) {
        Ok(ControlMessage::Pair { confirm, .. }) => confirm,
        _ => return None,
    };
    let Ok(confirm) = hex_decode(&confirm) else {
        reject_pairing(inner, stream, peer_addr.ip(), "PIN did not match");
        return None;
    };
    if !keys.verify_confirmation(&confirm) {
        reject_pairing(inner, stream, peer_addr.ip(), "PIN did not match");
        return None;
    }
    if write_frame(
        stream,
        &ControlMessage::PairConfirm {
            confirm: hex_encode(&keys.confirmation()),
        },
    )
    .is_err()
    {
        return None;
    }
    Some(keys)
}

fn reject_pairing(inner: &Arc<EngineInner>, stream: &mut TcpStream, addr: IpAddr, reason: &str) {
    inner.note_pairing_failure(addr);
    let _ = write_frame(
        stream,
        &ControlMessage::PairFail {
            reason: reason.into(),
        },
    );
}

/// Host-side resume of a session whose control link dropped. Re-authenticates
/// the client with a fresh SPAKE2 exchange, then takes over the control
/// watch. Only the control channel is rekeyed: the UDP audio workers never
/// stopped, so their keys and replay windows carry on untouched.
fn resume_peer_session(
    inner: &Arc<EngineInner>,
    id: SessionId,
    mut stream: TcpStream,
    peer_addr: SocketAddr,
    client_pake: &str,
) {
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

    let host_name = inner.config().device_name;
    let Some(keys) = host_pake_exchange(inner, &mut stream, peer_addr, client_pake, host_name)
    else {
        return;
    };
    inner.clear_pairing_failures(peer_addr.ip());
    let Ok((sealer, opener)) = keys.control_channel() else {
        return;
    };
    let mut cipher = ControlCipher { sealer, opener };

    // One takeover at a time: a racing reconnect loses cleanly.
    if record
        .resuming
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        let _ = cipher.send(
            &mut stream,
            &ControlMessage::PairFail {
                reason: "a resume is already in progress".into(),
            },
        );
        return;
    }
    if cipher
        .send(&mut stream, &ControlMessage::ResumeOk {})
        .is_err()
    {
        record.resuming.store(false, Ordering::Relaxed);
        return;
    }

    record.control_generation.fetch_add(1, Ordering::Relaxed);
    record.resuming.store(false, Ordering::Relaxed);
    let inner = Arc::clone(inner);
    host_control_loop(inner, record, stream, cipher);
}

/// Handle a clean control-watch exit that ends the session. Returns `true`
/// if the session was torn down (so the caller can return).
fn handle_teardown_exit(
    inner: &Arc<EngineInner>,
    record: &Arc<SessionRecord>,
    exit: ControlExit,
    on_drop: impl FnOnce(String) -> bool,
) -> bool {
    match exit {
        ControlExit::Stopped => {
            teardown(inner, record.id, "session stopped".into());
            true
        }
        ControlExit::PeerBye(reason) => {
            teardown(inner, record.id, format!("peer left: {reason}"));
            true
        }
        ControlExit::Dropped(reason) => on_drop(reason),
    }
}

/// Watch the control channel, waiting out link drops for [`RESUME_GRACE`]
/// so a reconnecting client can take over without losing the session. The
/// resuming thread runs its own watch, so every outcome ends this one.
fn host_control_loop(
    inner: Arc<EngineInner>,
    record: Arc<SessionRecord>,
    stream: TcpStream,
    cipher: ControlCipher,
) {
    let result = watch_control(Arc::clone(&inner), Arc::clone(&record), stream, cipher);
    handle_teardown_exit(&inner, &record, result, |reason| {
        if await_resume_grace(&inner, &record) {
            return false;
        }
        teardown(&inner, record.id, reason);
        true
    });
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

    let client = pake_start(Side::Client, &pin);
    let hello = ControlMessage::Hello {
        protocol: PROTOCOL_VERSION as u32,
        device_name: config.device_name.clone(),
        device_kind: config.device_kind,
        roles,
        sample_rate: config.sample_rate,
        channels: config.channels,
        pake: hex_encode(&client.message),
    };
    if write_frame(&mut stream, &hello).is_err() {
        fail_attempt(&inner, id, "handshake failed while sending hello".into());
        return;
    }

    let (host_pake, host_name) = match read_frame(&mut stream) {
        Ok(ControlMessage::Challenge {
            protocol,
            pake,
            host_name,
        }) if protocol == PROTOCOL_VERSION as u32 => (pake, host_name),
        Ok(ControlMessage::PairFail { reason }) => {
            fail_attempt(&inner, id, format!("host rejected pairing: {reason}"));
            return;
        }
        Ok(_) => {
            fail_attempt(&inner, id, "host sent an unexpected message".into());
            return;
        }
        Err(error) => {
            fail_attempt(&inner, id, format!("handshake failed: {error}"));
            return;
        }
    };
    let Ok(host_message) = hex_decode(&host_pake) else {
        fail_attempt(&inner, id, "host sent a malformed pairing message".into());
        return;
    };
    let keys = match client.finish(&host_message) {
        Ok(keys) => keys,
        Err(error) => {
            fail_attempt(&inner, id, format!("pairing failed: {error}"));
            return;
        }
    };
    if write_frame(
        &mut stream,
        &ControlMessage::Pair {
            pake: String::new(),
            confirm: hex_encode(&keys.confirmation()),
        },
    )
    .is_err()
    {
        fail_attempt(&inner, id, "handshake failed while pairing".into());
        return;
    }
    // The host's confirmation is what tells the client its PIN was right —
    // without it a wrong PIN would only show up as traffic that never
    // decrypts, several messages later.
    match read_frame(&mut stream) {
        Ok(ControlMessage::PairConfirm { confirm }) => {
            let matched = hex_decode(&confirm)
                .map(|confirm| keys.verify_confirmation(&confirm))
                .unwrap_or(false);
            if !matched {
                fail_attempt(&inner, id, "the PIN did not match the host".into());
                return;
            }
        }
        Ok(ControlMessage::PairFail { reason }) => {
            fail_attempt(&inner, id, format!("host rejected pairing: {reason}"));
            return;
        }
        Ok(_) | Err(_) => {
            fail_attempt(&inner, id, "pairing response was malformed".into());
            return;
        }
    }
    let Ok((sealer, opener)) = keys.control_channel() else {
        fail_attempt(&inner, id, "session keys could not be prepared".into());
        return;
    };
    let mut cipher = ControlCipher { sealer, opener };

    let (audio_port, wire_id) = match cipher.receive(&mut stream) {
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
    if cipher.send(&mut stream, &start).is_err() {
        fail_attempt(&inner, id, "handshake failed during session setup".into());
        return;
    }
    match cipher.receive(&mut stream) {
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
    let Ok((audio_sealer, audio_opener)) = keys.audio_channel() else {
        fail_attempt(&inner, id, "audio keys could not be prepared".into());
        return;
    };

    let socket = match UdpSocket::bind((bind.unwrap_or(Ipv4Addr::UNSPECIFIED), 0)) {
        Ok(socket) => socket,
        Err(error) => {
            fail_attempt(&inner, id, format!("could not open audio socket: {error}"));
            return;
        }
    };
    tune_audio_socket(&socket);
    let host_audio_addr = SocketAddr::new(target.ip(), audio_port);

    let format = AudioFormat::new(config.sample_rate, config.channels, config.frame_ms);
    let local = config.local_format();
    let mut audio_sealer = audio_sealer;
    // Teach the host our UDP address before real audio flows. The announce is
    // sealed with the session key, so only the paired client can move it.
    if let Ok(announce) = announce_packet(&mut audio_sealer, config.codec) {
        let _ = socket.send_to(&announce, host_audio_addr);
    }
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
        incoming: crate::PcmQueue::new(crate::DEFAULT_QUEUE_CAPACITY),
        capture_convert: Mutex::new(prepared_capture_converter(local, format)),
        audio_sealer: Mutex::new(audio_sealer),
        audio_opener: Mutex::new(audio_opener),
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

    client_control_loop(inner, record, stream, cipher, socket, target, pin);
}

/// Client-side control watch: on a link drop the host is re-dialed and the
/// session resumed, so Wi-Fi roaming or brief outages do not end the session.
fn client_control_loop(
    inner: Arc<EngineInner>,
    record: Arc<SessionRecord>,
    stream: TcpStream,
    cipher: ControlCipher,
    socket: Arc<UdpSocket>,
    target: SocketAddr,
    pin: String,
) {
    let socket_codec = record.codec;
    let mut stream = Some((stream, cipher));
    loop {
        let (taken, cipher) = stream.take().expect("stream is set between iterations");
        let result = watch_control(Arc::clone(&inner), Arc::clone(&record), taken, cipher);
        let torn_down = handle_teardown_exit(&inner, &record, result, |reason| {
            if record.bye_requested.load(Ordering::Relaxed) || !inner.session_alive(record.id) {
                teardown(&inner, record.id, reason);
                return true;
            }
            inner.emit(RelayEvent::Error {
                message: format!("control link to host lost ({reason}); attempting to resume"),
            });
            match resume_client_control(&inner, &record, target, &pin) {
                Some(resumed) => {
                    // Re-announce our UDP address from the real audio socket:
                    // the route may have changed link (e.g. Wi-Fi to USB
                    // tethering), and the host must learn the new source
                    // address. The announce is sealed with the session's
                    // unchanged audio key, which is exactly what authorises
                    // the host to follow us.
                    if let (Ok(slot), Ok(mut sealer)) =
                        (record.peer_audio_addr.lock(), record.audio_sealer.lock())
                    {
                        if let Some(addr) = *slot {
                            if let Ok(announce) = announce_packet(&mut sealer, socket_codec) {
                                let _ = socket.send_to(&announce, addr);
                            }
                        }
                    }
                    stream = Some(resumed);
                    false
                }
                None => {
                    teardown(&inner, record.id, reason);
                    true
                }
            }
        });
        if torn_down {
            return;
        }
    }
}

/// Re-dial the host and resume an established session. Returns the new
/// control stream and its freshly rekeyed cipher on success.
fn resume_client_control(
    inner: &Arc<EngineInner>,
    record: &Arc<SessionRecord>,
    target: SocketAddr,
    pin: &str,
) -> Option<(TcpStream, ControlCipher)> {
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
        // A resume re-runs the full PAKE: the control channel gets fresh keys
        // so a captured earlier transcript is useless against it.
        let client = pake_start(Side::Client, pin);
        if write_frame(
            &mut stream,
            &ControlMessage::Resume {
                session_id: record.wire_id,
                pake: hex_encode(&client.message),
            },
        )
        .is_err()
        {
            continue;
        }
        let host_pake = match read_frame(&mut stream) {
            Ok(ControlMessage::Challenge { protocol, pake, .. })
                if protocol == PROTOCOL_VERSION as u32 =>
            {
                pake
            }
            Ok(ControlMessage::PairFail { reason }) => {
                inner.emit(RelayEvent::Error {
                    message: format!("host rejected resume: {reason}"),
                });
                return None;
            }
            _ => continue,
        };
        let Ok(host_message) = hex_decode(&host_pake) else {
            continue;
        };
        let Ok(keys) = client.finish(&host_message) else {
            continue;
        };
        if write_frame(
            &mut stream,
            &ControlMessage::Pair {
                pake: String::new(),
                confirm: hex_encode(&keys.confirmation()),
            },
        )
        .is_err()
        {
            continue;
        }
        match read_frame(&mut stream) {
            Ok(ControlMessage::PairConfirm { confirm }) => {
                let matched = hex_decode(&confirm)
                    .map(|confirm| keys.verify_confirmation(&confirm))
                    .unwrap_or(false);
                if !matched {
                    inner.emit(RelayEvent::Error {
                        message: "the host failed key confirmation on resume".into(),
                    });
                    return None;
                }
            }
            Ok(ControlMessage::PairFail { reason }) => {
                inner.emit(RelayEvent::Error {
                    message: format!("host rejected resume: {reason}"),
                });
                return None;
            }
            _ => continue,
        }
        let Ok((sealer, opener)) = keys.control_channel() else {
            continue;
        };
        let mut cipher = ControlCipher { sealer, opener };
        match cipher.receive(&mut stream) {
            Ok(ControlMessage::ResumeOk {}) => {
                let _ = stream.set_read_timeout(Some(Duration::from_millis(250)));
                return Some((stream, cipher));
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

/// Build this session's transmit converter with every buffer already grown
/// for the largest quantum a realtime callback can present.
///
/// `broadcast_capture` runs on the PipeWire process thread. A converter built
/// by `Converter::new` allocates inside its first `convert` — and again on any
/// quantum larger than one it has already seen — which is exactly what the
/// realtime contract forbids. Sizing here, on the session-setup thread, is
/// what makes `try_push_capture` allocation-free from the very first callback
/// rather than only "after warm-up".
fn prepared_capture_converter(local: AudioFormat, wire: AudioFormat) -> (Converter, Vec<f32>) {
    let max_input = crate::MAX_REALTIME_QUANTUM_SAMPLES;
    let converter = Converter::with_capacity(
        local.sample_rate,
        local.channels,
        wire.sample_rate,
        wire.channels,
        max_input,
    );
    // The identity path pushes `samples` straight through and never touches
    // this buffer, but a geometry change writes the full converted quantum
    // into it, so it is sized for the worst supported expansion.
    let out = Vec::with_capacity(converter.output_capacity_for(max_input));
    (converter, out)
}

/// Whether an authenticated audio packet's header agrees with what the
/// session negotiated.
///
/// Only applies to packets carrying audio: an announce packet's header is
/// filler (see [`crate::audio::announce_packet`]) and is filtered out by its
/// empty payload before this is consulted.
fn packet_matches_negotiation(
    packet: &AudioPacket<'_>,
    codec: CodecKind,
    format: AudioFormat,
) -> bool {
    packet.codec == codec && packet.stereo == format.is_stereo()
}

fn validate_negotiation(
    codec: CodecKind,
    frame_ms: u16,
    sample_rate: u32,
    channels: u16,
) -> RelayResult<()> {
    if !is_supported_frame_ms(frame_ms) {
        return Err(RelayError::Protocol(format!(
            "unsupported frame duration {frame_ms} ms"
        )));
    }
    if !crate::is_supported_sample_rate(sample_rate) {
        return Err(RelayError::Protocol(format!(
            "unsupported sample rate {sample_rate} Hz"
        )));
    }
    if !crate::is_supported_channels(channels) {
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

/// Receive loop: datagrams → authenticate → jitter buffer → decoder →
/// convert to the local format → this session's playback queue.
///
/// Every fatal error tears the session down. Leaving the session registered
/// after its audio path has permanently died — as an earlier version did for
/// socket and encoder failures — makes the engine report a healthy, silent
/// connection that will never carry audio again.
fn run_rx(
    inner: Arc<EngineInner>,
    record: Arc<SessionRecord>,
    socket: Arc<UdpSocket>,
    host_side: bool,
) {
    request_realtime_thread();
    let _ = socket.set_read_timeout(Some(Duration::from_millis(500)));
    let local = inner.config().local_format();
    // Bound what a stalled playback consumer can turn into standing delay:
    // decoded audio waiting here is latency, not safety. The depth is this
    // session's own, in local-format samples.
    let local_frame_samples = (local.sample_rate as usize / 1000)
        * record.format.frame_ms as usize
        * local.channels as usize;
    record
        .incoming
        .set_target_depth(local_frame_samples.max(1) * crate::PLAYBACK_DEPTH_FRAMES);
    let mut decoder = match make_decoder(record.codec, record.format) {
        Ok(decoder) => decoder,
        Err(error) => {
            fail_session(&inner, &record, format!("decoder init failed: {error}"));
            return;
        }
    };
    // Decode output is exactly one frame per call, so the receive converter
    // only ever needs that much. Sizing it here keeps the steady state free of
    // reallocation even though this thread is not itself realtime.
    let mut converter = Converter::with_capacity(
        record.format.sample_rate,
        record.format.channels,
        local.sample_rate,
        local.channels,
        record.format.frame_samples(),
    );

    let mut jitter = JitterBuffer::new(JITTER_DEPTH_FRAMES);
    let mut datagram = vec![0u8; MAX_DATAGRAM];
    let mut frame_buf = vec![0.0f32; record.format.frame_samples()];
    let mut converted = Vec::with_capacity(converter.output_capacity_for(record.format.frame_samples()));
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
            Err(error) => {
                fail_session(&inner, &record, format!("audio socket failed: {error}"));
                return;
            }
        };
        let Some(packet) = AudioPacket::parse(&datagram[..len]) else {
            continue;
        };
        // Authenticate *before* anything else observes the datagram. A packet
        // that does not open never reaches the address bookkeeping, the
        // jitter buffer, or the decoder, so a stranger who can reach this
        // port cannot inject audio or redirect ours.
        let payload = {
            let Ok(mut opener) = record.audio_opener.lock() else {
                break;
            };
            match packet.open(&mut opener) {
                Ok(payload) => payload,
                Err(_) => continue,
            }
        };

        if host_side {
            // Keep the peer's audio address current: after a link roam the
            // client may return from a different source address. Only an
            // authenticated datagram gets to move it.
            if let Ok(mut slot) = record.peer_audio_addr.lock() {
                if *slot != Some(addr) {
                    *slot = Some(addr);
                }
            }
        }
        if payload.is_empty() {
            // An announce packet: address bookkeeping only. Its header carries
            // no meaningful geometry (the sender hardcodes mono), so the
            // metadata check below deliberately sits after this.
            continue;
        }
        // Authentication proves *who* sent the datagram, not that they are
        // still speaking the format this session negotiated. A paired but
        // buggy or hostile peer that flips its codec id or stereo flag
        // mid-stream would otherwise feed the decoder and the jitter buffer
        // frames they cannot interpret — a decode error per packet at best,
        // and silently mis-framed audio at worst. The negotiated format is
        // the authority; a packet that disagrees with it is dropped before
        // it reaches any stateful audio machinery.
        if !packet_matches_negotiation(&packet, record.codec, record.format) {
            continue;
        }
        if packet.keyframe {
            jitter.set_anchor(packet.sequence);
        }
        if !jitter.push(packet.sequence, payload) {
            continue;
        }

        loop {
            let decoded = match jitter.pop() {
                JitterPop::Buffering => break,
                JitterPop::Frame(payload) => match decoder.decode(&payload, &mut frame_buf) {
                    Ok(samples) => {
                        for sample in &frame_buf[..samples] {
                            sumsq += (*sample as f64) * (*sample as f64);
                        }
                        level_samples += samples;
                        Some(samples)
                    }
                    Err(error) => {
                        inner.emit(RelayEvent::Error {
                            message: format!("audio decode failed: {error}"),
                        });
                        None
                    }
                },
                JitterPop::Lost => decoder.conceal(&mut frame_buf).ok(),
            };
            let Some(samples) = decoded else {
                continue;
            };
            converter.convert(&frame_buf[..samples], &mut converted);
            record.incoming.push(&converted);
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

/// Tear a session down after an unrecoverable worker failure, surfacing the
/// reason once. Both are idempotent, so a deliberate shutdown that races a
/// worker failure still produces exactly one `SessionLost`.
fn fail_session(inner: &Arc<EngineInner>, record: &Arc<SessionRecord>, reason: String) {
    // A stop that was already requested is an orderly shutdown, not a fault.
    if record.stop.load(Ordering::Relaxed) || !inner.session_alive(record.id) {
        return;
    }
    inner.emit(RelayEvent::Error {
        message: reason.clone(),
    });
    teardown(inner, record.id, reason);
}

/// Send loop: outgoing queue → encoder → sealed datagrams.
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
            fail_session(&inner, &record, format!("encoder init failed: {error}"));
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
                let header = AudioHeader {
                    stereo: record.format.is_stereo(),
                    keyframe: sequence == 0,
                    codec: record.codec,
                    sequence,
                    timestamp_ms,
                };
                let datagram = {
                    let Ok(mut sealer) = record.audio_sealer.lock() else {
                        fail_session(&inner, &record, "audio sealer is poisoned".into());
                        return;
                    };
                    match seal_datagram(&mut sealer, &header, &payload) {
                        Ok(datagram) => datagram,
                        Err(error) => {
                            fail_session(&inner, &record, format!("audio seal failed: {error}"));
                            return;
                        }
                    }
                };
                if let Err(error) = socket.send_to(&datagram, peer_addr) {
                    fail_session(&inner, &record, format!("audio send failed: {error}"));
                    return;
                }
                sequence = sequence.wrapping_add(1);
                timestamp_ms = timestamp_ms.wrapping_add(record.format.frame_ms as u32);
            }
            Err(error) => {
                fail_session(&inner, &record, format!("audio encode failed: {error}"));
                return;
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
    mut cipher: ControlCipher,
) -> ControlExit {
    let _ = stream.set_read_timeout(Some(Duration::from_millis(250)));
    let mut last_seen = Instant::now();
    let mut last_keepalive = Instant::now();

    loop {
        if !inner.running.load(Ordering::Relaxed) || record.stop.load(Ordering::Relaxed) {
            return ControlExit::Stopped;
        }
        if record.bye_requested.load(Ordering::Relaxed) {
            let _ = cipher.send(
                &mut stream,
                &ControlMessage::Bye {
                    reason: "user disconnected".into(),
                },
            );
            return ControlExit::Stopped;
        }
        match cipher.receive(&mut stream) {
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
            if cipher
                .send(&mut stream, &ControlMessage::Keepalive {})
                .is_err()
            {
                return ControlExit::Dropped("control channel closed".into());
            }
            last_keepalive = now;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::{seal_datagram, AudioHeader};
    use crate::crypto::{pake_start, Side};

    fn audio_keys() -> (crate::crypto::Sealer, crate::crypto::Opener) {
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

    /// Seal a frame with the given header metadata, then take it back apart
    /// the way `run_rx` does: parse, authenticate, and only then judge the
    /// header against the negotiated format.
    fn authenticated_packet_is_accepted(
        header_codec: CodecKind,
        header_stereo: bool,
        negotiated_codec: CodecKind,
        negotiated: AudioFormat,
    ) -> bool {
        let (mut sealer, mut opener) = audio_keys();
        let datagram = seal_datagram(
            &mut sealer,
            &AudioHeader {
                stereo: header_stereo,
                keyframe: true,
                codec: header_codec,
                sequence: 0,
                timestamp_ms: 0,
            },
            &[1, 2, 3],
        )
        .expect("frame seals");
        let packet = AudioPacket::parse(&datagram).expect("parses");
        let payload = packet.open(&mut opener).expect("authenticates");
        assert!(!payload.is_empty(), "this is an audio frame, not an announce");
        packet_matches_negotiation(&packet, negotiated_codec, negotiated)
    }

    #[test]
    fn an_authenticated_packet_matching_the_negotiation_is_accepted() {
        let format = AudioFormat::new(48_000, 1, 20);
        assert!(authenticated_packet_is_accepted(
            CodecKind::Opus,
            false,
            CodecKind::Opus,
            format
        ));
        let stereo = AudioFormat::new(48_000, 2, 20);
        assert!(authenticated_packet_is_accepted(
            CodecKind::Opus,
            true,
            CodecKind::Opus,
            stereo
        ));
    }

    #[test]
    fn an_authenticated_packet_with_the_wrong_codec_is_rejected() {
        // The peer is paired — the packet opens — but it is now claiming a
        // codec the session never agreed to. Handing that to a decoder built
        // for the negotiated codec is at best an error per packet.
        let format = AudioFormat::new(48_000, 1, 20);
        assert!(!authenticated_packet_is_accepted(
            CodecKind::Pcm,
            false,
            CodecKind::Opus,
            format
        ));
        assert!(!authenticated_packet_is_accepted(
            CodecKind::Opus,
            false,
            CodecKind::Pcm,
            format
        ));
    }

    #[test]
    fn an_authenticated_packet_with_the_wrong_stereo_flag_is_rejected() {
        // A stereo flag that disagrees with the negotiated channel count means
        // every frame would be de-interleaved against the wrong geometry.
        let mono = AudioFormat::new(48_000, 1, 20);
        assert!(!authenticated_packet_is_accepted(
            CodecKind::Opus,
            true,
            CodecKind::Opus,
            mono
        ));
        let stereo = AudioFormat::new(48_000, 2, 20);
        assert!(!authenticated_packet_is_accepted(
            CodecKind::Opus,
            false,
            CodecKind::Opus,
            stereo
        ));
    }

    #[test]
    fn prepared_capture_converters_are_sized_for_the_realtime_quantum() {
        // Session setup must leave nothing for `broadcast_capture` to grow.
        let quantum = crate::MAX_REALTIME_QUANTUM_SAMPLES;
        for local_channels in [1u16, 2] {
            for wire_rate in crate::SAMPLE_RATES_HZ {
                for wire_channels in [1u16, 2] {
                    let local = AudioFormat::new(48_000, local_channels, 20);
                    let wire = AudioFormat::new(wire_rate, wire_channels, 20);
                    let (mut converter, mut out) = prepared_capture_converter(local, wire);
                    if converter.is_identity() {
                        continue;
                    }
                    let mapped_before = converter.output_capacity_for(quantum);
                    assert!(out.capacity() >= mapped_before);
                    let out_capacity = out.capacity();
                    let input = vec![0.1f32; quantum];
                    for _ in 0..4 {
                        converter.convert(&input, &mut out);
                        assert_eq!(
                            out.capacity(),
                            out_capacity,
                            "{local:?} -> {wire:?} grew its transmit buffer"
                        );
                    }
                }
            }
        }
    }
}
