//! Windows Core Audio backend.
//!
//! Core Audio exposes endpoint and application-session state, but it does not
//! expose PipeWire's arbitrary patchbay graph. This driver therefore presents
//! the relationships Windows reports as an observed graph and deliberately
//! rejects topology mutations. All COM interfaces stay on the worker thread;
//! the public driver communicates with that thread through owned commands and
//! snapshots.
//!
//! | Module | Owns |
//! | --- | --- |
//! | [`driver`] | the public `GraphDriver`, which owns no COM pointer |
//! | [`worker`] | the Core Audio thread: enumeration, meters, the graph |
//! | [`callbacks`] | the COM notification sinks Core Audio calls back on |
//! | [`identity`] | stable graph ids derived from Core Audio's strings |

use super::api::{
    AudioMeter, BackendCapabilities, BackendError, BackendResult, GraphDriver, MeterPolicy,
    NodeAudioState, NodeCapabilities,
};
use pw_graph_core::{
    encode_backend_id, BackendNamespace, Direction, Graph, GraphError, Link, LinkId, Node, NodeId,
    NodeType, Port, PortId, PortType, LOCAL_ID_MASK,
};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use windows::core::{Interface, GUID, PCWSTR, PWSTR};
use windows::Win32::Devices::Properties;
use windows::Win32::Foundation::{CloseHandle, PROPERTYKEY};
use windows::Win32::Media::Audio;
use windows::Win32::Media::Audio::Endpoints::{IAudioEndpointVolume, IAudioMeterInformation};
use windows::Win32::System::Com::{
    self, StructuredStorage, CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ,
};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::System::Variant::VT_LPWSTR;
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
use windows_core::BOOL;

mod callbacks;
mod driver;
mod identity;
mod worker;

#[cfg(test)]
mod tests;

// One 2,100-line file before; `pub(super)` keeps the reach a bare item had
// there, which is private to `windows`.
use self::callbacks::*;
pub use self::driver::WindowsAudioDriver;
use self::driver::*;
use self::identity::*;
use self::worker::*;
