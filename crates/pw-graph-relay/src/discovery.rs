//! Peer discovery over mDNS/DNS-SD (`_qpw-relay._udp`).
//!
//! A hosting engine advertises its control endpoint while listening; client
//! engines can browse for hosts instead of requiring a manual address. TXT
//! records are small and versioned (`v=1`) so third-party browsers can list
//! relay hosts without speaking the control protocol.

use crate::netlink::{LinkKind, LocalLink};
use crate::protocol::{DeviceKind, Roles};
use crate::{EngineInner, PeerInfo, RelayError, RelayResult};
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
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
        links: &[LocalLink],
    ) -> RelayResult<Self> {
        let daemon = ServiceDaemon::new()
            .map_err(|error| RelayError::Engine(format!("mDNS daemon failed: {error}")))?;
        let link = links.first().map(|link| link.kind);
        let properties: Vec<(String, String)> =
            txt_properties(device_name, device_kind, caps, link)
                .into_iter()
                .collect();
        // `()` lets the daemon announce every suitable interface address,
        // so peers on USB, Wi-Fi, or LAN all resolve the same instance.
        let info = ServiceInfo::new(
            SERVICE_TYPE,
            device_name,
            &format!("{device_name}.local."),
            (),
            port,
            properties.as_slice(),
        )
        .map_err(|error| RelayError::Engine(format!("mDNS service info invalid: {error}")))?;
        let fullname = info.get_fullname().to_string();
        daemon.register(info).map_err(|error| {
            RelayError::Engine(format!("mDNS registration failed: {error}"))
        })?;
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
}

impl Browser {
    fn start(inner: &Arc<EngineInner>) -> RelayResult<Self> {
        let daemon = ServiceDaemon::new()
            .map_err(|error| RelayError::Engine(format!("mDNS daemon failed: {error}")))?;
        let receiver = daemon
            .browse(SERVICE_TYPE)
            .map_err(|error| RelayError::Engine(format!("mDNS browse failed: {error}")))?;
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let inner = Arc::clone(inner);
        std::thread::Builder::new()
            .name("relay-discovery".into())
            .spawn(move || {
                browse_loop(&inner, daemon, receiver, thread_stop);
            })
            .map_err(RelayError::Io)?;
        Ok(Self { stop })
    }

    fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }
}

fn browse_loop(
    inner: &Arc<EngineInner>,
    daemon: ServiceDaemon,
    receiver: mdns_sd::Receiver<ServiceEvent>,
    stop: Arc<AtomicBool>,
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
                let name = meta
                    .name
                    .clone()
                    .unwrap_or_else(|| info.get_fullname().to_string());
                for addr in info.get_addresses_v4() {
                    let peer = PeerInfo {
                        name: name.clone(),
                        kind: meta.kind,
                        addr: SocketAddr::new(IpAddr::V4(*addr), info.get_port()),
                    };
                    inner.discovered_peer(peer);
                }
            }
            ServiceEvent::ServiceRemoved(_ty, fullname) => {
                inner.lost_peer_by_name(&fullname);
            }
            _ => {}
        }
    }
    let _ = daemon.stop_browse(SERVICE_TYPE);
    let _ = daemon.shutdown();
}

/// Engine-side discovery state and event plumbing.
impl EngineInner {
    pub(crate) fn start_advertiser(&self, port: u16) {
        let config = self.config();
        let links = crate::netlink::local_links();
        match Advertiser::start(
            &config.device_name,
            config.device_kind,
            port,
            Roles::both(),
            &links,
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
        if let Ok(mut slot) = self.advertiser.lock() {
            if let Some(advertiser) = slot.take() {
                advertiser.stop();
            }
        }
    }

    pub(crate) fn start_browser(self: &Arc<Self>) -> RelayResult<()> {
        if let Ok(slot) = self.browser.lock() {
            if slot.as_ref().is_some_and(|browser| !browser.stopped()) {
                return Ok(());
            }
        }
        let browser = Browser::start(self)?;
        if let Ok(mut slot) = self.browser.lock() {
            *slot = Some(browser);
        }
        Ok(())
    }

    pub(crate) fn stop_browser(&self) {
        if let Ok(mut slot) = self.browser.lock() {
            if let Some(browser) = slot.take() {
                browser.stop();
            }
        }
    }

    pub(crate) fn discovered_peer(&self, peer: PeerInfo) {
        let fresh = if let Ok(mut peers) = self.peers.lock() {
            peers.insert(peer.addr, peer.clone()).is_none()
        } else {
            false
        };
        if fresh {
            self.emit(crate::RelayEvent::PeerDiscovered { peer });
        }
    }

    pub(crate) fn lost_peer_by_name(&self, fullname: &str) {
        // The removed instance name carries the label prefix; match by the
        // leading instance label before the service type.
        let label = fullname.split('.').next().unwrap_or(fullname);
        let mut lost = Vec::new();
        if let Ok(mut peers) = self.peers.lock() {
            let stale: Vec<SocketAddr> = peers
                .iter()
                .filter(|(_, peer)| peer.name == label)
                .map(|(addr, _)| *addr)
                .collect();
            for addr in stale {
                if let Some(peer) = peers.remove(&addr) {
                    lost.push(peer);
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
}
