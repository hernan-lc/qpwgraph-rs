use jni::objects::{JClass, JFloatArray, JString};
use jni::sys::{jboolean, jint, jlong};
use jni::JNIEnv;
use pw_graph_relay_sdk::{CodecKind, DeviceKind, RelayClient, RelayClientBuilder, RelayEvent, Role, TransportPreference};
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

fn json_string(env: &mut JNIEnv<'_>, value: serde_json::Value) -> jni::errors::Result<jni::sys::jstring> {
    let text = env.new_string(value.to_string())?;
    Ok(text.into_raw())
}

fn error_json(env: &mut JNIEnv<'_>, error: impl ToString) -> jni::errors::Result<jni::sys::jstring> {
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
            .audio(sample_rate.max(1) as u32, channels.max(1) as u16, frame_ms.max(1) as u16)
            .build()
            .map_err(|error| error.to_string())?;
        let handle = NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);
        let mut guard = clients().lock().map_err(|_| "client store poisoned".to_string())?;
        guard.insert(handle, ClientSlot::Prepared(client));
        Ok(handle)
    })();
    match value {
        Ok(handle) => handle,
        Err(_) => 0,
    }
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
        let mut guard = clients().lock().map_err(|_| "client store poisoned".to_string())?;
        let slot = guard.remove(&handle).ok_or_else(|| "unknown client handle".to_string())?;
        let ClientSlot::Prepared(client) = slot else {
            return Err("client is already connected".into());
        };
        match client.connect(&target, &pin) {
            Ok(client) => {
                guard.insert(handle, ClientSlot::Connected(client));
                Ok(json!({"type":"connected"}))
            }
            Err(error) => {
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
                None => return Err::<Vec<serde_json::Value>, String>("unknown client handle".into()),
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
        let guard = clients().lock().map_err(|_| "client store poisoned".to_string())?;
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
        let guard = clients().lock().map_err(|_| "client store poisoned".to_string())?;
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
        if let Some(slot) = guard.remove(&handle) {
            if let ClientSlot::Connected(client) = slot {
                let _ = client.disconnect();
            }
        }
    }
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
}
