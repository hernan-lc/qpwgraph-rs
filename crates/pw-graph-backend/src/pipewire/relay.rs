//! Virtual relay devices: client-owned `pw_filter` nodes that bridge the
//! network relay engine into the PipeWire graph.
//!
//! Two nodes exist while the relay is active:
//!
//! - **Relay Microphone** — an output-only filter published as
//!   `Audio/Source/Virtual`. Audio decoded from peer datagrams is pulled from
//!   the relay engine and published here, so any application can capture a
//!   phone's microphone like a regular input device.
//! - **Relay Speaker** — an input-only filter published as `Audio/Sink`.
//!   Whatever applications play into it is drained, mixed to mono, and
//!   transmitted to receiving peers.
//!
//! The realtime callbacks follow the same discipline as `effects.rs`: no
//! allocation, atomics for port pointers, and only `try_lock`-style access
//! into the engine's PCM queues so a busy network worker can cost at most one
//! bypassed quantum instead of an xrun.

use super::*;
use pw_graph_relay::{RelayEngine, RelayHandle};
use std::ffi::c_void;
use std::ptr::{self, NonNull};
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::Mutex;

const RELAY_MAX_FRAMES: u32 = 16_384;
const RELAY_CHANNELS: usize = 2;

pub(super) const RELAY_SOURCE_NAME: &str = "qpwgraph-rs.relay.source";
pub(super) const RELAY_SINK_NAME: &str = "qpwgraph-rs.relay.sink";

/// Which virtual device a filter represents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RelayNodeKind {
    /// Output-only: network audio appears as a capture device.
    Microphone,
    /// Input-only: captured audio is transmitted to peers.
    Speaker,
}

/// Callback state owned by one relay filter. Only PipeWire's realtime data
/// thread touches `scratch`; it sits behind a mutex purely for interior
/// mutability under the shared callback pointer, and the callback uses
/// `try_lock` so a pathological hold elsewhere could cost at most one
/// bypassed quantum. The port pointers are published once before the filter
/// connects and never change.
struct RelayCallbackState {
    kind: RelayNodeKind,
    ports: [AtomicPtr<c_void>; RELAY_CHANNELS],
    handle: RelayHandle,
    scratch: Mutex<Vec<f32>>,
}

impl RelayCallbackState {
    fn new(kind: RelayNodeKind, handle: RelayHandle) -> Self {
        Self {
            kind,
            ports: std::array::from_fn(|_| AtomicPtr::new(ptr::null_mut())),
            handle,
            scratch: Mutex::new(vec![0.0; RELAY_MAX_FRAMES as usize]),
        }
    }

    /// # Safety
    ///
    /// PipeWire invokes this on its realtime data thread with the callback
    /// data supplied to `pw_filter_new_simple`. Port pointers were returned
    /// by `pw_filter_add_port` and remain valid until the filter is destroyed.
    unsafe fn process(&self, position: *mut pw::spa::sys::spa_io_position) {
        if position.is_null() {
            return;
        }
        let frames = (*position).clock.duration;
        if frames == 0 || frames > u64::from(RELAY_MAX_FRAMES) {
            return;
        }
        let frames = frames as u32 as usize;
        let ports = self.ports.each_ref().map(|port| port.load(Ordering::Acquire));
        let Ok(mut scratch_guard) = self.scratch.try_lock() else {
            return;
        };
        let scratch = &mut scratch_guard[..frames];

        match self.kind {
            RelayNodeKind::Microphone => {
                let outputs: [*mut c_void; RELAY_CHANNELS] = std::array::from_fn(|channel| {
                    if ports[channel].is_null() {
                        ptr::null_mut()
                    } else {
                        pw::sys::pw_filter_get_dsp_buffer(ports[channel], frames as u32)
                    }
                });
                if outputs.iter().all(|buffer| buffer.is_null()) {
                    return;
                }
                let available = self.handle.try_pull_playback(scratch);
                if available < frames {
                    scratch[available..].fill(0.0);
                }
                for frame in 0..frames {
                    let sample = scratch[frame];
                    for channel in 0..RELAY_CHANNELS {
                        if !outputs[channel].is_null() {
                            *outputs[channel].cast::<f32>().add(frame) = sample;
                        }
                    }
                }
            }
            RelayNodeKind::Speaker => {
                let inputs: [*mut c_void; RELAY_CHANNELS] = std::array::from_fn(|channel| {
                    if ports[channel].is_null() {
                        ptr::null_mut()
                    } else {
                        pw::sys::pw_filter_get_dsp_buffer(ports[channel], frames as u32)
                    }
                });
                if inputs.iter().all(|buffer| buffer.is_null()) {
                    return;
                }
                for frame in 0..frames {
                    let mut sum = 0.0f32;
                    let mut count = 0.0f32;
                    for channel in 0..RELAY_CHANNELS {
                        if !inputs[channel].is_null() {
                            sum += *inputs[channel].cast::<f32>().add(frame);
                            count += 1.0;
                        }
                    }
                    scratch[frame] = if count > 0.0 { sum / count } else { 0.0 };
                }
                self.handle.try_push_capture(scratch);
            }
        }
    }
}

unsafe extern "C" fn relay_filter_process(
    data: *mut c_void,
    position: *mut pw::spa::sys::spa_io_position,
) {
    // `data` is a Box<RelayCallbackState> retained by RelayNodeRuntime until
    // `pw_filter_destroy` has detached all callbacks. Never unwind over C.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        let Some(state) = data.cast::<RelayCallbackState>().as_ref() else {
            return;
        };
        state.process(position);
    }));
}

static RELAY_FILTER_EVENTS: pw::sys::pw_filter_events = pw::sys::pw_filter_events {
    version: pw::sys::PW_VERSION_FILTER_EVENTS,
    destroy: None,
    state_changed: None,
    io_changed: None,
    param_changed: None,
    add_buffer: None,
    remove_buffer: None,
    process: Some(relay_filter_process),
    drained: None,
    command: None,
};

/// RAII owner for one virtual relay node.
///
/// Like `EffectRuntime`, this may only be dropped while the driver's
/// ThreadLoop lock is held: PipeWire must detach the realtime callback before
/// the callback state is released.
pub(super) struct RelayNodeRuntime {
    filter: NonNull<pw::sys::pw_filter>,
    _callback: Box<RelayCallbackState>,
}

impl RelayNodeRuntime {
    pub(super) fn create(
        thread_loop: &pw::thread_loop::ThreadLoop,
        handle: RelayHandle,
        kind: RelayNodeKind,
    ) -> BackendResult<Self> {
        let (node_name, description, media_class, icon) = match kind {
            RelayNodeKind::Microphone => (
                RELAY_SOURCE_NAME,
                "Relay Microphone",
                "Audio/Source/Virtual",
                "audio-input-microphone",
            ),
            RelayNodeKind::Speaker => (
                RELAY_SINK_NAME,
                "Relay Speaker",
                "Audio/Sink",
                "audio-card",
            ),
        };

        let callback = Box::new(RelayCallbackState::new(kind, handle));
        let callback_ptr = callback.as_ref() as *const RelayCallbackState as *mut c_void;
        let loop_ = unsafe { pw::sys::pw_thread_loop_get_loop(thread_loop.as_raw_ptr()) };
        if loop_.is_null() {
            return Err(BackendError::Native(
                "PipeWire relay filter has no thread loop".into(),
            ));
        }

        let properties = pw::properties::properties! {
            "node.name" => node_name,
            "node.description" => description,
            "media.type" => "Audio",
            "media.class" => media_class,
            "node.virtual" => "true",
            // Relay endpoints are patchable graph nodes; never let a session
            // manager silently route them to a default device.
            "node.autoconnect" => "false",
            "node.group" => "qpwgraph-rs",
            "device.icon-name" => icon,
            "qpwgraph-rs.relay.kind" => match kind {
                RelayNodeKind::Microphone => "source",
                RelayNodeKind::Speaker => "sink",
            },
        };
        let filter = unsafe {
            pw::sys::pw_filter_new_simple(
                loop_,
                std::ffi::CString::new(node_name)
                    .expect("relay node names are static C strings")
                    .as_ptr(),
                properties.into_raw(),
                &RELAY_FILTER_EVENTS,
                callback_ptr,
            )
        };
        let Some(filter) = NonNull::new(filter) else {
            return Err(BackendError::Native(
                "PipeWire relay filter creation returned null".into(),
            ));
        };

        let direction = match kind {
            RelayNodeKind::Microphone => pw::spa::sys::SPA_DIRECTION_OUTPUT,
            RelayNodeKind::Speaker => pw::spa::sys::SPA_DIRECTION_INPUT,
        };
        let mut ports = [ptr::null_mut(); RELAY_CHANNELS];
        for (index, channel) in ["FL", "FR"].iter().enumerate() {
            let port_properties = pw::properties::properties! {
                "format.dsp" => "32 bit float mono audio",
                "port.name" => format!("{channel}"),
                "audio.channel" => *channel,
            };
            ports[index] = unsafe {
                pw::sys::pw_filter_add_port(
                    filter.as_ptr(),
                    direction,
                    pw::sys::pw_filter_port_flags_PW_FILTER_PORT_FLAG_MAP_BUFFERS,
                    0,
                    port_properties.into_raw(),
                    ptr::null_mut(),
                    0,
                )
            };
            if ports[index].is_null() {
                unsafe { pw::sys::pw_filter_destroy(filter.as_ptr()) };
                return Err(BackendError::Native(format!(
                    "PipeWire relay {channel} port creation returned null"
                )));
            }
        }
        for channel in 0..RELAY_CHANNELS {
            callback.ports[channel].store(ports[channel], Ordering::Release);
        }

        let result = unsafe {
            pw::sys::pw_filter_connect(
                filter.as_ptr(),
                pw::sys::pw_filter_flags_PW_FILTER_FLAG_RT_PROCESS,
                ptr::null_mut(),
                0,
            )
        };
        if result < 0 {
            unsafe { pw::sys::pw_filter_destroy(filter.as_ptr()) };
            return Err(BackendError::Native(format!(
                "PipeWire relay filter connection failed ({result})"
            )));
        }

        Ok(Self {
            filter,
            _callback: callback,
        })
    }
}

impl Drop for RelayNodeRuntime {
    fn drop(&mut self) {
        // The driver drops relay runtimes under the ThreadLoop lock, matching
        // the effect filter lifecycle. PipeWire detaches the realtime
        // callback before this returns, so the callback Box can be released.
        unsafe { pw::sys::pw_filter_destroy(self.filter.as_ptr()) };
    }
}

/// Everything the PipeWire driver owns for the relay feature.
///
/// `_engine` and the two runtimes are held for their lifetimes: dropping the
/// set tears down the filters (under the ThreadLoop lock, enforced by the
/// driver) and stops the engine's worker threads.
pub(super) struct RelayRuntimeSet {
    _engine: RelayEngine,
    handle: RelayHandle,
    _source: RelayNodeRuntime,
    _sink: RelayNodeRuntime,
}

impl RelayRuntimeSet {
    /// Create the engine and both virtual nodes. Caller holds the ThreadLoop
    /// lock.
    pub(super) fn create(
        thread_loop: &pw::thread_loop::ThreadLoop,
        device_name: &str,
    ) -> BackendResult<Self> {
        let config = pw_graph_relay::EngineConfig {
            device_name: device_name.to_owned(),
            ..Default::default()
        };
        let engine = RelayEngine::start(config)
            .map_err(|error| BackendError::Native(format!("relay engine start: {error}")))?;
        let handle = engine.handle();
        let source = RelayNodeRuntime::create(thread_loop, handle.clone(), RelayNodeKind::Microphone)?;
        let sink = match RelayNodeRuntime::create(thread_loop, handle.clone(), RelayNodeKind::Speaker)
        {
            Ok(created) => created,
            Err(error) => {
                // The engine must not outlive a half-built device set.
                engine.shutdown();
                return Err(error);
            }
        };
        Ok(Self {
            _engine: engine,
            handle,
            _source: source,
            _sink: sink,
        })
    }

    pub(super) fn handle(&self) -> &RelayHandle {
        &self.handle
    }
}
