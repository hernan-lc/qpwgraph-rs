//! Peer discovery over mDNS/DNS-SD (`_qpw-relay._udp`).
//!
//! A hosting engine advertises its control endpoint while listening; client
//! engines can browse for hosts instead of requiring a manual address. TXT
//! records are small and versioned (`v=1`) so third-party browsers can list
//! relay hosts without speaking the control protocol.

use crate::netlink::LinkKind;
use crate::protocol::{DeviceKind, Roles};
use crate::{EngineInner, PeerInfo, RelayError, RelayResult};
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// DNS-SD service type used by all relay peers.
pub const SERVICE_TYPE: &str = "_qpw-relay._udp.local.";

/// TXT record protocol version.
pub const TXT_VERSION: &str = "1";

/// Build the TXT record for an advertised relay service.
pub fn txt_properties(
    device_name: &str,
    device_kind: DeviceKind,
    caps: Roles,
    link: Option<LinkKind>,
) -> BTreeMap<String, String> {
    let mut caps_value = Vec::new();
    if caps.emit {
        caps_value.push("emit");
    }
    if caps.receive {
        caps_value.push("recv");
    }
    let mut properties = BTreeMap::new();
    properties.insert("v".into(), TXT_VERSION.into());
    properties.insert("name".into(), device_name.to_string());
    properties.insert("kind".into(), device_kind.as_str().into());
    properties.insert("caps".into(), caps_value.join(","));
    if let Some(link) = link {
        properties.insert("link".into(), link.as_str().into());
    }
    properties
}

/// Parse a discovered TXT record back into displayable facts.
pub struct DiscoveredMeta {
    pub version: String,
    pub name: Option<String>,
    pub kind: DeviceKind,
    pub caps_emit: bool,
    pub caps_receive: bool,
    pub link: Option<String>,
}

pub fn parse_txt_properties(properties: &BTreeMap<String, String>) -> DiscoveredMeta {
    let caps = properties.get("caps").cloned().unwrap_or_default();
    let kind = match properties.get("kind").map(String::as_str) {
        Some("android") => DeviceKind::Android,
        Some("linux") => DeviceKind::Linux,
        _ => DeviceKind::Other,
    };
    DiscoveredMeta {
        version: properties.get("v").cloned().unwrap_or_default(),
        name: properties.get("name").cloned(),
        kind,
        caps_emit: caps.split(',').any(|token| token.trim() == "emit"),
        caps_receive: caps.split(',').any(|token| token.trim() == "recv"),
        link: properties.get("link").cloned(),
    }
}

/// Advertise the local relay host. Dropping or stopping the advertiser
/// unregisters the service and shuts the mDNS daemon down.
pub(crate) struct Advertiser {
    daemon: ServiceDaemon,
    fullname: String,
}

impl Advertiser {
    fn start(
        device_name: &str,
        device_kind: DeviceKind,
        port: u16,
        caps: Roles,
        bind_addr: Option<std::net::Ipv4Addr>,
        link: Option<LinkKind>,
    ) -> RelayResult<Self> {
        let daemon = ServiceDaemon::new()
            .map_err(|error| RelayError::Engine(format!("mDNS daemon failed: {error}")))?;
        let properties: Vec<(String, String)> =
            txt_properties(device_name, device_kind, caps, link)
                .into_iter()
                .collect();
        // Advertise precisely the address selected for the listener. The
        // empty-address form is reserved for the documented no-link fallback
        // where the TCP listener really does bind every IPv4 interface.
        let info = match bind_addr {
            Some(addr) => ServiceInfo::new(
                SERVICE_TYPE,
                device_name,
                &format!("{device_name}.local."),
                IpAddr::V4(addr),
                port,
                properties.as_slice(),
            ),
            None => ServiceInfo::new(
                SERVICE_TYPE,
                device_name,
                &format!("{device_name}.local."),
                (),
                port,
                properties.as_slice(),
            ),
        }
        .map_err(|error| RelayError::Engine(format!("mDNS service info invalid: {error}")))?;
        let fullname = info.get_fullname().to_string();
        daemon
            .register(info)
            .map_err(|error| RelayError::Engine(format!("mDNS registration failed: {error}")))?;
        Ok(Self { daemon, fullname })
    }

    fn stop(self) {
        let _ = self.daemon.unregister(&self.fullname);
        let _ = self.daemon.shutdown();
    }
}

/// A running browse; stopped by flag and daemon shutdown.
pub(crate) struct Browser {
    stop: Arc<AtomicBool>,
    active: Arc<AtomicBool>,
    daemon: ServiceDaemon,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Browser {
    fn start(inner: &Arc<EngineInner>) -> RelayResult<Self> {
        let daemon = ServiceDaemon::new()
            .map_err(|error| RelayError::Engine(format!("mDNS daemon failed: {error}")))?;
        let receiver = match daemon.browse(SERVICE_TYPE) {
            Ok(receiver) => receiver,
            Err(error) => {
                let _ = daemon.shutdown();
                return Err(RelayError::Engine(format!("mDNS browse failed: {error}")));
            }
        };
        let stop = Arc::new(AtomicBool::new(false));
        let active = Arc::new(AtomicBool::new(true));
        let thread_stop = Arc::clone(&stop);
        let thread_active = Arc::clone(&active);
        let inner = Arc::clone(inner);
        let worker_daemon = daemon.clone();
        let worker = match std::thread::Builder::new()
            .name("relay-discovery".into())
            .spawn(move || {
                browse_loop(&inner, worker_daemon, receiver, thread_stop, thread_active);
            }) {
            Ok(worker) => worker,
            Err(error) => {
                let _ = daemon.stop_browse(SERVICE_TYPE);
                let _ = daemon.shutdown();
                return Err(RelayError::Io(error));
            }
        };
        Ok(Self {
            stop,
            active,
            daemon,
            worker: Some(worker),
        })
    }

    fn is_running(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    fn stop(self) {
        self.stop.store(true, Ordering::Release);
        // Wake the worker immediately instead of waiting for the next
        // receive timeout. The worker repeats these calls on its way out so
        // an engine-side exit is also cleaned up.
        let _ = self.daemon.stop_browse(SERVICE_TYPE);
        let _ = self.daemon.shutdown();
        if let Some(worker) = self.worker {
            let _ = worker.join();
        }
    }
}

fn browse_loop(
    inner: &Arc<EngineInner>,
    daemon: ServiceDaemon,
    receiver: mdns_sd::Receiver<ServiceEvent>,
    stop: Arc<AtomicBool>,
    active: Arc<AtomicBool>,
) {
    loop {
        if !inner.running.load(Ordering::Relaxed) || stop.load(Ordering::Relaxed) {
            break;
        }
        let event = match receiver.recv_timeout(Duration::from_millis(250)) {
            Ok(event) => event,
            Err(_) => continue,
        };
        match event {
            ServiceEvent::ServiceResolved(info) => {
                let mut properties = BTreeMap::new();
                for property in info.get_properties().iter() {
                    properties.insert(property.key().to_string(), property.val_str().to_string());
                }
                let meta = parse_txt_properties(&properties);
                if meta.version != TXT_VERSION {
                    continue;
                }
                let service_id = info.get_fullname().to_string();
                let name = meta.name.clone().unwrap_or_else(|| service_id.clone());
                let peers = info
                    .get_addresses_v4()
                    .iter()
                    .map(|addr| PeerInfo {
                        name: name.clone(),
                        kind: meta.kind,
                        addr: SocketAddr::new(IpAddr::V4(**addr), info.get_port()),
                    })
                    .collect();
                inner.refresh_service(&service_id, peers);
            }
            ServiceEvent::ServiceRemoved(_ty, fullname) => {
                inner.lost_peer(&fullname);
            }
            _ => {}
        }
    }
    let _ = daemon.stop_browse(SERVICE_TYPE);
    let _ = daemon.shutdown();
    active.store(false, Ordering::Release);
}

/// Engine-side discovery state and event plumbing.
impl EngineInner {
    pub(crate) fn start_advertiser(&self, port: u16, bind_addr: Option<std::net::Ipv4Addr>) {
        let config = self.config();
        let links = crate::netlink::local_links();
        let link = bind_addr.and_then(|addr| {
            links
                .iter()
                .find(|link| link.addr == addr)
                .map(|link| link.kind)
        });
        match Advertiser::start(
            &config.device_name,
            config.device_kind,
            port,
            Roles::both(),
            bind_addr,
            link,
        ) {
            Ok(advertiser) => {
                if let Ok(mut slot) = self.advertiser.lock() {
                    if let Some(previous) = slot.take() {
                        previous.stop();
                    }
                    *slot = Some(advertiser);
                }
            }
            Err(error) => {
                self.emit(crate::RelayEvent::Error {
                    message: format!("mDNS advertisement unavailable: {error}"),
                });
            }
        }
    }

    pub(crate) fn stop_advertiser(&self) {
        stop_slot(&self.advertiser);
    }

    pub(crate) fn start_browser(self: &Arc<Self>) -> RelayResult<()> {
        start_stoppable(&self.browser, || Browser::start(self))
    }

    pub(crate) fn stop_browser(&self) {
        stop_slot(&self.browser);
    }

    pub(crate) fn start_usb_scanner(self: &Arc<Self>) -> RelayResult<()> {
        start_stoppable(&self.usb_scanner, || {
            crate::usb_probe::UsbScanner::start(self)
        })
    }

    pub(crate) fn stop_usb_scanner(&self) {
        stop_slot(&self.usb_scanner);
    }

    pub(crate) fn refresh_service(&self, service_id: &str, current: Vec<PeerInfo>) {
        let mut discovered = Vec::new();
        let mut lost = Vec::new();
        if let (Ok(mut services), Ok(mut peers)) = (self.peer_services.lock(), self.peers.lock()) {
            let previous = services.insert(
                service_id.to_owned(),
                current
                    .iter()
                    .map(|peer| (peer.addr, peer.clone()))
                    .collect(),
            );
            for peer in &current {
                if previous
                    .as_ref()
                    .is_none_or(|old| !old.contains_key(&peer.addr))
                {
                    discovered.push(peer.clone());
                }
                peers.insert(peer.addr, peer.clone());
            }
            if let Some(previous) = previous {
                for (addr, peer) in previous {
                    if !current.iter().any(|current| current.addr == addr)
                        && !services.values().any(|service| service.contains_key(&addr))
                    {
                        peers.remove(&addr);
                        lost.push(peer);
                    }
                }
            }
        }
        for peer in discovered {
            self.emit(crate::RelayEvent::PeerDiscovered { peer });
        }
        for peer in lost {
            self.emit(crate::RelayEvent::PeerLost { peer });
        }
    }

    pub(crate) fn lost_peer(&self, service_id: &str) {
        let mut lost = Vec::new();
        if let (Ok(mut services), Ok(mut peers)) = (self.peer_services.lock(), self.peers.lock()) {
            if let Some(addresses) = services.remove(service_id) {
                for (addr, peer) in addresses {
                    let still_advertised =
                        services.values().any(|service| service.contains_key(&addr));
                    if !still_advertised && peers.remove(&addr).is_some() {
                        lost.push(peer);
                    }
                }
            }
        }
        for peer in lost {
            self.emit(crate::RelayEvent::PeerLost { peer });
        }
    }

    pub(crate) fn discovered_peers(&self) -> Vec<PeerInfo> {
        self.peers
            .lock()
            .map(|peers| peers.values().cloned().collect())
            .unwrap_or_default()
    }

    /// Remove every transient discovery result and emit the corresponding
    /// loss events. This is called after both discovery workers have stopped,
    /// so a later restart starts with a clean peer set and cannot display a
    /// stale USB address from the previous tether.
    pub(crate) fn clear_discovered(&self) {
        let lost = if let (Ok(mut services), Ok(mut peers)) =
            (self.peer_services.lock(), self.peers.lock())
        {
            services.clear();
            std::mem::take(&mut *peers)
                .into_values()
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        for peer in lost {
            self.emit(crate::RelayEvent::PeerLost { peer });
        }
    }
}

/// A background service that can be stopped by taking it out of its slot.
trait Stoppable: Send {
    fn stop(self);
    fn is_active(&self) -> bool;
}

impl Stoppable for Browser {
    fn stop(self) {
        Browser::stop(self)
    }
    fn is_active(&self) -> bool {
        self.is_running()
    }
}

impl Stoppable for Advertiser {
    fn stop(self) {
        Advertiser::stop(self)
    }
    fn is_active(&self) -> bool {
        false
    }
}

impl Stoppable for crate::usb_probe::UsbScanner {
    fn stop(self) {
        crate::usb_probe::UsbScanner::stop(self)
    }
    fn is_active(&self) -> bool {
        crate::usb_probe::UsbScanner::is_active(self)
    }
}

/// Replace the content of a `Mutex<Option<T>>`, stopping the previous value.
fn stop_slot<T: Stoppable>(slot: &Mutex<Option<T>>) {
    let service = slot.lock().ok().and_then(|mut slot| slot.take());
    if let Some(service) = service {
        service.stop();
    }
}

/// Start a background service inside a slot, unless one is already running.
fn start_stoppable<T: Stoppable, F: FnOnce() -> RelayResult<T>>(
    slot: &Mutex<Option<T>>,
    make: F,
) -> RelayResult<()> {
    let mut slot = slot
        .lock()
        .map_err(|_| RelayError::Engine("discovery state is locked".into()))?;
    if slot.as_ref().is_some_and(|service| service.is_active()) {
        return Ok(());
    }
    if let Some(previous) = slot.take() {
        previous.stop();
    }
    *slot = Some(make()?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn txt_properties_round_trip() {
        let properties = txt_properties(
            "studio-pc",
            DeviceKind::Linux,
            Roles::both(),
            Some(LinkKind::Wifi),
        );
        assert_eq!(properties.get("v").map(String::as_str), Some(TXT_VERSION));
        assert_eq!(
            properties.get("name").map(String::as_str),
            Some("studio-pc")
        );
        assert_eq!(properties.get("kind").map(String::as_str), Some("linux"));
        assert_eq!(
            properties.get("caps").map(String::as_str),
            Some("emit,recv")
        );
        assert_eq!(properties.get("link").map(String::as_str), Some("wifi"));

        let meta = parse_txt_properties(&properties);
        assert_eq!(meta.version, TXT_VERSION);
        assert_eq!(meta.name.as_deref(), Some("studio-pc"));
        assert_eq!(meta.kind, DeviceKind::Linux);
        assert!(meta.caps_emit);
        assert!(meta.caps_receive);
        assert_eq!(meta.link.as_deref(), Some("wifi"));
    }

    #[test]
    fn unsupported_txt_versions_are_not_accepted() {
        let mut properties = BTreeMap::new();
        properties.insert("v".into(), "2".into());
        let meta = parse_txt_properties(&properties);
        assert_ne!(meta.version, TXT_VERSION);
    }

    #[test]
    fn service_refresh_replaces_stale_addresses() {
        let engine = crate::RelayEngine::start(crate::EngineConfig::default()).unwrap();
        let inner = &engine.inner;
        let first = PeerInfo {
            name: "host".into(),
            kind: DeviceKind::Linux,
            addr: "192.168.1.10:48123".parse().unwrap(),
        };
        let second = PeerInfo {
            name: "host".into(),
            kind: DeviceKind::Linux,
            addr: "192.168.1.11:48123".parse().unwrap(),
        };
        inner.refresh_service("host._qpw-relay._udp.local.", vec![first.clone()]);
        assert_eq!(inner.discovered_peers(), vec![first.clone()]);
        inner.refresh_service("host._qpw-relay._udp.local.", vec![second.clone()]);
        assert_eq!(inner.discovered_peers(), vec![second]);
        engine.shutdown();
    }

    #[test]
    fn clearing_discovery_removes_all_transient_peers() {
        let engine = crate::RelayEngine::start(crate::EngineConfig::default()).unwrap();
        let inner = &engine.inner;
        let peer = PeerInfo {
            name: "usb-host".into(),
            kind: DeviceKind::Linux,
            addr: "192.168.42.1:48123".parse().unwrap(),
        };
        inner.refresh_service(crate::usb_probe::USB_PROBE_SERVICE, vec![peer.clone()]);
        assert_eq!(inner.discovered_peers(), vec![peer]);
        inner.clear_discovered();
        assert!(inner.discovered_peers().is_empty());
        engine.shutdown();
    }

    #[test]
    fn discovery_workers_stop_and_restart_without_duplicate_slots() {
        let engine = crate::RelayEngine::start(crate::EngineConfig::default()).unwrap();
        let inner = &engine.inner;

        inner.start_usb_scanner().expect("USB scanner starts");
        assert!(inner
            .usb_scanner
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|scanner| scanner.is_active()));
        // Starting an already-live scanner is idempotent, not a second worker.
        inner
            .start_usb_scanner()
            .expect("second start is idempotent");
        assert!(inner
            .usb_scanner
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|scanner| scanner.is_active()));
        inner.stop_usb_scanner();
        assert!(inner.usb_scanner.lock().unwrap().is_none());

        inner.start_usb_scanner().expect("scanner restarts");
        assert!(inner
            .usb_scanner
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|scanner| scanner.is_active()));
        inner.stop_usb_scanner();
        assert!(inner.usb_scanner.lock().unwrap().is_none());

        // mDNS startup depends on the host's multicast environment. When it
        // is available, exercise the same stop/restart contract for Browser;
        // a sandbox that cannot create the daemon is valid for the USB-only
        // fallback and is handled by the independent-mechanism test.
        if inner.start_browser().is_ok() {
            assert!(inner
                .browser
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|browser| browser.is_running()));
            inner
                .start_browser()
                .expect("second browser start is idempotent");
            inner.stop_browser();
            assert!(inner.browser.lock().unwrap().is_none());
            inner.start_browser().expect("browser restarts");
            inner.stop_browser();
            assert!(inner.browser.lock().unwrap().is_none());
        }
        engine.shutdown();
    }

    #[test]
    fn usb_link_loss_removes_usb_peers_but_keeps_mdns_peers() {
        let engine = crate::RelayEngine::start(crate::EngineConfig::default()).unwrap();
        let inner = &engine.inner;
        let mdns_peer = PeerInfo {
            name: "wifi-host".into(),
            kind: DeviceKind::Linux,
            addr: "192.168.1.20:48123".parse().unwrap(),
        };
        let usb_peer = PeerInfo {
            name: "usb-host".into(),
            kind: DeviceKind::Linux,
            addr: "192.168.42.1:48123".parse().unwrap(),
        };
        inner.refresh_service("wifi-host._qpw-relay._udp.local.", vec![mdns_peer.clone()]);
        inner.refresh_service(crate::usb_probe::USB_PROBE_SERVICE, vec![usb_peer]);
        inner.lost_peer(crate::usb_probe::USB_PROBE_SERVICE);
        assert_eq!(inner.discovered_peers(), vec![mdns_peer]);
        engine.shutdown();
    }

    #[test]
    fn emit_only_caps_omit_recv() {
        let properties = txt_properties(
            "phone",
            DeviceKind::Android,
            Roles::emit_only(),
            Some(LinkKind::Usb),
        );
        assert_eq!(properties.get("caps").map(String::as_str), Some("emit"));
        assert_eq!(properties.get("kind").map(String::as_str), Some("android"));
        let meta = parse_txt_properties(&properties);
        assert!(meta.caps_emit);
        assert!(!meta.caps_receive);
    }
}
