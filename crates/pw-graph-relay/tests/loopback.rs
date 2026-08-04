//! End-to-end loopback tests: host and client engines in one process,
//! connected over localhost. PCM codec keeps the payload deterministic.

use pw_graph_relay::{
    CodecKind, EngineConfig, RelayEngine, RelayEvent, RelayHandle, Roles, SessionId,
};
use std::net::SocketAddr;
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(8);

fn await_event(
    handle: &RelayHandle,
    predicate: impl Fn(&RelayEvent) -> bool,
) -> Option<RelayEvent> {
    let start = Instant::now();
    while start.elapsed() < TIMEOUT {
        for event in handle.events() {
            if predicate(&event) {
                return Some(event);
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    None
}

fn wait_until(mut condition: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < TIMEOUT {
        if condition() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

fn host_engine(pin: &str) -> (RelayEngine, RelayHandle, u16) {
    let engine = RelayEngine::start(EngineConfig {
        pin: pin.into(),
        ..EngineConfig::default()
    })
    .expect("host engine starts");
    let handle = engine.handle();
    let port = handle.host_start().expect("host listens");
    (engine, handle, port)
}

fn client_engine() -> (RelayEngine, RelayHandle) {
    let engine = RelayEngine::start(EngineConfig {
        codec: CodecKind::Pcm,
        device_name: "loopback-client".into(),
        ..EngineConfig::default()
    })
    .expect("client engine starts");
    let handle = engine.handle();
    (engine, handle)
}

fn ramp(samples: usize) -> Vec<f32> {
    (0..samples).map(|i| i as f32 / samples as f32).collect()
}

fn establish(host: &RelayHandle, client: &RelayHandle, port: u16, pin: &str, roles: Roles) -> SessionId {
    let target: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let session = client.connect(target, pin, roles);
    assert!(
        await_event(client, |event| matches!(
            event,
            RelayEvent::SessionEstablished { id, .. } if *id == session
        ))
        .is_some(),
        "client session should establish"
    );
    assert!(
        await_event(host, |event| matches!(event, RelayEvent::SessionEstablished { .. })).is_some(),
        "host session should establish"
    );
    session
}

#[test]
fn client_emit_delivers_audio_to_host() {
    let (_host, host_handle, port) = host_engine("123456");
    let (_client, client_handle) = client_engine();
    let session = establish(&host_handle, &client_handle, port, "123456", Roles::emit_only());

    const FRAME: usize = 960;
    let signal = ramp(FRAME * 10);
    client_handle.push_capture(&signal);

    let mut received = Vec::new();
    let mut buffer = [0.0f32; FRAME];
    assert!(
        wait_until(|| {
            let count = host_handle.pull_playback(&mut buffer);
            received.extend_from_slice(&buffer[..count]);
            received.len() >= signal.len()
        }),
        "host should receive all emitted samples, got {}",
        received.len()
    );
    assert_eq!(&received[..signal.len()], &signal[..]);

    // Graceful disconnect surfaces on the host side.
    client_handle.disconnect(session).unwrap();
    assert!(
        await_event(&host_handle, |event| matches!(
            event,
            RelayEvent::SessionLost { id, reason } if *id == session && reason.contains("peer left")
        ))
        .is_some(),
        "host should see the client leave"
    );
}

#[test]
fn host_sends_audio_to_receiving_client() {
    let (_host, host_handle, port) = host_engine("123456");
    let (_client, client_handle) = client_engine();
    establish(&host_handle, &client_handle, port, "123456", Roles::receive_only());

    const FRAME: usize = 960;
    let signal = ramp(FRAME * 8);
    host_handle.push_capture(&signal);

    let mut received = Vec::new();
    let mut buffer = [0.0f32; FRAME];
    assert!(
        wait_until(|| {
            let count = client_handle.pull_playback(&mut buffer);
            received.extend_from_slice(&buffer[..count]);
            received.len() >= signal.len()
        }),
        "client should receive all host samples, got {}",
        received.len()
    );
    assert_eq!(&received[..signal.len()], &signal[..]);
}

#[test]
fn wrong_pin_is_rejected() {
    let (_host, _host_handle, port) = host_engine("123456");
    let (_client, client_handle) = client_engine();
    let target: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let session = client_handle.connect(target, "999999", Roles::emit_only());

    let event = await_event(&client_handle, |event| matches!(
        event,
        RelayEvent::SessionLost { id, .. } if *id == session
    ));
    match event {
        Some(RelayEvent::SessionLost { reason, .. }) => {
            assert!(
                reason.contains("rejected pairing"),
                "reason should mention the rejection, got: {reason}"
            );
        }
        other => panic!("expected SessionLost, got {other:?}"),
    }
}

#[test]
fn host_requires_a_pin_before_listening() {
    let engine = RelayEngine::start(EngineConfig::default()).unwrap();
    let handle = engine.handle();
    assert!(handle.host_start().is_err());
}

#[test]
fn status_reflects_host_and_sessions() {
    let (_host, host_handle, port) = host_engine("123456");
    let (_client, client_handle) = client_engine();

    let status = host_handle.status();
    assert!(status.host_active);
    assert_eq!(status.host_port, Some(port));
    assert!(status.sessions.is_empty());

    establish(&host_handle, &client_handle, port, "123456", Roles::emit_only());
    let status = host_handle.status();
    assert_eq!(status.sessions.len(), 1);
    assert!(status.sessions[0].receiving);
    assert!(!status.sessions[0].sending);

    host_handle.host_stop().unwrap();
    assert!(!host_handle.status().host_active);
}
