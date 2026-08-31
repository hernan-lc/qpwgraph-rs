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

use super::filter_runtime::FilterRuntime;
use super::*;
use pw_graph_effects::{
    apply_parameters, AudioSpec, EffectHost, EffectInstanceConfig, EffectProcessor,
};
use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering};
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
    /// Seed for the low-level diagnostic signal used when an input channel is
    /// not connected. A filter can still have a live output connection while
    /// one or both inputs are dangling; leaving the output buffer untouched in
    /// that case is both silent and unsafe.
    fallback_noise: AtomicU32,
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
            fallback_noise: AtomicU32::new(0x6d2b_79f5),
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
        if output_ports.iter().all(|port| port.is_null()) {
            return;
        }

        // PipeWire may not provide a DSP buffer for an unconnected port. Keep
        // each channel independent: a connected FL input must still reach its
        // output when FR is not patched, and an output-only effect should emit
        // a small diagnostic signal instead of leaving the output undefined.
        let inputs: [*mut c_void; DSP_CHANNELS] = std::array::from_fn(|channel| {
            let port = input_ports[channel];
            if port.is_null() {
                ptr::null_mut()
            } else {
                pw::sys::pw_filter_get_dsp_buffer(port, frames)
            }
        });
        let outputs: [*mut c_void; DSP_CHANNELS] = std::array::from_fn(|channel| {
            let port = output_ports[channel];
            if port.is_null() {
                ptr::null_mut()
            } else {
                pw::sys::pw_filter_get_dsp_buffer(port, frames)
            }
        });
        if outputs.iter().all(|buffer| buffer.is_null()) {
            return;
        }

        let enabled = self.enabled.load(Ordering::Acquire);
        if !enabled {
            // Disabled effects remain transparent for connected channels and
            // produce silence for dangling inputs. The diagnostic signal is a
            // useful enabled-effect indication, not a bypass-side effect.
            for frame in 0..frames as usize {
                for channel in 0..DSP_CHANNELS {
                    if !outputs[channel].is_null() {
                        *outputs[channel].cast::<f32>().add(frame) = if inputs[channel].is_null() {
                            0.0
                        } else {
                            *inputs[channel].cast::<f32>().add(frame)
                        };
                    }
                }
            }
            return;
        }

        let Ok(mut state) = self.processor.try_lock() else {
            // Parameter edits are intentionally allowed to cost one bypassed
            // quantum. Waiting for a non-realtime UI thread here would risk an
            // xrun for the entire PipeWire graph.
            for frame in 0..frames as usize {
                for channel in 0..DSP_CHANNELS {
                    if !outputs[channel].is_null() {
                        let sample = if inputs[channel].is_null() {
                            next_diagnostic_noise(&self.fallback_noise)
                        } else {
                            *inputs[channel].cast::<f32>().add(frame)
                        };
                        *outputs[channel].cast::<f32>().add(frame) = sample;
                    }
                }
            }
            return;
        };
        let ProcessorState {
            processor,
            interleaved,
        } = &mut *state;
        let samples = &mut interleaved[..frames as usize * DSP_CHANNELS];
        for frame in 0..frames as usize {
            for channel in 0..DSP_CHANNELS {
                samples[frame * DSP_CHANNELS + channel] = if inputs[channel].is_null() {
                    // Keep this deliberately quiet (about -34 dBFS) so an
                    // accidentally dangling effect is noticeable without
                    // being an abrupt full-scale burst.
                    next_diagnostic_noise(&self.fallback_noise)
                } else {
                    *inputs[channel].cast::<f32>().add(frame)
                };
            }
        }
        // Initialize the outputs before invoking the processor. If a future
        // processor rejects a buffer or panics, the callback still publishes a
        // valid pass-through/diagnostic signal for this quantum.
        for frame in 0..frames as usize {
            for channel in 0..DSP_CHANNELS {
                if !outputs[channel].is_null() {
                    *outputs[channel].cast::<f32>().add(frame) =
                        samples[frame * DSP_CHANNELS + channel];
                }
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
                        if !outputs[channel].is_null() {
                            *outputs[channel].cast::<f32>().add(frame) =
                                samples[frame * DSP_CHANNELS + channel];
                        }
                    }
                }
                self.processor_failed.store(false, Ordering::Relaxed);
            }
            _ => self.processor_failed.store(true, Ordering::Relaxed),
        }
    }
}

/// Generate a bounded, allocation-free diagnostic sample for a dangling
/// effect input. An xorshift stream is sufficient here; this is a routing
/// indicator, not an audio-quality noise source.
fn next_diagnostic_noise(state: &AtomicU32) -> f32 {
    let value = state
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
            let mut next = value;
            next ^= next << 13;
            next ^= next >> 17;
            next ^= next << 5;
            Some(next)
        })
        .unwrap_or(0x6d2b_79f5);
    ((value as f32 / u32::MAX as f32) * 2.0 - 1.0) * 0.02
}

unsafe extern "C" fn filter_process(
    data: *mut c_void,
    position: *mut pw::spa::sys::spa_io_position,
) {
    // `data` is a Box<CallbackState> retained by FilterRuntime until after
    // `pw_filter_destroy` has detached all callbacks. Never unwind over C.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        let Some(state) = data.cast::<CallbackState>().as_ref() else {
            return;
        };
        state.process(position);
    }));
}

/// Backend-owned metadata paired with a live PipeWire filter.
pub(super) struct NativeEffect {
    pub(super) instance: EffectInstance,
    runtime: FilterRuntime<CallbackState>,
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
            return Err(BackendError::unsupported(
                "WASM/native effect modules are not yet hosted by the PipeWire filter runtime",
            ));
        }

        // All setup and parameter validation happens before the raw filter is
        // published to PipeWire. Each filter exposes a planar FL/FR pair while
        // the processor receives the matching interleaved stereo buffer.
        let mut processor = host
            .create(&request.effect_id)
            .map_err(BackendError::effect_create_failed)?;
        processor
            .prepare(AudioSpec {
                sample_rate: PREPARED_SAMPLE_RATE,
                channels: DSP_CHANNELS as u16,
                max_frames: MAX_DSP_FRAMES,
            })
            .map_err(BackendError::native)?;
        apply_parameters(&mut *processor, &request.parameters).map_err(BackendError::native)?;

        let effect_name = processor.descriptor().name.clone();
        validate_pipewire_text("effect name", &effect_name)?;
        // This value is intentionally both friendly and unique. PortKey uses
        // its node name, so two Noise Gate nodes must not collapse into the
        // same persistence/undo endpoint after a registry refresh.
        let node_name = format!("{effect_name} ({})", request.instance_id);

        let callback = Box::new(CallbackState::new(processor, request.enabled));
        let filter_properties = pw::properties::properties! {
            NODE_NAME => node_name.as_str(),
            NODE_DESCRIPTION => node_name.as_str(),
            MEDIA_TYPE => MEDIA_TYPE_AUDIO,
            PROP_MEDIA_CATEGORY => MEDIA_CATEGORY_FILTER,
            PROP_MEDIA_ROLE => MEDIA_ROLE_DSP,
            MEDIA_CLASS => MEDIA_CLASS_AUDIO_FILTER,
            PROP_NODE_VIRTUAL => "true",
            // Filters are deliberately patchable graph nodes. Never let a
            // session manager route a newly-created effect to a default device.
            PROP_NODE_AUTOCONNECT => "false",
            PROP_NODE_GROUP => "qpwgraph-rs",
            "qpwgraph-rs.effect.instance" => request.instance_id.as_str(),
            "qpwgraph-rs.effect.id" => request.effect_id.as_str(),
        };
        let runtime = FilterRuntime::create(
            thread_loop,
            &node_name,
            filter_properties,
            Some(filter_process),
            callback,
        )?;

        let mut input_ports = [ptr::null_mut(); DSP_CHANNELS];
        let mut output_ports = [ptr::null_mut(); DSP_CHANNELS];
        for (index, channel) in ["FL", "FR"].iter().enumerate() {
            input_ports[index] = runtime.add_port(
                pw::spa::sys::SPA_DIRECTION_INPUT,
                &format!("input_{channel}"),
                channel,
            )?;
            output_ports[index] = runtime.add_port(
                pw::spa::sys::SPA_DIRECTION_OUTPUT,
                &format!("output_{channel}"),
                channel,
            )?;
        }
        for channel in 0..DSP_CHANNELS {
            runtime.callback().input_ports[channel].store(input_ports[channel], Ordering::Release);
            runtime.callback().output_ports[channel]
                .store(output_ports[channel], Ordering::Release);
        }
        runtime.connect()?;

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
        if self
            .runtime
            .callback()
            .processor_failed
            .load(Ordering::Relaxed)
        {
            instance.error =
                Some("effect processor rejected the most recent realtime buffer".into());
        }
        instance
    }

    pub(super) fn set_enabled(&mut self, enabled: bool) {
        self.runtime
            .callback()
            .enabled
            .store(enabled, Ordering::Release);
        self.instance.config.enabled = enabled;
    }

    pub(super) fn set_parameter(&mut self, parameter: &str, value: f32) -> BackendResult<()> {
        let mut state = self
            .runtime
            .callback()
            .processor
            .lock()
            .map_err(|_| BackendError::native("effect processor lock was poisoned"))?;
        state
            .processor
            .set_parameter(parameter, value)
            .map_err(BackendError::native)?;
        self.instance
            .config
            .parameters
            .insert(parameter.to_owned(), value);
        Ok(())
    }
}

fn validate_request(request: &EffectNodeRequest) -> BackendResult<()> {
    if request.instance_id.trim().is_empty() {
        return Err(BackendError::native("effect instance id cannot be empty"));
    }
    validate_pipewire_text("effect instance id", &request.instance_id)?;
    validate_pipewire_text("effect id", &request.effect_id)?;
    Ok(())
}

fn validate_pipewire_text(label: &str, value: &str) -> BackendResult<()> {
    if value.contains('\0') {
        return Err(BackendError::native(format!(
            "{label} contains a NUL byte and cannot be passed to PipeWire"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::next_diagnostic_noise;
    use std::sync::atomic::AtomicU32;

    #[test]
    fn dangling_input_signal_is_bounded_and_changes() {
        let state = AtomicU32::new(0x6d2b_79f5);
        let samples: Vec<_> = (0..32).map(|_| next_diagnostic_noise(&state)).collect();
        assert!(samples.iter().all(|sample| sample.is_finite()));
        assert!(samples.iter().all(|sample| sample.abs() <= 0.02));
        assert!(samples.windows(2).any(|pair| pair[0] != pair[1]));
    }
}
