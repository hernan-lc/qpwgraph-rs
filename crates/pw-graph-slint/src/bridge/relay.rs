#[cfg(feature = "relay")]
use pw_graph_backend::{
    relay_build_qr_payload, relay_parse_qr_payload, relay_qr, RelayCodecKind, RelayEvent,
    RelayHostRequest, RelayPeerInfo, RelayRoles, RelaySessionId, RelayTransportPreference,
    RelayTrustedPeer,
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
#[cfg(feature = "relay")]
use std::time::{Duration, Instant};

#[cfg(feature = "relay")]
use pw_graph_utils::hex::{hex_decode, hex_encode};

use pw_graph_i18n::I18n;

use super::app::Application;
#[cfg(feature = "relay")]
use super::app::RelayAttempt;
use super::RelayRow;

#[cfg(feature = "relay")]
fn relay_device_id(application: &mut Application) -> String {
    if application.config.relay_device_id.trim().is_empty() {
        application.config.relay_device_id = pw_graph_backend::relay_generate_device_id();
    }
    application.config.relay_device_id.clone()
}

#[cfg(feature = "relay")]
fn configured_trusted_peers(application: &Application) -> Vec<RelayTrustedPeer> {
    application
        .config
        .relay_trusted_peers
        .iter()
        .filter_map(|stored| {
            let peer_id = stored.peer_id.trim();
            if peer_id.is_empty() {
                return None;
            }
            let secret = hex_decode(stored.secret.trim()).ok()?.try_into().ok()?;
            Some(RelayTrustedPeer {
                peer_id: peer_id.to_owned(),
                secret,
            })
        })
        .collect()
}

#[cfg(feature = "relay")]
fn trusted_secret_for(application: &Application, peer_id: &str) -> Option<[u8; 32]> {
    application
        .config
        .relay_trusted_peers
        .iter()
        .find(|stored| stored.peer_id == peer_id)
        .and_then(|stored| hex_decode(stored.secret.trim()).ok()?.try_into().ok())
}

#[cfg(feature = "relay")]
fn remember_trusted_peer(
    application: &mut Application,
    peer_id: &str,
    peer: &RelayPeerInfo,
    secret: [u8; 32],
) {
    let peer_id = peer_id.trim();
    if peer_id.is_empty() {
        return;
    }
    let encoded = hex_encode(&secret);
    let mut changed = false;
    if let Some(stored) = application
        .config
        .relay_trusted_peers
        .iter_mut()
        .find(|stored| stored.peer_id == peer_id)
    {
        if stored.secret != encoded {
            stored.secret = encoded;
            changed = true;
        }
        if stored.name != peer.name {
            stored.name = peer.name.clone();
            changed = true;
        }
        let address = peer.addr.to_string();
        if stored.address != address {
            stored.address = address;
            changed = true;
        }
    } else {
        application
            .config
            .relay_trusted_peers
            .push(pw_graph_config::PersistedRelayPeer {
                peer_id: peer_id.to_owned(),
                secret: encoded,
                name: peer.name.clone(),
                address: peer.addr.to_string(),
            });
        changed = true;
    }
    if changed {
        // autosave_config observes the snapshot difference, but recording a
        // dirty time here makes the persistence intent explicit for hosts
        // that receive an enrollment while the preferences window is idle.
        application
            .config_dirty_since
            .get_or_insert_with(Instant::now);
    }
}

#[cfg(feature = "relay")]
fn refresh_trusted_peer_address(application: &mut Application, peer: &RelayPeerInfo) {
    let Some(stored) = application
        .config
        .relay_trusted_peers
        .iter_mut()
        .find(|stored| stored.peer_id == peer.id)
    else {
        return;
    };
    let address = peer.addr.to_string();
    if stored.name != peer.name || stored.address != address {
        stored.name = peer.name.clone();
        stored.address = address;
        application
            .config_dirty_since
            .get_or_insert_with(Instant::now);
    }
}

#[cfg(feature = "relay")]
fn configure_relay_identity(application: &mut Application) -> Result<(), String> {
    let device_id = relay_device_id(application);
    let trusted_peers = configured_trusted_peers(application);
    let transport = relay_transport(&application.config.relay_transport);
    application
        .source
        .relay_configure_identity(device_id, trusted_peers, transport)
}

#[cfg(feature = "relay")]
fn session_or_attempt_for_peer(application: &Application, peer: &RelayPeerInfo) -> bool {
    let status = application.source.relay_status();
    status.sessions.iter().any(|session| {
        (!peer.id.is_empty() && session.peer.id == peer.id) || session.peer.addr == peer.addr
    }) || application
        .relay_connecting
        .as_ref()
        .is_some_and(|attempt| {
            attempt.peer_id.as_deref() == Some(peer.id.as_str())
                || attempt.target == peer.addr.to_string()
        })
}

/// Start a connection using the credential learned during an earlier PIN
/// pairing. This is deliberately only called for a discovered peer whose
/// stable identity has a matching stored secret.
#[cfg(feature = "relay")]
fn connect_trusted_peer(application: &mut Application, peer: &RelayPeerInfo) -> bool {
    let Some(secret) = trusted_secret_for(application, &peer.id) else {
        return false;
    };
    application.relay_trusted_auto_attempt_at = Some(Instant::now());
    if session_or_attempt_for_peer(application, peer) {
        return true;
    }
    if let Err(error) = configure_relay_identity(application) {
        application.status = application.tf("relay.error", &[("error", error)]);
        return true;
    }
    match application.source.relay_connect_trusted(
        peer.addr,
        &peer.id,
        secret,
        relay_roles(&application.config.relay_role),
    ) {
        Ok(session) => {
            application.relay_connecting = Some(RelayAttempt {
                target: peer.addr.to_string(),
                session: session.0,
                peer_id: Some(peer.id.clone()),
            });
            application.status = application.t("relay.connecting");
        }
        Err(error) => application.status = application.tf("relay.error", &[("error", error)]),
    }
    true
}

/// Retry a trusted peer whose discovery record is still present. Discovery
/// reports address changes, but a failed TCP attempt does not necessarily
/// produce a second discovery event; bounded retries are what make a cable
/// insertion work through a short host-start or route-setup race.
fn retry_trusted_auto_connect(application: &mut Application) {
    const RETRY_INTERVAL: Duration = Duration::from_secs(5);
    if !application.config.relay_auto_connect_trusted
        || application.relay_connecting.is_some()
        || application
            .source
            .relay_status()
            .sessions
            .iter()
            .any(|session| {
                application
                    .config
                    .relay_trusted_peers
                    .iter()
                    .any(|trusted| trusted.peer_id == session.peer.id)
            })
        || application
            .relay_trusted_auto_attempt_at
            .is_some_and(|last| last.elapsed() < RETRY_INTERVAL)
    {
        return;
    }
    let peer = application
        .source
        .relay_peers()
        .into_iter()
        .find(|peer| trusted_secret_for(application, &peer.id).is_some());
    if let Some(peer) = peer {
        let _ = connect_trusted_peer(application, &peer);
    }
}

pub(crate) fn start_relay_discovery(application: &mut Application) {
    #[cfg(feature = "relay")]
    {
        if let Err(error) = application.source.relay_discovery_start() {
            application.status = application.tf("relay.error", &[("error", error)]);
        } else {
            application.relay_discovery_active = true;
        }
    }
    #[cfg(not(feature = "relay"))]
    {
        application.status = application.t("relay.unavailable");
    }
}

pub(crate) fn stop_relay_discovery(application: &mut Application) {
    #[cfg(feature = "relay")]
    {
        application.source.relay_discovery_stop();
        application.relay_discovery_active = false;
    }
    #[cfg(not(feature = "relay"))]
    let _ = application;
}

/// Start discovery when a real USB tether link appears and remove its peers
/// immediately when the link disappears. A discovered peer is eligible for
/// immediate connection only after this installation has explicitly paired
/// with the same stable peer identity and stored its credential.
#[cfg(feature = "relay")]
pub(crate) fn poll_relay_usb_hotplug(application: &mut Application) {
    const POLL_INTERVAL: Duration = Duration::from_secs(1);
    if application
        .relay_usb_last_poll
        .is_some_and(|last| last.elapsed() < POLL_INTERVAL)
    {
        return;
    }
    application.relay_usb_last_poll = Some(std::time::Instant::now());
    let present = application.source.relay_usb_link_present();
    let appeared = present && !application.relay_usb_present;
    let disappeared = !present && application.relay_usb_present;
    application.relay_usb_present = present;

    if disappeared {
        application.source.relay_discovery_usb_link_lost();
        application.relay_usb_auto_attempted = false;
    }
    if appeared {
        application.relay_usb_auto_attempted = false;
    }
    if appeared && !application.relay_usb_auto_attempted {
        application.relay_usb_auto_attempted = true;
        if !application.relay_discovery_active {
            match application.source.relay_discovery_start() {
                Ok(()) => {
                    application.relay_discovery_active = true;
                    application.status = application.t("relay.discovery_started");
                }
                Err(error) => {
                    application.status = application.tf("relay.error", &[("error", error)]);
                }
            }
        }
    }
}

#[cfg(not(feature = "relay"))]
pub(crate) fn poll_relay_usb_hotplug(_application: &mut Application) {}

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

/// A host PIN is ephemeral: each hosting session gets a fresh random one
/// rather than a stored (or, worse, shipped) value.
///
/// The two halves of that promise are split into these functions so the
/// lifecycle can be tested without standing up a relay backend. Together they
/// hold three properties the UI depends on:
///
/// - a first start always has a PIN;
/// - the PIN does not move while a session is live, so the panel and the
///   pairing QR code keep showing one that actually works;
/// - a stop retires it, so the next start generates a new one.
///
/// Generating unconditionally in [`host_pin_on_start`] would satisfy the third
/// property too, but it would also throw away a PIN a user had deliberately
/// typed into the field, so the retirement happens on stop instead.
pub(crate) fn host_pin_on_start(pin: &mut String, generate: impl FnOnce() -> String) {
    if pin.trim().is_empty() {
        *pin = generate();
    }
}

/// Retire the PIN along with the session it belonged to.
///
/// Leaving it set meant the next start silently reused it, so a PIN already
/// shown on screen, photographed as a QR code or read out loud kept working
/// across sessions — the opposite of the per-session freshness above. It is
/// never written to disk (`AppConfig::relay_host_pin` is `serde(skip)`), so
/// clearing it loses nothing.
pub(crate) fn host_pin_on_stop(pin: &mut String) {
    pin.clear();
}

pub(crate) fn start_relay_host(application: &mut Application) {
    #[cfg(feature = "relay")]
    {
        host_pin_on_start(
            &mut application.config.relay_host_pin,
            pw_graph_backend::relay_generate_pin,
        );
        let device_id = relay_device_id(application);
        let trusted_peers = configured_trusted_peers(application);
        let request = RelayHostRequest {
            device_id,
            trusted_peers,
            trust_new_peers: true,
            device_name: application.config.relay_device_name.trim().to_owned(),
            pin: application.config.relay_host_pin.trim().to_owned(),
            port: application.config.relay_host_port,
            codec: relay_codec(&application.config.relay_codec),
            // Snap to a duration the protocol actually negotiates. Clamping
            // let a hand-edited config carry something like 7 ms all the way
            // to the far end of a handshake before it was rejected.
            frame_ms: pw_graph_backend::relay_normalize_frame_ms(application.config.relay_frame_ms),
            transport: relay_transport(&application.config.relay_transport),
        };
        match application.source.relay_start_host(request) {
            Ok(port) => {
                application.status =
                    application.tf("relay.host_started", &[("port", port.to_string())])
            }
            Err(error) => {
                // A failed bind/start is not a hosting session. Retire the
                // generated PIN so a later retry gets a fresh credential and
                // the UI never leaves a failed session's PIN displayed.
                host_pin_on_stop(&mut application.config.relay_host_pin);
                application.status = application.tf("relay.error", &[("error", error)])
            }
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
            Ok(()) => {
                host_pin_on_stop(&mut application.config.relay_host_pin);
                application.status = application.t("relay.host_stopped");
            }
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
        // A discovered address with a stored credential can be reconnected
        // without asking for the old PIN again. QR/manual targets still use
        // the explicit PIN unless discovery can identify the same peer.
        if let Some(peer) = application
            .source
            .relay_peers()
            .into_iter()
            .find(|peer| peer.addr == target)
        {
            if connect_trusted_peer(application, &peer) {
                return;
            }
        }
        if let Err(error) = configure_relay_identity(application) {
            application.status = application.tf("relay.error", &[("error", error)]);
            return;
        }
        match application.source.relay_connect(
            target,
            application.config.relay_client_pin.trim(),
            relay_roles(&application.config.relay_role),
        ) {
            Ok(session) => {
                application.relay_connecting = Some(RelayAttempt {
                    target: target.to_string(),
                    session: session.0,
                    peer_id: None,
                });
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
                    application.tf("relay.peer_discovered", &[("name", peer.name.clone())]);
                if application.config.relay_auto_connect_trusted {
                    let _ = connect_trusted_peer(application, &peer);
                }
            }
            RelayEvent::PeerLost { peer } => {
                application.status = application.tf("relay.peer_lost", &[("name", peer.name)]);
            }
            RelayEvent::TrustedPeerAvailable {
                peer_id,
                peer,
                secret,
            } => {
                remember_trusted_peer(application, &peer_id, &peer, secret);
            }
            RelayEvent::SessionEstablished { id, peer, .. } => {
                if application
                    .relay_connecting
                    .as_ref()
                    .is_some_and(|attempt| attempt.session == id.0)
                {
                    application.relay_connecting = None;
                }
                refresh_trusted_peer_address(application, &peer);
                application.status =
                    application.tf("relay.session_connected", &[("name", peer.name)]);
            }
            RelayEvent::SessionLost { id, reason } => {
                application.relay_levels.remove(&id.0);
                // An attempt that fails before a session exists reports its
                // loss this way, so this is what clears a stuck "connecting"
                // row. Matching on the id keeps an unrelated session dropping
                // from clearing a live attempt.
                if application
                    .relay_connecting
                    .as_ref()
                    .is_some_and(|attempt| attempt.session == id.0)
                {
                    application.relay_connecting = None;
                }
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
    retry_trusted_auto_connect(application);
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
    let status = application.source.relay_status();
    let port = status.host_port?;
    let addr = host_link_addr(application)?;
    Some(relay_build_qr_payload(
        addr,
        port,
        application.config.relay_host_pin.trim(),
    ))
}

/// The local address to publish for pairing.
///
/// The host binds the link its transport preference selects, so the QR code
/// and the endpoint label must name that same link — otherwise the app shows
/// an address nothing is listening on.
#[cfg(feature = "relay")]
fn host_link_addr(application: &Application) -> Option<std::net::Ipv4Addr> {
    let status = application.source.relay_status();
    status.host_addr.or_else(|| {
        // A listener with no currently classified link intentionally binds
        // INADDR_ANY. It is still useful to publish a real, reachable address
        // when the display-side link enumerator has one, rather than showing
        // 0.0.0.0 in the endpoint and QR code.
        let links = application.source.relay_local_links();
        let preference = relay_transport(&application.config.relay_transport);
        let selected = pw_graph_backend::relay_select_links(&links, preference);
        selected.first().map(|link| link.addr).or_else(|| {
            let fallback =
                pw_graph_backend::relay_select_links(&links, RelayTransportPreference::Auto);
            fallback.first().map(|link| link.addr)
        })
    })
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
            if !session.peer.id.is_empty() {
                connected.insert(format!("id:{}", session.peer.id));
            }
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
        let connecting = application
            .relay_connecting
            .as_ref()
            .map(|attempt| attempt.target.as_str());
        let mut peers = application.source.relay_peers();
        peers.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.addr.cmp(&b.addr)));
        for peer in peers {
            let address = peer.addr.to_string();
            if connected.contains(&address)
                || (!peer.id.is_empty() && connected.contains(&format!("id:{}", peer.id)))
            {
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

/// Frame durations offered by the settings combo box.
///
/// The settings panel exists whether or not the relay feature is compiled in,
/// so this list cannot live behind the relay re-exports. It mirrors
/// `pw_graph_relay::FRAME_DURATIONS_MS`, and
/// `the_picker_offers_exactly_the_negotiable_frame_durations` fails if the two
/// ever drift — the duplication is checked, not trusted.
const FRAME_DURATIONS_MS: [u16; 5] = [5, 10, 20, 40, 60];

/// Combo-box index for a frame duration. A value that is not exactly one of
/// the offered durations snaps to the nearest, so a hand-edited config shows
/// the duration it will actually negotiate rather than silently disagreeing
/// with the wire.
pub(crate) fn relay_frame_index(frame_ms: u16) -> i32 {
    FRAME_DURATIONS_MS
        .iter()
        .enumerate()
        .min_by_key(|(_, candidate)| candidate.abs_diff(frame_ms))
        .map(|(index, _)| index as i32)
        .unwrap_or(1)
}

pub(crate) fn relay_frame_from_index(index: i32) -> u16 {
    FRAME_DURATIONS_MS
        .get(index.clamp(0, FRAME_DURATIONS_MS.len() as i32 - 1) as usize)
        .copied()
        .unwrap_or(10)
}

pub(crate) fn relay_transport_index(value: &str) -> i32 {
    match value {
        "wifi" => 1,
        "bluetooth" => 2,
        "lan" => 3,
        "adb" => 4,
        _ => 0,
    }
}

pub(crate) fn relay_transport_from_index(index: i32) -> &'static str {
    match index {
        1 => "wifi",
        2 => "bluetooth",
        3 => "lan",
        4 => "adb",
        _ => "auto",
    }
}

#[cfg(feature = "relay")]
pub(crate) fn relay_host_endpoint(application: &Application, port: Option<u16>) -> String {
    let Some(port) = port else {
        return String::new();
    };
    host_link_addr(application)
        .map(|addr| format!("{addr}:{port}"))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "relay")]
    #[test]
    fn the_picker_offers_exactly_the_negotiable_frame_durations() {
        assert_eq!(
            FRAME_DURATIONS_MS,
            pw_graph_backend::RELAY_FRAME_DURATIONS_MS,
            "the settings picker and the wire protocol disagree about frame durations"
        );
    }

    #[test]
    fn frame_durations_round_trip_through_the_picker() {
        for (index, duration) in FRAME_DURATIONS_MS.iter().enumerate() {
            assert_eq!(relay_frame_index(*duration), index as i32);
            assert_eq!(relay_frame_from_index(index as i32), *duration);
        }
    }

    #[test]
    fn an_unsupported_stored_duration_snaps_to_a_real_one() {
        // A hand-edited or older config could hold any u16; the picker must
        // resolve it to a duration the protocol will actually accept.
        for (stored, expected) in [(0u16, 5u16), (7, 5), (13, 10), (35, 40), (9_000, 60)] {
            assert_eq!(relay_frame_from_index(relay_frame_index(stored)), expected);
        }
    }
}

#[cfg(test)]
mod host_pin_tests {
    use super::{host_pin_on_start, host_pin_on_stop};

    /// Stand-in for `relay_generate_pin`, so the test controls what "fresh"
    /// means and can tell a regenerated PIN from a reused one.
    fn counting_generator(next: &std::cell::Cell<u32>) -> impl FnOnce() -> String + '_ {
        move || {
            next.set(next.get() + 1);
            format!("pin-{}", next.get())
        }
    }

    #[test]
    fn the_first_host_start_gets_a_pin() {
        let counter = std::cell::Cell::new(0);
        let mut pin = String::new();
        host_pin_on_start(&mut pin, counting_generator(&counter));
        assert_eq!(pin, "pin-1");
        assert!(!pin.trim().is_empty());
    }

    #[test]
    fn the_pin_is_stable_for_the_life_of_one_hosting_session() {
        // The panel and the QR code both read this field while the host runs;
        // moving it mid-session would show a PIN that does not pair.
        let counter = std::cell::Cell::new(0);
        let mut pin = String::new();
        host_pin_on_start(&mut pin, counting_generator(&counter));
        let during = pin.clone();
        for _ in 0..3 {
            host_pin_on_start(&mut pin, counting_generator(&counter));
        }
        assert_eq!(pin, during);
        assert_eq!(counter.get(), 1, "the PIN was regenerated mid-session");
    }

    #[test]
    fn stopping_then_starting_produces_a_different_pin() {
        // The regression: stopping used to leave the PIN in place, so the
        // next start silently reused a PIN that had already been displayed.
        let counter = std::cell::Cell::new(0);
        let mut pin = String::new();
        host_pin_on_start(&mut pin, counting_generator(&counter));
        let first = pin.clone();
        host_pin_on_stop(&mut pin);
        assert!(pin.is_empty(), "the stopped session left its PIN behind");
        host_pin_on_start(&mut pin, counting_generator(&counter));
        assert_ne!(pin, first);
        assert_eq!(counter.get(), 2);
    }

    #[test]
    fn a_deliberately_typed_pin_survives_until_the_session_ends() {
        // The field is editable, so a start must not clobber a PIN the user
        // chose — only a stop retires it.
        let counter = std::cell::Cell::new(0);
        let mut pin = String::from("246813");
        host_pin_on_start(&mut pin, counting_generator(&counter));
        assert_eq!(pin, "246813");
        assert_eq!(counter.get(), 0);
        host_pin_on_stop(&mut pin);
        host_pin_on_start(&mut pin, counting_generator(&counter));
        assert_eq!(pin, "pin-1");
    }

    #[test]
    fn a_whitespace_only_pin_counts_as_absent() {
        let counter = std::cell::Cell::new(0);
        let mut pin = String::from("   ");
        host_pin_on_start(&mut pin, counting_generator(&counter));
        assert_eq!(pin, "pin-1");
    }

    #[test]
    fn generated_pins_are_not_all_the_same() {
        // Guards the real generator rather than the lifecycle: a constant
        // "fresh" PIN would pass every test above.
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..32 {
            let mut pin = String::new();
            host_pin_on_start(&mut pin, pw_graph_backend::relay_generate_pin);
            assert!(!pin.trim().is_empty());
            seen.insert(pin);
        }
        assert!(seen.len() > 1, "relay_generate_pin returned a constant");
    }
}
