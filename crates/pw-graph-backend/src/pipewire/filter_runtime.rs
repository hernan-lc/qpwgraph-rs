//! Shared raw `pw_filter` lifecycle helpers for the effect and relay runtimes.
//!
//! `pipewire-rs` 0.8 does not wrap `pw_filter`, so every client-owned helper
//! node has to go through the same small set of FFI calls: look up the
//! `ThreadLoop`'s main loop, build the events table, create the filter, add
//! its ports, and connect. The two owners (realtime effect processors and the
//! virtual relay devices) differ only in their callback payloads and DSP
//! bodies, so this module owns the shared skeleton and each caller supplies
//! its own callback type and `process` trampoline.
//!
//! A [`FilterRuntime`] is always created and destroyed while the driver's
//! `ThreadLoop` lock is held. That is the lifetime boundary PipeWire requires
//! before a callback data pointer may be released.

use super::*;
use std::ffi::c_void;
use std::ptr::{self, NonNull};

/// RAII owner for a C `pw_filter` and its Rust callback state. Dropping the
/// runtime destroys the filter, which detaches the realtime callback before
/// the callback `Box` is released.
pub(super) struct FilterRuntime<T> {
    filter: NonNull<pw::sys::pw_filter>,
    callback: Box<T>,
}

impl<T> FilterRuntime<T> {
    /// Creates the filter on `thread_loop` with `callback` as its data
    /// pointer and `process` as the DSP entry point. Takes ownership of
    /// `properties` (PipeWire consumes it on both success and failure).
    pub(super) fn create(
        thread_loop: &pw::thread_loop::ThreadLoop,
        node_name: &str,
        properties: pw::properties::Properties,
        process: Option<
            unsafe extern "C" fn(*mut c_void, *mut pw::spa::sys::spa_io_position),
        >,
        callback: Box<T>,
    ) -> BackendResult<Self> {
        let callback_ptr = callback.as_ref() as *const T as *mut c_void;
        let loop_ = unsafe { pw::sys::pw_thread_loop_get_loop(thread_loop.as_raw_ptr()) };
        if loop_.is_null() {
            return Err(BackendError::native("PipeWire filter has no thread loop"));
        }

        // `pw_filter_new_simple` creates the helper core/listener internally
        // while still running on the driver's existing thread loop. The
        // events table is copied into the filter, so a per-call struct is
        // enough even though the previous owners kept statics.
        let events = pw::sys::pw_filter_events {
            version: pw::sys::PW_VERSION_FILTER_EVENTS,
            destroy: None,
            state_changed: None,
            io_changed: None,
            param_changed: None,
            add_buffer: None,
            remove_buffer: None,
            process,
            drained: None,
            command: None,
        };
        let filter = unsafe {
            pw::sys::pw_filter_new_simple(
                loop_,
                std::ffi::CString::new(node_name)
                    .expect("filter node names are validated before reaching FFI")
                    .as_ptr(),
                properties.into_raw(),
                &events,
                callback_ptr,
            )
        };
        let Some(filter) = NonNull::new(filter) else {
            return Err(BackendError::native(
                "PipeWire filter creation returned null",
            ));
        };
        Ok(Self { filter, callback })
    }

    /// Adds a mapped DSP port with the standard stereo property set. The
    /// caller stores the returned pointer; a later failure must drop the
    /// runtime, which destroys the partially built filter.
    pub(super) fn add_port(
        &self,
        direction: pw::spa::sys::spa_direction,
        port_name: &str,
        channel: &str,
    ) -> Result<*mut c_void, BackendError> {
        let port_properties = pw::properties::properties! {
            FORMAT_DSP => PROP_FORMAT_DSP_VALUE,
            PORT_NAME => port_name,
            AUDIO_CHANNEL => channel,
        };
        let port = unsafe {
            pw::sys::pw_filter_add_port(
                self.filter.as_ptr(),
                direction,
                pw::sys::pw_filter_port_flags_PW_FILTER_PORT_FLAG_MAP_BUFFERS,
                0,
                port_properties.into_raw(),
                ptr::null_mut(),
                0,
            )
        };
        if port.is_null() {
            return Err(BackendError::native(format!(
                "PipeWire filter port {port_name} creation returned null"
            )));
        }
        Ok(port)
    }

    /// Connects the filter to the graph with the realtime process flag.
    pub(super) fn connect(&self) -> BackendResult<()> {
        let result = unsafe {
            pw::sys::pw_filter_connect(
                self.filter.as_ptr(),
                pw::sys::pw_filter_flags_PW_FILTER_FLAG_RT_PROCESS,
                ptr::null_mut(),
                0,
            )
        };
        if result < 0 {
            return Err(BackendError::native(format!(
                "PipeWire filter connection failed ({result})"
            )));
        }
        Ok(())
    }

    /// The node id PipeWire assigned to the filter, once it exists.
    pub(super) fn node_id(&self) -> Option<NodeId> {
        let id = unsafe { pw::sys::pw_filter_get_node_id(self.filter.as_ptr()) };
        (id != u32::MAX).then_some(NodeId(id as u64))
    }

    /// Access to the callback state owned by the runtime. Callers use this to
    /// publish port pointers after `create` and to drive the callback from the
    /// driver thread.
    pub(super) fn callback(&self) -> &T {
        &self.callback
    }
}

impl<T> Drop for FilterRuntime<T> {
    fn drop(&mut self) {
        // The only owners are the PipeWire driver's effect and relay sets,
        // which are dropped/rolled back under the ThreadLoop lock and before
        // the Context/Core are released. PipeWire detaches the realtime
        // callback before this Box is dropped.
        unsafe { pw::sys::pw_filter_destroy(self.filter.as_ptr()) };
    }
}
