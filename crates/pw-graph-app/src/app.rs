use crate::panels::PreferencesTab;
use pw_graph_backend::MeterPolicy;
use pw_graph_command::CommandStack;
use pw_graph_config::AppConfig;
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
pub(crate) use relay::{RelayPanelTab, RelayUiState};

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
}
