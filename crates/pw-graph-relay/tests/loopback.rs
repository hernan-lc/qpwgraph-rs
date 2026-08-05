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

/// One codec frame at the default configuration (10 ms, mono, 48 kHz).
const FRAME: usize = 480;
/// Frames the producer may run ahead of what the consumer has taken
/// delivery of. The relay keeps only the freshest couple of frames, so a
/// test that races further ahead than this would see its own surplus
/// discarded — correctly — as stale backlog.
const PIPELINE_SLACK: usize = 2;

/// Empty a playback queue completely, appending everything to `received`.
///
/// The relay caps its queues at a small multiple of the frame size on
/// purpose, so a consumer that takes only one buffer per poll can fall
/// behind and lose audio that the transport delivered correctly.
fn drain_playback(handle: &RelayHandle, received: &mut Vec<f32>) {
    let mut buffer = [0.0f32; FRAME];
    loop {
        let count = handle.pull_playback(&mut buffer);
        if count == 0 {
            return;
        }
        received.extend_from_slice(&buffer[..count]);
    }
}

/// Feed a signal the way a capture callback does: one frame at a time,
/// draining the far end as we go.
///
/// Pushing the whole signal in a single call would be dropped as stale
/// backlog — the queues keep only the freshest couple of frames so that a
/// stalled consumer costs a glitch instead of permanent added latency. The
/// producer therefore paces itself against actual delivery rather than a
/// fixed sleep, which also keeps the test honest on a loaded machine.
fn stream_frames(push: impl Fn(&[f32]), receiver: &RelayHandle, signal: &[f32]) -> Vec<f32> {
    let mut received = Vec::new();
    for (index, chunk) in signal.chunks(FRAME).enumerate() {
        push(chunk);
        let settled = index.saturating_sub(PIPELINE_SLACK) * FRAME;
        wait_until(|| {
            drain_playback(receiver, &mut received);
            received.len() >= settled
        });
    }
    wait_until(|| {
        drain_playback(receiver, &mut received);
        received.len() >= signal.len()
    });
    received
}

fn establish(
    host: &RelayHandle,
    client: &RelayHandle,
    port: u16,
    pin: &str,
    roles: Roles,
) -> SessionId {
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
        await_event(host, |event| matches!(
            event,
            RelayEvent::SessionEstablished { .. }
        ))
        .is_some(),
        "host session should establish"
    );
    session
}

#[test]
fn client_emit_delivers_audio_to_host() {
    let (_host, host_handle, port) = host_engine("123456");
    let (_client, client_handle) = client_engine();
    let session = establish(
        &host_handle,
        &client_handle,
        port,
        "123456",
        Roles::emit_only(),
    );

    let signal = ramp(FRAME * 10);
    let received = stream_frames(
        |frame| client_handle.push_capture(frame),
        &host_handle,
        &signal,
    );
    assert!(
        received.len() >= signal.len(),
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
    establish(
        &host_handle,
        &client_handle,
        port,
        "123456",
        Roles::receive_only(),
    );

    let signal = ramp(FRAME * 8);
    let received = stream_frames(
        |frame| host_handle.push_capture(frame),
        &client_handle,
        &signal,
    );
    assert!(
        received.len() >= signal.len(),
        "client should receive all host samples, got {}",
        received.len()
    );
    assert_eq!(&received[..signal.len()], &signal[..]);
}

#[test]
fn host_capture_fans_out_to_multiple_receivers() {
    let (_host, host_handle, port) = host_engine("123456");
    let (_client_a, client_a_handle) = client_engine();
    let (_client_b, client_b_handle) = client_engine();
    establish(
        &host_handle,
        &client_a_handle,
        port,
        "123456",
        Roles::receive_only(),
    );
    establish(
        &host_handle,
        &client_b_handle,
        port,
        "123456",
        Roles::receive_only(),
    );

    let signal = ramp(FRAME * 4);
    let mut received_a = Vec::new();
    let mut received_b = Vec::new();
    for (index, chunk) in signal.chunks(FRAME).enumerate() {
        host_handle.push_capture(chunk);
        let settled = index.saturating_sub(PIPELINE_SLACK) * FRAME;
        wait_until(|| {
            drain_playback(&client_a_handle, &mut received_a);
            drain_playback(&client_b_handle, &mut received_b);
            received_a.len() >= settled && received_b.len() >= settled
        });
    }
    assert!(wait_until(|| {
        drain_playback(&client_a_handle, &mut received_a);
        drain_playback(&client_b_handle, &mut received_b);
        received_a.len() >= signal.len() && received_b.len() >= signal.len()
    }));
    assert_eq!(&received_a[..signal.len()], &signal[..]);
    assert_eq!(&received_b[..signal.len()], &signal[..]);
}

#[test]
fn frames_traverse_the_relay_promptly() {
    let (_host, host_handle, port) = host_engine("123456");
    let (_client, client_handle) = client_engine();
    establish(
        &host_handle,
        &client_handle,
        port,
        "123456",
        Roles::emit_only(),
    );

    // Prime the path: the jitter buffer holds the first frames back until it
    // has an anchor, which is startup cost rather than steady-state delay.
    let warmup = ramp(FRAME * 8);
    stream_frames(
        |frame| client_handle.push_capture(frame),
        &host_handle,
        &warmup,
    );

    // Now time one frame from capture to playback. Over loopback this is
    // dominated by the relay's own buffering, which is exactly what the
    // queue depths and jitter tolerance are there to bound.
    let mut worst = Duration::ZERO;
    for _ in 0..10 {
        let mut received = Vec::new();
        let sent = Instant::now();
        client_handle.push_capture(&ramp(FRAME));
        assert!(
            wait_until(|| {
                drain_playback(&host_handle, &mut received);
                received.len() >= FRAME
            }),
            "a frame must arrive within the test timeout"
        );
        worst = worst.max(sent.elapsed());
    }

    // Generous next to the ~10 ms design target: this runs alongside the
    // rest of the suite on shared cores, and the point is to catch a
    // regression back to the hundreds of milliseconds an unbounded queue
    // used to accumulate, not to measure the network.
    assert!(
        worst < Duration::from_millis(150),
        "worst-case capture-to-playback delay was {worst:?}"
    );
}

#[test]
fn wrong_pin_is_rejected() {
    let (_host, _host_handle, port) = host_engine("123456");
    let (_client, client_handle) = client_engine();
    let target: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let session = client_handle.connect(target, "999999", Roles::emit_only());

    let event = await_event(&client_handle, |event| {
        matches!(
            event,
            RelayEvent::SessionLost { id, .. } if *id == session
        )
    });
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

    establish(
        &host_handle,
        &client_handle,
        port,
        "123456",
        Roles::emit_only(),
    );
    let status = host_handle.status();
    assert_eq!(status.sessions.len(), 1);
    assert!(status.sessions[0].receiving);
    assert!(!status.sessions[0].sending);

    host_handle.host_stop().unwrap();
    assert!(!host_handle.status().host_active);
}
