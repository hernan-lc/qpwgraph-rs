//! Raw `pw_filter` hosting for the built-in realtime effects.
//!
//! `pipewire-rs` 0.8 intentionally does not wrap `pw_filter`, so this module
//! keeps the small amount of FFI needed by the backend in one place.  A filter
//! owns its callback state and is always created and destroyed while the
//! driver's `ThreadLoop` lock is held.  That is the lifetime boundary PipeWire
//! requires before a callback data pointer may be released.
//!
//! The current effect SDK has one builtin processor and no native WASM host.
//! Consequently this runtime exposes a stereo FL/FR pair of F32 DSP ports.
//! The callback converts PipeWire's planar buffers into the interleaved form
//! expected by the built-in processors, without allocating on the realtime
//! thread. Out-of-process/WASM module hosting still needs a richer realtime
//! control channel before it can be added safely.

use super::*;
use pw_graph_effects::{
    apply_parameters, AudioSpec, EffectHost, EffectInstanceConfig, EffectProcessor,
};
use std::ffi::c_void;
use std::ptr::{self, NonNull};
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::Mutex;

/// PipeWire's standard quantum is normally far smaller than this.  Keeping a
/// finite ceiling lets the callback build an exactly-sized Rust slice without
/// trusting an arbitrarily large duration supplied by an external graph.
const MAX_DSP_FRAMES: u32 = 16_384;
const PREPARED_SAMPLE_RATE: u32 = 48_000;
const DSP_CHANNELS: usize = 2;
const UNRESOLVED_ID: u64 = u64::MAX;

/// Processing state that is never reallocated from the realtime callback.
struct ProcessorState {
    processor: Box<dyn EffectProcessor>,
    interleaved: Vec<f32>,
}

/// The callback owns a planar FL/FR pair and an interleaved processor buffer.
/// The port pointers are published before `pw_filter_connect`, then never
/// changed, so loading them atomically is sufficient even when PipeWire calls
/// `process` on its separate realtime data thread.
struct CallbackState {
    input_ports: [AtomicPtr<c_void>; DSP_CHANNELS],
    output_ports: [AtomicPtr<c_void>; DSP_CHANNELS],
    enabled: AtomicBool,
    processor_failed: AtomicBool,
    /// A control update may run while PipeWire is processing.  The realtime
    /// callback uses `try_lock` and transparently bypasses a single quantum if
    /// the UI owns this mutex, rather than ever blocking the audio thread.
    ///
    /// This is intentionally a conservative bridge for the builtin Rust
    /// processors.  A future plugin ABI should replace it with a lock-free,
    /// preallocated control queue before hosting third-party processors.
    processor: Mutex<ProcessorState>,
}

impl CallbackState {
    fn new(processor: Box<dyn EffectProcessor>, enabled: bool) -> Self {
        Self {
            input_ports: std::array::from_fn(|_| AtomicPtr::new(ptr::null_mut())),
            output_ports: std::array::from_fn(|_| AtomicPtr::new(ptr::null_mut())),
            enabled: AtomicBool::new(enabled),
            processor_failed: AtomicBool::new(false),
            processor: Mutex::new(ProcessorState {
                processor,
                interleaved: vec![0.0; MAX_DSP_FRAMES as usize * DSP_CHANNELS],
            }),
        }
    }

    /// # Safety
    ///
    /// PipeWire invokes this with the callback data supplied to
    /// `pw_filter_new_simple`.  Both port pointers were returned by
    /// `pw_filter_add_port`, and the filter owns their lifetime. The DSP API
    /// guarantees F32 sample storage for each planar FL/FR channel.
    unsafe fn process(&self, position: *mut pw::spa::sys::spa_io_position) {
        if position.is_null() {
            return;
        }
        let frames = (*position).clock.duration;
        if frames == 0 || frames > u64::from(MAX_DSP_FRAMES) {
            self.processor_failed.store(true, Ordering::Relaxed);
            return;
        }
        let frames = frames as u32;
        let input_ports = self
            .input_ports
            .each_ref()
            .map(|port| port.load(Ordering::Acquire));
        let output_ports = self
            .output_ports
            .each_ref()
            .map(|port| port.load(Ordering::Acquire));
        if input_ports.iter().any(|port| port.is_null())
            || output_ports.iter().any(|port| port.is_null())
        {
            return;
        }
        let inputs = input_ports.map(|port| pw::sys::pw_filter_get_dsp_buffer(port, frames));
        let outputs = output_ports.map(|port| pw::sys::pw_filter_get_dsp_buffer(port, frames));
        if inputs.iter().any(|buffer| buffer.is_null())
            || outputs.iter().any(|buffer| buffer.is_null())
        {
            return;
        }

        // `pw_filter_get_dsp_buffer` owns the buffer lifecycle.  We only
        // borrow its valid F32 region for this callback and always begin from
        // a transparent pass-through signal. `copy` also permits the unlikely
        // case that PipeWire aliases the two DSP pointers.
        for channel in 0..DSP_CHANNELS {
            ptr::copy(
                inputs[channel].cast::<f32>(),
                outputs[channel].cast::<f32>(),
                frames as usize,
            );
        }
        if !self.enabled.load(Ordering::Acquire) {
            return;
        }

        let Ok(mut state) = self.processor.try_lock() else {
            // Parameter edits are intentionally allowed to cost one bypassed
            // quantum. Waiting for a non-realtime UI thread here would risk an
            // xrun for the entire PipeWire graph.
            return;
        };
        let ProcessorState {
            processor,
            interleaved,
        } = &mut *state;
        let samples = &mut interleaved[..frames as usize * DSP_CHANNELS];
        for frame in 0..frames as usize {
            for channel in 0..DSP_CHANNELS {
                samples[frame * DSP_CHANNELS + channel] = *inputs[channel].cast::<f32>().add(frame);
            }
        }
        // No Rust panic may cross the C callback boundary. Builtin processors
        // are specified not to panic, but a defensive catch keeps a malformed
        // future implementation from invoking undefined behaviour here.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            processor.process(samples, frames)
        }));
        match result {
            Ok(Ok(())) => {
                for frame in 0..frames as usize {
                    for channel in 0..DSP_CHANNELS {
                        *outputs[channel].cast::<f32>().add(frame) =
                            samples[frame * DSP_CHANNELS + channel];
                    }
                }
                self.processor_failed.store(false, Ordering::Relaxed);
            }
            _ => self.processor_failed.store(true, Ordering::Relaxed),
        }
    }
}

unsafe extern "C" fn filter_process(
    data: *mut c_void,
    position: *mut pw::spa::sys::spa_io_position,
) {
    // `data` is a Box<CallbackState> retained by EffectRuntime until after
    // `pw_filter_destroy` has detached all callbacks. Never unwind over C.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        let Some(state) = data.cast::<CallbackState>().as_ref() else {
            return;
        };
        state.process(position);
    }));
}

static FILTER_EVENTS: pw::sys::pw_filter_events = pw::sys::pw_filter_events {
    version: pw::sys::PW_VERSION_FILTER_EVENTS,
    destroy: None,
    state_changed: None,
    io_changed: None,
    param_changed: None,
    add_buffer: None,
    remove_buffer: None,
    process: Some(filter_process),
    drained: None,
    command: None,
};

/// RAII owner for a C `pw_filter` and its Rust callback state.
struct EffectRuntime {
    filter: NonNull<pw::sys::pw_filter>,
    callback: Box<CallbackState>,
}

impl EffectRuntime {
    fn create(
        thread_loop: &pw::thread_loop::ThreadLoop,
        node_name: &str,
        instance_id: &str,
        effect_id: &str,
        processor: Box<dyn EffectProcessor>,
        enabled: bool,
    ) -> BackendResult<Self> {
        let callback = Box::new(CallbackState::new(processor, enabled));
        let callback_ptr = callback.as_ref() as *const CallbackState as *mut c_void;
        let loop_ = unsafe { pw::sys::pw_thread_loop_get_loop(thread_loop.as_raw_ptr()) };
        if loop_.is_null() {
            return Err(BackendError::Native(
                "PipeWire effect filter has no thread loop".into(),
            ));
        }

        // `pw_filter_new_simple` creates the helper core/listener internally,
        // while still running on the driver's existing thread loop. It takes
        // ownership of the properties object on both success and failure.
        let filter_properties = pw::properties::properties! {
            "node.name" => node_name,
            "node.description" => node_name,
            "media.type" => "Audio",
            "media.category" => "Filter",
            "media.role" => "DSP",
            "media.class" => "Audio/Filter",
            "node.virtual" => "true",
            // Filters are deliberately patchable graph nodes. Never let a
            // session manager route a newly-created effect to a default device.
            "node.autoconnect" => "false",
            "node.group" => "qpwgraph-rs",
            "qpwgraph-rs.effect.instance" => instance_id,
            "qpwgraph-rs.effect.id" => effect_id,
        };
        let filter = unsafe {
            pw::sys::pw_filter_new_simple(
                loop_,
                std::ffi::CString::new(node_name)
                    .expect("effect node names are validated before reaching FFI")
                    .as_ptr(),
                filter_properties.into_raw(),
                &FILTER_EVENTS,
                callback_ptr,
            )
        };
        let Some(filter) = NonNull::new(filter) else {
            return Err(BackendError::Native(
                "PipeWire effect filter creation returned null".into(),
            ));
        };

        let mut input_ports = [ptr::null_mut(); DSP_CHANNELS];
        let mut output_ports = [ptr::null_mut(); DSP_CHANNELS];
        for (index, channel) in ["FL", "FR"].iter().enumerate() {
            let input_properties = pw::properties::properties! {
                "format.dsp" => "32 bit float mono audio",
                "port.name" => format!("input_{channel}"),
                "audio.channel" => *channel,
            };
            input_ports[index] = unsafe {
                pw::sys::pw_filter_add_port(
                    filter.as_ptr(),
                    pw::spa::sys::SPA_DIRECTION_INPUT,
                    pw::sys::pw_filter_port_flags_PW_FILTER_PORT_FLAG_MAP_BUFFERS,
                    0,
                    input_properties.into_raw(),
                    ptr::null_mut(),
                    0,
                )
            };
            if input_ports[index].is_null() {
                unsafe { pw::sys::pw_filter_destroy(filter.as_ptr()) };
                return Err(BackendError::Native(
                    "PipeWire effect input-port creation returned null".into(),
                ));
            }

            let output_properties = pw::properties::properties! {
                "format.dsp" => "32 bit float mono audio",
                "port.name" => format!("output_{channel}"),
                "audio.channel" => *channel,
            };
            output_ports[index] = unsafe {
                pw::sys::pw_filter_add_port(
                    filter.as_ptr(),
                    pw::spa::sys::SPA_DIRECTION_OUTPUT,
                    pw::sys::pw_filter_port_flags_PW_FILTER_PORT_FLAG_MAP_BUFFERS,
                    0,
                    output_properties.into_raw(),
                    ptr::null_mut(),
                    0,
                )
            };
            if output_ports[index].is_null() {
                unsafe { pw::sys::pw_filter_destroy(filter.as_ptr()) };
                return Err(BackendError::Native(
                    "PipeWire effect output-port creation returned null".into(),
                ));
            }
        }

        for channel in 0..DSP_CHANNELS {
            callback.input_ports[channel].store(input_ports[channel], Ordering::Release);
            callback.output_ports[channel].store(output_ports[channel], Ordering::Release);
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
                "PipeWire effect filter connection failed ({result})"
            )));
        }

        Ok(Self { filter, callback })
    }

    /// This may only be called while the parent's ThreadLoop lock is held.
    fn node_id(&self) -> Option<NodeId> {
        let id = unsafe { pw::sys::pw_filter_get_node_id(self.filter.as_ptr()) };
        (id != u32::MAX).then_some(NodeId(id as u64))
    }

    fn set_enabled(&self, enabled: bool) {
        self.callback.enabled.store(enabled, Ordering::Release);
    }

    fn set_parameter(&self, parameter: &str, value: f32) -> BackendResult<()> {
        let mut state = self
            .callback
            .processor
            .lock()
            .map_err(|_| BackendError::Native("effect processor lock was poisoned".into()))?;
        state
            .processor
            .set_parameter(parameter, value)
            .map_err(|error| BackendError::Native(error.to_string()))
    }

    fn processor_failed(&self) -> bool {
        self.callback.processor_failed.load(Ordering::Relaxed)
    }
}

impl Drop for EffectRuntime {
    fn drop(&mut self) {
        // The only owner is PipewireDriver, which drops/rolls these instances
        // under the ThreadLoop lock and before its Context/Core are released.
        // PipeWire detaches the realtime callback before this Box is dropped.
        unsafe { pw::sys::pw_filter_destroy(self.filter.as_ptr()) };
    }
}

/// Backend-owned metadata paired with a live PipeWire filter.
pub(super) struct NativeEffect {
    pub(super) instance: EffectInstance,
    runtime: EffectRuntime,
    node_name: String,
    position: [f32; 2],
}

impl NativeEffect {
    pub(super) fn create(
        host: &EffectHost,
        thread_loop: &pw::thread_loop::ThreadLoop,
        request: EffectNodeRequest,
    ) -> BackendResult<Self> {
        validate_request(&request)?;
        if request.module_path.is_some() {
            return Err(BackendError::Unsupported(
                "WASM/native effect modules are not yet hosted by the PipeWire filter runtime"
                    .into(),
            ));
        }

        // All setup and parameter validation happens before the raw filter is
        // published to PipeWire. Each filter exposes a planar FL/FR pair while
        // the processor receives the matching interleaved stereo buffer.
        let mut processor = host
            .create(&request.effect_id)
            .map_err(|error| BackendError::Native(format!("could not create effect: {error}")))?;
        processor
            .prepare(AudioSpec {
                sample_rate: PREPARED_SAMPLE_RATE,
                channels: DSP_CHANNELS as u16,
                max_frames: MAX_DSP_FRAMES,
            })
            .map_err(|error| BackendError::Native(error.to_string()))?;
        apply_parameters(&mut *processor, &request.parameters)
            .map_err(|error| BackendError::Native(error.to_string()))?;

        let effect_name = processor.descriptor().name.clone();
        validate_pipewire_text("effect name", &effect_name)?;
        // This value is intentionally both friendly and unique. PortKey uses
        // its node name, so two Noise Gate nodes must not collapse into the
        // same persistence/undo endpoint after a registry refresh.
        let node_name = format!("{effect_name} ({})", request.instance_id);
        let runtime = EffectRuntime::create(
            thread_loop,
            &node_name,
            &request.instance_id,
            &request.effect_id,
            processor,
            request.enabled,
        )?;
        let instance = EffectInstance {
            config: EffectInstanceConfig {
                instance_id: request.instance_id,
                effect_id: request.effect_id,
                module_path: request.module_path,
                enabled: request.enabled,
                parameters: request.parameters,
            },
            node_id: NodeId(UNRESOLVED_ID),
            input_port: PortId(UNRESOLVED_ID),
            output_port: PortId(UNRESOLVED_ID),
            source: None,
            destination: None,
            error: None,
        };
        Ok(Self {
            instance,
            runtime,
            node_name,
            position: request.position,
        })
    }

    pub(super) fn node_name(&self) -> &str {
        &self.node_name
    }

    pub(super) fn runtime_node_id(&self) -> Option<NodeId> {
        self.runtime.node_id()
    }

    pub(super) fn position(&self) -> [f32; 2] {
        self.position
    }

    pub(super) fn set_position(&mut self, position: [f32; 2]) {
        self.position = position;
    }

    pub(super) fn set_identity(
        &mut self,
        node_id: NodeId,
        input_port: PortId,
        output_port: PortId,
    ) {
        self.instance.node_id = node_id;
        self.instance.input_port = input_port;
        self.instance.output_port = output_port;
    }

    pub(super) fn resolved(&self) -> bool {
        self.instance.node_id.0 != UNRESOLVED_ID
            && self.instance.input_port.0 != UNRESOLVED_ID
            && self.instance.output_port.0 != UNRESOLVED_ID
    }

    pub(super) fn snapshot(&self) -> EffectInstance {
        let mut instance = self.instance.clone();
        if self.runtime.processor_failed() {
            instance.error =
                Some("effect processor rejected the most recent realtime buffer".into());
        }
        instance
    }

    pub(super) fn set_enabled(&mut self, enabled: bool) {
        self.runtime.set_enabled(enabled);
        self.instance.config.enabled = enabled;
    }

    pub(super) fn set_parameter(&mut self, parameter: &str, value: f32) -> BackendResult<()> {
        self.runtime.set_parameter(parameter, value)?;
        self.instance
            .config
            .parameters
            .insert(parameter.to_owned(), value);
        Ok(())
    }
}

fn validate_request(request: &EffectNodeRequest) -> BackendResult<()> {
    if request.instance_id.trim().is_empty() {
        return Err(BackendError::Native(
            "effect instance id cannot be empty".into(),
        ));
    }
    validate_pipewire_text("effect instance id", &request.instance_id)?;
    validate_pipewire_text("effect id", &request.effect_id)?;
    Ok(())
}

fn validate_pipewire_text(label: &str, value: &str) -> BackendResult<()> {
    if value.contains('\0') {
        return Err(BackendError::Native(format!(
            "{label} contains a NUL byte and cannot be passed to PipeWire"
        )));
    }
    Ok(())
}
