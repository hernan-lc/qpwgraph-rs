use super::QpwgraphApp;
use pw_graph_backend::{
    BackendError, BackendResult, RelayCodecKind, RelayEvent, RelayHostRequest, RelayPeerInfo,
    RelayRoles, RelaySessionId, RelayTransportPreference,
};
use std::net::{SocketAddr, ToSocketAddrs};
use std::str::FromStr;

#[derive(Default)]
pub(crate) struct RelayUiState {
    pub(crate) discovery_active: bool,
    pub(crate) peers: Vec<RelayPeerInfo>,
    pub(crate) message: String,
}

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
