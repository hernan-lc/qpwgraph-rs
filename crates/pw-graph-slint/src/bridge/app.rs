use crate::model::{GraphSnapshot, MeterReading, UiGraphState};
use crate::source::ReadOnlyGraphSource;
use pw_graph_config::AppConfig;
use pw_graph_i18n::I18n;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub(crate) enum UiEvent {
    Action(String),
    SelectNode(i32, bool),
    SelectLink(i32, bool),
    ClearSelection,
    SelectBox(f32, f32, f32, f32, bool),
    LinkRequested(i32, i32),
    LinkCancelled,
    /// An Easy-mode drag of a whole card was dropped at a world position.
    NodeConnectDropped(i32, f32, f32, i32),
    /// A pin drag was dropped away from any pin.
    LinkDropped(i32, f32, f32),
    ToggleCollapse(i32),
    DragCommitted(i32, f32, f32),
    SetAudioVolume(i32, f32),
    ToggleAudioMute(i32),
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct PreviewAudioControl {
    pub(crate) volume_position: f32,
    pub(crate) muted: bool,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub(crate) struct PersistedSlintState {
    pub(crate) audio_controls: BTreeMap<String, PreviewAudioControl>,
}

impl Default for PreviewAudioControl {
    fn default() -> Self {
        Self {
            volume_position: 0.9,
            muted: false,
        }
    }
}

pub(crate) struct PreviewApp {
    pub(crate) source: ReadOnlyGraphSource,
    pub(crate) config: AppConfig,
    pub(crate) config_file: PathBuf,
    pub(crate) config_saved_snapshot: AppConfig,
    pub(crate) config_dirty_since: Option<Instant>,
    pub(crate) state_file: PathBuf,
    pub(crate) state_saved_snapshot: PersistedSlintState,
    pub(crate) state_dirty_since: Option<Instant>,
    pub(crate) i18n: I18n,
    pub(crate) view: UiGraphState,
    pub(crate) snapshot: GraphSnapshot,
    pub(crate) status: String,
    pub(crate) toast_message: String,
    pub(crate) toast_until: Option<Instant>,
    pub(crate) toast_error: bool,
    pub(crate) pending_connection_pin: Option<i32>,
    pub(crate) debug: bool,
    pub(crate) last_refresh: Instant,
    pub(crate) meters: BTreeMap<pw_graph_core::NodeId, MeterReading>,
    pub(crate) meter_error: Option<String>,
    pub(crate) audio_controls: BTreeMap<pw_graph_core::NodeId, PreviewAudioControl>,
    #[cfg(feature = "relay")]
    pub(crate) relay_levels: BTreeMap<u64, f32>,
    #[cfg(feature = "relay")]
    pub(crate) relay_connecting: Option<String>,
}

const CONNECTION_TOAST_DURATION: Duration = Duration::from_secs(4);

pub(crate) fn set_connection_feedback(
    preview: &mut PreviewApp,
    message: impl Into<String>,
    error: bool,
) {
    let message = message.into();
    preview.status = message.clone();
    preview.toast_message = message;
    preview.toast_error = error;
    preview.toast_until = Some(Instant::now() + CONNECTION_TOAST_DURATION);
}

pub(crate) fn toast_visible(preview: &PreviewApp) -> bool {
    preview
        .toast_until
        .is_some_and(|deadline| Instant::now() < deadline)
}
