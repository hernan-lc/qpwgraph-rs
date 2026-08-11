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

use super::app::PreviewApp;
use super::RelayRow;

pub(crate) fn start_relay_discovery(preview: &mut PreviewApp) {
    #[cfg(feature = "relay")]
    {
        if let Err(error) = preview.source.relay_discovery_start() {
            preview.status = format!("Relay discovery unavailable: {error}");
        }
    }
    #[cfg(not(feature = "relay"))]
    {
        preview.status = "Relay support is not enabled in this build".into();
    }
}

pub(crate) fn stop_relay_discovery(preview: &mut PreviewApp) {
    #[cfg(feature = "relay")]
    preview.source.relay_discovery_stop();
    #[cfg(not(feature = "relay"))]
    let _ = preview;
}

pub(crate) fn relay_host_active(preview: &PreviewApp) -> bool {
    #[cfg(feature = "relay")]
    {
        return preview.source.relay_status().host_active;
    }
    #[cfg(not(feature = "relay"))]
    {
        let _ = preview;
        false
    }
}

pub(crate) fn relay_nodes_visible(preview: &PreviewApp) -> bool {
    #[cfg(feature = "relay")]
    {
        let status = preview.source.relay_status();
        return status.host_active
            || !status.sessions.is_empty()
            || preview.relay_connecting.is_some();
    }
    #[cfg(not(feature = "relay"))]
    {
        let _ = preview;
        false
    }
}

pub(crate) fn start_relay_host(preview: &mut PreviewApp) {
    #[cfg(feature = "relay")]
    {
        let request = RelayHostRequest {
            device_name: preview.config.relay_device_name.trim().to_owned(),
            pin: preview.config.relay_host_pin.trim().to_owned(),
            port: preview.config.relay_host_port,
            codec: relay_codec(&preview.config.relay_codec),
            frame_ms: preview.config.relay_frame_ms.clamp(5, 60),
            transport: relay_transport(&preview.config.relay_transport),
        };
        match preview.source.relay_start_host(request) {
            Ok(port) => preview.status = format!("Relay host started on port {port}"),
            Err(error) => preview.status = format!("Could not start relay host: {error}"),
        }
    }
    #[cfg(not(feature = "relay"))]
    {
        preview.status = "Relay support is not enabled in this build".into();
    }
}

pub(crate) fn stop_relay_host(preview: &mut PreviewApp) {
    #[cfg(feature = "relay")]
    {
        match preview.source.relay_stop_host() {
            Ok(()) => preview.status = "Relay host stopped".into(),
            Err(error) => preview.status = format!("Could not stop relay host: {error}"),
        }
    }
    #[cfg(not(feature = "relay"))]
    {
        preview.status = "Relay support is not enabled in this build".into();
    }
}

pub(crate) fn connect_relay(preview: &mut PreviewApp, requested_target: Option<&str>) {
    #[cfg(feature = "relay")]
    {
        let raw_target = requested_target
            .map(str::to_owned)
            .unwrap_or_else(|| preview.config.relay_client_target.clone());
        let raw_target = raw_target.trim().to_owned();
        if raw_target.is_empty() {
            preview.status = "Enter a relay address before connecting".into();
            return;
        }
        let target_text = match relay_parse_qr_payload(&raw_target) {
            Some(payload) => {
                preview.config.relay_client_target = payload.target.clone();
                if let Some(pin) = payload.pin {
                    preview.config.relay_client_pin = pin;
                }
                payload.target
            }
            None => {
                if requested_target.is_some() {
                    preview.config.relay_client_target = raw_target.clone();
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
                preview.status = format!("Invalid relay target: {target_text}");
                return;
            }
        };
        match preview.source.relay_connect(
            target,
            preview.config.relay_client_pin.trim(),
            relay_roles(&preview.config.relay_role),
        ) {
            Ok(()) => {
                preview.relay_connecting = Some(target.to_string());
                preview.status = format!("Connecting to relay peer {target}");
            }
            Err(error) => preview.status = format!("Could not connect to relay peer: {error}"),
        }
    }
    #[cfg(not(feature = "relay"))]
    {
        let _ = requested_target;
        preview.status = "Relay support is not enabled in this build".into();
    }
}

pub(crate) fn disconnect_relay(preview: &mut PreviewApp, session: Option<u64>) {
    #[cfg(feature = "relay")]
    {
        let Some(session) = session else {
            preview.status = "Invalid relay session".into();
            return;
        };
        match preview.source.relay_disconnect(RelaySessionId(session)) {
            Ok(()) => preview.status = "Disconnecting relay peer".into(),
            Err(error) => preview.status = format!("Could not disconnect relay peer: {error}"),
        }
    }
    #[cfg(not(feature = "relay"))]
    {
        let _ = session;
        preview.status = "Relay support is not enabled in this build".into();
    }
}

#[cfg(feature = "relay")]
pub(crate) fn poll_relay_events(preview: &mut PreviewApp) {
    for event in preview.source.relay_events() {
        match event {
            RelayEvent::HostStarted { port } => {
                preview.status = format!("Relay host started on port {port}");
            }
            RelayEvent::HostStopped => preview.status = "Relay host stopped".into(),
            RelayEvent::PeerDiscovered { peer } => {
                preview.status = format!("Relay peer discovered: {}", peer.name);
            }
            RelayEvent::PeerLost { peer } => {
                preview.status = format!("Relay peer lost: {}", peer.name);
            }
            RelayEvent::SessionEstablished { peer, .. } => {
                preview.relay_connecting = None;
                preview.status = format!("Relay connected: {}", peer.name);
            }
            RelayEvent::SessionLost { id, reason } => {
                preview.relay_levels.remove(&id.0);
                preview.status = format!("Relay session lost: {reason}");
            }
            RelayEvent::AudioLevel { id, rms } => {
                preview.relay_levels.insert(id.0, rms.clamp(0.0, 1.0));
            }
            RelayEvent::Error { message } => {
                preview.relay_connecting = None;
                preview.status = format!("Relay error: {message}");
            }
        }
    }
}

#[cfg(not(feature = "relay"))]
pub(crate) fn poll_relay_events(_preview: &mut PreviewApp) {}

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
pub(crate) fn relay_qr_payload(preview: &PreviewApp) -> Option<String> {
    let port = preview.source.relay_status().host_port?;
    let link = preview.source.relay_local_links().into_iter().next()?;
    Some(relay_build_qr_payload(
        link.addr,
        port,
        preview.config.relay_host_pin.trim(),
    ))
}

#[cfg(not(feature = "relay"))]
pub(crate) fn relay_qr_payload(_preview: &PreviewApp) -> Option<String> {
    None
}
pub(crate) fn relay_rows(preview: &PreviewApp) -> Vec<RelayRow> {
    #[cfg(not(feature = "relay"))]
    let _ = preview;
    #[cfg(feature = "relay")]
    {
        let status = preview.source.relay_status();
        let mut rows = Vec::new();
        let mut connected = BTreeSet::new();
        for session in status.sessions {
            let address = session.peer.addr.to_string();
            connected.insert(address.clone());
            let direction = match (session.sending, session.receiving) {
                (true, true) => "send + receive",
                (true, false) => "send",
                (false, true) => "receive",
                (false, false) => "connected",
            };
            rows.push(RelayRow {
                id: SharedString::from(session.id.0.to_string()),
                name: SharedString::from(session.peer.name),
                address: SharedString::from(address),
                state: SharedString::from(format!("connected · {direction}")),
                level: preview
                    .relay_levels
                    .get(&session.id.0)
                    .copied()
                    .unwrap_or_default(),
            });
        }
        let connecting = preview.relay_connecting.as_deref();
        let mut peers = preview.source.relay_peers();
        peers.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.addr.cmp(&b.addr)));
        for peer in peers {
            let address = peer.addr.to_string();
            if connected.contains(&address) {
                continue;
            }
            let state = if connecting == Some(address.as_str()) {
                "connecting"
            } else {
                "available"
            };
            rows.push(RelayRow {
                id: SharedString::from(address.clone()),
                name: SharedString::from(peer.name),
                address: SharedString::from(address),
                state: SharedString::from(state),
                level: 0.0,
            });
        }
        if let Some(target) = connecting {
            if !rows.iter().any(|row| row.address == target) {
                rows.push(RelayRow {
                    id: SharedString::from(target),
                    name: SharedString::from(target),
                    address: SharedString::from(target),
                    state: "connecting".into(),
                    level: 0.0,
                });
            }
        }
        if rows.is_empty() && !preview.config.relay_client_target.trim().is_empty() {
            rows.push(RelayRow {
                id: SharedString::from(preview.config.relay_client_target.clone()),
                name: "Configured peer".into(),
                address: SharedString::from(preview.config.relay_client_target.clone()),
                state: "configured".into(),
                level: 0.0,
            });
        }
        if rows.is_empty() {
            rows.push(RelayRow {
                id: SharedString::new(),
                name: "No relay peers discovered".into(),
                address: "Open discovery or enter an address above".into(),
                state: "idle".into(),
                level: 0.0,
            });
        }
        return rows;
    }
    #[cfg(not(feature = "relay"))]
    {
        vec![RelayRow {
            id: SharedString::new(),
            name: "Relay support not compiled".into(),
            address: "Build with the relay feature to connect peers".into(),
            state: "unavailable".into(),
            level: 0.0,
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
pub(crate) fn relay_host_endpoint(preview: &PreviewApp, port: Option<u16>) -> String {
    let Some(port) = port else {
        return String::new();
    };
    preview
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
