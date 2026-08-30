use jni::objects::{JClass, JFloatArray, JString};
use jni::sys::{jboolean, jint, jlong};
use jni::JNIEnv;
use pw_graph_relay_sdk::{
    CodecKind, DeviceKind, RelayBrowser, RelayClient, RelayClientBuilder, RelayEvent, RelayHost,
    RelayHostBuilder, RelayHostPrepared, Role, SessionId, TransportPreference,
};
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Mutex, OnceLock};

static NEXT_HANDLE: AtomicI64 = AtomicI64::new(1);
static CLIENTS: OnceLock<Mutex<HashMap<i64, ClientSlot>>> = OnceLock::new();

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

fn parse_role(value: &str) -> Role {
    match value.trim().to_ascii_lowercase().as_str() {
        "receive" => Role::Receive,
        "both" => Role::Both,
        _ => Role::Emit,
    }
}

fn parse_codec(value: &str) -> CodecKind {
    if value.eq_ignore_ascii_case("pcm") {
        CodecKind::Pcm
    } else {
        CodecKind::Opus
    }
}

fn parse_transport(value: &str) -> TransportPreference {
    value.parse().unwrap_or_default()
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
) -> jlong {
    let value = (|| -> Result<jlong, String> {
        let device_name = string(&mut env, device_name)?;
        let role = parse_role(&string(&mut env, role)?);
        let codec = parse_codec(&string(&mut env, codec)?);
        let transport = parse_transport(&string(&mut env, transport)?);
        let client = RelayClientBuilder::new()
            .device_name(device_name)
            .device_kind(DeviceKind::Android)
            .role(role)
            .codec(codec)
            .transport(transport)
            .audio(
                sample_rate.max(1) as u32,
                channels.max(1) as u16,
                frame_ms.max(1) as u16,
            )
            .build()
            .map_err(|error| error.to_string())?;
        let handle = NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);
        let mut guard = clients()
            .lock()
            .map_err(|_| "client store poisoned".to_string())?;
        guard.insert(handle, ClientSlot::Prepared(client));
        Ok(handle)
    })();
    value.unwrap_or_default()
}

enum ClientSlot {
    Prepared(pw_graph_relay_sdk::RelayClientPrepared),
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
        let mut guard = clients()
            .lock()
            .map_err(|_| "client store poisoned".to_string())?;
        // Clone the prepared configuration rather than consuming the stored
        // one: a failed connect must leave the handle usable, or the Java
        // side has to throw away its native client after every transient
        // network error and build a new one.
        let prepared = match guard.get(&handle) {
            Some(ClientSlot::Prepared(client)) => client.clone(),
            Some(ClientSlot::Connected(_)) => return Err("client is already connected".into()),
            None => return Err("unknown client handle".into()),
        };
        match prepared.connect(&target, &pin) {
            Ok(client) => {
                guard.insert(handle, ClientSlot::Connected(client));
                Ok(json!({"type":"connected"}))
            }
            Err(error) => Err(error.to_string()),
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
        .and_then(|mut guard| guard.remove(&handle))
        .map(|slot| match slot {
            ClientSlot::Prepared(_) => Ok(()),
            ClientSlot::Connected(client) => client.disconnect().map_err(|error| error.to_string()),
        });
    u8::from(matches!(result, Some(Ok(()))))
}

#[no_mangle]
pub extern "system" fn Java_io_qpwgraph_relay_NativeBridge_pollEvents(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    handle: jlong,
) -> jni::sys::jstring {
    let result = clients()
        .lock()
        .map_err(|_| "client store poisoned".to_string())
        .map(|mut guard| {
            Ok(match guard.get_mut(&handle) {
                Some(ClientSlot::Connected(client)) => client
                    .events()
                    .into_iter()
                    .map(event_json)
                    .collect::<Vec<_>>(),
                Some(ClientSlot::Prepared(_)) => Vec::new(),
                None => {
                    return Err::<Vec<serde_json::Value>, String>("unknown client handle".into())
                }
            })
        });
    match result {
        Ok(Ok(events)) => json_string(&mut env, json!(events)).unwrap_or(std::ptr::null_mut()),
        Ok(Err(error)) => error_json(&mut env, error).unwrap_or(std::ptr::null_mut()),
        Err(error) => error_json(&mut env, error).unwrap_or(std::ptr::null_mut()),
    }
}

#[no_mangle]
pub extern "system" fn Java_io_qpwgraph_relay_NativeBridge_pushCapture(
    env: JNIEnv<'_>,
    _class: JClass<'_>,
    handle: jlong,
    samples: JFloatArray<'_>,
) -> jint {
    let result = (|| -> Result<jint, String> {
        let length = env
            .get_array_length(&samples)
            .map_err(|error| error.to_string())?;
        let mut values = vec![0.0f32; length as usize];
        env.get_float_array_region(&samples, 0, &mut values)
            .map_err(|error| error.to_string())?;
        if values.iter().any(|value| !value.is_finite()) {
            return Err("PCM contains a non-finite sample".into());
        }
        let guard = clients()
            .lock()
            .map_err(|_| "client store poisoned".to_string())?;
        let Some(ClientSlot::Connected(client)) = guard.get(&handle) else {
            return Ok(0);
        };
        client.send_capture(&values);
        Ok(values.len() as jint)
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
        let mut values = vec![0.0f32; length as usize];
        let guard = clients()
            .lock()
            .map_err(|_| "client store poisoned".to_string())?;
        let Some(ClientSlot::Connected(client)) = guard.get(&handle) else {
            return Ok(0);
        };
        let count = client.pull_playback(&mut values);
        drop(guard);
        env.set_float_array_region(&output, 0, &values[..count])
            .map_err(|error| error.to_string())?;
        Ok(count as jint)
    })();
    result.unwrap_or(0)
}

#[no_mangle]
pub extern "system" fn Java_io_qpwgraph_relay_NativeBridge_release(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
    handle: jlong,
) {
    if let Ok(mut guard) = clients().lock() {
        if let Some(ClientSlot::Connected(client)) = guard.remove(&handle) {
            let _ = client.disconnect();
        }
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
    Running(RelayHost),
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
) -> jlong {
    let value = (|| -> Result<jlong, String> {
        let device_name = string(&mut env, device_name)?;
        let pin = string(&mut env, pin)?;
        let codec = parse_codec(&string(&mut env, codec)?);
        let transport = parse_transport(&string(&mut env, transport)?);
        let host = RelayHostBuilder::new()
            .device_name(device_name)
            .device_kind(DeviceKind::Android)
            .pin(pin)
            .port(port.clamp(0, 65535) as u16)
            .codec(codec)
            .transport(transport)
            .audio(
                sample_rate.max(1) as u32,
                channels.max(1) as u16,
                frame_ms.max(1) as u16,
            )
            .build()
            .map_err(|error| error.to_string())?;
        let handle = NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);
        let mut guard = hosts()
            .lock()
            .map_err(|_| "host store poisoned".to_string())?;
        guard.insert(handle, HostSlot::Prepared(host));
        Ok(handle)
    })();
    value.unwrap_or_default()
}

#[no_mangle]
pub extern "system" fn Java_io_qpwgraph_relay_NativeBridge_hostStart(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    handle: jlong,
) -> jni::sys::jstring {
    let result = (|| -> Result<serde_json::Value, String> {
        let mut guard = hosts()
            .lock()
            .map_err(|_| "host store poisoned".to_string())?;
        // Same reasoning as the client: a host whose first `start` fails
        // (port already in use, say) must stay startable.
        let prepared = match guard.get(&handle) {
            Some(HostSlot::Prepared(prepared)) => prepared.clone(),
            Some(HostSlot::Running(_)) => return Err("host is already running".into()),
            None => return Err("unknown host handle".into()),
        };
        match prepared.start() {
            Ok(host) => {
                let port = host.port();
                guard.insert(handle, HostSlot::Running(host));
                Ok(json!({"type": "host_started", "port": port}))
            }
            Err(error) => Err(error.to_string()),
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
    let removed = hosts()
        .lock()
        .ok()
        .and_then(|mut guard| guard.remove(&handle))
        .is_some();
    let value = if removed {
        json!({"type": "host_stopped"})
    } else {
        json!({"type": "error", "message": "unknown host handle"})
    };
    json_string(&mut env, value).unwrap_or(std::ptr::null_mut())
}

#[no_mangle]
pub extern "system" fn Java_io_qpwgraph_relay_NativeBridge_hostPollEvents(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    handle: jlong,
) -> jni::sys::jstring {
    let result = hosts()
        .lock()
        .map_err(|_| "host store poisoned".to_string())
        .map(|mut guard| {
            Ok(match guard.get_mut(&handle) {
                Some(HostSlot::Running(host)) => host
                    .events()
                    .into_iter()
                    .map(event_json)
                    .collect::<Vec<_>>(),
                Some(HostSlot::Prepared(_)) => Vec::new(),
                None => return Err::<Vec<serde_json::Value>, String>("unknown host handle".into()),
            })
        });
    match result {
        Ok(Ok(events)) => json_string(&mut env, json!(events)).unwrap_or(std::ptr::null_mut()),
        Ok(Err(error)) => error_json(&mut env, error).unwrap_or(std::ptr::null_mut()),
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
        let guard = hosts()
            .lock()
            .map_err(|_| "host store poisoned".to_string())?;
        match guard.get(&handle) {
            Some(HostSlot::Running(host)) => {
                let status = host.status();
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
                    "sessions": sessions,
                }))
            }
            Some(HostSlot::Prepared(_)) => Ok(json!({
                "type": "status",
                "host_active": false,
                "port": null,
                "sessions": [],
            })),
            None => Err("unknown host handle".into()),
        }
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
        let guard = hosts()
            .lock()
            .map_err(|_| "host store poisoned".to_string())?;
        match guard.get(&handle) {
            Some(HostSlot::Running(host)) => host
                .disconnect(SessionId(session.max(0) as u64))
                .map(|()| json!({"type": "disconnecting", "session": session}))
                .map_err(|error| error.to_string()),
            Some(HostSlot::Prepared(_)) => Err("host is not running".into()),
            None => Err("unknown host handle".into()),
        }
    })();
    match result {
        Ok(value) => json_string(&mut env, value).unwrap_or(std::ptr::null_mut()),
        Err(error) => error_json(&mut env, error).unwrap_or(std::ptr::null_mut()),
    }
}

#[no_mangle]
pub extern "system" fn Java_io_qpwgraph_relay_NativeBridge_hostPushCapture(
    env: JNIEnv<'_>,
    _class: JClass<'_>,
    handle: jlong,
    samples: JFloatArray<'_>,
) -> jint {
    let result = (|| -> Result<jint, String> {
        let length = env
            .get_array_length(&samples)
            .map_err(|error| error.to_string())?;
        let mut values = vec![0.0f32; length as usize];
        env.get_float_array_region(&samples, 0, &mut values)
            .map_err(|error| error.to_string())?;
        if values.iter().any(|value| !value.is_finite()) {
            return Err("PCM contains a non-finite sample".into());
        }
        let guard = hosts()
            .lock()
            .map_err(|_| "host store poisoned".to_string())?;
        let Some(HostSlot::Running(host)) = guard.get(&handle) else {
            return Ok(0);
        };
        host.push_capture(&values);
        Ok(values.len() as jint)
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
        let mut values = vec![0.0f32; length as usize];
        let guard = hosts()
            .lock()
            .map_err(|_| "host store poisoned".to_string())?;
        let Some(HostSlot::Running(host)) = guard.get(&handle) else {
            return Ok(0);
        };
        let count = host.pull_playback(&mut values);
        drop(guard);
        env.set_float_array_region(&output, 0, &values[..count])
            .map_err(|error| error.to_string())?;
        Ok(count as jint)
    })();
    result.unwrap_or(0)
}

#[no_mangle]
pub extern "system" fn Java_io_qpwgraph_relay_NativeBridge_hostRelease(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
    handle: jlong,
) {
    if let Ok(mut guard) = hosts().lock() {
        guard.remove(&handle);
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
) -> jlong {
    let value = (|| -> Result<jlong, String> {
        let device_name = string(&mut env, device_name)?;
        let browser = RelayBrowser::start(device_name).map_err(|error| error.to_string())?;
        let handle = NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);
        let mut guard = browsers()
            .lock()
            .map_err(|_| "browser store poisoned".to_string())?;
        guard.insert(handle, browser);
        Ok(handle)
    })();
    value.unwrap_or_default()
}

#[no_mangle]
pub extern "system" fn Java_io_qpwgraph_relay_NativeBridge_discoveryStart(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    handle: jlong,
) -> jni::sys::jstring {
    let result = (|| -> Result<serde_json::Value, String> {
        let guard = browsers()
            .lock()
            .map_err(|_| "browser store poisoned".to_string())?;
        let browser = guard
            .get(&handle)
            .ok_or_else(|| "unknown discovery handle".to_string())?;
        browser
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
        let guard = browsers()
            .lock()
            .map_err(|_| "browser store poisoned".to_string())?;
        let browser = guard
            .get(&handle)
            .ok_or_else(|| "unknown discovery handle".to_string())?;
        browser.discovery_stop();
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
        let guard = browsers()
            .lock()
            .map_err(|_| "browser store poisoned".to_string())?;
        let browser = guard
            .get(&handle)
            .ok_or_else(|| "unknown discovery handle".to_string())?;
        let peers = browser
            .peers()
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
        let guard = browsers()
            .lock()
            .map_err(|_| "browser store poisoned".to_string())?;
        let browser = guard
            .get(&handle)
            .ok_or_else(|| "unknown discovery handle".to_string())?;
        let events = browser
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
    if let Ok(mut guard) = browsers().lock() {
        if let Some(browser) = guard.remove(&handle) {
            browser.shutdown();
        }
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
        assert_eq!(parse_role("receive"), Role::Receive);
        assert_eq!(parse_role("both"), Role::Both);
        assert_eq!(parse_codec("pcm"), CodecKind::Pcm);
        assert_eq!(parse_transport("wifi"), TransportPreference::Wifi);
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
