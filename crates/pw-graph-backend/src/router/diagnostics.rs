//! What the router will tell you when the audio is wrong.
//!
//! Section 19.3 of the Windows parity roadmap lists the counters a route has
//! to expose, and section 19.2 requires that reading them never touches the
//! audio thread. Both are satisfied the same way: every counter is an atomic
//! the audio thread bumps and any other thread can snapshot.
//!
//! The one thing that cannot be an atomic is an error *message*, so the audio
//! thread stores a [`RouteFault`] code instead and the text is attached on the
//! reader's side. Formatting a string in a real-time callback is exactly the
//! allocation §8.1 forbids.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

/// The last thing that went wrong on a route.
///
/// Deliberately a small closed set: the audio thread can only store a code,
/// and a code nobody can render is worse than no code at all.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(u32)]
pub enum RouteFault {
    #[default]
    None = 0,
    /// The source produced fewer frames than asked for and the gap was filled
    /// with silence.
    SourceStarved = 1,
    /// The sink accepted fewer frames than offered and the rest was dropped.
    SinkBackedUp = 2,
    /// A source or sink reported that its device is gone.
    DeviceLost = 3,
    /// An effect returned an error and was bypassed for that block.
    ProcessorFailed = 4,
    /// A source or sink reported a geometry the route was not built for.
    FormatChanged = 5,
}

impl RouteFault {
    fn from_code(code: u32) -> Self {
        match code {
            1 => Self::SourceStarved,
            2 => Self::SinkBackedUp,
            3 => Self::DeviceLost,
            4 => Self::ProcessorFailed,
            5 => Self::FormatChanged,
            _ => Self::None,
        }
    }

    /// A message for the UI, built on the reader's thread.
    pub fn message(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::SourceStarved => Some("the route's source did not keep up and silence was used"),
            Self::SinkBackedUp => {
                Some("the route's destination did not keep up and audio was dropped")
            }
            Self::DeviceLost => Some("an audio device on this route is no longer available"),
            Self::ProcessorFailed => Some("an effect on this route failed and was bypassed"),
            Self::FormatChanged => Some("an audio device on this route changed format"),
        }
    }
}

/// Live counters for one route, shared between the audio thread and everyone
/// who wants to know how it is doing.
#[derive(Debug)]
pub struct RouteDiagnostics {
    source_underruns: AtomicU64,
    source_overruns: AtomicU64,
    sink_underruns: AtomicU64,
    sink_overruns: AtomicU64,
    discontinuities: AtomicU64,
    restarts: AtomicU64,
    frames_processed: AtomicU64,
    queue_depth: AtomicU64,
    /// Conversion ratio and drift as bit patterns, so the audio thread can
    /// publish `f64` without a lock.
    resampler_ratio: AtomicU64,
    clock_drift_ppm: AtomicU64,
    process_us: AtomicU64,
    effect_us: AtomicU64,
    fault: AtomicU32,
}

/// A consistent-enough picture of a route for the UI or a log line.
///
/// "Consistent enough" is deliberate: the counters are read one at a time
/// while audio keeps flowing, so two of them can come from either side of a
/// block boundary. Stopping the audio thread to make a diagnostic snapshot
/// atomic would be the wrong trade.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RouteMetrics {
    pub source_underruns: u64,
    pub source_overruns: u64,
    pub sink_underruns: u64,
    pub sink_overruns: u64,
    pub discontinuities: u64,
    pub restarts: u64,
    pub frames_processed: u64,
    /// Frames waiting in the route's buffers at the last block.
    pub queue_depth: u64,
    /// Input frames per output frame. A route fanning out to destinations at
    /// different rates has one of these per destination; the last one reached
    /// is what is reported, so read it as "this route is converting" rather
    /// than as the whole picture.
    pub resampler_ratio: f64,
    pub clock_drift_ppm: f64,
    /// Wall time the last block spent in the whole route.
    pub process_us: u64,
    /// Wall time the last block spent inside effects.
    pub effect_us: u64,
    pub fault: RouteFault,
}

impl RouteMetrics {
    /// The fault message, if any. Rendered here rather than on the audio
    /// thread.
    pub fn fault_message(&self) -> Option<&'static str> {
        self.fault.message()
    }
}

impl Default for RouteDiagnostics {
    /// Every counter starts at zero except the conversion ratio, which starts
    /// at unity. A route with no conversion still has a ratio, and a zero
    /// would read as a stopped clock on the diagnostics page.
    fn default() -> Self {
        Self {
            source_underruns: AtomicU64::new(0),
            source_overruns: AtomicU64::new(0),
            sink_underruns: AtomicU64::new(0),
            sink_overruns: AtomicU64::new(0),
            discontinuities: AtomicU64::new(0),
            restarts: AtomicU64::new(0),
            frames_processed: AtomicU64::new(0),
            queue_depth: AtomicU64::new(0),
            resampler_ratio: AtomicU64::new(1.0f64.to_bits()),
            clock_drift_ppm: AtomicU64::new(0.0f64.to_bits()),
            process_us: AtomicU64::new(0),
            effect_us: AtomicU64::new(0),
            fault: AtomicU32::new(RouteFault::None as u32),
        }
    }
}

impl RouteDiagnostics {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// The source could not supply a full block.
    pub fn note_source_underrun(&self, frames: u64) {
        self.source_underruns.fetch_add(frames, Ordering::Relaxed);
    }

    /// The source's buffer overflowed before the router drained it.
    pub fn note_source_overrun(&self, frames: u64) {
        self.source_overruns.fetch_add(frames, Ordering::Relaxed);
    }

    /// The sink asked for more than the route had.
    pub fn note_sink_underrun(&self, frames: u64) {
        self.sink_underruns.fetch_add(frames, Ordering::Relaxed);
    }

    /// The sink could not take everything the route offered.
    pub fn note_sink_overrun(&self, frames: u64) {
        self.sink_overruns.fetch_add(frames, Ordering::Relaxed);
    }

    /// The stream jumped: a device reset, a format change, or a gap large
    /// enough that interpolating across it would be a lie.
    pub fn note_discontinuity(&self) {
        self.discontinuities.fetch_add(1, Ordering::Relaxed);
    }

    /// A device was reopened after being lost.
    pub fn note_restart(&self) {
        self.restarts.fetch_add(1, Ordering::Relaxed);
    }

    pub fn note_frames(&self, frames: u64) {
        self.frames_processed.fetch_add(frames, Ordering::Relaxed);
    }

    pub fn set_queue_depth(&self, frames: u64) {
        self.queue_depth.store(frames, Ordering::Relaxed);
    }

    pub fn set_conversion(&self, ratio: f64, drift_ppm: f64) {
        self.resampler_ratio
            .store(ratio.to_bits(), Ordering::Relaxed);
        self.clock_drift_ppm
            .store(drift_ppm.to_bits(), Ordering::Relaxed);
    }

    pub fn set_timings(&self, process_us: u64, effect_us: u64) {
        self.process_us.store(process_us, Ordering::Relaxed);
        self.effect_us.store(effect_us, Ordering::Relaxed);
    }

    /// Record a fault. Only a code crosses the thread boundary.
    pub fn set_fault(&self, fault: RouteFault) {
        self.fault.store(fault as u32, Ordering::Relaxed);
    }

    /// Clear the fault once the condition that caused it is gone.
    pub fn clear_fault(&self) {
        self.set_fault(RouteFault::None);
    }

    /// Read every counter. Never blocks the audio thread.
    pub fn snapshot(&self) -> RouteMetrics {
        RouteMetrics {
            source_underruns: self.source_underruns.load(Ordering::Relaxed),
            source_overruns: self.source_overruns.load(Ordering::Relaxed),
            sink_underruns: self.sink_underruns.load(Ordering::Relaxed),
            sink_overruns: self.sink_overruns.load(Ordering::Relaxed),
            discontinuities: self.discontinuities.load(Ordering::Relaxed),
            restarts: self.restarts.load(Ordering::Relaxed),
            frames_processed: self.frames_processed.load(Ordering::Relaxed),
            queue_depth: self.queue_depth.load(Ordering::Relaxed),
            resampler_ratio: f64::from_bits(self.resampler_ratio.load(Ordering::Relaxed)),
            clock_drift_ppm: f64::from_bits(self.clock_drift_ppm.load(Ordering::Relaxed)),
            process_us: self.process_us.load(Ordering::Relaxed),
            effect_us: self.effect_us.load(Ordering::Relaxed),
            fault: RouteFault::from_code(self.fault.load(Ordering::Relaxed)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_route_reports_no_faults_and_a_unity_ratio() {
        let diagnostics = RouteDiagnostics::new();
        let metrics = diagnostics.snapshot();
        assert_eq!(metrics.fault, RouteFault::None);
        assert_eq!(metrics.fault_message(), None);
        // Unity, not zero: a route with no conversion still has a ratio, and a
        // zero would read as a broken clock in the diagnostics page.
        assert_eq!(metrics.resampler_ratio, 1.0);
    }

    #[test]
    fn counters_accumulate_rather_than_overwrite() {
        let diagnostics = RouteDiagnostics::new();
        diagnostics.note_source_underrun(64);
        diagnostics.note_source_underrun(32);
        diagnostics.note_sink_overrun(16);
        let metrics = diagnostics.snapshot();
        assert_eq!(metrics.source_underruns, 96);
        assert_eq!(metrics.sink_overruns, 16);
    }

    #[test]
    fn every_fault_a_route_can_record_renders_a_message() {
        for fault in [
            RouteFault::SourceStarved,
            RouteFault::SinkBackedUp,
            RouteFault::DeviceLost,
            RouteFault::ProcessorFailed,
            RouteFault::FormatChanged,
        ] {
            let diagnostics = RouteDiagnostics::new();
            diagnostics.set_fault(fault);
            let metrics = diagnostics.snapshot();
            assert_eq!(metrics.fault, fault);
            // A stored code with no text would be a silent failure wearing a
            // counter's clothes.
            assert!(metrics.fault_message().is_some());
        }
    }

    #[test]
    fn a_fault_can_be_cleared_once_the_condition_passes() {
        let diagnostics = RouteDiagnostics::new();
        diagnostics.set_fault(RouteFault::DeviceLost);
        diagnostics.clear_fault();
        assert_eq!(diagnostics.snapshot().fault, RouteFault::None);
    }

    #[test]
    fn floating_point_diagnostics_survive_the_trip_through_atomics_exactly() {
        let diagnostics = RouteDiagnostics::new();
        diagnostics.set_conversion(48_000.0 / 44_100.0, -37.5);
        let metrics = diagnostics.snapshot();
        assert_eq!(metrics.resampler_ratio, 48_000.0 / 44_100.0);
        assert_eq!(metrics.clock_drift_ppm, -37.5);
    }

    #[test]
    fn an_unknown_fault_code_degrades_to_none_instead_of_panicking() {
        assert_eq!(RouteFault::from_code(9_999), RouteFault::None);
    }
}
