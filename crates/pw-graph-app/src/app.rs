use crate::panels::PreferencesTab;
use pw_graph_backend::MeterPolicy;
use pw_graph_command::CommandStack;
use pw_graph_config::AppConfig;
use pw_graph_core::{Link, NodeId, PortKey};
use pw_graph_i18n::I18n;
use pw_graph_patchbay::Patchbay;
use pw_graph_ui::{GraphCanvas, UiDocument};
use std::path::PathBuf;
use std::time::Instant;

#[cfg(all(target_os = "linux", feature = "tray"))]
use crate::tray::tray_support;

mod bootstrap;
mod configuration;
pub(crate) mod effects;
mod graph_actions;
mod layout;
mod lifecycle;
mod metering;
mod patchbay;
#[cfg(feature = "relay")]
mod relay;
mod shortcuts;
mod ui_state;

pub(crate) use lifecycle::run;
#[cfg(feature = "relay")]
pub(crate) use relay::{RelayDeviceRow, RelayDeviceState, RelayPanelTab, RelayUiState};

pub(crate) struct QpwgraphApp {
    pub(crate) driver: Box<dyn crate::backend::AppDriver>,
    pub(crate) commands: CommandStack,
    pub(crate) canvas: GraphCanvas,
    /// Retained DOM-like state for reusable application controls and forms.
    pub(crate) ui_document: UiDocument,
    pub(crate) patchbay: Patchbay,
    pub(crate) config: AppConfig,
    config_saved_snapshot: AppConfig,
    config_dirty_since: Option<Instant>,
    pub(crate) config_file: PathBuf,
    pub(crate) patchbay_file: PathBuf,
    pub(crate) status: String,
    pub(crate) debug: bool,
    pub(crate) no_alsa_midi: bool,
    pub(crate) start_minimized: bool,
    pub(crate) i18n: I18n,
    pub(crate) backend_name: String,
    pub(crate) show_shortcuts: bool,
    pub(crate) show_history: bool,
    pub(crate) shortcut_search: String,
    pub(crate) shortcut_focus_search: bool,
    pub(crate) shortcut_scroll_epoch: u32,
    pub(crate) show_preferences: bool,
    pub(crate) preferences_tab: PreferencesTab,
    /// Bumped whenever the Preferences modal opens so its `ScrollArea` starts
    /// back at the top instead of reusing a scroll offset left over from
    /// before.
    pub(crate) preferences_scroll_epoch: u32,
    pub(crate) profile_name: String,
    pub(crate) last_meter_refresh: Instant,
    pub(crate) last_graph_refresh: Instant,
    /// Mirrors `config.audio_meters` so a change in the panel is pushed to the
    /// driver exactly once instead of on every frame.
    pub(crate) meter_policy: MeterPolicy,
    pub(crate) effect_gallery: Option<effects::EffectGalleryState>,
    pub(crate) effect_gallery_scroll_epoch: u32,
    #[cfg(feature = "relay")]
    pub(crate) relay: RelayUiState,
    #[cfg(feature = "relay")]
    pub(crate) show_relay: bool,
    #[cfg(all(target_os = "linux", feature = "tray"))]
    pub(crate) tray: Option<tray_support::State>,
}

impl QpwgraphApp {
    pub(crate) fn t(&self, key: &str) -> String {
        self.i18n.text(key)
    }

    pub(crate) fn tf(&self, key: &str, variables: &[(&str, String)]) -> String {
        self.i18n.format(key, variables)
    }

    /// Reports an operation failure in the status bar. Every fallible driver
    /// call funnels through this so the "… failed" message shape stays one
    /// definition instead of fourteen inline copies.
    pub(crate) fn status_error(&mut self, key: &str, error: &impl std::fmt::Display) {
        self.status = self.tf(key, &[("error", error.to_string())]);
    }

    /// Persists a config or patchbay file, reporting failures through the
    /// status bar, and returns whether the save succeeded. Both document
    /// types share this shell so the failure status key can't drift.
    pub(crate) fn persist_report(
        &mut self,
        result: Result<(), impl std::fmt::Display>,
        failure_key: &str,
    ) -> bool {
        match result {
            Ok(()) => true,
            Err(error) => {
                self.status_error(failure_key, &error);
                false
            }
        }
    }

    /// Runs a relay-panel method that needs both the app and `&mut` relay
    /// state at once. `RelayUiState` owns several long-lived handles that
    /// borrow the app while it mutates them, so callers must take the state
    /// out, call, and put it back — this helper owns that take/restore pair.
    #[cfg(feature = "relay")]
    pub(crate) fn with_relay<R>(&mut self, f: impl FnOnce(&mut Self, &mut RelayUiState) -> R) -> R {
        let mut relay = std::mem::take(&mut self.relay);
        let result = f(self, &mut relay);
        self.relay = relay;
        result
    }

    /// Every link touching a node, whether on its output or input side.
    /// Disconnect and effect-removal both tear down a node's whole link set,
    /// so they share this single definition of "touches this node".
    pub(crate) fn links_touching_node(&self, node: NodeId) -> Vec<Link> {
        self.driver
            .graph()
            .links
            .values()
            .filter(|link| {
                self.driver
                    .graph()
                    .port(link.output_port)
                    .is_some_and(|port| port.node_id == node)
                    || self
                        .driver
                        .graph()
                        .port(link.input_port)
                        .is_some_and(|port| port.node_id == node)
            })
            .cloned()
            .collect()
    }

    /// Stable (node, port) name pairs for a set of links, which is what the
    /// patchbay tracks connections by.
    pub(crate) fn stable_link_pairs(&self, links: &[Link]) -> Vec<(PortKey, PortKey)> {
        links
            .iter()
            .filter_map(|link| {
                self.driver
                    .graph()
                    .port_key(link.output_port)
                    .zip(self.driver.graph().port_key(link.input_port))
            })
            .collect()
    }
}
