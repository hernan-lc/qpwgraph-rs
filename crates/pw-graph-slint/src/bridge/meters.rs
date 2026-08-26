use crate::model::{MeterReading, MeterState};
use crate::source::ApplicationDriver;
use pw_graph_backend::MeterPolicy;
use slint::ComponentHandle;
use std::collections::BTreeSet;

use super::app::Application;
use super::MainWindow;

pub(crate) fn refresh_meters(window: &MainWindow, application: &mut Application) {
    if application.source.meter_policy() == MeterPolicy::Disabled {
        // Keep the request lifecycle explicit even when the policy is
        // disabled. This is important for a live backend that may have
        // streams left over from a previous policy selection.
        let _ = application.source.request_meters(&BTreeSet::new());
        application.meters.clear();
        application.meter_error = None;
        return;
    }

    let visible_audio_nodes = if window.window().is_minimized() || !window.window().is_visible() {
        BTreeSet::new()
    } else {
        application
            .snapshot
            .nodes
            .iter()
            .filter(|node| node.has_audio_controls)
            .map(|node| node.node_id)
            .collect()
    };

    if let Err(error) = application.source.request_meters(&visible_audio_nodes) {
        record_meter_error(application, error);
        return;
    }

    match application.source.audio_meters() {
        Ok(readings) => {
            let live_state = if application.source.is_demo() {
                MeterState::Demo
            } else {
                MeterState::Live
            };
            application.meters = readings
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
            application.meter_error = None;
        }
        Err(error) => record_meter_error(application, error),
    }
}

fn record_meter_error(application: &mut Application, error: String) {
    if application.meter_error.as_deref() != Some(error.as_str()) {
        application.status = application.tf(
            "status.audio_monitoring_unavailable",
            &[("error", error.clone())],
        );
        application.meter_error = Some(error);
    }
    application.meters.clear();
}

pub(crate) fn meter_fallback(source: &ApplicationDriver) -> MeterState {
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
