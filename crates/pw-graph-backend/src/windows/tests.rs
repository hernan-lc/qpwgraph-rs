use super::*;
use pw_graph_core::{backend_for_node, backend_for_port, BackendKind};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

#[test]
fn stable_ids_are_deterministic_and_namespaced() {
    let endpoint = graph_id(endpoint_node_local_id("speaker-id"));
    let port = graph_id(endpoint_port_local_id("speaker-id"));
    assert_eq!(endpoint, graph_id(endpoint_node_local_id("speaker-id")));
    assert_ne!(endpoint, port);
    assert_eq!(
        backend_for_node(NodeId(endpoint)),
        Some(BackendKind::WindowsAudio)
    );
    assert_eq!(
        backend_for_port(PortId(port)),
        Some(BackendKind::WindowsAudio)
    );
}

#[test]
fn session_link_identity_depends_on_both_native_identifiers() {
    assert_ne!(
        session_link_local_id("endpoint-a", "session-a"),
        session_link_local_id("endpoint-b", "session-a")
    );
    assert_ne!(
        session_link_local_id("endpoint-a", "session-a"),
        session_link_local_id("endpoint-a", "session-b")
    );
}

#[test]
fn endpoint_and_session_direction_mapping_matches_core_audio_flow() {
    assert_eq!(endpoint_direction(Audio::eRender), Direction::Sink);
    assert_eq!(endpoint_direction(Audio::eCapture), Direction::Source);
    assert_eq!(session_direction(Audio::eRender), Direction::Source);
    assert_eq!(session_direction(Audio::eCapture), Direction::Sink);

    let session_port = PortId(10);
    let endpoint_port = PortId(20);
    assert_eq!(
        session_link_ports(Audio::eRender, session_port, endpoint_port),
        (session_port, endpoint_port)
    );
    assert_eq!(
        session_link_ports(Audio::eCapture, session_port, endpoint_port),
        (endpoint_port, session_port)
    );
}

#[test]
fn endpoint_notifications_mark_the_graph_dirty() {
    let dirty = Arc::new(AtomicBool::new(false));
    let topology_dirty = Arc::new(AtomicBool::new(false));
    let callback: Audio::IMMNotificationClient = EndpointNotificationClient {
        dirty: Arc::clone(&dirty),
        topology_dirty: Arc::clone(&topology_dirty),
    }
    .into();

    unsafe {
        callback
            .OnDeviceAdded(PCWSTR(std::ptr::null()))
            .expect("notification callback should accept a device event");
    }
    assert!(dirty.load(Ordering::Acquire));
    assert!(topology_dirty.load(Ordering::Acquire));
}

#[test]
fn session_notifications_record_only_the_owning_endpoint() {
    let dirty = Arc::new(AtomicBool::new(false));
    let endpoints = Arc::new(Mutex::new(BTreeSet::new()));
    mark_session_endpoint_dirty(&dirty, &endpoints, "endpoint-a");
    mark_session_endpoint_dirty(&dirty, &endpoints, "endpoint-a");
    assert!(dirty.load(Ordering::Acquire));
    assert_eq!(
        take_session_dirty_endpoints(&endpoints),
        BTreeSet::from(["endpoint-a".into()])
    );
    assert!(take_session_dirty_endpoints(&endpoints).is_empty());
}

#[test]
fn a_valid_volume_callback_promotes_an_initially_unknown_state() {
    let states: AudioStateMap = Arc::new(Mutex::new(BTreeMap::new()));
    let node = NodeId(42);

    apply_state_change(&states, node, 0.4, true);

    let state = states.lock().unwrap()[&node];
    assert_eq!(state.volume, Some(0.4));
    assert_eq!(state.muted, Some(true));
    assert!(state.volume_readable);
    assert!(state.mute_readable);
}

#[test]
fn live_backend_startup_is_optional_for_headless_windows_ci() {
    let Ok(mut driver) = WindowsAudioDriver::new() else {
        // Windows CI runners may not expose an audio service or endpoint.
        return;
    };
    let nodes = driver
        .refresh()
        .expect("Core Audio refresh should succeed after startup");
    assert!(nodes.iter().all(|node| {
        matches!(
            node.node_type,
            NodeType::WindowsAudioEndpoint | NodeType::WindowsAudioSession
        )
    }));
    assert!(driver
        .graph()
        .ports
        .values()
        .all(|port| port.port_type == PortType::Audio));
    assert!(driver.graph().links.values().all(|link| {
        driver.graph().port(link.output_port).is_some()
            && driver.graph().port(link.input_port).is_some()
    }));
}
