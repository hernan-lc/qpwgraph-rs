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
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// The fixed relay control port recommended for manual USB setups, probed by
/// default because ephemeral host ports cannot be discovered by scanning.
pub const DEFAULT_PROBE_PORT: u16 = 48123;

/// Do not enumerate subnets larger than this (USB tether links are /24).
const MAX_CANDIDATES: u32 = 1024;

/// How many addresses are probed concurrently.
const PROBE_BATCH: usize = 32;

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
pub fn probe_target(target: SocketAddr, timeout: Duration) -> Option<PeerInfo> {
    let mut stream = crate::netlink::connect_tcp(target, None, timeout).ok()?;
    stream.set_read_timeout(Some(timeout)).ok()?;
    let hello = ControlMessage::Hello {
        protocol: PROTOCOL_VERSION as u32,
        device_name: "qpw-relay-probe".into(),
        device_kind: DeviceKind::Other,
        roles: Roles::emit_only(),
        sample_rate: 48_000,
        channels: 1,
    };
    write_frame(&mut stream, &hello).ok()?;
    match read_frame(&mut stream) {
        Ok(ControlMessage::Challenge { host_name, .. }) => Some(PeerInfo {
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

/// Scan every USB tether link for relay hosts listening on `ports`.
///
/// Candidates are probed concurrently in small batches; the whole scan is
/// bounded by `timeout` per candidate so a /24 subnet stays fast enough to
/// repeat while discovery runs.
pub fn probe_usb_hosts(ports: &[u16], timeout: Duration) -> Vec<PeerInfo> {
    let links = local_links();
    let local_addrs: Vec<Ipv4Addr> = links.iter().map(|link| link.addr).collect();
    let mut candidates: Vec<SocketAddr> = Vec::new();
    for link in links.iter().filter(|link| link.kind == LinkKind::Usb) {
        for addr in candidate_hosts(link, &local_addrs) {
            for port in ports {
                candidates.push(SocketAddr::from((addr, *port)));
            }
        }
    }

    let mut found = Vec::new();
    for batch in candidates.chunks(PROBE_BATCH) {
        std::thread::scope(|scope| {
            let handles: Vec<_> = batch
                .iter()
                .map(|target| {
                    let target = *target;
                    scope.spawn(move || probe_target(target, timeout))
                })
                .collect();
            for handle in handles {
                if let Ok(Some(peer)) = handle.join() {
                    found.push(peer);
                }
            }
        });
    }
    found.sort_by_key(|peer| peer.addr);
    found.dedup_by(|a, b| a.addr == b.addr);
    found
}

/// How often discovery rescans USB subnets while active.
pub(crate) const SCAN_INTERVAL: Duration = Duration::from_secs(4);

/// Per-candidate timeout; bounded so a full /24 scan stays short.
const SCAN_TARGET_TIMEOUT: Duration = Duration::from_millis(250);

/// Background scanner that feeds USB-probed hosts into the same peer map the
/// mDNS browser uses, so they surface through the usual discovery events.
pub(crate) struct UsbScanner {
    stop: Arc<AtomicBool>,
}

impl UsbScanner {
    pub(crate) fn start(inner: &Arc<EngineInner>) -> RelayResult<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let inner = Arc::clone(inner);
        std::thread::Builder::new()
            .name("relay-usb-scan".into())
            .spawn(move || scan_loop(&inner, thread_stop))
            .map_err(RelayError::Io)?;
        Ok(Self { stop })
    }

    pub(crate) fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    pub(crate) fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }
}

/// Service identity under which probed USB hosts are tracked. Distinct from
/// any mDNS fullname, so probed and advertised peers never collide.
pub(crate) const USB_PROBE_SERVICE: &str = "usb-probe._qpw-relay._udp.local.";

fn scan_loop(inner: &Arc<EngineInner>, stop: Arc<AtomicBool>) {
    loop {
        if !inner.running.load(Ordering::Relaxed) || stop.load(Ordering::Relaxed) {
            break;
        }
        let peers = probe_usb_hosts(&[DEFAULT_PROBE_PORT], SCAN_TARGET_TIMEOUT);
        if !stop.load(Ordering::Relaxed) && inner.running.load(Ordering::Relaxed) {
            inner.refresh_service(USB_PROBE_SERVICE, peers);
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
        let probe = std::thread::spawn(move || probe_target(addr, Duration::from_millis(300)));
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
                    salt: "00".into(),
                    host_name: "phone".into(),
                },
            )
            .unwrap();
            let mut sink = [0u8; 1];
            let _ = stream.read(&mut sink);
        });
        let peer = probe_target(addr, Duration::from_secs(2)).unwrap();
        assert_eq!(peer.name, "phone");
        assert_eq!(peer.addr, addr);
        host.join().unwrap();
    }
}
