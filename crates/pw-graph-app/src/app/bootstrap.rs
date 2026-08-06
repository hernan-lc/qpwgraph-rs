#[cfg(feature = "relay")]
use super::RelayUiState;
use super::{layout, QpwgraphApp};
use crate::args::Args;
use crate::backend::CompositeDriver;
use crate::panels::PreferencesTab;
#[cfg(feature = "alsa")]
use pw_graph_alsamidi::AlsaMidiDriver;
#[cfg(feature = "pipewire")]
use pw_graph_backend::PipewireDriver;
use pw_graph_backend::{DemoDriver, GraphDriver, MeterPolicy};
use pw_graph_config::{config_path, AppConfig};
use pw_graph_core::Graph;
use pw_graph_i18n::I18n;
use pw_graph_patchbay::Patchbay;
use pw_graph_ui::{ConnectMode, GraphCanvas, MediaFilter};
use std::time::{Duration, Instant};

#[cfg(all(target_os = "linux", feature = "tray"))]
use crate::tray::tray_support;

impl QpwgraphApp {
    pub(crate) fn new(args: Args) -> Self {
        let config_file = config_path("qpwgraph-rs");
        let config = AppConfig::load_from(&config_file).unwrap_or_default();
        let language = args
            .language
            .clone()
            .unwrap_or_else(|| config.language.clone());
        let i18n = I18n::from_language(&language);
        let default_patchbay_file = config_file.with_file_name("default.qpwgraph");
        let patchbay_file = config
            .patchbay_profiles
            .get(&config.active_patchbay_profile)
            .cloned()
            .or_else(|| config.patchbay_path.clone())
            .unwrap_or(default_patchbay_file);
        let mut status = i18n.text("status.backend_unavailable");
        let (mut driver, backend_name): (Box<dyn crate::backend::AppDriver>, String) = if args.demo
        {
            status = i18n.text("status.demo_ready");
            (Box::new(DemoDriver::demo()), "demo".into())
        } else {
            let mut composite = CompositeDriver::default();
            #[allow(unused_mut)]
            let mut has_pipewire = false;
            #[allow(unused_mut)]
            let mut has_alsa = false;

            #[cfg(feature = "pipewire")]
            match PipewireDriver::new() {
                Ok(driver) => {
                    composite.pipewire = Some(driver);
                    has_pipewire = true;
                }
                Err(error) => {
                    status = i18n.format("status.pipewire_failed", &[("error", error.to_string())]);
                }
            }

            #[cfg(feature = "alsa")]
            if !args.no_alsa_midi {
                match AlsaMidiDriver::new() {
                    Ok(driver) => {
                        composite.alsa = Some(driver);
                        has_alsa = true;
                    }
                    Err(error) => {
                        status = i18n.format("status.alsa_failed", &[("error", error.to_string())]);
                    }
                }
            }

            if has_pipewire || has_alsa {
                match composite.refresh() {
                    Ok(_) => {
                        status = if has_pipewire {
                            i18n.text("status.pipewire_ready")
                        } else {
                            i18n.text("status.alsa_ready")
                        };
                        let backend_name = match (has_pipewire, has_alsa) {
                            (true, true) => "pipewire+alsa",
                            (true, false) => "pipewire",
                            (false, true) => "alsa",
                            (false, false) => "in-memory",
                        };
                        (Box::new(composite), backend_name.into())
                    }
                    Err(error) => {
                        status =
                            i18n.format("status.backend_failed", &[("error", error.to_string())]);
                        (Box::new(DemoDriver::new(Graph::default())), "none".into())
                    }
                }
            } else {
                (Box::new(DemoDriver::new(Graph::default())), "none".into())
            }
        };
        // Applied before anything else touches the graph so a launch under the
        // default on-demand policy never attaches a metering stream on its own.
        let meter_policy = MeterPolicy::parse(&config.audio_meters);
        let _ = driver.set_meter_policy(meter_policy);
        layout::restore_node_positions(driver.as_mut(), &config);
        let patchbay = Patchbay::load_from(&patchbay_file).unwrap_or_else(|_| {
            Patchbay::new(
                patchbay_file
                    .file_stem()
                    .and_then(|value| value.to_str())
                    .unwrap_or("default"),
            )
        });
        let mut canvas = GraphCanvas::default();
        canvas.zoom = config.zoom;
        canvas.node_text_scale = config.node_text_scale;
        canvas.sort_ports_by_name = config.sort_type != "id";
        canvas.sort_ports_descending = config.sort_order == "descending";
        canvas.thumbnail_mode = config.thumbnail_view;
        canvas.minimap_visible = config.minimap_visible;
        canvas.repel_overlapping_nodes = config.repel_overlapping_nodes;
        canvas.connect_through_nodes = config.connect_through_nodes;
        canvas.connect_mode = ConnectMode::parse(&config.connect_mode);
        canvas.media_filter = MediaFilter::parse(&config.media_filter);
        canvas.search_query = config.graph_search.clone();
        canvas.metering_disabled = meter_policy == MeterPolicy::Disabled;
        let profile_name = config.active_patchbay_profile.clone();
        #[cfg(all(target_os = "linux", feature = "tray"))]
        let tray = tray_support::start(
            i18n.text("tray.show"),
            i18n.text("tray.hide"),
            i18n.text("tray.quit"),
        );
        let mut app = Self {
            driver,
            commands: pw_graph_command::CommandStack::new(),
            canvas,
            ui_document: pw_graph_ui::UiDocument::new(),
            patchbay,
            config_saved_snapshot: config.clone(),
            config_dirty_since: None,
            config,
            config_file,
            patchbay_file,
            status,
            debug: args.debug,
            no_alsa_midi: args.no_alsa_midi,
            start_minimized: args.minimized,
            i18n,
            backend_name,
            show_shortcuts: false,
            show_history: false,
            shortcut_search: String::new(),
            shortcut_focus_search: false,
            shortcut_scroll_epoch: 0,
            show_preferences: true,
            preferences_tab: PreferencesTab::Patchbay,
            preferences_scroll_epoch: 0,
            patchbay_rule_expanded: None,
            profile_name,
            last_meter_refresh: Instant::now() - Duration::from_secs(1),
            last_graph_refresh: Instant::now(),
            meter_policy,
            effect_gallery: None,
            effect_gallery_scroll_epoch: 0,
            #[cfg(feature = "relay")]
            relay: RelayUiState::default(),
            #[cfg(feature = "relay")]
            show_relay: false,
            #[cfg(all(target_os = "linux", feature = "tray"))]
            tray,
        };
        // Free-standing effects must exist before Patchbay resolves saved
        // manual links to their ports. Effects inserted into a saved direct
        // route are restored after activation, so they can replace that route
        // instead of being bypassed by it.
        app.restore_standalone_effects();
        if app.config.patchbay_activated {
            match app.patchbay.activate(
                app.driver.as_mut(),
                app.config.patchbay_exclusive,
                app.config.patchbay_auto_disconnect,
            ) {
                Ok(report) => {
                    app.status = app.i18n.format(
                        "status.activated",
                        &[
                            ("connected", report.connected.to_string()),
                            ("present", report.already_present.to_string()),
                            ("disconnected", report.disconnected.to_string()),
                        ],
                    );
                }
                Err(error) => {
                    app.status = app
                        .i18n
                        .format("status.activation_failed", &[("error", error.to_string())]);
                }
            }
        }
        app.restore_inserted_effects();
        if !app.config.patchbay_activated {
            app.restore_effect_connections();
        }
        app
    }
}
