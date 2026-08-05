use super::QpwgraphApp;
use eframe::egui::TextureHandle;
use pw_graph_backend::{
    BackendError, BackendResult, RelayCodecKind, RelayEvent, RelayHostRequest, RelayLinkKind,
    RelayLocalLink, RelayPeerInfo, RelayRoles, RelaySessionId, RelayTransportPreference,
};
use std::net::{SocketAddr, ToSocketAddrs};
use std::str::FromStr;
use std::time::Instant;

/// Tabs inside the relay panel, one per relay activity.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub(crate) enum RelayPanelTab {
    #[default]
    Emitter,
    Receiver,
    Discover,
    Sessions,
}

#[derive(Default)]
pub(crate) struct RelayUiState {
    pub(crate) tab: RelayPanelTab,
    pub(crate) discovery_active: bool,
    pub(crate) peers: Vec<RelayPeerInfo>,
    pub(crate) message: String,
    /// Local IPv4 links, ranked best-first; refreshed on an interval.
    pub(crate) links: Vec<RelayLocalLink>,
    /// Active USB tether link, when one is detected. USB is never a select
    /// option: `Auto` prefers it and the panel simply reports the link.
    pub(crate) usb_link: Option<RelayLocalLink>,
    /// QR modal visibility and its cached payload/texture.
    pub(crate) show_qr: bool,
    pub(crate) qr_text: String,
    pub(crate) qr_texture: Option<TextureHandle>,
    last_link_check: Option<Instant>,
}

/// How often the panel rescans network interfaces for a USB tether.
const LINK_CHECK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

impl RelayUiState {
    fn role(value: &str) -> RelayRoles {
        match value {
            "emit" => RelayRoles::emit_only(),
            "receive" => RelayRoles::receive_only(),
            _ => RelayRoles::both(),
        }
    }

    fn codec(value: &str) -> RelayCodecKind {
        if value.eq_ignore_ascii_case("pcm") {
            RelayCodecKind::Pcm
        } else {
            RelayCodecKind::Opus
        }
    }

    fn transport(value: &str) -> RelayTransportPreference {
        RelayTransportPreference::from_str(value).unwrap_or_default()
    }

    fn resolve_target(target: &str) -> BackendResult<SocketAddr> {
        target
            .trim()
            .to_socket_addrs()
            .map_err(|error| BackendError::Native(format!("invalid relay target: {error}")))?
            .next()
            .ok_or_else(|| BackendError::Native("relay target did not resolve".into()))
    }

    pub(crate) fn poll(&mut self, app: &mut QpwgraphApp) {
        self.peers = app.driver.relay_peers();
        // Legacy configs may still name USB explicitly; it is now detected
        // automatically under `Auto` and no longer offered as a choice.
        if app.config.relay_transport == "usb" {
            app.config.relay_transport = "auto".to_owned();
        }
        let refresh_due = self
            .last_link_check
            .is_none_or(|at| at.elapsed() >= LINK_CHECK_INTERVAL);
        if refresh_due {
            self.last_link_check = Some(Instant::now());
            self.links = app.driver.relay_local_links();
            self.usb_link = self
                .links
                .iter()
                .find(|link| link.kind == RelayLinkKind::Usb)
                .cloned();
        }
        let events = app.driver.relay_events();
        for event in events {
            match event {
                RelayEvent::HostStarted { port } => {
                    self.message = app.tf("relay.host_started", &[("port", port.to_string())]);
                }
                RelayEvent::HostStopped => {
                    self.message = app.t("relay.host_stopped");
                }
                RelayEvent::PeerDiscovered { peer } => {
                    self.message = app.tf("relay.peer_discovered", &[("name", peer.name)]);
                }
                RelayEvent::PeerLost { peer } => {
                    self.message = app.tf("relay.peer_lost", &[("name", peer.name)]);
                }
                RelayEvent::SessionEstablished { peer, .. } => {
                    self.message = app.tf("relay.session_connected", &[("name", peer.name)]);
                }
                RelayEvent::SessionLost { reason, .. } => {
                    self.message = app.tf("relay.session_lost", &[("reason", reason)]);
                }
                RelayEvent::Error { message } => {
                    self.message = app.tf("relay.error", &[("error", message)]);
                }
                RelayEvent::AudioLevel { .. } => {}
            }
        }
    }

    pub(crate) fn host_request(&self, app: &QpwgraphApp) -> RelayHostRequest {
        RelayHostRequest {
            device_name: app.config.relay_device_name.clone(),
            pin: app.config.relay_host_pin.clone(),
            port: app.config.relay_host_port,
            codec: Self::codec(&app.config.relay_codec),
            frame_ms: app.config.relay_frame_ms,
            transport: Self::transport(&app.config.relay_transport),
        }
    }

    /// Payload encoded in the "scan to connect" QR: the best-ranked local
    /// address with the live control port and the pairing PIN. Links come
    /// back USB-first, so a tethered phone scans the tether address.
    pub(crate) fn qr_payload(app: &QpwgraphApp) -> Option<String> {
        let port = app.driver.relay_status().host_port?;
        let link = app.relay.links.first()?;
        let pin = app.config.relay_host_pin.trim();
        let mut payload = format!("qpw-relay://{}:{}", link.addr, port);
        if !pin.is_empty() {
            payload.push_str(&format!("?pin={pin}"));
        }
        Some(payload)
    }

    pub(crate) fn start_host(&mut self, app: &mut QpwgraphApp) {
        let request = self.host_request(app);
        match app.driver.relay_start_host(request) {
            Ok(port) => {
                self.message = app.tf("relay.host_started", &[("port", port.to_string())]);
                app.refresh_graph();
            }
            Err(error) => self.message = app.tf("relay.error", &[("error", error.to_string())]),
        }
    }

    pub(crate) fn stop_host(&mut self, app: &mut QpwgraphApp) {
        match app.driver.relay_stop_host() {
            Ok(()) => self.message = app.t("relay.host_stopped"),
            Err(error) => self.message = app.tf("relay.error", &[("error", error.to_string())]),
        }
    }

    pub(crate) fn connect(&mut self, app: &mut QpwgraphApp) {
        let target = match Self::resolve_target(&app.config.relay_client_target) {
            Ok(target) => target,
            Err(error) => {
                self.message = app.tf("relay.error", &[("error", error.to_string())]);
                return;
            }
        };
        match app.driver.relay_connect(
            target,
            &app.config.relay_client_pin,
            Self::role(&app.config.relay_role),
        ) {
            Ok(()) => self.message = app.t("relay.connecting"),
            Err(error) => self.message = app.tf("relay.error", &[("error", error.to_string())]),
        }
    }

    pub(crate) fn disconnect(&mut self, app: &mut QpwgraphApp, session: RelaySessionId) {
        match app.driver.relay_disconnect(session) {
            Ok(()) => self.message = app.t("relay.disconnecting"),
            Err(error) => self.message = app.tf("relay.error", &[("error", error.to_string())]),
        }
    }

    pub(crate) fn toggle_discovery(&mut self, app: &mut QpwgraphApp) {
        if self.discovery_active {
            app.driver.relay_discovery_stop();
            self.discovery_active = false;
            self.message = app.t("relay.discovery_stopped");
        } else {
            match app.driver.relay_discovery_start() {
                Ok(()) => {
                    self.discovery_active = true;
                    self.message = app.t("relay.discovery_started");
                }
                Err(error) => {
                    self.message = app.tf("relay.error", &[("error", error.to_string())]);
                }
            }
        }
    }
}
