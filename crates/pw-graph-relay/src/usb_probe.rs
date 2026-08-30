//! Direct host probing over USB tethering links.
//!
//! mDNS discovery frequently does not cross a USB tether: Android's
//! `NsdService` advertises on the phone's uplink network rather than on the
//! tethered interface, so a tethered phone never shows up in a browse. To
//! make USB connections work without manual address entry, the engine probes
//! the small USB subnets directly while discovery is active.
//!
//! A probe opens a TCP connection, sends a minimal [`ControlMessage::Hello`],
//! and waits for the host's [`ControlMessage::Challenge`] reply. Receiving a
//! valid challenge proves the port speaks the relay protocol and even yields
//! the host's device name; the probe then hangs up without pairing, which the
//! host treats as an abandoned handshake.

use crate::netlink::{local_links, LinkKind, LocalLink};
use crate::protocol::{
    read_frame, write_frame, ControlMessage, DeviceKind, Roles, PROTOCOL_VERSION,
};
use crate::{EngineInner, PeerInfo, RelayError, RelayResult};
use std::collections::HashSet;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The fixed relay control port recommended for manual USB setups, probed by
/// default because ephemeral host ports cannot be discovered by scanning.
pub const DEFAULT_PROBE_PORT: u16 = 48123;

/// Do not enumerate subnets larger than this (USB tether links are /24).
const MAX_CANDIDATES: u32 = 1024;

/// The scanner never creates more than this many probe workers at once.
const PROBE_WORKERS: usize = 16;

/// Bound the total work even when a caller supplies several ports.
const MAX_SCAN_TARGETS: usize = MAX_CANDIDATES as usize * 4;

/// Addresses commonly used as the phone/desktop side of a tether subnet.
const PRIORITY_OFFSETS: [u32; 3] = [1, 2, 254];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct ProbeTarget {
    target: SocketAddr,
    bind_addr: Ipv4Addr,
}

/// Host addresses worth checking on a USB link: every host address in the
/// subnet except the local ones, capped at [`MAX_CANDIDATES`].
pub fn candidate_hosts(link: &LocalLink, local_addrs: &[Ipv4Addr]) -> Vec<Ipv4Addr> {
    let network = u32::from(link.addr) & u32::from(link.netmask);
    let size = !u32::from(link.netmask);
    if size == u32::MAX || size + 1 > MAX_CANDIDATES {
        return Vec::new();
    }
    (1..size)
        .map(|offset| Ipv4Addr::from(network + offset))
        .filter(|addr| !local_addrs.contains(addr))
        .collect()
}

/// Probe one address:port for a relay host. Returns the peer when the port
/// answers the control handshake with a challenge.
pub fn probe_target(
    target: SocketAddr,
    bind_addr: Option<Ipv4Addr>,
    timeout: Duration,
) -> Option<PeerInfo> {
    probe_target_with_cancel(target, bind_addr, timeout, None)
}

fn probe_target_with_cancel(
    target: SocketAddr,
    bind_addr: Option<Ipv4Addr>,
    timeout: Duration,
    cancel: Option<&AtomicBool>,
) -> Option<PeerInfo> {
    if cancel.is_some_and(|cancel| cancel.load(Ordering::Acquire)) {
        return None;
    }
    let mut stream = match cancel {
        Some(cancel) => crate::netlink::connect_tcp_cancellable(target, bind_addr, timeout, cancel),
        None => crate::netlink::connect_tcp(target, bind_addr, timeout),
    }
    .ok()?;
    stream.set_write_timeout(Some(timeout)).ok()?;
    stream.set_read_timeout(Some(timeout)).ok()?;
    // A well-formed SPAKE2 message even though the probe never pairs: a
    // malformed one would count against the host's pairing-attempt budget and
    // let discovery lock this machine out of its own relay.
    let pake = crate::crypto::pake_start(crate::crypto::Side::Client, "probe");
    let hello = ControlMessage::Hello {
        protocol: PROTOCOL_VERSION as u32,
        device_id: "probe".into(),
        transport: String::new(),
        device_name: "qpw-relay-probe".into(),
        device_kind: DeviceKind::Other,
        roles: Roles::emit_only(),
        sample_rate: 48_000,
        channels: 1,
        pake: pw_graph_utils::hex::hex_encode(&pake.message),
    };
    write_frame(&mut stream, &hello).ok()?;
    match read_frame(&mut stream) {
        Ok(ControlMessage::Challenge {
            protocol,
            host_name,
            device_id,
            ..
        }) if protocol == PROTOCOL_VERSION as u32 => Some(PeerInfo {
            id: if device_id.trim().is_empty() {
                format!("USB {}", target.ip())
            } else {
                device_id
            },
            name: if host_name.trim().is_empty() {
                format!("USB {}", target.ip())
            } else {
                host_name
            },
            kind: DeviceKind::Other,
            addr: target,
        }),
        _ => None,
    }
}

fn add_probe_targets(
    output: &mut Vec<ProbeTarget>,
    seen: &mut HashSet<ProbeTarget>,
    addr: Ipv4Addr,
    bind_addr: Ipv4Addr,
    ports: &[u16],
) {
    for port in ports.iter().copied().filter(|port| *port != 0) {
        if output.len() >= MAX_SCAN_TARGETS {
            return;
        }
        let candidate = ProbeTarget {
            target: SocketAddr::from((addr, port)),
            bind_addr,
        };
        if seen.insert(candidate) {
            output.push(candidate);
        }
    }
}

/// Build a bounded, ordered list of USB probe targets. Previously successful
/// addresses and the usual tether peer addresses are tried before the full
/// subnet. Every target carries the exact local USB address that produced it,
/// preventing the OS from choosing a VPN/Wi-Fi route for an overlapping subnet.
fn probe_targets(
    links: &[LocalLink],
    local_addrs: &[Ipv4Addr],
    ports: &[u16],
    recent: &[SocketAddr],
) -> Vec<ProbeTarget> {
    let mut output = Vec::new();
    let mut seen = HashSet::new();
    for link in links.iter().filter(|link| link.kind == LinkKind::Usb) {
        for target in recent {
            let SocketAddr::V4(target) = target else {
                continue;
            };
            if target.port() != 0 && ports.contains(&target.port()) && link.contains(*target.ip()) {
                add_probe_targets(
                    &mut output,
                    &mut seen,
                    *target.ip(),
                    link.addr,
                    &[target.port()],
                );
            }
        }

        let network = u32::from(link.addr) & u32::from(link.netmask);
        let size = !u32::from(link.netmask);
        if size == u32::MAX || size + 1 > MAX_CANDIDATES {
            continue;
        }
        for offset in PRIORITY_OFFSETS {
            if offset < size {
                let addr = Ipv4Addr::from(network + offset);
                if !local_addrs.contains(&addr) {
                    add_probe_targets(&mut output, &mut seen, addr, link.addr, ports);
                }
            }
        }
        for addr in candidate_hosts(link, local_addrs) {
            add_probe_targets(&mut output, &mut seen, addr, link.addr, ports);
        }
    }
    output
}

fn probe_candidates(
    candidates: &[ProbeTarget],
    timeout: Duration,
    cancel: Option<&AtomicBool>,
) -> Vec<PeerInfo> {
    if candidates.is_empty() || cancel.is_some_and(|cancel| cancel.load(Ordering::Acquire)) {
        return Vec::new();
    }
    let next = AtomicUsize::new(0);
    let found = Mutex::new(Vec::new());
    let workers = PROBE_WORKERS.min(candidates.len());
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                if cancel.is_some_and(|cancel| cancel.load(Ordering::Acquire)) {
                    break;
                }
                let index = next.fetch_add(1, Ordering::Relaxed);
                let Some(candidate) = candidates.get(index).copied() else {
                    break;
                };
                if let Some(peer) = probe_target_with_cancel(
                    candidate.target,
                    Some(candidate.bind_addr),
                    timeout,
                    cancel,
                ) {
                    if let Ok(mut found) = found.lock() {
                        found.push(peer);
                    }
                }
            });
        }
    });
    let mut found = found.into_inner().unwrap_or_default();
    found.sort_by_key(|peer| peer.addr);
    found.dedup_by(|a, b| a.addr == b.addr);
    found
}

/// Scan every USB tether link for relay hosts listening on `ports`.
///
/// Candidates are probed concurrently by a fixed-size worker pool; the whole
/// scan is bounded by `timeout` per candidate so a /24 subnet stays fast
/// enough to repeat while discovery runs.
pub fn probe_usb_hosts(ports: &[u16], timeout: Duration) -> Vec<PeerInfo> {
    let links = local_links();
    let local_addrs: Vec<Ipv4Addr> = links.iter().map(|link| link.addr).collect();
    let candidates = probe_targets(&links, &local_addrs, ports, &[]);
    probe_candidates(&candidates, timeout, None)
}

/// How often discovery rescans USB subnets while active.
pub(crate) const SCAN_INTERVAL: Duration = Duration::from_secs(4);

/// Per-candidate timeout; bounded so a full /24 scan stays short.
const SCAN_TARGET_TIMEOUT: Duration = Duration::from_millis(250);

/// Background scanner that feeds USB-probed hosts into the same peer map the
/// mDNS browser uses, so they surface through the usual discovery events.
pub(crate) struct UsbScanner {
    stop: Arc<AtomicBool>,
    active: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl UsbScanner {
    pub(crate) fn start(inner: &Arc<EngineInner>) -> RelayResult<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let active = Arc::new(AtomicBool::new(true));
        let thread_stop = Arc::clone(&stop);
        let thread_active = Arc::clone(&active);
        let inner = Arc::clone(inner);
        let worker = std::thread::Builder::new()
            .name("relay-usb-scan".into())
            .spawn(move || scan_loop(&inner, thread_stop, thread_active))
            .map_err(RelayError::Io)?;
        Ok(Self {
            stop,
            active,
            worker: Some(worker),
        })
    }

    pub(crate) fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    pub(crate) fn stop(self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker {
            let _ = worker.join();
        }
    }
}

/// Service identity under which probed USB hosts are tracked. Distinct from
/// any mDNS fullname, so probed and advertised peers never collide.
pub(crate) const USB_PROBE_SERVICE: &str = "usb-probe._qpw-relay._udp.local.";

fn scan_loop(inner: &Arc<EngineInner>, stop: Arc<AtomicBool>, active: Arc<AtomicBool>) {
    let mut recent = Vec::new();
    loop {
        if !inner.running.load(Ordering::Relaxed) || stop.load(Ordering::Relaxed) {
            break;
        }
        let peers = {
            let links = local_links();
            let local_addrs: Vec<Ipv4Addr> = links.iter().map(|link| link.addr).collect();
            let candidates = probe_targets(&links, &local_addrs, &[DEFAULT_PROBE_PORT], &recent);
            probe_candidates(&candidates, SCAN_TARGET_TIMEOUT, Some(&stop))
        };
        if !stop.load(Ordering::Relaxed) && inner.running.load(Ordering::Relaxed) {
            recent = peers.iter().map(|peer| peer.addr).take(16).collect();
            inner.refresh_service(USB_PROBE_SERVICE, peers);
        } else {
            recent.clear();
        }
        // Sleep in small slices so shutdown stays responsive.
        let mut waited = Duration::ZERO;
        while waited < SCAN_INTERVAL {
            if !inner.running.load(Ordering::Relaxed) || stop.load(Ordering::Relaxed) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
            waited += Duration::from_millis(100);
        }
    }
    inner.refresh_service(USB_PROBE_SERVICE, Vec::new());
    active.store(false, Ordering::Release);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usb_link(addr: [u8; 4]) -> LocalLink {
        LocalLink {
            name: "usb0".into(),
            addr: Ipv4Addr::from(addr),
            netmask: Ipv4Addr::new(255, 255, 255, 0),
            kind: LinkKind::Usb,
        }
    }

    #[test]
    fn enumerates_subnet_hosts_without_the_local_address() {
        let link = usb_link([192, 168, 42, 129]);
        let candidates = candidate_hosts(&link, &[link.addr]);
        assert_eq!(candidates.len(), 253);
        assert!(!candidates.contains(&link.addr));
        assert!(candidates.contains(&Ipv4Addr::new(192, 168, 42, 1)));
        assert!(candidates.contains(&Ipv4Addr::new(192, 168, 42, 254)));
    }

    #[test]
    fn skips_huge_subnets() {
        let link = LocalLink {
            name: "usb0".into(),
            addr: Ipv4Addr::new(10, 0, 0, 1),
            netmask: Ipv4Addr::new(255, 0, 0, 0),
            kind: LinkKind::Usb,
        };
        assert!(candidate_hosts(&link, &[]).is_empty());
    }

    #[test]
    fn probe_rejects_non_relay_listeners() {
        use std::net::TcpListener;
        // A listener that accepts but never speaks the protocol is not a host.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let probe =
            std::thread::spawn(move || probe_target(addr, None, Duration::from_millis(300)));
        let (stream, _) = listener.accept().unwrap();
        drop(stream);
        assert_eq!(probe.join().unwrap(), None);
    }

    #[test]
    fn probe_recognizes_a_real_host_challenge() {
        use crate::protocol::{ControlMessage, PROTOCOL_VERSION};
        use std::io::Read;
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let host = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = read_frame(&mut stream).unwrap(); // consume the probe hello
            write_frame(
                &mut stream,
                &ControlMessage::Challenge {
                    protocol: PROTOCOL_VERSION as u32,
                    pake: "00".into(),
                    host_name: "phone".into(),
                    device_id: "phone-id".into(),
                },
            )
            .unwrap();
            let mut sink = [0u8; 1];
            let _ = stream.read(&mut sink);
        });
        let peer = probe_target(addr, None, Duration::from_secs(2)).unwrap();
        assert_eq!(peer.name, "phone");
        assert_eq!(peer.addr, addr);
        host.join().unwrap();
    }

    #[test]
    fn probe_rejects_a_challenge_for_another_protocol_version() {
        use crate::protocol::ControlMessage;
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let host = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = read_frame(&mut stream).unwrap();
            write_frame(
                &mut stream,
                &ControlMessage::Challenge {
                    protocol: PROTOCOL_VERSION as u32 + 1,
                    pake: "00".into(),
                    host_name: "not-relay-v3".into(),
                    device_id: "not-relay-id".into(),
                },
            )
            .unwrap();
        });
        assert_eq!(
            probe_target(addr, None, Duration::from_secs(2)),
            None,
            "an incompatible service must not appear as a relay peer"
        );
        host.join().unwrap();
    }

    #[test]
    fn usb_targets_keep_the_link_that_generated_them_as_the_bind_address() {
        let link = usb_link([192, 168, 42, 129]);
        let recent = [SocketAddr::from(([192, 168, 42, 7], DEFAULT_PROBE_PORT))];
        let targets = probe_targets(
            std::slice::from_ref(&link),
            &[link.addr],
            &[DEFAULT_PROBE_PORT],
            &recent,
        );
        assert_eq!(targets[0].target, recent[0]);
        assert_eq!(targets[0].bind_addr, link.addr);
        assert!(targets.iter().all(|target| target.bind_addr == link.addr));
    }
}
