use crate::model::{MeterReading, MeterState};
use crate::source::ReadOnlyGraphSource;
use pw_graph_backend::MeterPolicy;
use slint::ComponentHandle;
use std::collections::BTreeSet;

use super::app::PreviewApp;
use super::MainWindow;

pub(crate) fn refresh_meters(window: &MainWindow, preview: &mut PreviewApp) {
    if preview.source.meter_policy() == MeterPolicy::Disabled {
        preview.meters.clear();
        preview.meter_error = None;
        return;
    }

    let visible_audio_nodes = if window.window().is_minimized() {
        BTreeSet::new()
    } else {
        preview
            .snapshot
            .nodes
            .iter()
            .filter(|node| node.has_audio_controls)
            .map(|node| node.node_id)
            .collect()
    };

    if let Err(error) = preview.source.request_meters(&visible_audio_nodes) {
        record_meter_error(preview, error);
        return;
    }

    match preview.source.audio_meters() {
        Ok(readings) => {
            let live_state = if preview.source.is_demo() {
                MeterState::Demo
            } else {
                MeterState::Live
            };
            preview.meters = readings
                .into_iter()
                .map(|reading| {
                    let state = if reading.available && reading.age_ms <= 1_500 {
                        live_state
                    } else {
                        MeterState::Waiting
                    };
                    (
                        reading.node_id,
                        MeterReading {
                            rms: if state == MeterState::Waiting {
                                0.0
                            } else {
                                reading.rms.clamp(0.0, 1.0)
                            },
                            peak: if state == MeterState::Waiting {
                                0.0
                            } else {
                                reading.peak.clamp(0.0, 1.0)
                            },
                            state,
                        },
                    )
                })
                .collect();
            preview.meter_error = None;
        }
        Err(error) => record_meter_error(preview, error),
    }
}

fn record_meter_error(preview: &mut PreviewApp, error: String) {
    if preview.meter_error.as_deref() != Some(error.as_str()) {
        preview.status = format!("Audio monitoring is unavailable: {error}");
        preview.meter_error = Some(error);
    }
    preview.meters.clear();
}

pub(crate) fn meter_fallback(source: &ReadOnlyGraphSource) -> MeterState {
    if source.meter_policy() == MeterPolicy::Disabled {
        MeterState::Disabled
    } else if source.is_demo() {
        MeterState::Demo
    } else if source.has_meter_backend() {
        MeterState::Waiting
    } else {
        MeterState::Unavailable
    }
}
