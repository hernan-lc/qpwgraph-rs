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
use crate::crypto::{
    pake_start, resume_control_channel, resume_proof, verify_resume_proof, Opener, Sealer,
    SessionKeys, Side, RESUME_NONCE_LEN,
};
use crate::netlink;
use crate::protocol::{
    is_supported_frame_ms, read_frame, read_sealed_frame, write_frame, write_sealed_frame,
    CodecKind, ControlMessage, DeviceKind, Roles, PROTOCOL_VERSION,
};
use crate::realtime::{request_realtime_thread, tune_audio_socket};
use crate::{
    ControlState, EngineInner, PeerInfo, RelayError, RelayEvent, RelayResult, ResumeGraceResult,
    SessionId, SessionRecord,
};
use pw_graph_utils::hex::{hex_decode, hex_encode};
use rand::RngCore;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use subtle::ConstantTimeEq;

/// The two ends of an encrypted control channel, owned by one control thread.
struct ControlCipher {
    sealer: Sealer,
    opener: Opener,
}

/// One authenticated, full-duplex TCP stream used as the audio transport for
/// ADB. A slot rather than a stream is stored in the session so a resumed
/// control connection can install a replacement without destroying the
/// negotiated audio keys or queues.
pub(crate) struct TcpAudioSlot {
    current: Mutex<Option<Arc<TcpAudioConnection>>>,
    changed: Condvar,
    connecting: AtomicBool,
}

/// A replaceable, interface-scoped UDP socket. The slot lets an authenticated
/// resume install a socket on the newly selected link while the audio workers
/// keep their queues, AEAD counters, and replay window. Replacing the Arc
/// retires the old socket as soon as in-flight receives finish.
pub(crate) struct UdpAudioSlot {
    current: Mutex<Option<Arc<UdpSocket>>>,
}

impl UdpAudioSlot {
    pub(crate) fn new(socket: UdpSocket) -> std::io::Result<Arc<Self>> {
        socket.set_read_timeout(Some(Duration::from_millis(500)))?;
        Ok(Arc::new(Self {
            current: Mutex::new(Some(Arc::new(socket))),
        }))
    }

    pub(crate) fn install(&self, socket: UdpSocket) -> std::io::Result<()> {
        socket.set_read_timeout(Some(Duration::from_millis(500)))?;
        let mut current = self
            .current
            .lock()
            .map_err(|_| std::io::Error::other("UDP audio socket slot is poisoned"))?;
        *current = Some(Arc::new(socket));
        Ok(())
    }

    fn current(&self) -> Option<Arc<UdpSocket>> {
        self.current.lock().ok().and_then(|current| current.clone())
    }

    pub(crate) fn local_addr(&self) -> Option<SocketAddr> {
        self.current().and_then(|socket| socket.local_addr().ok())
    }

    /// Unlink the current socket from the slot and wait until this thread
    /// holds the only reference to it, so that dropping the returned socket
    /// really closes the file descriptor.
    ///
    /// Taking the `Arc` out of the slot is not enough on its own: the RX and
    /// TX workers lease a clone for each iteration, so the underlying socket
    /// stays open while any lease is outstanding. A migration that needs the
    /// old address released — a wildcard socket standing in the way of a
    /// specific bind on the same port, or the reverse — must wait for those
    /// leases. They are short: a worker holds one only across a single
    /// `recv_from`/`send_to`, and the slot's read timeout bounds that at
    /// 500ms, so this wait is bounded rather than open-ended.
    ///
    /// Returns `None` when the slot was already empty, `Some(Ok(socket))`
    /// when the caller now owns the socket outright, and `Some(Err(socket))`
    /// when the leases did not drain in time — the caller must then
    /// [`restore`](Self::restore) it.
    fn take_exclusive(&self, timeout: Duration) -> Option<Result<UdpSocket, Arc<UdpSocket>>> {
        let mut socket = self.current.lock().ok()?.take()?;
        let deadline = Instant::now() + timeout;
        loop {
            match Arc::try_unwrap(socket) {
                Ok(socket) => return Some(Ok(socket)),
                Err(still_leased) => {
                    if Instant::now() >= deadline {
                        return Some(Err(still_leased));
                    }
                    socket = still_leased;
                    std::thread::sleep(UDP_MIGRATION_DRAIN_POLL);
                }
            }
        }
    }

    fn restore(&self, socket: Arc<UdpSocket>) {
        if let Ok(mut current) = self.current.lock() {
            *current = Some(socket);
        }
    }
}

struct TcpAudioConnection {
    reader: Mutex<TcpStream>,
    writer: Mutex<TcpStream>,
}

impl TcpAudioSlot {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            current: Mutex::new(None),
            changed: Condvar::new(),
            connecting: AtomicBool::new(false),
        })
    }

    fn begin_connect(&self) -> bool {
        self.connecting
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    fn end_connect(&self) {
        self.connecting.store(false, Ordering::Release);
    }

    fn install(&self, stream: TcpStream) -> std::io::Result<()> {
        let reader = stream.try_clone()?;
        let connection = Arc::new(TcpAudioConnection {
            reader: Mutex::new(reader),
            writer: Mutex::new(stream),
        });
        let mut current = self
            .current
            .lock()
            .map_err(|_| std::io::Error::other("TCP audio slot is poisoned"))?;
        if let Some(previous) = current.replace(connection) {
            previous.shutdown();
        }
        self.changed.notify_all();
        Ok(())
    }

    fn current(&self) -> Option<Arc<TcpAudioConnection>> {
        self.current.lock().ok().and_then(|current| current.clone())
    }

    pub(crate) fn is_active(&self) -> bool {
        self.current().is_some()
    }

    fn wait(&self, stop: &AtomicBool) -> Option<Arc<TcpAudioConnection>> {
        let mut current = self.current.lock().ok()?;
        while current.is_none() && !stop.load(Ordering::Relaxed) {
            current = self
                .changed
                .wait_timeout(current, Duration::from_millis(250))
                .ok()?
                .0;
        }
        current.clone()
    }

    fn clear(&self, connection: &Arc<TcpAudioConnection>) {
        if let Ok(mut current) = self.current.lock() {
            if current
                .as_ref()
                .is_some_and(|installed| Arc::ptr_eq(installed, connection))
            {
                *current = None;
                connection.shutdown();
            }
        }
    }
}

impl TcpAudioConnection {
    /// Wake both worker directions when a resumed connection replaces this
    /// one. A reader may be blocked in `read_exact` while the slot already
    /// points at the replacement; shutting down the underlying stream is what
    /// makes that worker observe the replacement instead of waiting forever
    /// on the stale socket.
    fn shutdown(&self) {
        if let Ok(reader) = self.reader.lock() {
            let _ = reader.shutdown(Shutdown::Both);
        }
        if let Ok(writer) = self.writer.lock() {
            let _ = writer.shutdown(Shutdown::Both);
        }
    }
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

type Worker = Box<dyn FnOnce() + Send + 'static>;
type WorkerSpawner = fn(String, Worker) -> std::io::Result<std::thread::JoinHandle<()>>;

fn spawn_named(name: String, worker: Worker) -> std::io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new().name(name).spawn(worker)
}

fn spawn_worker_with_report(
    spawn: &mut impl FnMut(String, Worker) -> std::io::Result<std::thread::JoinHandle<()>>,
    stop: &AtomicBool,
    id: SessionId,
    direction: &str,
    worker: Worker,
) -> Result<(), String> {
    if let Err(error) = spawn(format!("relay-{direction}-{id}"), worker) {
        stop.store(true, Ordering::Relaxed);
        return Err(format!(
            "could not start {direction} worker for {id}: {error}"
        ));
    }
    Ok(())
}

fn wait_for_worker_startup(
    ready: Receiver<Result<(), String>>,
    stop: &AtomicBool,
    id: SessionId,
    direction: &str,
) -> Result<(), String> {
    match ready.recv_timeout(HANDSHAKE_TIMEOUT) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(reason)) => {
            stop.store(true, Ordering::Relaxed);
            Err(format!(
                "{direction} worker for {id} failed during startup: {reason}"
            ))
        }
        Err(error) => {
            stop.store(true, Ordering::Relaxed);
            Err(format!(
                "{direction} worker for {id} did not become ready: {error}"
            ))
        }
    }
}

fn report_worker_startup(
    ready: Option<SyncSender<Result<(), String>>>,
    result: Result<(), String>,
) {
    if let Some(ready) = ready {
        let _ = ready.send(result);
    }
}

fn fresh_resume_nonce() -> [u8; RESUME_NONCE_LEN] {
    let mut nonce = [0u8; RESUME_NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce);
    nonce
}

fn fresh_trust_secret() -> [u8; 32] {
    let mut secret = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut secret);
    secret
}

fn decode_resume_nonce(value: &str) -> Option<[u8; RESUME_NONCE_LEN]> {
    hex_decode(value).ok()?.try_into().ok()
}

/// Start every worker required by a session before advertising it as
/// established. If a later worker fails, the caller removes the session and
/// the already-started worker observes that removal and exits.
fn spawn_session_workers(
    inner: &Arc<EngineInner>,
    record: &Arc<SessionRecord>,
    socket: &Option<Arc<UdpAudioSlot>>,
    host_side: bool,
) -> Result<(), String> {
    spawn_session_workers_with(inner, record, socket, host_side, spawn_named)
}

fn spawn_session_workers_with(
    inner: &Arc<EngineInner>,
    record: &Arc<SessionRecord>,
    socket: &Option<Arc<UdpAudioSlot>>,
    host_side: bool,
    mut spawn: impl FnMut(String, Worker) -> std::io::Result<std::thread::JoinHandle<()>>,
) -> Result<(), String> {
    if host_side || record.receiving {
        let inner = Arc::clone(inner);
        let record = Arc::clone(record);
        let socket = socket.clone();
        let stop = Arc::clone(&record.stop);
        let id = record.id;
        let (ready_tx, ready_rx) = mpsc::sync_channel(0);
        let tcp_audio = record.tcp_audio.clone();
        spawn_worker_with_report(
            &mut spawn,
            &stop,
            id,
            "RX",
            Box::new(move || {
                if let Some(tcp_audio) = tcp_audio {
                    run_tcp_rx(inner, record, tcp_audio, Some(ready_tx));
                } else {
                    let Some(socket) = socket else {
                        report_worker_startup(
                            Some(ready_tx),
                            Err("UDP audio socket is unavailable".into()),
                        );
                        return;
                    };
                    run_rx(inner, record, socket, host_side, Some(ready_tx));
                }
            }),
        )?;
        wait_for_worker_startup(ready_rx, &stop, id, "RX")?;
    }
    if record.sending {
        let inner = Arc::clone(inner);
        let record = Arc::clone(record);
        let socket = socket.clone();
        let stop = Arc::clone(&record.stop);
        let id = record.id;
        let (ready_tx, ready_rx) = mpsc::sync_channel(0);
        let tcp_audio = record.tcp_audio.clone();
        spawn_worker_with_report(
            &mut spawn,
            &stop,
            id,
            "TX",
            Box::new(move || {
                if let Some(tcp_audio) = tcp_audio {
                    run_tcp_tx(inner, record, tcp_audio, Some(ready_tx));
                } else {
                    let Some(socket) = socket else {
                        report_worker_startup(
                            Some(ready_tx),
                            Err("UDP audio socket is unavailable".into()),
                        );
                        return;
                    };
                    run_tx(inner, record, socket, Some(ready_tx));
                }
            }),
        )?;
        wait_for_worker_startup(ready_rx, &stop, id, "TX")?;
    }
    // ADB's secondary stream is client-initiated. It gets one supervisor for
    // the lifetime of the session, independent from the control watcher and
    // from the two audio directions. The slot's connect gate prevents a
    // control resume and the supervisor from creating duplicate races.
    if !host_side {
        if let Some(tcp_audio) = record.tcp_audio.clone() {
            let inner = Arc::clone(inner);
            let record = Arc::clone(record);
            let stop = Arc::clone(&record.stop);
            let id = record.id;
            let target = record.peer.addr;
            spawn_worker_with_report(
                &mut spawn,
                &stop,
                id,
                "ADB-audio-supervisor",
                Box::new(move || run_tcp_audio_supervisor(inner, record, tcp_audio, target)),
            )?;
        }
    }
    Ok(())
}

/// Bookkeeping for a running host listener.
pub(crate) struct HostRecord {
    pub port: u16,
    /// The exact address the TCP listener was bound to. `None` is the
    /// documented no-link fallback, which intentionally uses INADDR_ANY.
    bind_addr: Arc<Mutex<Option<Ipv4Addr>>>,
    pub stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl HostRecord {
    pub(crate) fn bind_addr(&self) -> Option<Ipv4Addr> {
        self.bind_addr.lock().ok().and_then(|addr| *addr)
    }

    /// Stop the accept loop and wait for its listener to be dropped. Returning
    /// only after the join makes an immediate same-port restart deterministic
    /// and prevents callers from being tempted to hide the race by falling
    /// back to an ephemeral port.
    pub(crate) fn stop(mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub(crate) fn start_host(inner: &Arc<EngineInner>, port: u16) -> RelayResult<HostRecord> {
    // Binding every interface exposes pairing on the LAN, on any VPN, and on
    // whatever else happens to be up. Honour the configured address, or the
    // transport preference when no address is pinned, so the relay is offered
    // on the link it is meant to serve.
    let config = inner.config();
    let bind_ip = host_bind_addr(&config);
    if config.transport != crate::TransportPreference::Auto
        && config.transport != crate::TransportPreference::Adb
        && bind_ip.is_none()
    {
        return Err(RelayError::Engine(
            "the selected relay interface is not available".into(),
        ));
    }
    let (listener, bound_addr, bound) = bind_control_listener(bind_ip, port)?;
    let mut listeners = vec![(listener, bound_addr)];
    // Keep a loopback listener alongside a link-specific listener so an ADB
    // forward/reverse can reach the same host without changing the host's
    // network exposure policy. A wildcard listener already includes loopback.
    if bound_addr.is_some_and(|addr| addr != Ipv4Addr::LOCALHOST) {
        if let Ok((loopback, _, _)) = bind_control_listener(Some(Ipv4Addr::LOCALHOST), bound) {
            listeners.push((loopback, Some(Ipv4Addr::LOCALHOST)));
        }
    }
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    let inner = Arc::clone(inner);
    let current_addr = Arc::new(Mutex::new(bound_addr));
    let thread_current_addr = Arc::clone(&current_addr);
    let worker = std::thread::Builder::new()
        .name("relay-host".into())
        .spawn(move || accept_loop(inner, listeners, thread_stop, thread_current_addr, bound))?;
    Ok(HostRecord {
        port: bound,
        bind_addr: current_addr,
        stop,
        worker: Some(worker),
    })
}

fn bind_control_listener(
    bind_ip: Option<Ipv4Addr>,
    port: u16,
) -> RelayResult<(TcpListener, Option<Ipv4Addr>, u16)> {
    let bind_address = bind_ip.unwrap_or(Ipv4Addr::UNSPECIFIED);
    let listener = TcpListener::bind((bind_address, port)).map_err(|error| {
        RelayError::Engine(format!(
            "could not bind relay control port {port} on {bind_address}: {error}"
        ))
    })?;
    let local_addr = listener.local_addr()?;
    let bound_addr = match local_addr.ip() {
        IpAddr::V4(addr) if !addr.is_unspecified() => Some(addr),
        _ => None,
    };
    let bound = local_addr.port();
    listener.set_nonblocking(true)?;
    Ok((listener, bound_addr, bound))
}

/// The address a host listens on: an explicitly configured one wins, then the
/// selected active link. `None` is reserved for the documented Auto/no-link
/// fallback where the OS must provide a wildcard listener.
fn host_bind_addr(config: &crate::EngineConfig) -> Option<Ipv4Addr> {
    if config.transport == crate::TransportPreference::Adb {
        return Some(Ipv4Addr::LOCALHOST);
    }
    if config.bind_addr.is_some() {
        return config.bind_addr;
    }
    let links = netlink::local_links();
    if config.transport == crate::TransportPreference::Auto {
        netlink::listen_bind_addr(&links, config.transport)
    } else {
        netlink::select_links(&links, config.transport)
            .first()
            .map(|link| link.addr)
    }
}

/// Bind normal UDP audio to the interface selected for the control path. A
/// wildcard is used only when Auto has no classified link information at all;
/// this is the documented container/no-link fallback, not the migration
/// strategy used during normal operation.
fn bind_udp_audio_socket(
    inner: &Arc<EngineInner>,
    target: SocketAddr,
    host_side: bool,
) -> std::io::Result<UdpSocket> {
    bind_udp_audio_socket_on(inner, target, host_side, None)
}

/// The local address the audio socket for `target` belongs on, or an error
/// when the selected link is gone. `None` is the documented wildcard
/// fallback for an Auto host with no classified link at all.
fn audio_bind_addr(
    inner: &Arc<EngineInner>,
    target: SocketAddr,
    host_side: bool,
) -> std::io::Result<Option<Ipv4Addr>> {
    let config = inner.config();
    let links = netlink::local_links();
    let bind = if host_side {
        host_bind_addr(&config)
    } else {
        netlink::outbound_bind_addr(&links, target, config.transport)
    };
    if bind.is_none() && (config.transport != crate::TransportPreference::Auto || !links.is_empty())
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AddrNotAvailable,
            "the selected relay interface is not available",
        ));
    }
    Ok(bind)
}

fn bind_udp_audio_socket_on(
    inner: &Arc<EngineInner>,
    target: SocketAddr,
    host_side: bool,
    port: Option<u16>,
) -> std::io::Result<UdpSocket> {
    let bind = audio_bind_addr(inner, target, host_side)?;
    UdpSocket::bind((bind.unwrap_or(Ipv4Addr::UNSPECIFIED), port.unwrap_or(0)))
}

fn connect_control_tcp(
    target: SocketAddr,
    bind: Option<Ipv4Addr>,
    transport: crate::TransportPreference,
) -> std::io::Result<TcpStream> {
    if transport == crate::TransportPreference::Adb && !target.ip().is_loopback() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "ADB transport requires a localhost forwarding target",
        ));
    }
    netlink::connect_tcp(target, bind, CONNECT_TIMEOUT).map_err(|error| {
        if transport == crate::TransportPreference::Adb && target.ip().is_loopback() {
            std::io::Error::new(
                error.kind(),
                format!(
                    "ADB forwarding is not reachable on {target}. Create the adb reverse/forward rule and retry."
                ),
            )
        } else {
            error
        }
    })
}

/// How long a migration waits for outstanding UDP socket leases to drain
/// before giving up and restoring the socket it was trying to replace. The
/// audio workers' 500ms read timeout bounds a single lease, so this leaves
/// several timeouts of headroom while still guaranteeing the migration
/// cannot block a control thread indefinitely.
const UDP_MIGRATION_DRAIN_TIMEOUT: Duration = Duration::from_millis(2_000);
/// Poll interval used while waiting for those leases to drain.
const UDP_MIGRATION_DRAIN_POLL: Duration = Duration::from_millis(5);

/// Whether the socket at `old` must be closed before `desired:port` can be
/// bound.
///
/// A wildcard socket owns its port on every local address, so it conflicts
/// with any specific bind on that port, and a specific bind likewise blocks
/// a later wildcard on the same port. Two *different* specific addresses on
/// the same port do not conflict, and a migration that does not preserve the
/// port cannot conflict at all.
fn udp_binds_collide(old: SocketAddr, desired: Ipv4Addr, port: u16) -> bool {
    port != 0 && old.port() == port && (old.ip().is_unspecified() || desired.is_unspecified())
}

/// Move `slot` onto `desired`, keeping `preserve_port` when one is given.
///
/// This is the socket-lifecycle half of a migration, split out from
/// interface discovery so it can be driven deterministically in tests:
/// `bind` receives an already resolved local address, and injecting a
/// failing `bind` exercises the rollback path without a timing race.
///
/// When the old and new addresses collide on the port being preserved, the
/// old socket is removed from the slot, drained of outstanding worker
/// leases, and closed *before* the new bind is attempted. If that bind then
/// fails the slot is refilled with a wildcard socket on the original port,
/// so the audio workers are never left with an empty slot.
fn migrate_udp_slot(
    slot: &UdpAudioSlot,
    desired: Ipv4Addr,
    preserve_port: Option<u16>,
    bind: &dyn Fn(SocketAddr) -> std::io::Result<UdpSocket>,
) -> std::io::Result<()> {
    let port = preserve_port.unwrap_or(0);
    let wanted = SocketAddr::from((desired, port));
    let Some(old_addr) = slot.local_addr() else {
        // Nothing is installed, so there is no address to collide with and
        // nothing to roll back to: just fill the slot.
        return slot.install(bind(wanted)?);
    };
    if !udp_binds_collide(old_addr, desired, port) {
        // The old socket can stay open across the swap; installing the new
        // one retires it as soon as the last in-flight lease finishes.
        return slot.install(bind(wanted)?);
    }

    let Some(taken) = slot.take_exclusive(UDP_MIGRATION_DRAIN_TIMEOUT) else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotConnected,
            "UDP audio socket is not ready",
        ));
    };
    let old = match taken {
        Ok(socket) => socket,
        Err(still_leased) => {
            // Put the still-live socket back rather than leaving the session
            // without an audio endpoint; the caller reports the failure.
            slot.restore(still_leased);
            return Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                format!(
                    "UDP audio socket on {old_addr} is still in use; migration to {wanted} timed out"
                ),
            ));
        }
    };
    // Closing here is the whole point: `{old_addr}` and `{wanted}` share a
    // port, so the kernel refuses the new bind while the old socket lives.
    drop(old);

    match bind(wanted) {
        Ok(next) => slot.install(next),
        Err(error) => {
            // The old socket is gone for good. Restore a usable endpoint on
            // the negotiated port so the peer keeps reaching this session,
            // falling back to an ephemeral port if even that is refused.
            let fallback = bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, old_addr.port())))
                .or_else(|_| bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0))));
            match fallback {
                Ok(fallback) => {
                    let _ = slot.install(fallback);
                    Err(std::io::Error::new(
                        error.kind(),
                        format!(
                            "could not move UDP audio to {wanted}: {error}; restored a wildcard socket on port {}",
                            old_addr.port()
                        ),
                    ))
                }
                Err(fallback_error) => Err(std::io::Error::new(
                    error.kind(),
                    format!(
                        "could not move UDP audio to {wanted}: {error}; the fallback rebind also failed: {fallback_error}"
                    ),
                )),
            }
        }
    }
}

/// Move one authenticated session's UDP socket to the link selected for its
/// newly authenticated control path. The host preserves its UDP port so the
/// client can continue using the negotiated destination; the client may use a
/// fresh local port and announces it with the existing AEAD key.
fn migrate_udp_audio_socket(
    inner: &Arc<EngineInner>,
    record: &Arc<SessionRecord>,
    target: SocketAddr,
    host_side: bool,
) -> std::io::Result<()> {
    let Some(slot) = record.udp_audio.as_ref() else {
        return Ok(());
    };
    let old_addr = slot.local_addr();
    // A resume normally re-authenticates over the very link the socket is
    // already on. Rebinding then means asking the kernel for an address the
    // live socket still holds, which fails with EADDRINUSE and reports a
    // migration error for a session that never needed to move. Only rebind
    // when the selected interface actually changed.
    let desired = audio_bind_addr(inner, target, host_side)?;
    let unchanged = match (old_addr.map(|addr| addr.ip()), desired) {
        (Some(IpAddr::V4(current)), Some(next)) => current == next,
        (Some(current), None) => current.is_unspecified(),
        _ => false,
    };
    if unchanged {
        return Ok(());
    }
    let preserve_port = if host_side {
        old_addr.map(|addr| addr.port())
    } else {
        None
    };
    migrate_udp_slot(
        slot,
        desired.unwrap_or(Ipv4Addr::UNSPECIFIED),
        preserve_port,
        &|addr| {
            let socket = UdpSocket::bind(addr)?;
            tune_audio_socket(&socket);
            Ok(socket)
        },
    )
}

pub(crate) fn stop_host(inner: &EngineInner) {
    let taken = inner.host.lock().ok().and_then(|mut host| host.take());
    if let Some(record) = taken {
        record.stop();
    }
}

fn accept_loop(
    inner: Arc<EngineInner>,
    mut listeners: Vec<(TcpListener, Option<Ipv4Addr>)>,
    stop: Arc<AtomicBool>,
    current_addr: Arc<Mutex<Option<Ipv4Addr>>>,
    port: u16,
) {
    let mut bind_addr = current_addr.lock().ok().and_then(|addr| *addr);
    loop {
        if !inner.running.load(Ordering::Relaxed) || stop.load(Ordering::Relaxed) {
            break;
        }
        // Network interfaces can appear after the host starts (most notably
        // USB tethering). Add the new listener before dropping the old one,
        // preserving all session/PIN state while the control endpoint moves
        // to the preferred address.
        let desired = host_bind_addr(&inner.config());
        if desired != bind_addr {
            // A link-specific host also keeps a loopback listener for ADB.
            // If the configured preference changes to `adb`, that secondary
            // listener is already the desired endpoint; reuse it instead of
            // trying to bind the same address a second time.
            if listeners.iter().any(|(_, address)| *address == desired) {
                listeners.retain(|(_, address)| *address != bind_addr);
                bind_addr = desired;
                if let Ok(mut current) = current_addr.lock() {
                    *current = desired;
                }
                inner.start_advertiser(port, desired);
                continue;
            }
            // A wildcard listener conflicts with every specific-address
            // bind. Remove it only for the duration of this migration and
            // restore the wildcard fallback if the new bind fails.
            let replacing_wildcard = desired.is_none();
            let replacing_current_wildcard = !replacing_wildcard && bind_addr.is_none();
            if replacing_wildcard {
                listeners.clear();
            } else if replacing_current_wildcard {
                listeners.retain(|(_, address)| *address != bind_addr);
            }
            match bind_control_listener(desired, port) {
                Ok((next, next_addr, _)) => {
                    if !replacing_wildcard && bind_addr.is_some() {
                        listeners.retain(|(_, address)| *address != bind_addr);
                    }
                    listeners.push((next, next_addr));
                    if next_addr.is_some_and(|addr| addr != Ipv4Addr::LOCALHOST)
                        && !listeners
                            .iter()
                            .any(|(_, address)| *address == Some(Ipv4Addr::LOCALHOST))
                    {
                        if let Ok((loopback, _, _)) =
                            bind_control_listener(Some(Ipv4Addr::LOCALHOST), port)
                        {
                            listeners.push((loopback, Some(Ipv4Addr::LOCALHOST)));
                        }
                    }
                    if next_addr.is_none() {
                        listeners.retain(|(_, address)| *address != Some(Ipv4Addr::LOCALHOST));
                    }
                    bind_addr = next_addr;
                    if let Ok(mut current) = current_addr.lock() {
                        *current = next_addr;
                    }
                    inner.start_advertiser(port, next_addr);
                }
                Err(error) => {
                    if replacing_wildcard || replacing_current_wildcard {
                        if let Ok((fallback, fallback_addr, _)) =
                            bind_control_listener(bind_addr, port)
                        {
                            listeners.push((fallback, fallback_addr));
                            if fallback_addr.is_some_and(|addr| addr != Ipv4Addr::LOCALHOST) {
                                if let Ok((loopback, _, _)) =
                                    bind_control_listener(Some(Ipv4Addr::LOCALHOST), port)
                                {
                                    listeners.push((loopback, Some(Ipv4Addr::LOCALHOST)));
                                }
                            }
                        }
                    }
                    inner.emit(RelayEvent::Error {
                        message: format!("relay listener migration failed: {error}"),
                    });
                }
            }
        }
        let mut accepted = false;
        for (listener, listener_addr) in &listeners {
            match listener.accept() {
                Ok((mut stream, addr)) => {
                    accepted = true;
                    let listener_addr = *listener_addr;
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
                    // Keep a duplicate only for the exceptional path: the
                    // worker owns the accepted stream, but a failed spawn should
                    // still get one best-effort PairFail onto the peer.
                    let mut failure_stream = stream.try_clone().ok();
                    let worker_inner = Arc::clone(&inner);
                    match spawn_named(
                        format!("relay-peer-{addr}"),
                        Box::new(move || {
                            host_peer_thread(worker_inner, stream, addr, slot, listener_addr, port)
                        }),
                    ) {
                        Ok(_) => {}
                        Err(error) => {
                            if let Some(mut stream) = failure_stream.take() {
                                let _ = write_frame(
                                    &mut stream,
                                    &ControlMessage::PairFail {
                                        reason: "the host could not start a handshake worker"
                                            .into(),
                                    },
                                );
                            }
                            inner.emit(RelayEvent::Error {
                                message: format!("could not start peer handshake worker: {error}"),
                            });
                        }
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => {}
            }
            if accepted {
                break;
            }
        }
        if !accepted {
            std::thread::sleep(Duration::from_millis(50));
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
    connect_peer_with_spawner(inner, id, target, pin, roles, spawn_named);
}

pub(crate) fn connect_trusted_peer(
    inner: &Arc<EngineInner>,
    id: SessionId,
    target: SocketAddr,
    peer_id: String,
    secret: [u8; 32],
    roles: Roles,
) {
    let worker_inner = Arc::clone(inner);
    let failure_inner = Arc::clone(inner);
    let worker: Worker =
        Box::new(move || trusted_client_thread(worker_inner, id, target, peer_id, secret, roles));
    if let Err(error) = spawn_named(format!("relay-trusted-client-{target}"), worker) {
        fail_attempt(
            &failure_inner,
            id,
            format!("could not start trusted connection worker: {error}"),
        );
    }
}

fn connect_peer_with_spawner(
    inner: &Arc<EngineInner>,
    id: SessionId,
    target: SocketAddr,
    pin: String,
    roles: Roles,
    spawn: WorkerSpawner,
) {
    let worker_inner = Arc::clone(inner);
    let failure_inner = Arc::clone(inner);
    let worker: Worker = Box::new(move || client_thread(worker_inner, id, target, pin, roles));
    if let Err(error) = spawn(format!("relay-client-{target}"), worker) {
        fail_attempt(
            &failure_inner,
            id,
            format!("could not start relay connection worker: {error}"),
        );
    }
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
    _bind_addr: Option<Ipv4Addr>,
    control_port: u16,
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
    let (peer_id, peer_name, peer_kind, client_pake, audio_over_tcp) = match first {
        ControlMessage::ResumeHello {
            session_id,
            client_nonce,
        } => {
            resume_peer_session(&inner, SessionId(session_id), stream, &client_nonce);
            return;
        }
        ControlMessage::AudioHello {
            session_id,
            client_nonce,
        } => {
            host_audio_thread(&inner, stream, SessionId(session_id), &client_nonce);
            return;
        }
        ControlMessage::TrustedHello {
            protocol,
            device_id,
            device_name,
            device_kind,
            host_id,
            transport,
            client_nonce,
            // The codec parameters advertised here are deliberately dropped:
            // the shared post-authentication path negotiates them from the
            // sealed `SessionStart` frame, exactly as PIN pairing does.
            ..
        } if protocol == PROTOCOL_VERSION as u32 => {
            trusted_peer_thread(
                &inner,
                stream,
                peer_addr,
                control_port,
                TrustedHelloContext {
                    client_id: device_id,
                    peer_name: device_name,
                    peer_kind: device_kind,
                    host_id,
                    transport,
                    client_nonce,
                },
            );
            return;
        }
        ControlMessage::Hello {
            protocol,
            device_id,
            transport,
            device_name,
            device_kind,
            pake,
            ..
        } if protocol == PROTOCOL_VERSION as u32 => (
            if device_id.trim().is_empty() {
                device_name.clone()
            } else {
                device_id
            },
            device_name,
            device_kind,
            pake,
            transport.eq_ignore_ascii_case("adb"),
        ),
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

    let host_config = inner.config();
    let host_name = host_config.device_name;
    let Some(keys) = host_pake_exchange(
        &inner,
        &mut stream,
        peer_addr,
        &client_pake,
        host_name,
        host_config.device_id,
    ) else {
        return;
    };
    inner.clear_pairing_failures(peer_addr.ip());
    host_session_after_auth(
        inner,
        stream,
        peer_addr,
        HostAuthenticatedPeer {
            peer_id,
            peer_name,
            peer_kind,
            keys,
            audio_over_tcp,
            control_port,
            requested_id: None,
        },
    );
}

/// Authenticate and install the secondary TCP stream opened by an ADB
/// client. It is tied to the existing session's resume secret, so merely
/// reaching the forwarded localhost port cannot inject audio.
fn host_audio_thread(
    inner: &Arc<EngineInner>,
    mut stream: TcpStream,
    id: SessionId,
    client_nonce: &str,
) {
    let Some(record) = inner.session(id) else {
        let _ = write_frame(
            &mut stream,
            &ControlMessage::PairFail {
                reason: "unknown or expired audio session".into(),
            },
        );
        return;
    };
    let Some(slot) = record.tcp_audio.clone() else {
        let _ = write_frame(
            &mut stream,
            &ControlMessage::PairFail {
                reason: "session does not use the TCP audio transport".into(),
            },
        );
        return;
    };
    let Some(client_nonce) = decode_resume_nonce(client_nonce) else {
        return;
    };
    let server_nonce = fresh_resume_nonce();
    if write_frame(
        &mut stream,
        &ControlMessage::AudioChallenge {
            server_nonce: hex_encode(&server_nonce),
        },
    )
    .is_err()
    {
        return;
    }
    let proof = match read_frame(&mut stream) {
        Ok(ControlMessage::AudioProof { proof }) => hex_decode(&proof).ok(),
        _ => None,
    };
    let valid = proof.as_deref().is_some_and(|proof| {
        crate::crypto::tcp_audio_proof(
            &record.resume_secret,
            record.wire_id,
            &client_nonce,
            &server_nonce,
            Side::Client,
        )
        .ct_eq(proof)
        .into()
    });
    if !valid {
        let _ = write_frame(
            &mut stream,
            &ControlMessage::PairFail {
                reason: "TCP audio authentication failed".into(),
            },
        );
        return;
    }
    let server_proof = crate::crypto::tcp_audio_proof(
        &record.resume_secret,
        record.wire_id,
        &client_nonce,
        &server_nonce,
        Side::Host,
    );
    if write_frame(
        &mut stream,
        &ControlMessage::AudioReady {
            proof: hex_encode(&server_proof),
        },
    )
    .is_err()
    {
        return;
    }
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
    if !inner.session_alive(record.id) {
        return;
    }
    if let Err(error) = slot.install(stream) {
        inner.emit(RelayEvent::Error {
            message: format!("could not install authenticated ADB audio stream: {error}"),
        });
    }
}

/// Everything a `TrustedHello` frame carries that the trusted reconnect path
/// actually needs. The codec parameters the frame also advertises are not
/// kept here on purpose: `host_session_after_auth` re-negotiates them from
/// the sealed `SessionStart` frame so a trusted reconnect gets exactly the
/// same validation as a PIN pairing.
struct TrustedHelloContext {
    /// Stable peer identity the client claims; must match an enrolled
    /// trusted credential before anything else happens.
    client_id: String,
    peer_name: String,
    peer_kind: DeviceKind,
    /// Host identity the client believes it is reconnecting to.
    host_id: String,
    transport: String,
    /// Client half of the resume challenge, hex encoded.
    client_nonce: String,
}

fn trusted_peer_thread(
    inner: &Arc<EngineInner>,
    mut stream: TcpStream,
    peer_addr: SocketAddr,
    control_port: u16,
    hello: TrustedHelloContext,
) {
    let TrustedHelloContext {
        client_id,
        peer_name,
        peer_kind,
        host_id,
        transport,
        client_nonce,
    } = hello;
    let config = inner.config();
    if host_id != config.device_id {
        let _ = write_frame(
            &mut stream,
            &ControlMessage::PairFail {
                reason: "trusted host identity did not match".into(),
            },
        );
        return;
    }
    let Some(secret) = inner.trusted_secret(&client_id) else {
        let _ = write_frame(
            &mut stream,
            &ControlMessage::PairFail {
                reason: "peer is not trusted on this host".into(),
            },
        );
        return;
    };
    let Some(client_nonce) = decode_resume_nonce(&client_nonce) else {
        return;
    };
    let id = inner.next_session_id();
    let server_nonce = fresh_resume_nonce();
    if write_frame(
        &mut stream,
        &ControlMessage::TrustedChallenge {
            server_nonce: hex_encode(&server_nonce),
            session_id: id.0,
            host_id: config.device_id.clone(),
            host_name: config.device_name.clone(),
        },
    )
    .is_err()
    {
        return;
    }
    let proof = match read_frame(&mut stream) {
        Ok(ControlMessage::TrustedProof { proof }) => hex_decode(&proof).ok(),
        _ => None,
    };
    let valid = proof.as_deref().is_some_and(|proof| {
        crate::crypto::verify_trusted_proof(
            &secret,
            &client_id,
            &config.device_id,
            id.0,
            &client_nonce,
            &server_nonce,
            proof,
        )
    });
    if !valid {
        let _ = write_frame(
            &mut stream,
            &ControlMessage::PairFail {
                reason: "trusted authentication failed".into(),
            },
        );
        return;
    }
    let keys = crate::crypto::trusted_session_keys(
        &secret,
        &client_id,
        &config.device_id,
        id.0,
        &client_nonce,
        &server_nonce,
        Side::Host,
    );
    if write_frame(&mut stream, &ControlMessage::TrustedOk {}).is_err() {
        return;
    }
    let audio_over_tcp = transport.eq_ignore_ascii_case("adb");
    host_session_after_auth(
        Arc::clone(inner),
        stream,
        peer_addr,
        HostAuthenticatedPeer {
            peer_id: client_id,
            peer_name,
            peer_kind,
            keys,
            audio_over_tcp,
            control_port,
            requested_id: Some(id),
        },
    );
}

/// Finish a fresh or trusted authentication with one common negotiated
/// session setup. Keeping this after-auth path shared is important: a trusted
/// reconnect must receive exactly the same role/codec validation and worker
/// startup guarantees as a PIN pairing.
/// A peer that has proved possession of either the PIN or its trusted
/// credential, together with the transport facts the shared setup path needs.
struct HostAuthenticatedPeer {
    peer_id: String,
    peer_name: String,
    peer_kind: DeviceKind,
    /// Directional keys derived by the completed handshake. Moved, never
    /// cloned, so the session record keeps sole ownership.
    keys: SessionKeys,
    /// ADB peers carry audio on the authenticated secondary TCP stream
    /// instead of UDP.
    audio_over_tcp: bool,
    /// Control port this listener accepted on, reported back to ADB peers as
    /// their audio port.
    control_port: u16,
    /// Session id already allocated by a trusted reconnect, if any.
    requested_id: Option<SessionId>,
}

fn host_session_after_auth(
    inner: Arc<EngineInner>,
    mut stream: TcpStream,
    peer_addr: SocketAddr,
    peer: HostAuthenticatedPeer,
) {
    let HostAuthenticatedPeer {
        peer_id,
        peer_name,
        peer_kind,
        keys,
        audio_over_tcp,
        control_port,
        requested_id,
    } = peer;
    let Ok((control_sealer, control_opener)) = keys.control_channel() else {
        return;
    };
    let mut cipher = ControlCipher {
        sealer: control_sealer,
        opener: control_opener,
    };

    // ADB uses only its authenticated TCP secondary stream. Normal relay
    // audio is bound to the selected interface; wildcard is reserved for the
    // documented no-link fallback in `bind_udp_audio_socket`.
    let socket = if audio_over_tcp {
        None
    } else {
        match bind_udp_audio_socket(&inner, peer_addr, true) {
            Ok(socket) => {
                tune_audio_socket(&socket);
                match UdpAudioSlot::new(socket) {
                    Ok(slot) => Some(slot),
                    Err(error) => {
                        let _ = cipher.send(
                            &mut stream,
                            &ControlMessage::PairFail {
                                reason: format!("could not prepare audio socket: {error}"),
                            },
                        );
                        return;
                    }
                }
            }
            Err(error) => {
                let _ = cipher.send(
                    &mut stream,
                    &ControlMessage::PairFail {
                        reason: format!("could not open audio socket: {error}"),
                    },
                );
                return;
            }
        }
    };
    let udp_audio_port = socket
        .as_ref()
        .and_then(|socket| socket.local_addr())
        .map(|addr| addr.port())
        .unwrap_or(0);
    let audio_port = if audio_over_tcp {
        control_port
    } else {
        udp_audio_port
    };
    let id = requested_id.unwrap_or_else(|| inner.next_session_id());
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
    let local = inner.config().local_format();
    let tcp_audio = audio_over_tcp.then(TcpAudioSlot::new);
    let record = Arc::new(SessionRecord {
        id,
        wire_id: id.0,
        peer: PeerInfo {
            id: peer_id,
            name: peer_name,
            kind: peer_kind,
            addr: peer_addr,
        },
        roles,
        codec,
        format,
        sending: roles.receive,
        receiving: roles.emit,
        stop: Arc::new(AtomicBool::new(false)),
        bye_requested: AtomicBool::new(false),
        control_generation: AtomicU64::new(1),
        resume_secret: keys.resume_auth_key(),
        trust_secret: Mutex::new(None),
        tcp_audio,
        udp_audio: socket.clone(),
        control_peer_addr: Mutex::new(peer_addr),
        control_state: Mutex::new(ControlState::Active),
        peer_audio_addr: Mutex::new(None),
        outgoing: crate::PcmQueue::new(crate::DEFAULT_QUEUE_CAPACITY),
        incoming: crate::PcmQueue::new(crate::DEFAULT_QUEUE_CAPACITY),
        capture_convert: Mutex::new(prepared_capture_converter(local, format)),
        audio_sealer: Mutex::new(audio_sealer),
        audio_opener: Mutex::new(audio_opener),
    });
    if !inner.insert_session(Arc::clone(&record)) {
        let _ = cipher.send(
            &mut stream,
            &ControlMessage::PairFail {
                reason: "the host is already at its session limit".into(),
            },
        );
        return;
    }
    if let Err(reason) = spawn_session_workers(&inner, &record, &socket, true) {
        let _ = cipher.send(
            &mut stream,
            &ControlMessage::PairFail {
                reason: reason.clone(),
            },
        );
        teardown(&inner, id, reason);
        return;
    }
    if !inner.session_alive(id) {
        return;
    }
    if cipher
        .send(&mut stream, &ControlMessage::SessionReady {})
        .is_err()
    {
        teardown(
            &inner,
            id,
            "handshake failed while starting the session".into(),
        );
        return;
    }
    inner.emit(RelayEvent::SessionEstablished {
        id,
        peer: record.peer.clone(),
        roles,
        codec,
    });
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
    host_id: String,
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
            device_id: host_id,
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

/// Host-side resume of a session whose control link dropped. A resume is
/// challenge-response authenticated with the secret derived during the
/// original PAKE; the host-wide PIN is deliberately not sufficient here.
/// Only the control channel is rekeyed: the UDP audio workers never stopped,
/// so their keys and replay windows carry on untouched.
fn resume_peer_session(
    inner: &Arc<EngineInner>,
    id: SessionId,
    mut stream: TcpStream,
    client_nonce: &str,
) {
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT));
    let peer_addr = stream.peer_addr().ok();

    let Some(record) = inner.session(id) else {
        let _ = write_frame(
            &mut stream,
            &ControlMessage::PairFail {
                reason: "unknown or expired session".into(),
            },
        );
        return;
    };

    let Some(client_nonce) = decode_resume_nonce(client_nonce) else {
        let _ = write_frame(
            &mut stream,
            &ControlMessage::PairFail {
                reason: "malformed resume nonce".into(),
            },
        );
        return;
    };

    // The old control owner must have actually dropped before this state can
    // be claimed. This also consumes the one in-flight challenge, so racing
    // reconnects and a proof replay cannot both take over the session.
    let Some(generation) = record.begin_resume() else {
        let _ = write_frame(
            &mut stream,
            &ControlMessage::PairFail {
                reason: "the session control channel is still active".into(),
            },
        );
        return;
    };

    let server_nonce = fresh_resume_nonce();
    if write_frame(
        &mut stream,
        &ControlMessage::ResumeChallenge {
            server_nonce: hex_encode(&server_nonce),
            generation,
        },
    )
    .is_err()
    {
        record.cancel_resume(generation);
        return;
    }

    let proof = match read_frame(&mut stream) {
        Ok(ControlMessage::ResumeProof { proof }) => hex_decode(&proof).ok(),
        _ => None,
    };
    let valid = proof
        .as_deref()
        .map(|proof| {
            verify_resume_proof(
                &record.resume_secret,
                record.wire_id,
                &client_nonce,
                &server_nonce,
                generation,
                proof,
            )
        })
        .unwrap_or(false);
    if !valid {
        record.cancel_resume(generation);
        let _ = write_frame(
            &mut stream,
            &ControlMessage::PairFail {
                reason: "resume authentication failed".into(),
            },
        );
        return;
    }

    if !inner.session_alive(record.id) {
        record.cancel_resume(generation);
        return;
    }

    let Ok((sealer, opener)) = resume_control_channel(
        &record.resume_secret,
        Side::Host,
        record.wire_id,
        &client_nonce,
        &server_nonce,
        generation,
    ) else {
        record.cancel_resume(generation);
        return;
    };
    let mut cipher = ControlCipher { sealer, opener };
    if let Some(peer_addr) = peer_addr {
        let migrated = migrate_udp_audio_socket(inner, &record, peer_addr, true);
        if let Err(error) = migrated {
            // A failed rebind must not destroy a still-usable authenticated
            // session. The old socket remains installed and the control
            // resume continues; the next authenticated resume can retry.
            inner.emit(RelayEvent::Error {
                message: format!("UDP audio interface migration failed: {error}"),
            });
        }
        // The control address is learned from the authenticated resume, never
        // from discovery. It is reported independently from the stable peer
        // identity so diagnostics show the path actually in use.
        if let Ok(mut current) = record.control_peer_addr.lock() {
            *current = peer_addr;
        }
    }
    // Commit the state transition before acknowledging success. The grace
    // watcher may win the deadline race while this worker is deriving keys;
    // in that case the old control owner has already been declared gone and
    // this challenge must not produce a false-positive ResumeOk.
    if !record.finish_resume(generation) {
        return;
    }
    if cipher
        .send(&mut stream, &ControlMessage::ResumeOk {})
        .is_err()
    {
        // The old grace watcher has exited after the generation transition.
        // Re-enter the normal grace state so a failed response does not leave
        // the session permanently marked Active without a control owner.
        // `finish_resume` already rotated the control generation, so the
        // original watcher has correctly returned `Resumed` and cannot watch
        // this new owner. Re-enter the eligible state explicitly and give one
        // bounded replacement attempt. If no replacement arrives, tear the
        // record down here instead of leaving an Active zombie in the map.
        handle_failed_resume_ok(inner, &record);
        return;
    }
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
    await_resume_grace_with_deadlines(inner, record, RESUME_GRACE, HANDSHAKE_TIMEOUT)
}

/// Recover from committing a new control generation before its `ResumeOk`
/// could be delivered. This is kept as one helper so the failure path cannot
/// accidentally leave an active-but-ownerless record in the session map.
fn handle_failed_resume_ok(inner: &Arc<EngineInner>, record: &Arc<SessionRecord>) -> bool {
    handle_failed_resume_ok_with_deadlines(inner, record, RESUME_GRACE, HANDSHAKE_TIMEOUT)
}

fn handle_failed_resume_ok_with_deadlines(
    inner: &Arc<EngineInner>,
    record: &Arc<SessionRecord>,
    grace: Duration,
    handshake_timeout: Duration,
) -> bool {
    let _ = record.mark_control_dropped();
    if await_resume_grace_with_deadlines(inner, record, grace, handshake_timeout) {
        true
    } else {
        teardown(
            inner,
            record.id,
            "resumed control channel could not deliver ResumeOk".into(),
        );
        false
    }
}

fn await_resume_grace_with_deadlines(
    inner: &Arc<EngineInner>,
    record: &Arc<SessionRecord>,
    grace: Duration,
    handshake_timeout: Duration,
) -> bool {
    let _ = record.mark_control_dropped();
    let generation = record.control_generation.load(Ordering::Relaxed);
    let deadline = Instant::now() + grace;
    let mut in_flight_deadline = None;
    loop {
        if record.stop.load(Ordering::Relaxed) || !inner.session_alive(record.id) {
            return true;
        }
        if record.control_generation.load(Ordering::Acquire) != generation {
            return true;
        }
        if Instant::now() >= deadline {
            // Expiry and a successful resume serialize on the record state
            // lock. A stale watcher must not tear down a session after the
            // new control owner has completed its handshake.
            match record.expire_resume_grace(generation) {
                ResumeGraceResult::Expired => return false,
                ResumeGraceResult::Resumed => return true,
                ResumeGraceResult::InProgress { generation } => {
                    let challenge_deadline = *in_flight_deadline
                        .get_or_insert_with(|| Instant::now() + handshake_timeout);
                    if Instant::now() >= challenge_deadline && record.abort_resume(generation) {
                        return false;
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn fail_trusted_attempt(
    inner: &Arc<EngineInner>,
    id: SessionId,
    target: SocketAddr,
    host_id: &str,
    reason: String,
) {
    inner.note_candidate_failure(host_id, target);
    fail_attempt(inner, id, reason);
}

fn trusted_client_thread(
    inner: Arc<EngineInner>,
    id: SessionId,
    target: SocketAddr,
    host_id: String,
    secret: [u8; 32],
    roles: Roles,
) {
    if roles.is_empty() {
        fail_attempt(&inner, id, "no audio direction requested".into());
        return;
    }
    let config = inner.config();
    let bind = netlink::outbound_bind_addr(&netlink::local_links(), target, config.transport);
    let mut stream = match connect_control_tcp(target, bind, config.transport) {
        Ok(stream) => stream,
        Err(error) => {
            fail_trusted_attempt(
                &inner,
                id,
                target,
                &host_id,
                format!("trusted connection failed: {error}"),
            );
            return;
        }
    };
    let _ = stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT));
    let client_nonce = fresh_resume_nonce();
    if write_frame(
        &mut stream,
        &ControlMessage::TrustedHello {
            protocol: PROTOCOL_VERSION as u32,
            device_id: config.device_id.clone(),
            device_name: config.device_name.clone(),
            device_kind: config.device_kind,
            host_id: host_id.clone(),
            transport: config.transport.as_str().into(),
            roles,
            sample_rate: config.sample_rate,
            channels: config.channels,
            client_nonce: hex_encode(&client_nonce),
        },
    )
    .is_err()
    {
        fail_trusted_attempt(
            &inner,
            id,
            target,
            &host_id,
            "trusted handshake failed while sending hello".into(),
        );
        return;
    }
    let (server_nonce, wire_id, returned_host_id, host_name) = match read_frame(&mut stream) {
        Ok(ControlMessage::TrustedChallenge {
            server_nonce,
            session_id,
            host_id: challenge_host_id,
            host_name,
        }) => match decode_resume_nonce(&server_nonce) {
            Some(server_nonce) => (server_nonce, session_id, challenge_host_id, host_name),
            None => {
                fail_trusted_attempt(
                    &inner,
                    id,
                    target,
                    &host_id,
                    "trusted host sent a malformed challenge".into(),
                );
                return;
            }
        },
        Ok(ControlMessage::PairFail { reason }) => {
            fail_trusted_attempt(
                &inner,
                id,
                target,
                &host_id,
                format!("host rejected trusted connection: {reason}"),
            );
            return;
        }
        Ok(_) | Err(_) => {
            fail_trusted_attempt(
                &inner,
                id,
                target,
                &host_id,
                "trusted handshake response was malformed".into(),
            );
            return;
        }
    };
    if returned_host_id != host_id {
        fail_trusted_attempt(
            &inner,
            id,
            target,
            &host_id,
            "trusted host identity did not match".into(),
        );
        return;
    }
    let proof = crate::crypto::trusted_proof(
        &secret,
        &config.device_id,
        &host_id,
        wire_id,
        &client_nonce,
        &server_nonce,
    );
    if write_frame(
        &mut stream,
        &ControlMessage::TrustedProof {
            proof: hex_encode(&proof),
        },
    )
    .is_err()
    {
        fail_trusted_attempt(
            &inner,
            id,
            target,
            &host_id,
            "trusted handshake failed while proving credential".into(),
        );
        return;
    }
    match read_frame(&mut stream) {
        Ok(ControlMessage::TrustedOk {}) => {}
        Ok(ControlMessage::PairFail { reason }) => {
            fail_trusted_attempt(
                &inner,
                id,
                target,
                &host_id,
                format!("host rejected trusted connection: {reason}"),
            );
            return;
        }
        Ok(_) | Err(_) => {
            fail_trusted_attempt(
                &inner,
                id,
                target,
                &host_id,
                "trusted handshake was not accepted".into(),
            );
            return;
        }
    }
    let keys = crate::crypto::trusted_session_keys(
        &secret,
        &config.device_id,
        &host_id,
        wire_id,
        &client_nonce,
        &server_nonce,
        Side::Client,
    );
    let Ok((sealer, opener)) = keys.control_channel() else {
        fail_trusted_attempt(
            &inner,
            id,
            target,
            &host_id,
            "trusted control keys could not be prepared".into(),
        );
        return;
    };
    client_session_after_auth(
        inner,
        stream,
        ControlCipher { sealer, opener },
        ClientAuthenticatedSession {
            id,
            target,
            roles,
            config,
            host_name,
            host_id,
            keys,
        },
    );
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

    let mut stream = match connect_control_tcp(target, bind, config.transport) {
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
        device_id: config.device_id.clone(),
        transport: config.transport.as_str().into(),
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

    let (host_pake, host_name, host_id) = match read_frame(&mut stream) {
        Ok(ControlMessage::Challenge {
            protocol,
            pake,
            host_name,
            device_id,
        }) if protocol == PROTOCOL_VERSION as u32 => (pake, host_name, device_id),
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
    // PIN pairing is also authenticated and establishes the address that
    // should win if discovery later offers several addresses for this host.
    inner.note_candidate_success(&host_id, target);
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
    let audio_over_tcp = config.transport == crate::TransportPreference::Adb;
    // A legacy v3 host has no stable installation identity and does not know
    // the enrollment messages. Do not turn a successful connection to one
    // into an avoidable five-second wait for an acknowledgement that can
    // never arrive, and never persist a name-only bearer credential.
    let trust_secret =
        (config.trust_new_peers && !host_id.trim().is_empty()).then(fresh_trust_secret);

    let socket = if audio_over_tcp {
        None
    } else {
        match bind_udp_audio_socket(&inner, target, false) {
            Ok(socket) => {
                tune_audio_socket(&socket);
                match UdpAudioSlot::new(socket) {
                    Ok(slot) => Some(slot),
                    Err(error) => {
                        fail_attempt(
                            &inner,
                            id,
                            format!("could not prepare audio socket: {error}"),
                        );
                        return;
                    }
                }
            }
            Err(error) => {
                fail_attempt(&inner, id, format!("could not open audio socket: {error}"));
                return;
            }
        }
    };
    let host_audio_addr = SocketAddr::new(target.ip(), audio_port);

    let format = AudioFormat::new(config.sample_rate, config.channels, config.frame_ms);
    let local = config.local_format();
    let mut audio_sealer = audio_sealer;
    // Teach the host our UDP address before real audio flows. The announce is
    // sealed with the session key, so only the paired client can move it.
    if !audio_over_tcp {
        if let Ok(announce) = announce_packet(&mut audio_sealer, config.codec) {
            if let Some(socket) = socket.as_ref().and_then(|slot| slot.current()) {
                let _ = socket.send_to(&announce, host_audio_addr);
            }
        }
    }
    let tcp_audio = audio_over_tcp.then(TcpAudioSlot::new);
    let record = Arc::new(SessionRecord {
        id,
        wire_id,
        peer: PeerInfo {
            id: if host_id.trim().is_empty() {
                host_name.clone()
            } else {
                host_id
            },
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
        control_generation: AtomicU64::new(1),
        resume_secret: keys.resume_auth_key(),
        trust_secret: Mutex::new(trust_secret),
        tcp_audio: tcp_audio.clone(),
        udp_audio: socket.clone(),
        control_peer_addr: Mutex::new(target),
        control_state: Mutex::new(ControlState::Active),
        peer_audio_addr: Mutex::new(Some(host_audio_addr)),
        outgoing: crate::PcmQueue::new(crate::DEFAULT_QUEUE_CAPACITY),
        incoming: crate::PcmQueue::new(crate::DEFAULT_QUEUE_CAPACITY),
        capture_convert: Mutex::new(prepared_capture_converter(local, format)),
        audio_sealer: Mutex::new(audio_sealer),
        audio_opener: Mutex::new(audio_opener),
    });
    if !inner.insert_session(Arc::clone(&record)) {
        let reason = "the local session limit was reached".to_string();
        let _ = cipher.send(
            &mut stream,
            &ControlMessage::Bye {
                reason: reason.clone(),
            },
        );
        fail_attempt(&inner, id, reason);
        return;
    }
    if let Err(reason) = spawn_session_workers(&inner, &record, &socket, false) {
        let _ = cipher.send(
            &mut stream,
            &ControlMessage::Bye {
                reason: reason.clone(),
            },
        );
        // `spawn_worker_with_report` has already raised the stop flag so a
        // worker that did start will exit. Startup failure still needs an
        // explicit removal: `fail_session` treats an already-set stop flag as
        // an orderly shutdown and would otherwise leave this record in the
        // session map without a SessionLost event.
        inner.emit(RelayEvent::Error {
            message: reason.clone(),
        });
        teardown(&inner, id, reason);
        return;
    }

    if !inner.session_alive(id) {
        return;
    }

    // ADB audio is supervised independently. A temporary missing forwarding
    // rule must not turn an otherwise healthy authenticated control session
    // into a false disconnect.

    let trusted_secret = record.trust_secret.lock().ok().and_then(|slot| *slot);
    if let Some(secret) = trusted_secret {
        enroll_trusted_peer(
            &inner,
            &record,
            &mut stream,
            &mut cipher,
            &config.device_id,
            secret,
        );
    }

    inner.emit(RelayEvent::SessionEstablished {
        id,
        peer: record.peer.clone(),
        roles,
        codec: config.codec,
    });

    client_control_loop(inner, record, stream, cipher, socket, target);
}

/// Ask the host to retain the credential generated by an explicit PIN
/// pairing. The embedding is notified only after the authenticated host
/// acknowledges the write; otherwise it could persist a credential that the
/// host never actually accepted and every later auto-connect would fail.
fn enroll_trusted_peer(
    inner: &Arc<EngineInner>,
    record: &Arc<SessionRecord>,
    stream: &mut TcpStream,
    cipher: &mut ControlCipher,
    local_peer_id: &str,
    secret: [u8; 32],
) {
    if cipher
        .send(
            stream,
            &ControlMessage::TrustEnroll {
                peer_id: local_peer_id.to_owned(),
                secret: hex_encode(&secret),
            },
        )
        .is_err()
    {
        return;
    }
    let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
    while Instant::now() < deadline {
        match cipher.receive(stream) {
            Ok(ControlMessage::TrustAccepted {}) => {
                inner.emit(RelayEvent::TrustedPeerAvailable {
                    peer_id: record.peer.id.clone(),
                    peer: record.peer.clone(),
                    secret,
                });
                return;
            }
            Ok(ControlMessage::TrustRejected { .. }) => return,
            // Keepalive is legal immediately after SessionReady. It is a
            // one-way liveness hint, so consuming it while waiting for the
            // enrollment acknowledgement does not require a response.
            Ok(_) => {}
            Err(error) if is_timeout(&error) => {}
            Err(_) => return,
        }
    }
}

/// Shared post-authentication client setup used by both PIN and trusted
/// handshakes.
/// The locally decided session facts a client carries into the shared
/// post-authentication setup, once the host has been authenticated.
struct ClientAuthenticatedSession {
    id: SessionId,
    target: SocketAddr,
    roles: Roles,
    config: crate::EngineConfig,
    host_name: String,
    host_id: String,
    /// Directional keys derived by the completed handshake, moved into the
    /// session record below.
    keys: SessionKeys,
}

fn client_session_after_auth(
    inner: Arc<EngineInner>,
    mut stream: TcpStream,
    mut cipher: ControlCipher,
    session: ClientAuthenticatedSession,
) {
    let ClientAuthenticatedSession {
        id,
        target,
        roles,
        config,
        host_name,
        host_id,
        keys,
    } = session;
    let (audio_port, wire_id) = match cipher.receive(&mut stream) {
        Ok(ControlMessage::PairOk {
            audio_port,
            session_id,
        }) => (audio_port, session_id),
        Ok(ControlMessage::PairFail { reason }) => {
            fail_trusted_attempt(
                &inner,
                id,
                target,
                &host_id,
                format!("host rejected pairing: {reason}"),
            );
            return;
        }
        Ok(_) | Err(_) => {
            fail_trusted_attempt(
                &inner,
                id,
                target,
                &host_id,
                "pairing response was malformed".into(),
            );
            return;
        }
    };
    if cipher
        .send(
            &mut stream,
            &ControlMessage::SessionStart {
                roles,
                codec: config.codec,
                frame_ms: config.frame_ms,
                sample_rate: config.sample_rate,
                channels: config.channels,
            },
        )
        .is_err()
    {
        fail_trusted_attempt(
            &inner,
            id,
            target,
            &host_id,
            "handshake failed during session setup".into(),
        );
        return;
    }
    match cipher.receive(&mut stream) {
        Ok(ControlMessage::SessionReady {}) => {}
        Ok(ControlMessage::PairFail { reason }) => {
            fail_trusted_attempt(
                &inner,
                id,
                target,
                &host_id,
                format!("host rejected session: {reason}"),
            );
            return;
        }
        Ok(_) | Err(_) => {
            fail_trusted_attempt(
                &inner,
                id,
                target,
                &host_id,
                "host sent an unexpected session response".into(),
            );
            return;
        }
    }
    let Ok((audio_sealer, audio_opener)) = keys.audio_channel() else {
        fail_trusted_attempt(
            &inner,
            id,
            target,
            &host_id,
            "audio keys could not be prepared".into(),
        );
        return;
    };
    let audio_over_tcp = config.transport == crate::TransportPreference::Adb;
    // This helper is used for an already authenticated trusted reconnect;
    // trusted credentials are enrolled only by the explicit PIN path.
    let trust_secret = None;
    let socket = if audio_over_tcp {
        None
    } else {
        match bind_udp_audio_socket(&inner, target, false) {
            Ok(socket) => {
                tune_audio_socket(&socket);
                match UdpAudioSlot::new(socket) {
                    Ok(slot) => Some(slot),
                    Err(error) => {
                        fail_trusted_attempt(
                            &inner,
                            id,
                            target,
                            &host_id,
                            format!("could not prepare audio socket: {error}"),
                        );
                        return;
                    }
                }
            }
            Err(error) => {
                fail_trusted_attempt(
                    &inner,
                    id,
                    target,
                    &host_id,
                    format!("could not open audio socket: {error}"),
                );
                return;
            }
        }
    };
    let host_audio_addr = SocketAddr::new(target.ip(), audio_port);
    let format = AudioFormat::new(config.sample_rate, config.channels, config.frame_ms);
    let local = config.local_format();
    let mut audio_sealer = audio_sealer;
    if !audio_over_tcp {
        if let Ok(announce) = announce_packet(&mut audio_sealer, config.codec) {
            if let Some(socket) = socket.as_ref().and_then(|slot| slot.current()) {
                let _ = socket.send_to(&announce, host_audio_addr);
            }
        }
    }
    let tcp_audio = audio_over_tcp.then(TcpAudioSlot::new);
    let record = Arc::new(SessionRecord {
        id,
        wire_id,
        peer: PeerInfo {
            id: if host_id.trim().is_empty() {
                host_name.clone()
            } else {
                host_id
            },
            name: host_name,
            kind: DeviceKind::Other,
            addr: target,
        },
        roles,
        codec: config.codec,
        format,
        sending: roles.emit,
        receiving: roles.receive,
        stop: Arc::new(AtomicBool::new(false)),
        bye_requested: AtomicBool::new(false),
        control_generation: AtomicU64::new(1),
        resume_secret: keys.resume_auth_key(),
        trust_secret: Mutex::new(trust_secret),
        tcp_audio: tcp_audio.clone(),
        udp_audio: socket.clone(),
        control_peer_addr: Mutex::new(target),
        control_state: Mutex::new(ControlState::Active),
        peer_audio_addr: Mutex::new(Some(host_audio_addr)),
        outgoing: crate::PcmQueue::new(crate::DEFAULT_QUEUE_CAPACITY),
        incoming: crate::PcmQueue::new(crate::DEFAULT_QUEUE_CAPACITY),
        capture_convert: Mutex::new(prepared_capture_converter(local, format)),
        audio_sealer: Mutex::new(audio_sealer),
        audio_opener: Mutex::new(audio_opener),
    });
    if !inner.insert_session(Arc::clone(&record)) {
        let reason = "the local session limit was reached".to_string();
        let _ = cipher.send(
            &mut stream,
            &ControlMessage::Bye {
                reason: reason.clone(),
            },
        );
        fail_trusted_attempt(&inner, id, target, &record.peer.id, reason);
        return;
    }
    if let Err(reason) = spawn_session_workers(&inner, &record, &socket, false) {
        let _ = cipher.send(
            &mut stream,
            &ControlMessage::Bye {
                reason: reason.clone(),
            },
        );
        inner.note_candidate_failure(&record.peer.id, target);
        teardown(&inner, id, reason);
        return;
    }
    if !inner.session_alive(id) {
        return;
    }
    // A clear TrustedOk only proves that the candidate followed the wire
    // shape. Record the address only after the sealed session setup and all
    // requested workers have succeeded, which proves possession of the
    // trusted credential on the host side.
    inner.note_candidate_success(&record.peer.id, target);
    inner.emit(RelayEvent::SessionEstablished {
        id,
        peer: record.peer.clone(),
        roles,
        codec: config.codec,
    });
    client_control_loop(inner, record, stream, cipher, socket, target);
}

fn open_tcp_audio(
    inner: &Arc<EngineInner>,
    record: &Arc<SessionRecord>,
    slot: &Arc<TcpAudioSlot>,
    target: SocketAddr,
) -> std::io::Result<()> {
    if !slot.begin_connect() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "ADB audio reconnect is already in progress",
        ));
    }
    let result = open_tcp_audio_once(inner, record, slot, target);
    slot.end_connect();
    result
}

fn open_tcp_audio_once(
    inner: &Arc<EngineInner>,
    record: &Arc<SessionRecord>,
    slot: &Arc<TcpAudioSlot>,
    target: SocketAddr,
) -> std::io::Result<()> {
    let config = inner.config();
    let bind = netlink::outbound_bind_addr(&netlink::local_links(), target, config.transport);
    let mut stream = connect_control_tcp(target, bind, config.transport)?;
    stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    stream.set_write_timeout(Some(HANDSHAKE_TIMEOUT))?;
    let client_nonce = fresh_resume_nonce();
    write_frame(
        &mut stream,
        &ControlMessage::AudioHello {
            session_id: record.wire_id,
            client_nonce: hex_encode(&client_nonce),
        },
    )?;
    let server_nonce = match read_frame(&mut stream)? {
        ControlMessage::AudioChallenge { server_nonce } => decode_resume_nonce(&server_nonce)
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "malformed TCP audio challenge",
                )
            })?,
        ControlMessage::PairFail { reason } => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                reason,
            ))
        }
        _ => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "unexpected TCP audio challenge",
            ))
        }
    };
    let proof = crate::crypto::tcp_audio_proof(
        &record.resume_secret,
        record.wire_id,
        &client_nonce,
        &server_nonce,
        Side::Client,
    );
    write_frame(
        &mut stream,
        &ControlMessage::AudioProof {
            proof: hex_encode(&proof),
        },
    )?;
    let server_proof = match read_frame(&mut stream)? {
        ControlMessage::AudioReady { proof } => hex_decode(&proof).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "malformed TCP audio response",
            )
        })?,
        ControlMessage::PairFail { reason } => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                reason,
            ))
        }
        _ => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "unexpected TCP audio response",
            ))
        }
    };
    let expected = crate::crypto::tcp_audio_proof(
        &record.resume_secret,
        record.wire_id,
        &client_nonce,
        &server_nonce,
        Side::Host,
    );
    if !bool::from(expected.ct_eq(&server_proof)) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "TCP audio host authentication failed",
        ));
    }
    stream.set_read_timeout(Some(Duration::from_millis(500)))?;
    stream.set_write_timeout(Some(Duration::from_millis(500)))?;
    slot.install(stream)
}

/// Client-side control watch: on a link drop the host is re-dialed and the
/// session resumed, so Wi-Fi roaming or brief outages do not end the session.
fn client_control_loop(
    inner: Arc<EngineInner>,
    record: Arc<SessionRecord>,
    stream: TcpStream,
    cipher: ControlCipher,
    socket: Option<Arc<UdpAudioSlot>>,
    target: SocketAddr,
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
            match resume_client_control(&inner, &record, target) {
                Some((resumed_stream, resumed_cipher, resumed_target)) => {
                    if let Ok(mut current) = record.control_peer_addr.lock() {
                        *current = resumed_target;
                    }
                    if record.tcp_audio.is_none() {
                        // Re-announce our UDP address from the real audio socket:
                        // the route may have changed link (e.g. Wi-Fi to USB
                        // tethering), and the host must learn the new source
                        // address. The announce is sealed with the session's
                        // unchanged audio key, which is exactly what authorises
                        // the host to follow us.
                        let audio_port = record
                            .peer_audio_addr
                            .lock()
                            .ok()
                            .and_then(|slot| slot.map(|addr| addr.port()));
                        match migrate_udp_audio_socket(&inner, &record, resumed_target, false) {
                            Ok(()) => {
                                if let (
                                    Some(socket),
                                    Some(audio_port),
                                    Ok(mut slot),
                                    Ok(mut sealer),
                                ) = (
                                    socket.as_ref().and_then(|slot| slot.current()),
                                    audio_port,
                                    record.peer_audio_addr.lock(),
                                    record.audio_sealer.lock(),
                                ) {
                                    let addr = SocketAddr::new(resumed_target.ip(), audio_port);
                                    *slot = Some(addr);
                                    if let Ok(announce) = announce_packet(&mut sealer, socket_codec)
                                    {
                                        let _ = socket.send_to(&announce, addr);
                                    }
                                }
                            }
                            Err(error) => inner.emit(RelayEvent::Error {
                                message: format!("UDP audio interface migration failed: {error}"),
                            }),
                        }
                    }
                    stream = Some((resumed_stream, resumed_cipher));
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
) -> Option<(TcpStream, ControlCipher, SocketAddr)> {
    let config = inner.config();
    let mut backoff = Duration::from_millis(500);
    for _ in 0..RESUME_ATTEMPTS {
        if !inner.session_alive(record.id) || record.stop.load(Ordering::Relaxed) {
            return None;
        }
        std::thread::sleep(backoff);
        backoff = (backoff * 2).min(Duration::from_secs(4));

        let targets = resume_targets(inner, record, target);
        let mut connected = None;
        for candidate in targets {
            if !inner.candidate_allowed(&record.peer.id, candidate) {
                continue;
            }
            let links = netlink::local_links();
            let bind = netlink::outbound_bind_addr(&links, candidate, config.transport);
            match connect_control_tcp(candidate, bind, config.transport) {
                Ok(stream) => {
                    connected = Some((stream, candidate));
                    break;
                }
                Err(_) => inner.note_candidate_failure(&record.peer.id, candidate),
            }
        }
        let Some((mut stream, resumed_target)) = connected else {
            continue;
        };
        let _ = stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT));
        let client_nonce = fresh_resume_nonce();
        if write_frame(
            &mut stream,
            &ControlMessage::ResumeHello {
                session_id: record.wire_id,
                client_nonce: hex_encode(&client_nonce),
            },
        )
        .is_err()
        {
            inner.note_candidate_failure(&record.peer.id, resumed_target);
            continue;
        }
        let (server_nonce, generation) = match read_frame(&mut stream) {
            Ok(ControlMessage::ResumeChallenge {
                server_nonce,
                generation,
            }) => (server_nonce, generation),
            Ok(ControlMessage::PairFail { reason }) => {
                if reason == "the session control channel is still active" {
                    // A previous control watcher may not have observed the
                    // drop yet. Treat this as a retryable race, not as a
                    // terminal failure for the reconnecting client.
                    continue;
                }
                inner.note_candidate_failure(&record.peer.id, resumed_target);
                // PairFail is cleartext at this point. It is evidence only
                // about this address, not about the session-wide resume
                // secret or the peer identity. Continue with other bounded
                // candidates, including a real USB path behind a spoofed ID.
                let _ = reason;
                continue;
            }
            _ => {
                inner.note_candidate_failure(&record.peer.id, resumed_target);
                continue;
            }
        };
        let Some(server_nonce) = decode_resume_nonce(&server_nonce) else {
            inner.note_candidate_failure(&record.peer.id, resumed_target);
            continue;
        };
        let proof = resume_proof(
            &record.resume_secret,
            record.wire_id,
            &client_nonce,
            &server_nonce,
            generation,
        );
        if write_frame(
            &mut stream,
            &ControlMessage::ResumeProof {
                proof: hex_encode(&proof),
            },
        )
        .is_err()
        {
            inner.note_candidate_failure(&record.peer.id, resumed_target);
            continue;
        }
        let Ok((sealer, opener)) = resume_control_channel(
            &record.resume_secret,
            Side::Client,
            record.wire_id,
            &client_nonce,
            &server_nonce,
            generation,
        ) else {
            inner.note_candidate_failure(&record.peer.id, resumed_target);
            continue;
        };
        let mut cipher = ControlCipher { sealer, opener };
        match cipher.receive(&mut stream) {
            Ok(ControlMessage::ResumeOk {}) => {
                let _ = stream.set_read_timeout(Some(Duration::from_millis(250)));
                inner.note_candidate_success(&record.peer.id, resumed_target);
                return Some((stream, cipher, resumed_target));
            }
            Ok(ControlMessage::PairFail { reason }) => {
                inner.note_candidate_failure(&record.peer.id, resumed_target);
                // A rejection here is still candidate-local. Continue so a
                // legitimate address can complete the session's proof flow.
                let _ = reason;
                continue;
            }
            _ => {
                inner.note_candidate_failure(&record.peer.id, resumed_target);
                continue;
            }
        }
    }
    None
}

/// Return the original destination plus addresses discovered for the same
/// stable peer. Discovery is deliberately identity-scoped: a nearby host
/// appearing on USB must not become a resume target merely because its port
/// matches.
fn resume_targets(
    inner: &Arc<EngineInner>,
    record: &Arc<SessionRecord>,
    original: SocketAddr,
) -> Vec<SocketAddr> {
    let mut targets = vec![original];
    for peer in inner.discovered_peers() {
        if peer.id == record.peer.id
            || (record.peer.id == record.peer.name && peer.name == record.peer.name)
        {
            let candidate = SocketAddr::new(peer.addr.ip(), original.port());
            if !targets.contains(&candidate) {
                targets.push(candidate);
            }
        }
    }
    targets.sort_by_key(|candidate| candidate_rank(inner, record, *candidate, original));
    // Discovery metadata is untrusted and may contain many addresses for a
    // forged stable ID. Keep reconnect work bounded while retaining the
    // original target (it is always included before the ranked truncation).
    targets.truncate(crate::MAX_TRUSTED_CANDIDATE_ADDRESSES);
    targets
}

fn candidate_rank(
    inner: &Arc<EngineInner>,
    record: &Arc<SessionRecord>,
    candidate: SocketAddr,
    original: SocketAddr,
) -> (u8, SocketAddr) {
    if inner.last_successful_address(&record.peer.id) == Some(candidate) {
        return (0, candidate);
    }
    let links = netlink::local_links();
    let classified = inner.discovered_link(candidate).or_else(|| {
        let IpAddr::V4(address) = candidate.ip() else {
            return None;
        };
        links
            .iter()
            .find(|link| link.contains(address))
            .map(|link| link.kind)
    });
    let same_subnet = links.iter().any(|link| {
        link.kind != crate::LinkKind::Usb
            && match candidate.ip() {
                IpAddr::V4(address) => link.contains(address),
                IpAddr::V6(_) => false,
            }
    });
    // A candidate's link classification is only a routing preference. The
    // resume proof below remains the identity check, so a spoofed same-ID
    // advertisement cannot win merely by claiming USB.
    let rank = match classified {
        Some(crate::LinkKind::Usb) => 1,
        _ if same_subnet => 2,
        Some(crate::LinkKind::Wifi) => 3,
        Some(crate::LinkKind::BluetoothPan) => 4,
        Some(crate::LinkKind::Lan) => 5,
        None if candidate == original => 2,
        None => 6,
    };
    (rank, candidate)
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

fn is_recoverable_tcp_audio_error(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::NotConnected
            | std::io::ErrorKind::UnexpectedEof
            | std::io::ErrorKind::TimedOut
            | std::io::ErrorKind::WouldBlock
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
    socket: Arc<UdpAudioSlot>,
    host_side: bool,
    ready: Option<SyncSender<Result<(), String>>>,
) {
    run_rx_source(inner, record, host_side, ready, move |datagram| {
        let Some(socket) = socket.current() else {
            return Ok(None);
        };
        match socket.recv_from(datagram) {
            Ok((len, addr)) => Ok(Some((len, Some(addr)))),
            Err(error) if is_timeout(&error) => Ok(None),
            Err(error) => Err(error),
        }
    });
}

fn read_tcp_audio_frame(stream: &mut impl Read, output: &mut [u8]) -> std::io::Result<usize> {
    let mut length = [0u8; 4];
    stream.read_exact(&mut length)?;
    let length = u32::from_le_bytes(length) as usize;
    if length == 0 || length > output.len() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "TCP audio frame is out of range",
        ));
    }
    stream.read_exact(&mut output[..length])?;
    Ok(length)
}

fn write_tcp_audio_frame(stream: &mut impl Write, datagram: &[u8]) -> std::io::Result<()> {
    if datagram.is_empty() || datagram.len() > MAX_DATAGRAM {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "TCP audio frame is out of range",
        ));
    }
    let length = u32::try_from(datagram.len()).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "TCP audio frame is too large",
        )
    })?;
    stream.write_all(&length.to_le_bytes())?;
    stream.write_all(datagram)?;
    stream.flush()
}

/// authenticated control session. Only this one supervisor may dial for a
/// session; the slot gate also serializes a control-resume race.
fn run_tcp_audio_supervisor(
    inner: Arc<EngineInner>,
    record: Arc<SessionRecord>,
    audio: Arc<TcpAudioSlot>,
    target: SocketAddr,
) {
    const MAX_FAILURES: u32 = 8;
    let mut failures = 0u32;
    let mut backoff = Duration::from_millis(250);
    loop {
        if !inner.session_alive(record.id) || record.stop.load(Ordering::Relaxed) {
            return;
        }
        if audio.current().is_some() {
            failures = 0;
            backoff = Duration::from_millis(250);
            std::thread::sleep(Duration::from_millis(250));
            continue;
        }
        match open_tcp_audio(&inner, &record, &audio, target) {
            Ok(()) => {
                failures = 0;
                backoff = Duration::from_millis(250);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(error) => {
                failures = failures.saturating_add(1);
                if failures >= MAX_FAILURES {
                    let reason = if error.kind() == std::io::ErrorKind::PermissionDenied {
                        "ADB audio authentication was rejected; recreate forwarding and pair again"
                    } else {
                        "ADB audio forwarding is not reachable; create the adb reverse/forward rule and retry"
                    };
                    inner.emit(RelayEvent::Error {
                        message: reason.into(),
                    });
                    teardown(&inner, record.id, reason.into());
                    return;
                }
                std::thread::sleep(backoff);
                backoff = (backoff * 2).min(Duration::from_secs(4));
            }
        }
    }
}

fn run_tcp_rx(
    inner: Arc<EngineInner>,
    record: Arc<SessionRecord>,
    audio: Arc<TcpAudioSlot>,
    ready: Option<SyncSender<Result<(), String>>>,
) {
    let source_record = Arc::clone(&record);
    run_rx_source(inner, record, false, ready, move |datagram| loop {
        let Some(connection) = audio.wait(&source_record.stop) else {
            return Ok(None);
        };
        let result = connection
            .reader
            .lock()
            .map_err(|_| std::io::Error::other("TCP audio reader is poisoned"))
            .and_then(|mut reader| read_tcp_audio_frame(&mut *reader, datagram));
        match result {
            Ok(len) => return Ok(Some((len, None))),
            Err(error) if is_timeout(&error) => return Ok(None),
            Err(_) => {
                audio.clear(&connection);
                continue;
            }
        }
    });
}

fn run_rx_source(
    inner: Arc<EngineInner>,
    record: Arc<SessionRecord>,
    host_side: bool,
    ready: Option<SyncSender<Result<(), String>>>,
    mut receive: impl FnMut(&mut [u8]) -> std::io::Result<Option<(usize, Option<SocketAddr>)>>,
) {
    request_realtime_thread();
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
            let reason = format!("decoder init failed: {error}");
            report_worker_startup(ready, Err(reason.clone()));
            fail_session(&inner, &record, reason);
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
    let mut converted =
        Vec::with_capacity(converter.output_capacity_for(record.format.frame_samples()));
    report_worker_startup(ready, Ok(()));
    let mut sumsq = 0.0f64;
    let mut level_samples = 0usize;
    let mut frames_since_level = 0u32;

    loop {
        if !inner.session_alive(record.id) {
            break;
        }
        let Some((len, addr)) = (match receive(&mut datagram) {
            Ok(result) => result,
            Err(error) => {
                fail_session(&inner, &record, format!("audio socket failed: {error}"));
                return;
            }
        }) else {
            continue;
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
            if let Some(addr) = addr {
                if let Ok(mut slot) = record.peer_audio_addr.lock() {
                    if *slot != Some(addr) {
                        *slot = Some(addr);
                    }
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
fn run_tx(
    inner: Arc<EngineInner>,
    record: Arc<SessionRecord>,
    socket: Arc<UdpAudioSlot>,
    ready: Option<SyncSender<Result<(), String>>>,
) {
    let ready_record = Arc::clone(&record);
    let address_record = Arc::clone(&record);
    run_tx_source(
        inner,
        record,
        ready,
        move || {
            ready_record
                .peer_audio_addr
                .lock()
                .ok()
                .and_then(|slot| *slot)
                .is_some()
        },
        move |datagram| {
            let address = address_record
                .peer_audio_addr
                .lock()
                .ok()
                .and_then(|slot| *slot)
                .ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::WouldBlock,
                        "peer audio address unknown",
                    )
                })?;
            let Some(socket) = socket.current() else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "UDP audio socket is not ready",
                ));
            };
            socket.send_to(datagram, address).map(|_| ())
        },
    );
}

fn send_tcp_audio_datagram(audio: &Arc<TcpAudioSlot>, datagram: &[u8]) -> std::io::Result<()> {
    let Some(connection) = audio.current() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "TCP audio connection is not ready",
        ));
    };
    let result = connection
        .writer
        .lock()
        .map_err(|_| std::io::Error::other("TCP audio writer is poisoned"))
        .and_then(|mut writer| write_tcp_audio_frame(&mut *writer, datagram));
    match result {
        Ok(()) => Ok(()),
        Err(error) if is_recoverable_tcp_audio_error(&error) => {
            // ADB audio is a replaceable secondary stream. Its loss must wake
            // the supervisor without being interpreted by run_tx_source as a
            // fatal session failure.
            audio.clear(&connection);
            Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "ADB audio connection is reconnecting",
            ))
        }
        Err(error) => Err(error),
    }
}

fn run_tcp_tx(
    inner: Arc<EngineInner>,
    record: Arc<SessionRecord>,
    audio: Arc<TcpAudioSlot>,
    ready: Option<SyncSender<Result<(), String>>>,
) {
    let send_audio = Arc::clone(&audio);
    run_tx_source(
        inner,
        record,
        ready,
        move || send_audio.current().is_some(),
        move |datagram| send_tcp_audio_datagram(&audio, datagram),
    );
}

fn run_tx_source(
    inner: Arc<EngineInner>,
    record: Arc<SessionRecord>,
    ready: Option<SyncSender<Result<(), String>>>,
    mut transport_ready: impl FnMut() -> bool,
    mut send_datagram: impl FnMut(&[u8]) -> std::io::Result<()>,
) {
    request_realtime_thread();
    // Same reasoning as the receive side: captured audio that cannot be sent
    // promptly is better dropped than delivered late.
    record
        .outgoing
        .set_target_depth(record.format.frame_samples() * crate::CAPTURE_DEPTH_FRAMES);
    let mut encoder = match make_encoder(record.codec, record.format) {
        Ok(encoder) => encoder,
        Err(error) => {
            let reason = format!("encoder init failed: {error}");
            report_worker_startup(ready, Err(reason.clone()));
            fail_session(&inner, &record, reason);
            return;
        }
    };

    let frame_samples = record.format.frame_samples();
    report_worker_startup(ready, Ok(()));
    let mut sequence = 0u32;
    let mut timestamp_ms = 0u32;
    let mut payload = Vec::with_capacity(4096);
    // A send-only session decodes nothing, so the receive path never reports a
    // level for it and its meter would sit at zero however loud the transmitted
    // audio is. Meter the outgoing frames instead, and leave the meter to the
    // receive path whenever this session also receives: both directions share
    // one `AudioLevel` per session and the incoming level is the one the UI
    // documents.
    let meter_outgoing = !record.receiving;
    let mut frames_since_level = 0u32;
    let mut sumsq = 0f64;
    let mut level_samples = 0usize;

    loop {
        if !inner.session_alive(record.id) {
            break;
        }
        // The host learns the peer address from an authenticated announce;
        // TCP audio waits for the authenticated secondary stream. Either
        // transport can become ready again after a link migration.
        if !transport_ready() {
            std::thread::sleep(Duration::from_millis(10));
            continue;
        }
        let Some(samples) = record
            .outgoing
            .pop_exact_timeout(frame_samples, FRAME_WAIT_TIMEOUT)
        else {
            continue;
        };
        if meter_outgoing {
            for sample in &samples {
                sumsq += (*sample as f64) * (*sample as f64);
            }
            level_samples += samples.len();
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
                if let Err(error) = send_datagram(&datagram) {
                    if is_timeout(&error) {
                        // The datagram was already sealed, so its AEAD
                        // counter was consumed even though the transport
                        // dropped it. Keep the wire timeline monotonic too;
                        // the next frame must not reuse its sequence number.
                        sequence = sequence.wrapping_add(1);
                        timestamp_ms = timestamp_ms.wrapping_add(record.format.frame_ms as u32);
                        continue;
                    }
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
        if let Some(resolution) = inner.take_trusted_enrollment(record.id) {
            if resolution.accepted {
                // The embedding has already committed the secret to durable
                // storage before it can mark this transaction accepted. Only
                // now import it into the live map and acknowledge the client.
                inner.remember_trusted_peer(resolution.peer_id, resolution.secret);
                if let Ok(mut enrolled) = record.trust_secret.lock() {
                    *enrolled = Some(resolution.secret);
                }
                if cipher
                    .send(&mut stream, &ControlMessage::TrustAccepted {})
                    .is_err()
                {
                    return ControlExit::Dropped("control channel closed".into());
                }
            } else if cipher
                .send(
                    &mut stream,
                    &ControlMessage::TrustRejected {
                        reason: resolution
                            .reason
                            .unwrap_or_else(|| "trusted enrollment rejected".into()),
                    },
                )
                .is_err()
            {
                return ControlExit::Dropped("control channel closed".into());
            }
        }
        match cipher.receive(&mut stream) {
            Ok(ControlMessage::Bye { reason }) => return ControlExit::PeerBye(reason),
            Ok(ControlMessage::TrustEnroll { peer_id, secret }) => {
                let rejected = if !inner.config().trust_new_peers {
                    Some("this host requires explicit PIN pairing".to_string())
                } else if peer_id != record.peer.id {
                    Some("trusted peer identity did not match the session".to_string())
                } else {
                    match hex_decode(&secret)
                        .ok()
                        .and_then(|bytes| <[u8; 32]>::try_from(bytes.as_slice()).ok())
                    {
                        Some(secret) => {
                            match inner.begin_trusted_enrollment(
                                record.id,
                                peer_id,
                                record.peer.clone(),
                                secret,
                            ) {
                                Ok(transaction_id) => {
                                    inner.emit(RelayEvent::TrustedPeerEnrollmentRequested {
                                        transaction_id,
                                        peer_id: record.peer.id.clone(),
                                        peer: record.peer.clone(),
                                    });
                                    None
                                }
                                Err(reason) => Some(reason),
                            }
                        }
                        None => Some("trusted credential was malformed".to_string()),
                    }
                };
                if let Some(reason) = rejected {
                    let _ = cipher.send(&mut stream, &ControlMessage::TrustRejected { reason });
                }
                last_seen = Instant::now();
            }
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
    use crate::PAIRING_ATTEMPT_LIMIT;
    use std::net::{IpAddr, TcpListener, TcpStream};

    fn reject_worker_spawn(
        _name: String,
        _worker: Worker,
    ) -> std::io::Result<std::thread::JoinHandle<()>> {
        Err(std::io::Error::other("worker spawn rejected by test"))
    }

    #[test]
    fn outgoing_worker_spawn_failure_emits_session_lost() {
        let inner = EngineInner::new(crate::EngineConfig::default());
        let id = SessionId(7_001);
        connect_peer_with_spawner(
            &inner,
            id,
            "127.0.0.1:48123".parse().unwrap(),
            "123456".into(),
            Roles::emit_only(),
            reject_worker_spawn,
        );

        assert!(matches!(
            inner.drain_events().as_slice(),
            [RelayEvent::SessionLost { id: lost, reason }] if *lost == id
                && reason.contains("could not start relay connection worker")
        ));
    }

    #[test]
    fn abandoned_valid_usb_probes_do_not_consume_pairing_attempts() {
        let inner = EngineInner::new(crate::EngineConfig {
            pin: "123456".into(),
            ..crate::EngineConfig::default()
        });
        let peer_ip = IpAddr::V4(std::net::Ipv4Addr::LOCALHOST);

        for _ in 0..(PAIRING_ATTEMPT_LIMIT + 2) {
            let listener =
                TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).expect("probe test listener");
            let target = listener.local_addr().expect("probe test address");
            let server_inner = Arc::clone(&inner);
            let client_pake = pake_start(Side::Client, "123456");
            let client_message = hex_encode(&client_pake.message);
            let server = std::thread::spawn(move || {
                let (mut stream, peer) = listener.accept().expect("probe connects");
                host_pake_exchange(
                    &server_inner,
                    &mut stream,
                    peer,
                    &client_message,
                    "probe-host".into(),
                    "probe-host-id".into(),
                )
            });

            let mut client = TcpStream::connect(target).expect("probe TCP connection");
            assert!(matches!(
                read_frame(&mut client),
                Ok(ControlMessage::Challenge { .. })
            ));
            // A discovery probe intentionally stops here. It sent a valid
            // SPAKE2 message and must not be treated as an incorrect PIN.
            drop(client);
            assert!(server.join().expect("probe worker exits").is_none());
        }

        assert!(inner.pairing_allowed(peer_ip));
        assert!(inner.pairing_failures.lock().unwrap().is_empty());
    }

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

    fn resumable_session(id: u64) -> Arc<SessionRecord> {
        resumable_session_with_udp(id, None)
    }

    fn resumable_session_with_udp(
        id: u64,
        udp_audio: Option<Arc<UdpAudioSlot>>,
    ) -> Arc<SessionRecord> {
        let client = pake_start(Side::Client, "123456");
        let host = pake_start(Side::Host, "123456");
        let client_message = client.message.clone();
        let host_message = host.message.clone();
        let client_keys = client.finish(&host_message).expect("client pairs");
        let host_keys = host.finish(&client_message).expect("host pairs");
        let (audio_sealer, _) = client_keys.audio_channel().expect("audio sealer");
        let (_, audio_opener) = host_keys.audio_channel().expect("audio opener");
        let format = AudioFormat::new(48_000, 1, 10);
        let capture_converter =
            Converter::with_capacity(48_000, 1, 48_000, 1, crate::MAX_REALTIME_QUANTUM_SAMPLES);
        let capture_destination = Vec::with_capacity(
            capture_converter.output_capacity_for(crate::MAX_REALTIME_QUANTUM_SAMPLES),
        );
        Arc::new(SessionRecord {
            id: SessionId(id),
            wire_id: id,
            peer: PeerInfo {
                id: "resume-peer-id".into(),
                name: "resume-peer".into(),
                kind: DeviceKind::Other,
                addr: "127.0.0.1:1".parse().expect("peer address"),
            },
            roles: Roles::both(),
            codec: CodecKind::Pcm,
            format,
            sending: true,
            receiving: true,
            stop: Arc::new(AtomicBool::new(false)),
            bye_requested: AtomicBool::new(false),
            control_generation: AtomicU64::new(1),
            resume_secret: client_keys.resume_auth_key(),
            trust_secret: Mutex::new(None),
            tcp_audio: None,
            udp_audio,
            control_peer_addr: Mutex::new("127.0.0.1:1".parse().unwrap()),
            control_state: Mutex::new(ControlState::Active),
            peer_audio_addr: Mutex::new(None),
            outgoing: crate::PcmQueue::new(crate::DEFAULT_QUEUE_CAPACITY),
            incoming: crate::PcmQueue::new(crate::DEFAULT_QUEUE_CAPACITY),
            capture_convert: Mutex::new((capture_converter, capture_destination)),
            audio_sealer: Mutex::new(audio_sealer),
            audio_opener: Mutex::new(audio_opener),
        })
    }

    #[test]
    fn resuming_on_the_same_link_keeps_the_existing_audio_socket() {
        // ADB pins the host bind address to loopback, so the interface
        // selected for the resume is exactly the one the socket already
        // holds. Rebinding it would ask the kernel for an address this very
        // session still owns and fail with EADDRINUSE.
        let config = crate::EngineConfig {
            transport: crate::TransportPreference::Adb,
            ..crate::EngineConfig::default()
        };
        let inner = EngineInner::new(config);
        let target: SocketAddr = "127.0.0.1:48123".parse().expect("target");
        let socket = bind_udp_audio_socket(&inner, target, true).expect("audio socket");
        let slot = UdpAudioSlot::new(socket).expect("audio slot");
        let before = slot.local_addr().expect("bound address");
        let record = resumable_session_with_udp(7_050, Some(Arc::clone(&slot)));

        migrate_udp_audio_socket(&inner, &record, target, true).expect("resume keeps the socket");

        assert_eq!(slot.local_addr(), Some(before));
    }

    /// The real binder used by `migrate_udp_audio_socket`, reproduced so the
    /// lifecycle tests exercise the same kernel behaviour without depending
    /// on netlink interface discovery.
    fn real_udp_binder(addr: SocketAddr) -> std::io::Result<UdpSocket> {
        let socket = UdpSocket::bind(addr)?;
        tune_audio_socket(&socket);
        Ok(socket)
    }

    #[test]
    fn migrating_a_wildcard_socket_to_a_specific_address_keeps_the_port() {
        // Regression: `take_current` only unlinked the `Arc` from the slot.
        // Worker leases kept the wildcard socket open, so binding
        // `127.0.0.1:PORT` while `0.0.0.0:PORT` was still alive failed with
        // EADDRINUSE and the host lost its negotiated audio port.
        let slot = UdpAudioSlot::new(
            UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).expect("wildcard socket"),
        )
        .expect("audio slot");
        let port = slot.local_addr().expect("bound address").port();

        migrate_udp_slot(&slot, Ipv4Addr::LOCALHOST, Some(port), &real_udp_binder)
            .expect("wildcard migrates onto a specific address");

        let after = slot.local_addr().expect("migrated address");
        assert_eq!(after.ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_eq!(after.port(), port);
    }

    #[test]
    fn migrating_a_wildcard_socket_waits_for_outstanding_worker_leases() {
        // A worker holding a lease across its bounded `recv_from` must not
        // make the migration bind against a still-open wildcard socket.
        let slot = Arc::new(
            UdpAudioSlot::new(
                UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).expect("wildcard socket"),
            )
            .expect("audio slot"),
        );
        let port = slot.local_addr().expect("bound address").port();

        let lease = slot.current().expect("worker leases the socket");
        let holder = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(120));
            drop(lease);
        });

        migrate_udp_slot(&slot, Ipv4Addr::LOCALHOST, Some(port), &real_udp_binder)
            .expect("migration drains the lease and rebinds");
        holder.join().expect("lease holder finishes");

        let after = slot.local_addr().expect("migrated address");
        assert_eq!(after.ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_eq!(after.port(), port);
    }

    #[test]
    fn migrating_back_to_a_wildcard_socket_keeps_the_port() {
        // The reverse direction collides just as hard: `0.0.0.0:PORT` cannot
        // be bound while `127.0.0.1:PORT` is still open.
        let slot =
            UdpAudioSlot::new(UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("specific socket"))
                .expect("audio slot");
        let port = slot.local_addr().expect("bound address").port();

        migrate_udp_slot(&slot, Ipv4Addr::UNSPECIFIED, Some(port), &real_udp_binder)
            .expect("specific address migrates back onto the wildcard");

        let after = slot.local_addr().expect("migrated address");
        assert!(after.ip().is_unspecified());
        assert_eq!(after.port(), port);
    }

    #[test]
    fn a_failed_migration_restores_a_wildcard_socket_on_the_original_port() {
        // Rollback: once the old wildcard socket has been closed there is
        // nothing to put back, so the slot must be refilled rather than left
        // empty, and the caller must still learn that the move failed.
        let slot = UdpAudioSlot::new(
            UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).expect("wildcard socket"),
        )
        .expect("audio slot");
        let port = slot.local_addr().expect("bound address").port();

        // Injected failure for the desired address only; the wildcard
        // fallback still succeeds, exactly as a vanished interface behaves.
        let binder = |addr: SocketAddr| -> std::io::Result<UdpSocket> {
            if !addr.ip().is_unspecified() {
                return Err(std::io::Error::from(std::io::ErrorKind::AddrNotAvailable));
            }
            real_udp_binder(addr)
        };

        let error = migrate_udp_slot(&slot, Ipv4Addr::LOCALHOST, Some(port), &binder)
            .expect_err("the desired address cannot be bound");
        assert_eq!(error.kind(), std::io::ErrorKind::AddrNotAvailable);

        let after = slot.local_addr().expect("the slot is never left empty");
        assert!(after.ip().is_unspecified());
        assert_eq!(after.port(), port);
    }

    #[test]
    fn a_migration_that_cannot_drain_restores_the_live_socket() {
        // If worker leases outlive the migration window the old socket is
        // still usable, so it must go back into the slot instead of being
        // closed on a guess.
        let slot = UdpAudioSlot::new(
            UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).expect("wildcard socket"),
        )
        .expect("audio slot");
        let before = slot.local_addr().expect("bound address");
        let lease = slot.current().expect("worker leases the socket");

        let taken = slot
            .take_exclusive(Duration::from_millis(20))
            .expect("the slot was populated");
        let still_leased = taken.expect_err("an outstanding lease blocks the take");
        slot.restore(still_leased);
        drop(lease);

        assert_eq!(slot.local_addr(), Some(before));
    }

    #[test]
    fn migrating_between_two_specific_addresses_does_not_close_the_old_socket() {
        // Two different specific addresses do not contend for the port, so
        // the old socket may stay open while the new one is installed.
        let old: SocketAddr = "127.0.0.1:0".parse().expect("address");
        assert!(!udp_binds_collide(old, Ipv4Addr::new(127, 0, 0, 2), 40_000));
        let bound: SocketAddr = "127.0.0.1:40000".parse().expect("address");
        assert!(!udp_binds_collide(
            bound,
            Ipv4Addr::new(127, 0, 0, 2),
            40_000
        ));
        assert!(udp_binds_collide(
            "0.0.0.0:40000".parse().expect("address"),
            Ipv4Addr::new(127, 0, 0, 2),
            40_000
        ));
        assert!(udp_binds_collide(bound, Ipv4Addr::UNSPECIFIED, 40_000));
        // A client migration takes a fresh ephemeral port and never collides.
        assert!(!udp_binds_collide(bound, Ipv4Addr::UNSPECIFIED, 0));
    }

    #[test]
    fn original_session_owner_can_resume_over_the_challenge_flow() {
        let inner = EngineInner::new(crate::EngineConfig::default());
        let record = resumable_session(7_004);
        assert!(inner.insert_session(Arc::clone(&record)));
        assert!(record.mark_control_dropped());

        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("resume listener");
        let target = listener.local_addr().expect("resume address");
        let server_inner = Arc::clone(&inner);
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("resume client connects");
            let first = read_frame(&mut stream).expect("resume hello");
            let ControlMessage::ResumeHello {
                session_id,
                client_nonce,
            } = first
            else {
                panic!("resume client sent the wrong first message");
            };
            resume_peer_session(&server_inner, SessionId(session_id), stream, &client_nonce);
        });

        let (stream, cipher, _) = resume_client_control(&inner, &record, target)
            .expect("the original owner proves the session secret");
        assert_eq!(cipher.sealer.next_counter(), 0);
        assert_eq!(
            *record.control_state.lock().expect("control state"),
            ControlState::Active
        );
        drop(stream);
        teardown(&inner, record.id, "resume test complete".into());
        server.join().expect("resume worker exits");
        assert_eq!(record.control_generation.load(Ordering::Acquire), 2);
        assert!(!inner.session_alive(record.id));
    }

    #[test]
    fn unauthenticated_resume_pairfail_tries_the_next_peer_address() {
        let inner = EngineInner::new(crate::EngineConfig::default());
        let record = resumable_session(7_008);
        assert!(inner.insert_session(Arc::clone(&record)));
        assert!(record.mark_control_dropped());

        // Bind two loopback addresses to the same port. The original target
        // is the malicious candidate; the discovered address is the real
        // host and is intentionally ranked later for this test.
        let attacker = TcpListener::bind(("127.0.0.2", 0)).unwrap();
        let port = attacker.local_addr().unwrap().port();
        let legitimate = TcpListener::bind(("127.0.0.1", port)).unwrap();
        let attacker_target = attacker.local_addr().unwrap();
        let legitimate_target = legitimate.local_addr().unwrap();

        let attacker_thread = std::thread::spawn(move || {
            let (mut stream, _) = attacker.accept().unwrap();
            let _ = read_frame(&mut stream).unwrap();
            write_frame(
                &mut stream,
                &ControlMessage::PairFail {
                    reason: "unauthenticated candidate rejection".into(),
                },
            )
            .unwrap();
        });

        let server_inner = Arc::clone(&inner);
        let legitimate_thread = std::thread::spawn(move || {
            let (mut stream, _) = legitimate.accept().unwrap();
            let ControlMessage::ResumeHello {
                session_id,
                client_nonce,
            } = read_frame(&mut stream).unwrap()
            else {
                panic!("legitimate candidate received the wrong message");
            };
            resume_peer_session(&server_inner, SessionId(session_id), stream, &client_nonce);
        });

        let handle = crate::RelayHandle {
            inner: Arc::clone(&inner),
        };
        handle.update_discovered_peer_candidates(vec![(
            PeerInfo {
                id: record.peer.id.clone(),
                name: record.peer.name.clone(),
                kind: DeviceKind::Other,
                addr: legitimate_target,
            },
            Some(crate::LinkKind::Lan),
        )]);

        let (stream, _cipher, resumed_target) =
            resume_client_control(&inner, &record, attacker_target).expect("fallback resumes");
        assert_eq!(resumed_target, legitimate_target);
        assert_eq!(
            inner.last_successful_address(&record.peer.id),
            Some(legitimate_target)
        );
        assert!(!inner.candidate_allowed(&record.peer.id, attacker_target));

        drop(stream);
        teardown(&inner, record.id, "resume candidate test complete".into());
        attacker_thread.join().unwrap();
        legitimate_thread.join().unwrap();
    }

    #[test]
    fn clear_trusted_ok_without_sealed_setup_does_not_learn_candidate() {
        let inner = EngineInner::new(crate::EngineConfig {
            device_id: "client-id".into(),
            ..crate::EngineConfig::default()
        });
        let target_listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let target = target_listener.local_addr().unwrap();
        let host_id = "trusted-host".to_string();
        let secret = [0x27; 32];
        let server = std::thread::spawn({
            let host_id = host_id.clone();
            move || {
                let (mut stream, _) = target_listener.accept().unwrap();
                let (client_nonce, roles) = match read_frame(&mut stream).unwrap() {
                    ControlMessage::TrustedHello {
                        client_nonce,
                        roles,
                        ..
                    } => (decode_resume_nonce(&client_nonce).unwrap(), roles),
                    message => panic!("unexpected trusted hello: {message:?}"),
                };
                let server_nonce = [0x28; RESUME_NONCE_LEN];
                write_frame(
                    &mut stream,
                    &ControlMessage::TrustedChallenge {
                        server_nonce: hex_encode(&server_nonce),
                        session_id: 9_009,
                        host_id: host_id.clone(),
                        host_name: "fake host".into(),
                    },
                )
                .unwrap();
                assert!(matches!(
                    read_frame(&mut stream).unwrap(),
                    ControlMessage::TrustedProof { .. }
                ));
                write_frame(&mut stream, &ControlMessage::TrustedOk {}).unwrap();
                // The fake candidate cannot produce the sealed PairOk. The
                // client must reject it without recording this address.
                let _ = roles;
                drop(stream);
                let _ = client_nonce;
            }
        });

        trusted_client_thread(
            Arc::clone(&inner),
            SessionId(7_009),
            target,
            host_id.clone(),
            secret,
            Roles::emit_only(),
        );
        server.join().unwrap();
        assert_eq!(inner.last_successful_address(&host_id), None);
        assert!(matches!(
            inner.drain_events().as_slice(),
            [RelayEvent::SessionLost { id, .. }] if *id == SessionId(7_009)
        ));
    }

    #[test]
    fn failed_resume_ok_has_no_ownerless_zombie_session() {
        let inner = EngineInner::new(crate::EngineConfig::default());
        let record = resumable_session(7_005);
        assert!(inner.insert_session(Arc::clone(&record)));

        // Model the host having committed generation 2, then losing the
        // socket before ResumeOk reached the client. The recovery helper is
        // the same one used by the real host resume path; zero deadlines keep
        // this deterministic and avoid a 15-second test.
        assert!(record.mark_control_dropped());
        let generation = record.begin_resume().expect("resume generation 2");
        assert_eq!(generation, 2);
        assert!(record.finish_resume(generation));
        assert!(matches!(
            *record.control_state.lock().expect("control state"),
            ControlState::Active
        ));

        assert!(!handle_failed_resume_ok_with_deadlines(
            &inner,
            &record,
            Duration::ZERO,
            Duration::ZERO,
        ));
        assert!(!inner.session_alive(record.id));
        assert!(matches!(
            inner.drain_events().as_slice(),
            [RelayEvent::SessionLost { id, reason }]
                if *id == record.id && reason.contains("could not deliver ResumeOk")
        ));
    }

    #[test]
    fn failed_resume_ok_allows_one_bounded_replacement_resume() {
        let inner = EngineInner::new(crate::EngineConfig::default());
        let record = resumable_session(7_006);
        assert!(inner.insert_session(Arc::clone(&record)));
        assert!(record.mark_control_dropped());
        let generation = record.begin_resume().expect("resume generation 2");
        assert!(record.finish_resume(generation));

        let waiter_inner = Arc::clone(&inner);
        let waiter_record = Arc::clone(&record);
        let waiter = std::thread::spawn(move || {
            handle_failed_resume_ok_with_deadlines(
                &waiter_inner,
                &waiter_record,
                Duration::from_millis(250),
                Duration::from_millis(250),
            )
        });

        for _ in 0..1_000 {
            if matches!(
                *record.control_state.lock().expect("control state"),
                ControlState::ResumeEligible { generation: 2 }
            ) {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        let replacement = record.begin_resume().expect("one replacement resume");
        assert_eq!(replacement, 3);
        assert!(record.finish_resume(replacement));

        assert!(waiter.join().expect("resume grace watcher exits"));
        assert!(inner.session_alive(record.id));
        assert_eq!(
            *record.control_state.lock().expect("control state"),
            ControlState::Active
        );
        teardown(&inner, record.id, "resume replacement test complete".into());
    }

    #[test]
    fn resume_targets_follow_the_same_stable_peer_across_addresses() {
        let inner = EngineInner::new(crate::EngineConfig::default());
        let record = resumable_session(7_007);
        assert!(inner.insert_session(Arc::clone(&record)));
        let original: SocketAddr = "192.168.1.20:48123".parse().unwrap();
        let handle = crate::RelayHandle {
            inner: Arc::clone(&inner),
        };
        handle.update_discovered_peer_candidates(vec![
            (
                PeerInfo {
                    id: "resume-peer-id".into(),
                    name: "resume-peer".into(),
                    kind: DeviceKind::Other,
                    addr: "192.168.42.129:48123".parse().unwrap(),
                },
                Some(crate::LinkKind::Usb),
            ),
            (
                PeerInfo {
                    id: "unrelated-peer".into(),
                    name: "resume-peer".into(),
                    kind: DeviceKind::Other,
                    addr: "10.0.0.5:48123".parse().unwrap(),
                },
                None,
            ),
        ]);

        let targets = resume_targets(&inner, &record, original);
        assert_eq!(
            targets,
            vec!["192.168.42.129:48123".parse().unwrap(), original,]
        );
        assert!(targets.len() <= crate::MAX_TRUSTED_CANDIDATE_ADDRESSES);
        teardown(&inner, record.id, "resume target test complete".into());
    }

    #[test]
    fn replacing_tcp_audio_closes_the_stale_forwarded_stream() {
        use std::io::Read as _;

        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let target = listener.local_addr().unwrap();
        let first_client = TcpStream::connect(target).unwrap();
        let (mut first_server, _) = listener.accept().unwrap();
        first_server
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();

        let slot = TcpAudioSlot::new();
        slot.install(first_client).unwrap();
        let first = slot.current().expect("first forwarded stream is installed");

        let second_client = TcpStream::connect(target).unwrap();
        let (_second_server, _) = listener.accept().unwrap();
        slot.install(second_client).unwrap();
        let second = slot.current().expect("replacement stream is installed");
        assert!(!Arc::ptr_eq(&first, &second));

        let mut byte = [0u8; 1];
        assert_eq!(
            first_server.read(&mut byte).unwrap(),
            0,
            "installing a resumed ADB stream must wake workers on the old one"
        );
    }

    #[test]
    fn adb_audio_secondary_stream_runs_the_production_authenticated_handshake() {
        let inner = EngineInner::new(crate::EngineConfig {
            transport: crate::TransportPreference::Adb,
            ..crate::EngineConfig::default()
        });
        let record = resumable_session(7_004);
        let secret = record.resume_secret;
        let wire_id = record.wire_id;
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let target = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let client_nonce = match read_frame(&mut stream).unwrap() {
                ControlMessage::AudioHello {
                    session_id,
                    client_nonce,
                } => {
                    assert_eq!(session_id, wire_id);
                    decode_resume_nonce(&client_nonce).unwrap()
                }
                message => panic!("unexpected ADB hello: {message:?}"),
            };
            let server_nonce = [0x42; RESUME_NONCE_LEN];
            write_frame(
                &mut stream,
                &ControlMessage::AudioChallenge {
                    server_nonce: hex_encode(&server_nonce),
                },
            )
            .unwrap();
            let proof = match read_frame(&mut stream).unwrap() {
                ControlMessage::AudioProof { proof } => hex_decode(&proof).unwrap(),
                message => panic!("unexpected ADB proof: {message:?}"),
            };
            let expected = crate::crypto::tcp_audio_proof(
                &secret,
                wire_id,
                &client_nonce,
                &server_nonce,
                Side::Client,
            );
            assert!(bool::from(expected.ct_eq(&proof)));
            let host_proof = crate::crypto::tcp_audio_proof(
                &secret,
                wire_id,
                &client_nonce,
                &server_nonce,
                Side::Host,
            );
            write_frame(
                &mut stream,
                &ControlMessage::AudioReady {
                    proof: hex_encode(&host_proof),
                },
            )
            .unwrap();
        });

        let slot = TcpAudioSlot::new();
        open_tcp_audio_once(&inner, &record, &slot, target).unwrap();
        assert!(slot.is_active());
        server.join().unwrap();
    }

    #[test]
    fn adb_audio_wrong_host_proof_cannot_replace_the_active_slot() {
        let inner = EngineInner::new(crate::EngineConfig {
            transport: crate::TransportPreference::Adb,
            ..crate::EngineConfig::default()
        });
        let record = resumable_session(7_005);
        let wire_id = record.wire_id;
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let target = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let client_nonce = match read_frame(&mut stream).unwrap() {
                ControlMessage::AudioHello { client_nonce, .. } => {
                    decode_resume_nonce(&client_nonce).unwrap()
                }
                message => panic!("unexpected ADB hello: {message:?}"),
            };
            let server_nonce = [0x43; RESUME_NONCE_LEN];
            write_frame(
                &mut stream,
                &ControlMessage::AudioChallenge {
                    server_nonce: hex_encode(&server_nonce),
                },
            )
            .unwrap();
            let _ = read_frame(&mut stream).unwrap();
            let wrong = [0u8; crate::crypto::CONFIRM_LEN];
            write_frame(
                &mut stream,
                &ControlMessage::AudioReady {
                    proof: hex_encode(&wrong),
                },
            )
            .unwrap();
            let _ = (client_nonce, wire_id);
        });
        let slot = TcpAudioSlot::new();
        assert!(open_tcp_audio_once(&inner, &record, &slot, target).is_err());
        assert!(!slot.is_active());
        server.join().unwrap();
    }

    #[test]
    fn adb_tx_transport_loss_clears_the_slot_as_a_recoverable_write() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let target = listener.local_addr().unwrap();
        let client = TcpStream::connect(target).unwrap();
        let (server, _) = listener.accept().unwrap();
        let slot = TcpAudioSlot::new();
        slot.install(client).unwrap();

        // Force the installed writer into the same terminal state observed
        // after BrokenPipe/ConnectionReset, without relying on peer timing.
        slot.current()
            .unwrap()
            .writer
            .lock()
            .unwrap()
            .shutdown(Shutdown::Both)
            .unwrap();
        drop(server);

        let error = send_tcp_audio_datagram(&slot, &[0x01]).expect_err("write must fail");
        assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
        assert!(!slot.is_active(), "supervisor must see an empty slot");
    }

    #[test]
    fn adb_tcp_audio_transport_errors_are_distinct_from_fatal_errors() {
        for kind in [
            std::io::ErrorKind::BrokenPipe,
            std::io::ErrorKind::ConnectionReset,
            std::io::ErrorKind::ConnectionAborted,
            std::io::ErrorKind::NotConnected,
            std::io::ErrorKind::UnexpectedEof,
            std::io::ErrorKind::TimedOut,
            std::io::ErrorKind::WouldBlock,
        ] {
            assert!(is_recoverable_tcp_audio_error(&std::io::Error::from(kind)));
        }
        assert!(!is_recoverable_tcp_audio_error(&std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "locally invalid frame",
        )));
    }

    #[test]
    fn adb_tx_reconnect_signal_does_not_emit_session_loss() {
        let inner = EngineInner::new(crate::EngineConfig {
            transport: crate::TransportPreference::Adb,
            ..crate::EngineConfig::default()
        });
        let record = resumable_session(7_010);
        let frame_samples = record.format.frame_samples();
        record.outgoing.push(&vec![0.0; frame_samples]);
        assert!(inner.insert_session(Arc::clone(&record)));

        let attempted = Arc::new(AtomicBool::new(false));
        let attempted_by_worker = Arc::clone(&attempted);
        let worker_inner = Arc::clone(&inner);
        let worker_record = Arc::clone(&record);
        let worker = std::thread::spawn(move || {
            run_tx_source(
                worker_inner,
                worker_record,
                None,
                || true,
                move |_| {
                    attempted_by_worker.store(true, Ordering::Release);
                    Err(std::io::Error::new(
                        std::io::ErrorKind::WouldBlock,
                        "ADB audio connection is reconnecting",
                    ))
                },
            );
        });

        for _ in 0..100 {
            if attempted.load(Ordering::Acquire) {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(attempted.load(Ordering::Acquire));
        assert!(inner.session_alive(record.id));
        assert!(inner.drain_events().is_empty());

        teardown(&inner, record.id, "ADB TX reconnect test complete".into());
        worker.join().unwrap();
    }

    #[test]
    fn tcp_audio_framing_rejects_empty_and_oversized_payloads() {
        let mut output = Vec::new();
        assert!(write_tcp_audio_frame(&mut output, &[]).is_err());
        assert!(write_tcp_audio_frame(&mut output, &vec![0; MAX_DATAGRAM + 1]).is_err());
        let mut oversized = (u32::try_from(MAX_DATAGRAM + 1).unwrap())
            .to_be_bytes()
            .to_vec();
        oversized.extend_from_slice(&[0; 4]);
        let mut destination = vec![0; MAX_DATAGRAM];
        assert!(read_tcp_audio_frame(&mut &oversized[..], &mut destination).is_err());
    }

    #[test]
    fn session_worker_spawn_failures_are_reported_and_stop_the_record() {
        let stop = AtomicBool::new(false);
        let mut reject = reject_worker_spawn;
        let error =
            spawn_worker_with_report(&mut reject, &stop, SessionId(7_002), "RX", Box::new(|| {}))
                .expect_err("RX spawn is refused");
        assert!(error.contains("RX worker"));
        assert!(stop.load(Ordering::Relaxed));

        let stop = AtomicBool::new(false);
        let mut calls = 0;
        let mut spawn = |name, worker| {
            calls += 1;
            if calls == 2 {
                reject_worker_spawn(name, worker)
            } else {
                Ok(std::thread::Builder::new()
                    .name(name)
                    .spawn(worker)
                    .unwrap())
            }
        };
        spawn_worker_with_report(&mut spawn, &stop, SessionId(7_003), "RX", Box::new(|| {}))
            .expect("RX spawn succeeds");
        let error =
            spawn_worker_with_report(&mut spawn, &stop, SessionId(7_003), "TX", Box::new(|| {}))
                .expect_err("TX spawn is refused after RX starts");
        assert!(error.contains("TX worker"));
        assert!(stop.load(Ordering::Relaxed));
    }

    #[test]
    fn handshake_slot_is_released_when_worker_ownership_ends() {
        let inner = EngineInner::new(crate::EngineConfig {
            max_pending_handshakes: 1,
            ..crate::EngineConfig::default()
        });
        let slot = inner.claim_handshake().expect("first slot is available");
        assert!(inner.claim_handshake().is_none());
        drop(slot);
        assert!(inner.claim_handshake().is_some());
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
        assert!(
            !payload.is_empty(),
            "this is an audio frame, not an announce"
        );
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
