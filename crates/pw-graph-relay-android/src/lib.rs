use jni::objects::{JClass, JFloatArray, JString};
use jni::sys::{jboolean, jint, jlong};
use jni::JNIEnv;
use pw_graph_relay_sdk::{
    CodecKind, DeviceKind, RelayBrowser, RelayClient, RelayClientBuilder, RelayEvent, RelayHandle,
    RelayHost, RelayHostBuilder, RelayHostPrepared, Role, SessionId, TransportPreference,
    MAX_REALTIME_QUANTUM_SAMPLES,
};
use serde_json::json;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Mutex, OnceLock};

static NEXT_HANDLE: AtomicI64 = AtomicI64::new(1);
static NEXT_OPERATION: AtomicI64 = AtomicI64::new(1);
static CLIENTS: OnceLock<Mutex<HashMap<i64, ClientSlot>>> = OnceLock::new();

thread_local! {
    /// Per-JNI-thread PCM storage. It grows at most once to the realtime
    /// quantum and is filled before the engine call, so native audio methods
    /// do not allocate a Vec on every callback.
    static PCM_SCRATCH: RefCell<Vec<f32>> = RefCell::new(Vec::new());
}

fn clients() -> &'static Mutex<HashMap<i64, ClientSlot>> {
    CLIENTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn string(env: &mut JNIEnv<'_>, value: JString<'_>) -> Result<String, String> {
    env.get_string(&value)
        .map(|value| value.to_string_lossy().into_owned())
        .map_err(|error| error.to_string())
}

fn json_string(
    env: &mut JNIEnv<'_>,
    value: serde_json::Value,
) -> jni::errors::Result<jni::sys::jstring> {
    let text = env.new_string(value.to_string())?;
    Ok(text.into_raw())
}

fn error_json(
    env: &mut JNIEnv<'_>,
    error: impl ToString,
) -> jni::errors::Result<jni::sys::jstring> {
    json_string(env, json!({"type":"error","message":error.to_string()}))
}

fn parse_role(value: &str) -> Result<Role, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "emit" => Ok(Role::Emit),
        "receive" => Ok(Role::Receive),
        "both" => Ok(Role::Both),
        other => Err(format!("unknown client role '{other}'")),
    }
}

fn parse_codec(value: &str) -> Result<CodecKind, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "pcm" => Ok(CodecKind::Pcm),
        "opus" => Ok(CodecKind::Opus),
        other => Err(format!("unknown codec '{other}'")),
    }
}

fn parse_transport(value: &str) -> Result<TransportPreference, String> {
    value.parse()
}

fn positive_u16(name: &str, value: jint) -> Result<u16, String> {
    if value <= 0 {
        return Err(format!("{name} must be positive"));
    }
    u16::try_from(value).map_err(|_| format!("{name} is out of range"))
}

fn positive_u32(name: &str, value: jint) -> Result<u32, String> {
    if value <= 0 {
        return Err(format!("{name} must be positive"));
    }
    u32::try_from(value).map_err(|_| format!("{name} is out of range"))
}

fn port_u16(value: jint) -> Result<u16, String> {
    if value < 0 {
        return Err("port must not be negative".into());
    }
    u16::try_from(value).map_err(|_| "port is out of range".into())
}

fn next_operation() -> i64 {
    NEXT_OPERATION.fetch_add(1, Ordering::Relaxed)
}

fn requested_pcm_length(
    env: &mut JNIEnv<'_>,
    array: &JFloatArray<'_>,
    requested: jint,
) -> Result<Option<usize>, String> {
    if requested < 0 {
        return Err("PCM length must not be negative".into());
    }
    let array_length = env
        .get_array_length(array)
        .map_err(|error| error.to_string())? as usize;
    let requested = usize::try_from(requested).map_err(|_| "PCM length is invalid".to_string())?;
    if requested > array_length {
        return Err("PCM length exceeds the Java array".into());
    }
    if requested > MAX_REALTIME_QUANTUM_SAMPLES {
        return Ok(None);
    }
    Ok(Some(requested))
}

fn client_engine_handle(handle: jlong) -> Result<Option<RelayHandle>, String> {
    let guard = clients()
        .lock()
        .map_err(|_| "client store poisoned".to_string())?;
    Ok(match guard.get(&handle) {
        Some(ClientSlot::Connected(client)) => Some(client.handle()),
        Some(ClientSlot::Prepared(_) | ClientSlot::Connecting { .. }) => None,
        None => return Err("unknown client handle".into()),
    })
}

fn host_engine_handle(handle: jlong) -> Result<Option<RelayHandle>, String> {
    let guard = hosts()
        .lock()
        .map_err(|_| "host store poisoned".to_string())?;
    Ok(match guard.get(&handle) {
        Some(HostSlot::Running(running)) => Some(running.host.handle()),
        Some(HostSlot::Prepared(_) | HostSlot::Starting { .. } | HostSlot::Stopping { .. }) => None,
        None => return Err("unknown host handle".into()),
    })
}

/// JSON snapshot of the first USB tether link, or `{"type":"none"}` when no
/// USB link is up. USB is auto-detected rather than user-selected, matching
/// the desktop panel.
fn usb_link_json() -> serde_json::Value {
    match pw_graph_relay_sdk::LocalLink::find_usb() {
        Some(link) => link_json(&link),
        None => json!({"type": "none"}),
    }
}

/// JSON snapshot of every usable local link, ranked best-first. Lets the UI
/// render the addresses a peer should dial (with the host port appended).
fn local_links_json() -> serde_json::Value {
    let links: Vec<serde_json::Value> = pw_graph_relay_sdk::local_links()
        .iter()
        .map(link_json)
        .collect();
    json!({ "type": "links", "links": links })
}

fn link_json(link: &pw_graph_relay_sdk::LocalLink) -> serde_json::Value {
    json!({
        "type": "usb_link",
        "name": link.name,
        "addr": link.addr.to_string(),
        "kind": link.kind.as_str(),
    })
}

fn event_json(event: RelayEvent) -> serde_json::Value {
    match event {
        RelayEvent::SessionEstablished { id, peer, .. } => json!({
            "type": "connected", "session": id.0, "host": peer.name, "address": peer.addr.to_string()
        }),
        RelayEvent::SessionLost { id, reason } => {
            json!({"type":"disconnected","session":id.0,"message":reason})
        }
        RelayEvent::AudioLevel { id, rms } => json!({
            "type":"level", "session":id.0, "rms":rms
        }),
        RelayEvent::Error { message } => json!({"type":"error","message":message}),
        RelayEvent::PeerDiscovered { peer } => json!({
            "type":"peer","name":peer.name,"address":peer.addr.to_string()
        }),
        RelayEvent::PeerLost { peer } => json!({
            "type":"peer_lost","name":peer.name,"address":peer.addr.to_string()
        }),
        RelayEvent::HostStarted { port } => json!({"type":"host_started","port":port}),
        RelayEvent::HostStopped => json!({"type":"host_stopped"}),
    }
}

#[no_mangle]
pub extern "system" fn Java_io_qpwgraph_relay_NativeBridge_create(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    device_name: JString<'_>,
    role: JString<'_>,
    codec: JString<'_>,
    transport: JString<'_>,
    sample_rate: jint,
    channels: jint,
    frame_ms: jint,
) -> jni::sys::jstring {
    let result = (|| -> Result<serde_json::Value, String> {
        let device_name = string(&mut env, device_name)?;
        let role = parse_role(&string(&mut env, role)?)?;
        let codec = parse_codec(&string(&mut env, codec)?)?;
        let transport = parse_transport(&string(&mut env, transport)?)?;
        let sample_rate = positive_u32("sample rate", sample_rate)?;
        let channels = positive_u16("channels", channels)?;
        let frame_ms = positive_u16("frame duration", frame_ms)?;
        let client = RelayClientBuilder::new()
            .device_name(device_name)
            .device_kind(DeviceKind::Android)
            .role(role)
            .codec(codec)
            .transport(transport)
            .audio(sample_rate, channels, frame_ms)
            .build()
            .map_err(|error| error.to_string())?;
        let handle = NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);
        let mut guard = clients()
            .lock()
            .map_err(|_| "client store poisoned".to_string())?;
        guard.insert(handle, ClientSlot::Prepared(client));
        Ok(json!({"type":"created", "handle":handle}))
    })();
    match result {
        Ok(value) => json_string(&mut env, value).unwrap_or(std::ptr::null_mut()),
        Err(error) => error_json(&mut env, error).unwrap_or(std::ptr::null_mut()),
    }
}

enum ClientSlot {
    Prepared(pw_graph_relay_sdk::RelayClientPrepared),
    Connecting { token: i64 },
    Connected(RelayClient),
}

#[no_mangle]
pub extern "system" fn Java_io_qpwgraph_relay_NativeBridge_connect(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    handle: jlong,
    target: JString<'_>,
    pin: JString<'_>,
) -> jni::sys::jstring {
    let result = (|| -> Result<serde_json::Value, String> {
        let target = string(&mut env, target)?;
        let pin = string(&mut env, pin)?;
        let prepared = {
            let mut guard = clients()
                .lock()
                .map_err(|_| "client store poisoned".to_string())?;
            let prepared = match guard.get(&handle) {
                Some(ClientSlot::Prepared(client)) => client.clone(),
                Some(ClientSlot::Connecting { .. }) => {
                    return Err("client connection is already in progress".into())
                }
                Some(ClientSlot::Connected(_)) => return Err("client is already connected".into()),
                None => return Err("unknown client handle".into()),
            };
            let token = next_operation();
            guard.insert(handle, ClientSlot::Connecting { token });
            (token, prepared)
        };

        let (token, prepared) = prepared;
        // The potentially multi-second resolve/TCP/PAKE/negotiation operation
        // happens with no process-wide registry mutex held.
        let connected = prepared.clone().connect(&target, &pin);
        match connected {
            Ok(client) => {
                let mut guard = clients()
                    .lock()
                    .map_err(|_| "client store poisoned".to_string())?;
                let same_attempt = matches!(
                    guard.get(&handle),
                    Some(ClientSlot::Connecting { token: current, .. }) if *current == token
                );
                if same_attempt {
                    guard.insert(handle, ClientSlot::Connected(client));
                    Ok(json!({"type":"connected"}))
                } else {
                    drop(guard);
                    let _ = client.disconnect();
                    Err("client handle changed while connecting".into())
                }
            }
            Err(error) => {
                let mut guard = clients()
                    .lock()
                    .map_err(|_| "client store poisoned".to_string())?;
                if matches!(
                    guard.get(&handle),
                    Some(ClientSlot::Connecting { token: current, .. }) if *current == token
                ) {
                    // Keep the validated configuration reusable after a
                    // refused, timed-out, or otherwise failed connection.
                    guard.insert(handle, ClientSlot::Prepared(prepared));
                }
                Err(error.to_string())
            }
        }
    })();
    match result {
        Ok(value) => json_string(&mut env, value).unwrap_or(std::ptr::null_mut()),
        Err(error) => error_json(&mut env, error).unwrap_or(std::ptr::null_mut()),
    }
}

#[no_mangle]
pub extern "system" fn Java_io_qpwgraph_relay_NativeBridge_disconnect(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
    handle: jlong,
) -> jboolean {
    let result = clients()
        .lock()
        .ok()
        .and_then(|mut guard| guard.remove(&handle));
    let result = match result {
        Some(ClientSlot::Connected(client)) => {
            client.disconnect().map_err(|error| error.to_string())
        }
        Some(ClientSlot::Prepared(_) | ClientSlot::Connecting { .. }) => Ok(()),
        None => Err("unknown client handle".into()),
    };
    u8::from(result.is_ok())
}

#[no_mangle]
pub extern "system" fn Java_io_qpwgraph_relay_NativeBridge_pollEvents(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    handle: jlong,
) -> jni::sys::jstring {
    let result = (|| -> Result<Vec<serde_json::Value>, String> {
        let engine = client_engine_handle(handle)?;
        Ok(engine
            .map(|engine| {
                engine
                    .events()
                    .into_iter()
                    .map(event_json)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default())
    })();
    match result {
        Ok(events) => json_string(&mut env, json!(events)).unwrap_or(std::ptr::null_mut()),
        Err(error) => error_json(&mut env, error).unwrap_or(std::ptr::null_mut()),
    }
}

#[no_mangle]
pub extern "system" fn Java_io_qpwgraph_relay_NativeBridge_pushCapture(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    handle: jlong,
    samples: JFloatArray<'_>,
    requested: jint,
) -> jint {
    let result = (|| -> Result<jint, String> {
        let Some(length) = requested_pcm_length(&mut env, &samples, requested)? else {
            return Ok(0);
        };
        let Some(engine) = client_engine_handle(handle)? else {
            return Ok(0);
        };
        PCM_SCRATCH.with(|scratch| {
            let mut values = scratch.borrow_mut();
            values.resize(length, 0.0);
            env.get_float_array_region(&samples, 0, &mut values[..])
                .map_err(|error| error.to_string())?;
            if values.iter().any(|value| !value.is_finite()) {
                return Err("PCM contains a non-finite sample".into());
            }
            Ok(if engine.try_push_capture(&values[..]) {
                length as jint
            } else {
                0
            })
        })
    })();
    result.unwrap_or(0)
}

#[no_mangle]
pub extern "system" fn Java_io_qpwgraph_relay_NativeBridge_pullPlayback(
    env: JNIEnv<'_>,
    _class: JClass<'_>,
    handle: jlong,
    output: JFloatArray<'_>,
) -> jint {
    let result = (|| -> Result<jint, String> {
        let length = env
            .get_array_length(&output)
            .map_err(|error| error.to_string())?;
        let length = (length as usize).min(MAX_REALTIME_QUANTUM_SAMPLES);
        let Some(engine) = client_engine_handle(handle)? else {
            return Ok(0);
        };
        PCM_SCRATCH.with(|scratch| {
            let mut values = scratch.borrow_mut();
            values.resize(length, 0.0);
            let count = engine.try_pull_playback(&mut values[..]);
            env.set_float_array_region(&output, 0, &values[..count])
                .map_err(|error| error.to_string())?;
            Ok(count as jint)
        })
    })();
    result.unwrap_or(0)
}

#[no_mangle]
pub extern "system" fn Java_io_qpwgraph_relay_NativeBridge_release(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
    handle: jlong,
) {
    let slot = clients()
        .lock()
        .ok()
        .and_then(|mut guard| guard.remove(&handle));
    if let Some(ClientSlot::Connected(client)) = slot {
        let _ = client.disconnect();
    }
}

// ---------------------------------------------------------------------------
// Host (emitter broadcast) support: an Android device can also be the host
// that other relay peers connect to.
// ---------------------------------------------------------------------------

static HOSTS: OnceLock<Mutex<HashMap<i64, HostSlot>>> = OnceLock::new();

fn hosts() -> &'static Mutex<HashMap<i64, HostSlot>> {
    HOSTS.get_or_init(|| Mutex::new(HashMap::new()))
}

enum HostSlot {
    Prepared(RelayHostPrepared),
    Starting { token: i64 },
    Running(RunningHost),
    Stopping { token: i64 },
}

struct RunningHost {
    host: RelayHost,
    /// Preserve the validated configuration so stop returns this slot to a
    /// restartable prepared state.
    prepared: RelayHostPrepared,
}

#[no_mangle]
pub extern "system" fn Java_io_qpwgraph_relay_NativeBridge_hostCreate(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    device_name: JString<'_>,
    pin: JString<'_>,
    port: jint,
    codec: JString<'_>,
    transport: JString<'_>,
    sample_rate: jint,
    channels: jint,
    frame_ms: jint,
) -> jni::sys::jstring {
    let result = (|| -> Result<serde_json::Value, String> {
        let device_name = string(&mut env, device_name)?;
        let pin = string(&mut env, pin)?;
        let codec = parse_codec(&string(&mut env, codec)?)?;
        let transport = parse_transport(&string(&mut env, transport)?)?;
        let port = port_u16(port)?;
        let sample_rate = positive_u32("sample rate", sample_rate)?;
        let channels = positive_u16("channels", channels)?;
        let frame_ms = positive_u16("frame duration", frame_ms)?;
        let host = RelayHostBuilder::new()
            .device_name(device_name)
            .device_kind(DeviceKind::Android)
            .pin(pin)
            .port(port)
            .codec(codec)
            .transport(transport)
            .audio(sample_rate, channels, frame_ms)
            .build()
            .map_err(|error| error.to_string())?;
        let handle = NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);
        let mut guard = hosts()
            .lock()
            .map_err(|_| "host store poisoned".to_string())?;
        guard.insert(handle, HostSlot::Prepared(host));
        Ok(json!({"type":"created", "handle":handle}))
    })();
    match result {
        Ok(value) => json_string(&mut env, value).unwrap_or(std::ptr::null_mut()),
        Err(error) => error_json(&mut env, error).unwrap_or(std::ptr::null_mut()),
    }
}

#[no_mangle]
pub extern "system" fn Java_io_qpwgraph_relay_NativeBridge_hostStart(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    handle: jlong,
) -> jni::sys::jstring {
    let result = (|| -> Result<serde_json::Value, String> {
        let (token, prepared) = {
            let mut guard = hosts()
                .lock()
                .map_err(|_| "host store poisoned".to_string())?;
            let prepared = match guard.get(&handle) {
                Some(HostSlot::Prepared(prepared)) => prepared.clone(),
                Some(HostSlot::Starting { .. } | HostSlot::Stopping { .. }) => {
                    return Err("host state transition is already in progress".into())
                }
                Some(HostSlot::Running(_)) => return Err("host is already running".into()),
                None => return Err("unknown host handle".into()),
            };
            let token = next_operation();
            guard.insert(handle, HostSlot::Starting { token });
            (token, prepared)
        };

        // Binding and starting the host's accept thread do not run under the
        // process-wide host registry mutex.
        match prepared.clone().start() {
            Ok(host) => {
                let port = host.port();
                let mut guard = hosts()
                    .lock()
                    .map_err(|_| "host store poisoned".to_string())?;
                let same_attempt = matches!(
                    guard.get(&handle),
                    Some(HostSlot::Starting { token: current, .. }) if *current == token
                );
                if same_attempt {
                    guard.insert(handle, HostSlot::Running(RunningHost { host, prepared }));
                    Ok(json!({"type": "host_started", "port": port}))
                } else {
                    drop(guard);
                    let _ = host.handle().host_stop();
                    Err("host handle changed while starting".into())
                }
            }
            Err(error) => {
                let mut guard = hosts()
                    .lock()
                    .map_err(|_| "host store poisoned".to_string())?;
                if matches!(
                    guard.get(&handle),
                    Some(HostSlot::Starting { token: current, .. }) if *current == token
                ) {
                    guard.insert(handle, HostSlot::Prepared(prepared));
                }
                Err(error.to_string())
            }
        }
    })();
    match result {
        Ok(value) => json_string(&mut env, value).unwrap_or(std::ptr::null_mut()),
        Err(error) => error_json(&mut env, error).unwrap_or(std::ptr::null_mut()),
    }
}

#[no_mangle]
pub extern "system" fn Java_io_qpwgraph_relay_NativeBridge_hostStop(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    handle: jlong,
) -> jni::sys::jstring {
    let result = (|| -> Result<serde_json::Value, String> {
        let (token, prepared, host) = {
            let mut guard = hosts()
                .lock()
                .map_err(|_| "host store poisoned".to_string())?;
            match guard.remove(&handle) {
                Some(HostSlot::Running(running)) => {
                    let token = next_operation();
                    let prepared = running.prepared.clone();
                    guard.insert(handle, HostSlot::Stopping { token });
                    (token, prepared, running.host)
                }
                Some(HostSlot::Prepared(prepared)) => {
                    guard.insert(handle, HostSlot::Prepared(prepared));
                    return Ok(json!({"type": "host_stopped"}));
                }
                Some(other @ (HostSlot::Starting { .. } | HostSlot::Stopping { .. })) => {
                    guard.insert(handle, other);
                    return Err("host state transition is already in progress".into());
                }
                None => return Err("unknown host handle".into()),
            }
        };

        // Stop the engine outside the global registry lock. The prepared
        // configuration remains available for a later start.
        let stop_result = host.handle().host_stop().map_err(|error| error.to_string());
        let mut guard = hosts()
            .lock()
            .map_err(|_| "host store poisoned".to_string())?;
        let same_attempt = matches!(
            guard.get(&handle),
            Some(HostSlot::Stopping { token: current, .. }) if *current == token
        );
        if !same_attempt {
            drop(guard);
            return stop_result.map(|()| json!({"type": "host_stopped"}));
        }
        match stop_result {
            Ok(()) => {
                guard.insert(handle, HostSlot::Prepared(prepared));
                Ok(json!({"type": "host_stopped"}))
            }
            Err(error) => {
                guard.insert(handle, HostSlot::Running(RunningHost { host, prepared }));
                Err(error)
            }
        }
    })();
    match result {
        Ok(value) => json_string(&mut env, value).unwrap_or(std::ptr::null_mut()),
        Err(error) => error_json(&mut env, error).unwrap_or(std::ptr::null_mut()),
    }
}

#[no_mangle]
pub extern "system" fn Java_io_qpwgraph_relay_NativeBridge_hostPollEvents(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    handle: jlong,
) -> jni::sys::jstring {
    let result = (|| -> Result<Vec<serde_json::Value>, String> {
        let engine = host_engine_handle(handle)?;
        Ok(engine
            .map(|engine| {
                engine
                    .events()
                    .into_iter()
                    .map(event_json)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default())
    })();
    match result {
        Ok(events) => json_string(&mut env, json!(events)).unwrap_or(std::ptr::null_mut()),
        Err(error) => error_json(&mut env, error).unwrap_or(std::ptr::null_mut()),
    }
}

#[no_mangle]
pub extern "system" fn Java_io_qpwgraph_relay_NativeBridge_hostStatus(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    handle: jlong,
) -> jni::sys::jstring {
    let result = (|| -> Result<serde_json::Value, String> {
        let engine = host_engine_handle(handle)?;
        let Some(engine) = engine else {
            return Ok(json!({
                "type": "status",
                "host_active": false,
                "port": null,
                "address": null,
                "sessions": [],
            }));
        };
        let status = engine.status();
        let sessions = status
            .sessions
            .iter()
            .map(|session| {
                json!({
                    "id": session.id.0,
                    "name": session.peer.name,
                    "address": session.peer.addr.to_string(),
                    "sending": session.sending,
                    "receiving": session.receiving,
                })
            })
            .collect::<Vec<_>>();
        Ok(json!({
            "type": "status",
            "host_active": status.host_active,
            "port": status.host_port,
            "address": status.host_addr.map(|address| address.to_string()),
            "sessions": sessions,
        }))
    })();
    match result {
        Ok(value) => json_string(&mut env, value).unwrap_or(std::ptr::null_mut()),
        Err(error) => error_json(&mut env, error).unwrap_or(std::ptr::null_mut()),
    }
}

#[no_mangle]
pub extern "system" fn Java_io_qpwgraph_relay_NativeBridge_hostDisconnectSession(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    handle: jlong,
    session: jlong,
) -> jni::sys::jstring {
    let result = (|| -> Result<serde_json::Value, String> {
        let engine =
            host_engine_handle(handle)?.ok_or_else(|| "host is not running".to_string())?;
        if session < 0 {
            return Err("session id must not be negative".into());
        }
        engine
            .disconnect(SessionId(session as u64))
            .map(|()| json!({"type": "disconnecting", "session": session}))
            .map_err(|error| error.to_string())
    })();
    match result {
        Ok(value) => json_string(&mut env, value).unwrap_or(std::ptr::null_mut()),
        Err(error) => error_json(&mut env, error).unwrap_or(std::ptr::null_mut()),
    }
}

#[no_mangle]
pub extern "system" fn Java_io_qpwgraph_relay_NativeBridge_hostPushCapture(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    handle: jlong,
    samples: JFloatArray<'_>,
    requested: jint,
) -> jint {
    let result = (|| -> Result<jint, String> {
        let Some(length) = requested_pcm_length(&mut env, &samples, requested)? else {
            return Ok(0);
        };
        let Some(engine) = host_engine_handle(handle)? else {
            return Ok(0);
        };
        PCM_SCRATCH.with(|scratch| {
            let mut values = scratch.borrow_mut();
            values.resize(length, 0.0);
            env.get_float_array_region(&samples, 0, &mut values[..])
                .map_err(|error| error.to_string())?;
            if values.iter().any(|value| !value.is_finite()) {
                return Err("PCM contains a non-finite sample".into());
            }
            Ok(if engine.try_push_capture(&values[..]) {
                length as jint
            } else {
                0
            })
        })
    })();
    result.unwrap_or(0)
}

#[no_mangle]
pub extern "system" fn Java_io_qpwgraph_relay_NativeBridge_hostPullPlayback(
    env: JNIEnv<'_>,
    _class: JClass<'_>,
    handle: jlong,
    output: JFloatArray<'_>,
) -> jint {
    let result = (|| -> Result<jint, String> {
        let length = env
            .get_array_length(&output)
            .map_err(|error| error.to_string())?;
        let length = (length as usize).min(MAX_REALTIME_QUANTUM_SAMPLES);
        let Some(engine) = host_engine_handle(handle)? else {
            return Ok(0);
        };
        PCM_SCRATCH.with(|scratch| {
            let mut values = scratch.borrow_mut();
            values.resize(length, 0.0);
            let count = engine.try_pull_playback(&mut values[..]);
            env.set_float_array_region(&output, 0, &values[..count])
                .map_err(|error| error.to_string())?;
            Ok(count as jint)
        })
    })();
    result.unwrap_or(0)
}

#[no_mangle]
pub extern "system" fn Java_io_qpwgraph_relay_NativeBridge_hostRelease(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
    handle: jlong,
) {
    let slot = hosts()
        .lock()
        .ok()
        .and_then(|mut guard| guard.remove(&handle));
    if let Some(HostSlot::Running(running)) = slot {
        let _ = running.host.handle().host_stop();
    }
}

// ---------------------------------------------------------------------------
// Discovery support: browse the LAN for relay hosts from Android.
// ---------------------------------------------------------------------------

static BROWSERS: OnceLock<Mutex<HashMap<i64, RelayBrowser>>> = OnceLock::new();

fn browsers() -> &'static Mutex<HashMap<i64, RelayBrowser>> {
    BROWSERS.get_or_init(|| Mutex::new(HashMap::new()))
}

#[no_mangle]
pub extern "system" fn Java_io_qpwgraph_relay_NativeBridge_discoveryCreate(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    device_name: JString<'_>,
) -> jni::sys::jstring {
    let result = (|| -> Result<serde_json::Value, String> {
        let device_name = string(&mut env, device_name)?;
        let browser = RelayBrowser::start(device_name).map_err(|error| error.to_string())?;
        let handle = NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);
        let mut guard = browsers()
            .lock()
            .map_err(|_| "browser store poisoned".to_string())?;
        guard.insert(handle, browser);
        Ok(json!({"type":"created", "handle":handle}))
    })();
    match result {
        Ok(value) => json_string(&mut env, value).unwrap_or(std::ptr::null_mut()),
        Err(error) => error_json(&mut env, error).unwrap_or(std::ptr::null_mut()),
    }
}

#[no_mangle]
pub extern "system" fn Java_io_qpwgraph_relay_NativeBridge_discoveryStart(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    handle: jlong,
) -> jni::sys::jstring {
    let result = (|| -> Result<serde_json::Value, String> {
        let engine = {
            let guard = browsers()
                .lock()
                .map_err(|_| "browser store poisoned".to_string())?;
            guard
                .get(&handle)
                .ok_or_else(|| "unknown discovery handle".to_string())?
                .handle()
        };
        engine
            .discovery_start()
            .map(|()| json!({"type": "discovery_started"}))
            .map_err(|error| error.to_string())
    })();
    match result {
        Ok(value) => json_string(&mut env, value).unwrap_or(std::ptr::null_mut()),
        Err(error) => error_json(&mut env, error).unwrap_or(std::ptr::null_mut()),
    }
}

#[no_mangle]
pub extern "system" fn Java_io_qpwgraph_relay_NativeBridge_discoveryStop(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    handle: jlong,
) -> jni::sys::jstring {
    let result = (|| -> Result<serde_json::Value, String> {
        let engine = {
            let guard = browsers()
                .lock()
                .map_err(|_| "browser store poisoned".to_string())?;
            guard
                .get(&handle)
                .ok_or_else(|| "unknown discovery handle".to_string())?
                .handle()
        };
        engine.discovery_stop();
        Ok(json!({"type": "discovery_stopped"}))
    })();
    match result {
        Ok(value) => json_string(&mut env, value).unwrap_or(std::ptr::null_mut()),
        Err(error) => error_json(&mut env, error).unwrap_or(std::ptr::null_mut()),
    }
}

#[no_mangle]
pub extern "system" fn Java_io_qpwgraph_relay_NativeBridge_discoveryPeers(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    handle: jlong,
) -> jni::sys::jstring {
    let result = (|| -> Result<serde_json::Value, String> {
        let engine = {
            let guard = browsers()
                .lock()
                .map_err(|_| "browser store poisoned".to_string())?;
            guard
                .get(&handle)
                .ok_or_else(|| "unknown discovery handle".to_string())?
                .handle()
        };
        let peers = engine
            .discovered_peers()
            .into_iter()
            .map(|peer| json!({"name": peer.name, "address": peer.addr.to_string()}))
            .collect::<Vec<_>>();
        Ok(json!(peers))
    })();
    match result {
        Ok(value) => json_string(&mut env, value).unwrap_or(std::ptr::null_mut()),
        Err(error) => error_json(&mut env, error).unwrap_or(std::ptr::null_mut()),
    }
}

#[no_mangle]
pub extern "system" fn Java_io_qpwgraph_relay_NativeBridge_discoveryPollEvents(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    handle: jlong,
) -> jni::sys::jstring {
    let result = (|| -> Result<serde_json::Value, String> {
        let engine = {
            let guard = browsers()
                .lock()
                .map_err(|_| "browser store poisoned".to_string())?;
            guard
                .get(&handle)
                .ok_or_else(|| "unknown discovery handle".to_string())?
                .handle()
        };
        let events = engine
            .events()
            .into_iter()
            .map(event_json)
            .collect::<Vec<_>>();
        Ok(json!(events))
    })();
    match result {
        Ok(value) => json_string(&mut env, value).unwrap_or(std::ptr::null_mut()),
        Err(error) => error_json(&mut env, error).unwrap_or(std::ptr::null_mut()),
    }
}

#[no_mangle]
pub extern "system" fn Java_io_qpwgraph_relay_NativeBridge_discoveryRelease(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
    handle: jlong,
) {
    let browser = browsers()
        .lock()
        .ok()
        .and_then(|mut guard| guard.remove(&handle));
    if let Some(browser) = browser {
        browser.shutdown();
    }
}

// ---------------------------------------------------------------------------
// Link detection: report an active USB tether so the UI can show it without
// exposing USB as a manual transport choice.
// ---------------------------------------------------------------------------

#[no_mangle]
pub extern "system" fn Java_io_qpwgraph_relay_NativeBridge_usbLink(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jni::sys::jstring {
    json_string(&mut env, usb_link_json()).unwrap_or(std::ptr::null_mut())
}

#[no_mangle]
pub extern "system" fn Java_io_qpwgraph_relay_NativeBridge_localLinks(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jni::sys::jstring {
    json_string(&mut env, local_links_json()).unwrap_or(std::ptr::null_mut())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_android_client_options() {
        assert_eq!(parse_role("emit").unwrap(), Role::Emit);
        assert_eq!(parse_role("receive").unwrap(), Role::Receive);
        assert_eq!(parse_role("both").unwrap(), Role::Both);
        assert_eq!(parse_codec("pcm").unwrap(), CodecKind::Pcm);
        assert_eq!(parse_codec("opus").unwrap(), CodecKind::Opus);
        assert_eq!(parse_transport("wifi").unwrap(), TransportPreference::Wifi);
    }

    #[test]
    fn invalid_android_enum_options_are_errors_instead_of_defaults() {
        assert!(parse_role("not-a-role").is_err());
        assert!(parse_codec("not-a-codec").is_err());
        assert!(parse_transport("not-a-transport").is_err());
    }

    #[test]
    fn jint_audio_values_are_checked_before_narrowing() {
        for value in [0, -1, -65536] {
            assert!(positive_u16("channels", value).is_err());
            assert!(positive_u32("sample rate", value).is_err());
        }
        for value in [65_536, 65_538] {
            assert!(positive_u16("channels", value).is_err());
        }
        assert_eq!(positive_u16("channels", 1).unwrap(), 1);
        assert_eq!(positive_u16("channels", 2).unwrap(), 2);
        assert_eq!(positive_u32("sample rate", 48_000).unwrap(), 48_000);
        assert_eq!(port_u16(0).unwrap(), 0);
        assert!(port_u16(-1).is_err());
        assert!(port_u16(65_536).is_err());
    }

    #[test]
    fn checked_frame_values_reach_builder_validation_unchanged() {
        for frame_ms in [5, 10, 20, 40, 60] {
            let frame_ms = positive_u16("frame duration", frame_ms).unwrap();
            assert!(RelayClientBuilder::new()
                .audio(48_000, 1, frame_ms)
                .build()
                .is_ok());
        }
        for frame_ms in [1, 7, 61] {
            let frame_ms = positive_u16("frame duration", frame_ms).unwrap();
            assert!(RelayClientBuilder::new()
                .audio(48_000, 1, frame_ms)
                .build()
                .is_err());
        }
    }

    #[test]
    fn pcm_scratch_capacity_is_reused_after_the_first_quantum() {
        PCM_SCRATCH.with(|scratch| {
            let mut scratch = scratch.borrow_mut();
            scratch.clear();
            scratch.resize(MAX_REALTIME_QUANTUM_SAMPLES, 0.0);
            let capacity = scratch.capacity();
            for _ in 0..16 {
                scratch.resize(MAX_REALTIME_QUANTUM_SAMPLES, 0.0);
            }
            assert_eq!(scratch.capacity(), capacity);
        });
    }

    #[test]
    fn usb_link_json_is_well_formed_without_a_tether() {
        // A desktop test box normally has no USB tether up; whatever the
        // result, it must be a JSON object with a `type` field.
        let value = usb_link_json();
        let kind = value.get("type").and_then(|field| field.as_str());
        assert!(matches!(kind, Some("usb_link") | Some("none")));
        if kind == Some("usb_link") {
            assert!(value.get("name").is_some());
            assert!(value.get("addr").is_some());
        }
    }

    #[test]
    fn local_links_json_lists_every_link_with_kind() {
        let value = local_links_json();
        assert_eq!(
            value.get("type").and_then(|field| field.as_str()),
            Some("links")
        );
        let links = value
            .get("links")
            .and_then(|field| field.as_array())
            .unwrap();
        for link in links {
            assert!(matches!(
                link.get("kind").and_then(|field| field.as_str()),
                Some("usb") | Some("wifi") | Some("bluetooth") | Some("lan")
            ));
            assert!(link.get("name").is_some());
            assert!(link.get("addr").is_some());
        }
    }
}
