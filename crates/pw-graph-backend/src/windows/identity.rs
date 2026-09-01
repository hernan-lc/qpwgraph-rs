//! Turning Core Audio's names and GUIDs into stable graph identities.
//!
//! Windows has no persistent numeric ids for endpoints or sessions, so every
//! graph id here is derived from a stable string. The same endpoint has to
//! keep its id across a rebuild or the UI would lose selection and layout on
//! every refresh.

use super::*;

pub(super) fn native_error(operation: &str, error: impl std::fmt::Display) -> BackendError {
    BackendError::Native(format!("{operation} failed: {error}"))
}

pub(super) fn graph_id(local_id: u64) -> u64 {
    encode_backend_id(BackendNamespace::WindowsAudio, local_id)
}

pub(super) fn endpoint_direction(flow: Audio::EDataFlow) -> Direction {
    if flow == Audio::eRender {
        Direction::Sink
    } else {
        Direction::Source
    }
}

pub(super) fn session_direction(flow: Audio::EDataFlow) -> Direction {
    if flow == Audio::eRender {
        Direction::Source
    } else {
        Direction::Sink
    }
}

pub(super) fn session_link_ports(
    flow: Audio::EDataFlow,
    session_port: PortId,
    endpoint_port: PortId,
) -> (PortId, PortId) {
    if flow == Audio::eRender {
        (session_port, endpoint_port)
    } else {
        (endpoint_port, session_port)
    }
}

pub(super) fn stable_local_id(value: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    let local = hash & LOCAL_ID_MASK;
    if local == 0 {
        1
    } else {
        local
    }
}

pub(super) fn endpoint_node_local_id(endpoint_id: &str) -> u64 {
    stable_local_id(&format!("endpoint-node:{endpoint_id}"))
}

pub(super) fn endpoint_port_local_id(endpoint_id: &str) -> u64 {
    stable_local_id(&format!("endpoint-port:{endpoint_id}"))
}

/// A playback endpoint's monitor port: what that endpoint is playing, read
/// back through WASAPI loopback.
///
/// PipeWire gives every sink a monitor, and it is what makes "send the
/// speakers somewhere else" a link the user can draw rather than a hidden
/// setting. Windows has the same capability through loopback capture, so the
/// port exists here for the same reason.
pub(super) fn endpoint_monitor_port_local_id(endpoint_id: &str) -> u64 {
    stable_local_id(&format!("endpoint-monitor-port:{endpoint_id}"))
}

/// A link qpwgraph itself owns, as opposed to a relationship Core Audio
/// merely reports. Derived from the pair so the same route keeps its identity
/// across a rebuild.
pub(super) fn managed_link_local_id(output: PortId, input: PortId) -> u64 {
    stable_local_id(&format!("route-link:{}:{}", output.0, input.0))
}

pub(super) fn session_node_local_id(endpoint_id: &str, session_id: &str) -> u64 {
    stable_local_id(&format!("session-node:{endpoint_id}:{session_id}"))
}

pub(super) fn session_port_local_id(endpoint_id: &str, session_id: &str) -> u64 {
    stable_local_id(&format!("session-port:{endpoint_id}:{session_id}"))
}

pub(super) fn session_link_local_id(endpoint_id: &str, session_id: &str) -> u64 {
    stable_local_id(&format!("session-link:{endpoint_id}:{session_id}"))
}

pub(super) fn take_pwstr(value: PWSTR) -> String {
    let text = unsafe { value.to_string() }.unwrap_or_default();
    unsafe { Com::CoTaskMemFree(Some(value.0 as *mut _)) };
    text
}

pub(super) fn endpoint_name(device: &Audio::IMMDevice) -> Option<String> {
    unsafe {
        property_string(
            device,
            &Properties::DEVPKEY_Device_FriendlyName as *const _ as *const _,
        )
    }
}

pub(super) unsafe fn property_string(
    device: &Audio::IMMDevice,
    key: *const PROPERTYKEY,
) -> Option<String> {
    let store: IPropertyStore = device.OpenPropertyStore(STGM_READ).ok()?;
    let mut value = store.GetValue(key).ok()?;
    let prop_variant = &value.Anonymous.Anonymous;
    if prop_variant.vt != VT_LPWSTR {
        let _ = StructuredStorage::PropVariantClear(&mut value);
        return None;
    }
    let ptr = *(&prop_variant.Anonymous as *const _ as *const *const u16);
    if ptr.is_null() {
        let _ = StructuredStorage::PropVariantClear(&mut value);
        return None;
    }
    let mut length = 0usize;
    while length < 32_768 && *ptr.add(length) != 0 {
        length += 1;
    }
    let text = if length == 32_768 {
        None
    } else {
        Some(
            OsString::from_wide(std::slice::from_raw_parts(ptr, length))
                .to_string_lossy()
                .into_owned(),
        )
    };
    let _ = StructuredStorage::PropVariantClear(&mut value);
    text
}

pub(super) fn process_name(process_id: u32) -> Option<String> {
    if process_id == 0 {
        return None;
    }
    let process =
        unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id) }.ok()?;
    let mut buffer = [0u16; 512];
    let mut length = buffer.len() as u32;
    let result = unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut length,
        )
    };
    let _ = unsafe { CloseHandle(process) };
    result.ok()?;
    let path = OsString::from_wide(&buffer[..length as usize]);
    let name = std::path::Path::new(&path).file_stem()?.to_string_lossy();
    (!name.is_empty()).then(|| name.into_owned())
}
