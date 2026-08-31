//! The merged graph and the per-child refresh clocks.
//!
//! Each child keeps its own deadline: an event-driven backend still gets an
//! infrequent safety reconciliation, while a polling-only one is checked on
//! its own cadence instead of forcing every native backend to rebuild on
//! every UI tick.

use super::*;
#[cfg(any(
    test,
    target_os = "windows",
    all(target_os = "linux", any(feature = "pipewire", feature = "alsa"))
))]
use std::time::{Duration, Instant};

/// The merged graph has one refresh clock per child. Event-driven children
/// still get an infrequent safety reconciliation, while polling-only children
/// are checked on their own cadence instead of forcing every native backend to
/// rebuild on every UI tick.
#[cfg(any(
    test,
    target_os = "windows",
    all(target_os = "linux", any(feature = "pipewire", feature = "alsa"))
))]
#[derive(Default)]
pub(super) struct RefreshSchedule {
    #[cfg(all(target_os = "linux", feature = "pipewire"))]
    pub(super) pipewire_deadline: Option<Instant>,
    #[cfg(all(target_os = "linux", feature = "alsa"))]
    pub(super) alsa_deadline: Option<Instant>,
    #[cfg(target_os = "windows")]
    pub(super) windows_audio_deadline: Option<Instant>,
    #[cfg(target_os = "windows")]
    pub(super) windows_midi_deadline: Option<Instant>,
}

#[cfg(any(target_os = "windows", all(target_os = "linux", feature = "pipewire")))]
pub(super) const EVENT_REFRESH_INTERVAL: Duration = Duration::from_secs(5);
#[cfg(all(target_os = "linux", feature = "alsa"))]
pub(super) const ALSA_REFRESH_INTERVAL: Duration = Duration::from_secs(3);
// MIDI device arrival on Windows is polled rather than evented, so it gets a
// shorter interval than the audio side. Gated like the ALSA constant above:
// otherwise every non-Windows build reports it as dead code.
#[cfg(target_os = "windows")]
pub(super) const WINDOWS_MIDI_REFRESH_INTERVAL: Duration = Duration::from_secs(2);

#[cfg(any(
    test,
    target_os = "windows",
    all(target_os = "linux", any(feature = "pipewire", feature = "alsa"))
))]
pub(crate) fn refresh_due(deadline: Option<Instant>, dirty: bool, now: Instant) -> bool {
    dirty || deadline.is_none_or(|deadline| now >= deadline)
}

#[cfg(any(
    test,
    target_os = "windows",
    all(target_os = "linux", any(feature = "pipewire", feature = "alsa"))
))]
pub(crate) fn merge_graph_into(destination: &mut Graph, source: &Graph) -> Result<(), GraphError> {
    for node in source.nodes.values().cloned() {
        destination.add_node(node)?;
    }
    for port in source.ports.values().cloned() {
        destination.add_port(port)?;
    }
    for link in source.links.values().cloned() {
        destination.insert_existing_link(link)?;
    }
    Ok(())
}

impl CompositeDriver {
    #[allow(dead_code)]
    #[allow(unused_mut)]
    pub(super) fn rebuild_merged_graph(&mut self) -> Result<(), GraphError> {
        let mut graph = Graph::default();
        #[cfg(all(target_os = "linux", feature = "pipewire"))]
        if let Some(driver) = self.pipewire.as_ref() {
            merge_graph_into(&mut graph, driver.graph())?;
        }
        #[cfg(all(target_os = "linux", feature = "alsa"))]
        if let Some(driver) = self.alsa.as_ref() {
            merge_graph_into(&mut graph, driver.graph())?;
        }
        #[cfg(target_os = "windows")]
        if let Some(driver) = self.windows_audio.as_ref() {
            merge_graph_into(&mut graph, driver.graph())?;
        }
        #[cfg(target_os = "windows")]
        if let Some(driver) = self.windows_midi.as_ref() {
            merge_graph_into(&mut graph, driver.graph())?;
        }
        self.graph = graph;
        Ok(())
    }

    pub(super) fn refresh_all(&mut self) -> BackendResult<Vec<Node>> {
        #[cfg(any(
            target_os = "windows",
            all(target_os = "linux", any(feature = "pipewire", feature = "alsa"))
        ))]
        let now = Instant::now();
        #[cfg(all(target_os = "linux", feature = "pipewire"))]
        if let Some(driver) = self.pipewire.as_mut() {
            driver.refresh()?;
            self.refresh_schedule.pipewire_deadline = Some(now + EVENT_REFRESH_INTERVAL);
        }
        #[cfg(all(target_os = "linux", feature = "alsa"))]
        if let Some(driver) = self.alsa.as_mut() {
            driver.refresh()?;
            self.refresh_schedule.alsa_deadline = Some(now + ALSA_REFRESH_INTERVAL);
        }
        #[cfg(target_os = "windows")]
        if let Some(driver) = self.windows_audio.as_mut() {
            driver.refresh()?;
            self.refresh_schedule.windows_audio_deadline = Some(now + EVENT_REFRESH_INTERVAL);
        }
        #[cfg(target_os = "windows")]
        if let Some(driver) = self.windows_midi.as_mut() {
            driver.refresh()?;
            self.refresh_schedule.windows_midi_deadline = Some(now + WINDOWS_MIDI_REFRESH_INTERVAL);
        }
        self.rebuild_merged_graph()?;
        Ok(self.graph.nodes.values().cloned().collect())
    }

    pub(super) fn refresh_due_children(&mut self) -> BackendResult<Vec<Node>> {
        #[cfg(any(
            target_os = "windows",
            all(target_os = "linux", any(feature = "pipewire", feature = "alsa"))
        ))]
        let now = Instant::now();
        #[cfg(any(
            target_os = "windows",
            all(target_os = "linux", any(feature = "pipewire", feature = "alsa"))
        ))]
        let mut changed = false;

        #[cfg(all(target_os = "linux", feature = "pipewire"))]
        if let Some(driver) = self.pipewire.as_mut() {
            if refresh_due(
                self.refresh_schedule.pipewire_deadline,
                driver.graph_dirty(),
                now,
            ) {
                driver.refresh_if_needed()?;
                self.refresh_schedule.pipewire_deadline = Some(now + EVENT_REFRESH_INTERVAL);
                changed = true;
            }
        }
        #[cfg(all(target_os = "linux", feature = "alsa"))]
        if let Some(driver) = self.alsa.as_mut() {
            if refresh_due(
                self.refresh_schedule.alsa_deadline,
                driver.graph_dirty(),
                now,
            ) {
                driver.refresh_if_needed()?;
                self.refresh_schedule.alsa_deadline = Some(now + ALSA_REFRESH_INTERVAL);
                changed = true;
            }
        }
        #[cfg(target_os = "windows")]
        if let Some(driver) = self.windows_audio.as_mut() {
            if refresh_due(
                self.refresh_schedule.windows_audio_deadline,
                driver.graph_dirty(),
                now,
            ) {
                driver.refresh_if_needed()?;
                self.refresh_schedule.windows_audio_deadline = Some(now + EVENT_REFRESH_INTERVAL);
                changed = true;
            }
        }
        #[cfg(target_os = "windows")]
        if let Some(driver) = self.windows_midi.as_mut() {
            if refresh_due(
                self.refresh_schedule.windows_midi_deadline,
                driver.graph_dirty(),
                now,
            ) {
                driver.refresh_if_needed()?;
                self.refresh_schedule.windows_midi_deadline =
                    Some(now + WINDOWS_MIDI_REFRESH_INTERVAL);
                changed = true;
            }
        }

        #[cfg(any(
            target_os = "windows",
            all(target_os = "linux", any(feature = "pipewire", feature = "alsa"))
        ))]
        if changed {
            self.rebuild_merged_graph()?;
        }
        Ok(self.graph.nodes.values().cloned().collect())
    }

    /// Only trustworthy when every live child reports its own changes: one
    /// child that must be polled means the composite must be polled.
    pub(super) fn children_report_graph_changes(&self) -> bool {
        #[cfg(any(
            target_os = "windows",
            all(target_os = "linux", any(feature = "pipewire", feature = "alsa"))
        ))]
        {
            let mut children = 0;
            let mut reporting = 0;
            #[cfg(all(target_os = "linux", feature = "pipewire"))]
            if let Some(driver) = self.pipewire.as_ref() {
                children += 1;
                reporting += usize::from(driver.reports_graph_changes());
            }
            #[cfg(all(target_os = "linux", feature = "alsa"))]
            if let Some(driver) = self.alsa.as_ref() {
                children += 1;
                reporting += usize::from(driver.reports_graph_changes());
            }
            #[cfg(target_os = "windows")]
            if let Some(driver) = self.windows_audio.as_ref() {
                children += 1;
                reporting += usize::from(driver.reports_graph_changes());
            }
            #[cfg(target_os = "windows")]
            if let Some(driver) = self.windows_midi.as_ref() {
                children += 1;
                reporting += usize::from(driver.reports_graph_changes());
            }
            children > 0 && children == reporting
        }
        #[cfg(not(any(
            target_os = "windows",
            all(target_os = "linux", any(feature = "pipewire", feature = "alsa"))
        )))]
        {
            false
        }
    }

    pub(super) fn merged_graph(&self) -> &Graph {
        &self.graph
    }

    pub(super) fn any_child_dirty(&self) -> bool {
        #[cfg(all(target_os = "linux", feature = "pipewire"))]
        if self
            .pipewire
            .as_ref()
            .is_some_and(|driver| driver.graph_dirty())
        {
            return true;
        }
        #[cfg(all(target_os = "linux", feature = "alsa"))]
        if self
            .alsa
            .as_ref()
            .is_some_and(|driver| driver.graph_dirty())
        {
            return true;
        }
        #[cfg(target_os = "windows")]
        if self
            .windows_audio
            .as_ref()
            .is_some_and(|driver| driver.graph_dirty())
        {
            return true;
        }
        #[cfg(target_os = "windows")]
        if self
            .windows_midi
            .as_ref()
            .is_some_and(|driver| driver.graph_dirty())
        {
            return true;
        }
        false
    }
}
