use crate::model::{GraphSnapshot, MeterReading, UiGraphState};
use crate::source::ApplicationDriver;
use pw_graph_command::CommandStack;
use pw_graph_config::AppConfig;
use pw_graph_core::PortKey;
use pw_graph_i18n::I18n;
use pw_graph_patchbay::Patchbay;
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

pub(crate) struct Application {
    pub(crate) source: ApplicationDriver,
    pub(crate) commands: CommandStack,
    pub(crate) patchbay: Patchbay,
    pub(crate) patchbay_file: PathBuf,
    pub(crate) config: AppConfig,
    pub(crate) config_file: PathBuf,
    pub(crate) config_saved_snapshot: AppConfig,
    pub(crate) config_dirty_since: Option<Instant>,
    pub(crate) i18n: I18n,
    pub(crate) view: UiGraphState,
    pub(crate) snapshot: GraphSnapshot,
    pub(crate) status: String,
    pub(crate) toast_message: String,
    pub(crate) toast_until: Option<Instant>,
    pub(crate) toast_error: bool,
    pub(crate) pending_connection_pin: Option<i32>,
    /// Transient effect-gallery configuration. It is deliberately kept out
    /// of `AppConfig` until the user confirms creation.
    pub(crate) effect_draft_id: Option<String>,
    pub(crate) effect_draft_enabled: bool,
    pub(crate) effect_draft_parameters: BTreeMap<String, f32>,
    pub(crate) debug: bool,
    pub(crate) last_refresh: Instant,
    pub(crate) meters: BTreeMap<pw_graph_core::NodeId, MeterReading>,
    pub(crate) meter_error: Option<String>,
    /// Audio controls are live UI state only. They are intentionally not
    /// restored from a second Slint-specific file on startup.
    #[cfg(feature = "relay")]
    pub(crate) relay_levels: BTreeMap<u64, f32>,
    #[cfg(feature = "relay")]
    pub(crate) relay_connecting: Option<String>,
}

const CONNECTION_TOAST_DURATION: Duration = Duration::from_secs(4);

pub(crate) fn set_connection_feedback(
    application: &mut Application,
    message: impl Into<String>,
    error: bool,
) {
    let message = message.into();
    application.status = message.clone();
    application.toast_message = message;
    application.toast_error = error;
    application.toast_until = Some(Instant::now() + CONNECTION_TOAST_DURATION);
}

pub(crate) fn toast_visible(application: &Application) -> bool {
    application
        .toast_until
        .is_some_and(|deadline| Instant::now() < deadline)
}

impl Application {
    /// Keep durable rules synchronized with the live graph while preserving
    /// the original endpoints of inserted effects. Numeric IDs are only a
    /// cache; names and port names are the durable identity.
    pub(crate) fn live_connection_keys(&self) -> Vec<(PortKey, PortKey)> {
        self.source
            .graph()
            .links
            .values()
            .filter(|link| self.source.is_link_mutable(link.id))
            .filter_map(|link| {
                self.source
                    .graph()
                    .port_key(link.output_port)
                    .zip(self.source.graph().port_key(link.input_port))
            })
            .collect()
    }

    pub(crate) fn sync_patchbay_connections(&mut self) {
        for (output, input) in self.live_connection_keys() {
            let Some(output_id) = self.source.graph().resolve_port_key(&output) else {
                continue;
            };
            let Some(input_id) = self.source.graph().resolve_port_key(&input) else {
                continue;
            };
            self.patchbay.add_graph_connection(
                self.source.graph(),
                output_id,
                input_id,
                self.config.patchbay_auto_pin,
            );
        }
    }

    /// Remove only rules corresponding to a deliberate user disconnect. A
    /// refresh must never discard unresolved saved intent: nodes can be
    /// temporarily absent while PipeWire or ALSA is starting.
    pub(crate) fn remove_patchbay_connections(&mut self, pairs: &[(PortKey, PortKey)]) {
        for (output, input) in pairs {
            self.patchbay.remove_stable_connection(output, input);
        }
    }

    pub(crate) fn autosave_patchbay(&mut self) {
        if let Err(error) = self.patchbay.save_to(&self.patchbay_file) {
            self.status = self.tf(
                "status.patchbay_save_failed",
                &[("error", error.to_string())],
            );
        }
    }

    pub(crate) fn t(&self, key: &str) -> String {
        self.i18n.text(key)
    }

    pub(crate) fn tf(&self, key: &str, values: &[(&str, String)]) -> String {
        self.i18n.format(key, values)
    }

    pub(crate) fn history(&self) -> (Vec<String>, Vec<String>) {
        (self.commands.undo_history(), self.commands.redo_history())
    }
}
