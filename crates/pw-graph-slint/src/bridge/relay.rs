#[cfg(feature = "relay")]
use pw_graph_backend::{
    relay_build_qr_payload, relay_parse_qr_payload, relay_qr, RelayCodecKind, RelayEvent,
    RelayHostRequest, RelayRoles, RelaySessionId, RelayTransportPreference,
};
use slint::SharedString;
#[cfg(feature = "relay")]
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};
#[cfg(feature = "relay")]
use std::collections::BTreeSet;
#[cfg(feature = "relay")]
use std::net::ToSocketAddrs;
#[cfg(feature = "relay")]
use std::str::FromStr;

use pw_graph_i18n::I18n;

use super::app::Application;
use super::RelayRow;

pub(crate) fn start_relay_discovery(application: &mut Application) {
    #[cfg(feature = "relay")]
    {
        if let Err(error) = application.source.relay_discovery_start() {
            application.status = application.tf("relay.error", &[("error", error)]);
        }
    }
    #[cfg(not(feature = "relay"))]
    {
        application.status = application.t("relay.unavailable");
    }
}

pub(crate) fn stop_relay_discovery(application: &mut Application) {
    #[cfg(feature = "relay")]
    application.source.relay_discovery_stop();
    #[cfg(not(feature = "relay"))]
    let _ = application;
}

pub(crate) fn relay_host_active(application: &Application) -> bool {
    #[cfg(feature = "relay")]
    {
        application.source.relay_status().host_active
    }
    #[cfg(not(feature = "relay"))]
    {
        let _ = application;
        false
    }
}

pub(crate) fn relay_nodes_visible(application: &Application) -> bool {
    #[cfg(feature = "relay")]
    {
        let status = application.source.relay_status();
        status.host_active || !status.sessions.is_empty() || application.relay_connecting.is_some()
    }
    #[cfg(not(feature = "relay"))]
    {
        let _ = application;
        false
    }
}

pub(crate) fn start_relay_host(application: &mut Application) {
    #[cfg(feature = "relay")]
    {
        let request = RelayHostRequest {
            device_name: application.config.relay_device_name.trim().to_owned(),
            pin: application.config.relay_host_pin.trim().to_owned(),
            port: application.config.relay_host_port,
            codec: relay_codec(&application.config.relay_codec),
            frame_ms: application.config.relay_frame_ms.clamp(5, 60),
            transport: relay_transport(&application.config.relay_transport),
        };
        match application.source.relay_start_host(request) {
            Ok(port) => {
                application.status =
                    application.tf("relay.host_started", &[("port", port.to_string())])
            }
            Err(error) => application.status = application.tf("relay.error", &[("error", error)]),
        }
    }
    #[cfg(not(feature = "relay"))]
    {
        application.status = application.t("relay.unavailable");
    }
}

pub(crate) fn stop_relay_host(application: &mut Application) {
    #[cfg(feature = "relay")]
    {
        match application.source.relay_stop_host() {
            Ok(()) => application.status = application.t("relay.host_stopped"),
            Err(error) => application.status = application.tf("relay.error", &[("error", error)]),
        }
    }
    #[cfg(not(feature = "relay"))]
    {
        application.status = application.t("relay.unavailable");
    }
}

pub(crate) fn connect_relay(application: &mut Application, requested_target: Option<&str>) {
    #[cfg(feature = "relay")]
    {
        let raw_target = requested_target
            .map(str::to_owned)
            .unwrap_or_else(|| application.config.relay_client_target.clone());
        let raw_target = raw_target.trim().to_owned();
        if raw_target.is_empty() {
            application.status = application.t("status.relay_target_required");
            return;
        }
        let target_text = match relay_parse_qr_payload(&raw_target) {
            Some(payload) => {
                application.config.relay_client_target = payload.target.clone();
                if let Some(pin) = payload.pin {
                    application.config.relay_client_pin = pin;
                }
                payload.target
            }
            None => {
                if requested_target.is_some() {
                    application.config.relay_client_target = raw_target.clone();
                }
                raw_target
            }
        };
        let target = match target_text
            .to_socket_addrs()
            .ok()
            .and_then(|mut addrs| addrs.next())
        {
            Some(target) => target,
            None => {
                application.status =
                    application.tf("relay.invalid_target", &[("target", target_text)]);
                return;
            }
        };
        match application.source.relay_connect(
            target,
            application.config.relay_client_pin.trim(),
            relay_roles(&application.config.relay_role),
        ) {
            Ok(()) => {
                application.relay_connecting = Some(target.to_string());
                application.status = application.t("relay.connecting");
            }
            Err(error) => application.status = application.tf("relay.error", &[("error", error)]),
        }
    }
    #[cfg(not(feature = "relay"))]
    {
        let _ = requested_target;
        application.status = application.t("relay.unavailable");
    }
}

pub(crate) fn disconnect_relay(application: &mut Application, session: Option<u64>) {
    #[cfg(feature = "relay")]
    {
        let Some(session) = session else {
            application.status = application.t("status.relay_session_invalid");
            return;
        };
        match application.source.relay_disconnect(RelaySessionId(session)) {
            Ok(()) => application.status = application.t("relay.disconnecting"),
            Err(error) => application.status = application.tf("relay.error", &[("error", error)]),
        }
    }
    #[cfg(not(feature = "relay"))]
    {
        let _ = session;
        application.status = application.t("relay.unavailable");
    }
}

#[cfg(feature = "relay")]
pub(crate) fn poll_relay_events(application: &mut Application) {
    for event in application.source.relay_events() {
        match event {
            RelayEvent::HostStarted { port } => {
                application.status =
                    application.tf("relay.host_started", &[("port", port.to_string())]);
            }
            RelayEvent::HostStopped => application.status = application.t("relay.host_stopped"),
            RelayEvent::PeerDiscovered { peer } => {
                application.status =
                    application.tf("relay.peer_discovered", &[("name", peer.name)]);
            }
            RelayEvent::PeerLost { peer } => {
                application.status = application.tf("relay.peer_lost", &[("name", peer.name)]);
            }
            RelayEvent::SessionEstablished { peer, .. } => {
                application.relay_connecting = None;
                application.status =
                    application.tf("relay.session_connected", &[("name", peer.name)]);
            }
            RelayEvent::SessionLost { id, reason } => {
                application.relay_levels.remove(&id.0);
                application.status = application.tf("relay.session_lost", &[("reason", reason)]);
            }
            RelayEvent::AudioLevel { id, rms } => {
                application.relay_levels.insert(id.0, rms.clamp(0.0, 1.0));
            }
            RelayEvent::Error { message } => {
                application.relay_connecting = None;
                application.status = application.tf("relay.error", &[("error", message)]);
            }
        }
    }
}

#[cfg(not(feature = "relay"))]
pub(crate) fn poll_relay_events(_application: &mut Application) {}

#[cfg(feature = "relay")]
fn relay_roles(value: &str) -> RelayRoles {
    match value {
        "emit" => RelayRoles::emit_only(),
        "receive" => RelayRoles::receive_only(),
        _ => RelayRoles::both(),
    }
}

#[cfg(feature = "relay")]
fn relay_codec(value: &str) -> RelayCodecKind {
    if value.eq_ignore_ascii_case("pcm") {
        RelayCodecKind::Pcm
    } else {
        RelayCodecKind::Opus
    }
}

#[cfg(feature = "relay")]
fn relay_transport(value: &str) -> RelayTransportPreference {
    RelayTransportPreference::from_str(value).unwrap_or_default()
}

#[cfg(feature = "relay")]
pub(crate) fn relay_qr_payload(application: &Application) -> Option<String> {
    let port = application.source.relay_status().host_port?;
    let link = application.source.relay_local_links().into_iter().next()?;
    Some(relay_build_qr_payload(
        link.addr,
        port,
        application.config.relay_host_pin.trim(),
    ))
}

#[cfg(not(feature = "relay"))]
pub(crate) fn relay_qr_payload(_application: &Application) -> Option<String> {
    None
}
pub(crate) fn relay_rows(application: &Application, i18n: &I18n) -> Vec<RelayRow> {
    #[cfg(not(feature = "relay"))]
    let _ = application;
    #[cfg(feature = "relay")]
    {
        let status = application.source.relay_status();
        let mut rows = Vec::new();
        let mut connected = BTreeSet::new();
        for session in status.sessions {
            let address = session.peer.addr.to_string();
            connected.insert(address.clone());
            let direction = match (session.sending, session.receiving) {
                (true, true) => i18n.text("relay.direction_both"),
                (true, false) => i18n.text("relay.direction_send"),
                (false, true) => i18n.text("relay.direction_receive"),
                (false, false) => i18n.text("relay.direction_connected"),
            };
            rows.push(RelayRow {
                id: SharedString::from(session.id.0.to_string()),
                name: SharedString::from(session.peer.name),
                address: SharedString::from(address.clone()),
                state: SharedString::from(format!(
                    "{} · {direction}",
                    i18n.text("relay.group_connected")
                )),
                level: application
                    .relay_levels
                    .get(&session.id.0)
                    .copied()
                    .unwrap_or_default(),
                connected: true,
                connecting: false,
            });
        }
        let connecting = application.relay_connecting.as_deref();
        let mut peers = application.source.relay_peers();
        peers.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.addr.cmp(&b.addr)));
        for peer in peers {
            let address = peer.addr.to_string();
            if connected.contains(&address) {
                continue;
            }
            let state = if connecting == Some(address.as_str()) {
                i18n.text("relay.state_connecting")
            } else {
                i18n.text("relay.state_available")
            };
            rows.push(RelayRow {
                id: SharedString::from(address.clone()),
                name: SharedString::from(peer.name),
                address: SharedString::from(address.clone()),
                state: SharedString::from(state),
                level: 0.0,
                connected: false,
                connecting: connecting == Some(address.as_str()),
            });
        }
        if let Some(target) = connecting {
            if !rows.iter().any(|row| row.address == target) {
                rows.push(RelayRow {
                    id: SharedString::from(target),
                    name: SharedString::from(target),
                    address: SharedString::from(target),
                    state: i18n.text("relay.state_connecting").into(),
                    level: 0.0,
                    connected: false,
                    connecting: true,
                });
            }
        }
        if rows.is_empty() && !application.config.relay_client_target.trim().is_empty() {
            rows.push(RelayRow {
                id: SharedString::from(application.config.relay_client_target.clone()),
                name: i18n.text("relay.configured_peer").into(),
                address: SharedString::from(application.config.relay_client_target.clone()),
                state: i18n.text("relay.state_configured").into(),
                level: 0.0,
                connected: false,
                connecting: false,
            });
        }
        if rows.is_empty() {
            rows.push(RelayRow {
                id: SharedString::new(),
                name: i18n.text("relay.no_peers").into(),
                address: i18n.text("relay.discovery_help").into(),
                state: i18n.text("relay.state_idle").into(),
                level: 0.0,
                connected: false,
                connecting: false,
            });
        }
        rows
    }
    #[cfg(not(feature = "relay"))]
    {
        vec![RelayRow {
            id: SharedString::new(),
            name: i18n.text("relay.unavailable").into(),
            address: i18n.text("relay.advanced_help").into(),
            state: i18n.text("relay.state_unavailable").into(),
            level: 0.0,
            connected: false,
            connecting: false,
        }]
    }
}

pub(crate) fn relay_role_index(value: &str) -> i32 {
    match value {
        "emit" => 0,
        "receive" => 1,
        _ => 2,
    }
}

pub(crate) fn relay_role_from_index(index: i32) -> &'static str {
    match index {
        0 => "emit",
        1 => "receive",
        _ => "both",
    }
}

pub(crate) fn relay_codec_index(value: &str) -> i32 {
    if value.eq_ignore_ascii_case("pcm") {
        1
    } else {
        0
    }
}

pub(crate) fn relay_codec_from_index(index: i32) -> &'static str {
    if index == 1 {
        "pcm"
    } else {
        "opus"
    }
}

pub(crate) fn relay_frame_index(frame_ms: u16) -> i32 {
    match frame_ms {
        0..=5 => 0,
        6..=15 => 1,
        16..=30 => 2,
        31..=50 => 3,
        _ => 4,
    }
}

pub(crate) fn relay_frame_from_index(index: i32) -> u16 {
    match index {
        1 => 10,
        2 => 20,
        3 => 40,
        4 => 60,
        _ => 5,
    }
}

pub(crate) fn relay_transport_index(value: &str) -> i32 {
    match value {
        "wifi" => 1,
        "bluetooth" => 2,
        "lan" => 3,
        _ => 0,
    }
}

pub(crate) fn relay_transport_from_index(index: i32) -> &'static str {
    match index {
        1 => "wifi",
        2 => "bluetooth",
        3 => "lan",
        _ => "auto",
    }
}

#[cfg(feature = "relay")]
pub(crate) fn relay_host_endpoint(application: &Application, port: Option<u16>) -> String {
    let Some(port) = port else {
        return String::new();
    };
    application
        .source
        .relay_local_links()
        .into_iter()
        .next()
        .map(|link| format!("{}:{port}", link.addr))
        .unwrap_or_else(|| format!("0.0.0.0:{port}"))
}

#[cfg(feature = "relay")]
pub(crate) fn qr_image(payload: &str) -> Image {
    let Some(scale) = relay_qr::module_scale_for(payload, 236) else {
        return Image::default();
    };
    let Some(bitmap) = relay_qr::render(payload, scale, relay_qr::DEFAULT_QUIET_MODULES) else {
        return Image::default();
    };
    let pixels: Vec<Rgba8Pixel> = bitmap
        .dark
        .into_iter()
        .map(|dark| {
            if dark {
                Rgba8Pixel {
                    r: 0,
                    g: 0,
                    b: 0,
                    a: 255,
                }
            } else {
                Rgba8Pixel {
                    r: 255,
                    g: 255,
                    b: 255,
                    a: 255,
                }
            }
        })
        .collect();
    let mut buffer =
        SharedPixelBuffer::<Rgba8Pixel>::new(bitmap.width as u32, bitmap.height as u32);
    buffer.make_mut_slice().copy_from_slice(&pixels);
    Image::from_rgba8(buffer)
}
