use crate::args::Args;
use crate::canvas::CanvasGeometry;
use crate::model::{restore_node_positions, GraphSnapshot, UiGraphState};
use crate::source::ApplicationDriver;
use pw_graph_backend::MeterPolicy;
use pw_graph_config::{config_path, AppConfig};
use pw_graph_i18n::I18n;
use pw_graph_patchbay::Patchbay;
use slint::{ComponentHandle, ModelRc, PhysicalSize, SharedString, Timer, TimerMode, VecModel};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

mod actions;
mod app;
mod callbacks;
mod config;
mod connections;
mod effects;
mod events;
mod meters;
mod models;
mod patchbay;
mod relay;
mod utils;
use app::*;
use callbacks::*;
use config::*;
use effects::*;
use events::*;
use meters::*;
use models::*;
use patchbay::*;
use relay::*;
use utils::*;

slint::include_modules!();

pub(crate) struct UiBridge {
    window: MainWindow,
    app: Rc<RefCell<Application>>,
    nodes: Rc<VecModel<NodeRow>>,
    links: Rc<VecModel<LinkRow>>,
    minimap_nodes: Rc<VecModel<MinimapNode>>,
    shortcuts: Rc<VecModel<ShortcutRow>>,
    events: Rc<RefCell<Vec<UiEvent>>>,
    geometry: Rc<RefCell<CanvasGeometry>>,
    geometry_version: Rc<Cell<i32>>,
}

impl UiBridge {
    pub(crate) fn new(args: Args) -> Result<Self, slint::PlatformError> {
        let config_file = config_path("qpwgraph-rs");
        let config = AppConfig::load_from(&config_file).unwrap_or_default();
        let language = args
            .language
            .clone()
            .unwrap_or_else(|| config.language.clone());
        let i18n = I18n::from_language(&language);
        let meter_policy = MeterPolicy::parse(&config.audio_meters);
        let (mut source, mut status) = ApplicationDriver::new(&args, meter_policy, &i18n);
        restore_node_positions(&mut source, &config);
        let patchbay_file = selected_patchbay_path(&config);
        let patchbay = Patchbay::load_from(&patchbay_file)
            .unwrap_or_else(|_| Patchbay::new(patchbay_file.display().to_string()));
        restore_standalone_effects(&mut source, &config, &mut status, &i18n);
        if config.patchbay_activated {
            match patchbay.activate(
                &mut source,
                config.patchbay_exclusive,
                config.patchbay_auto_disconnect,
            ) {
                Ok(report) if report.failed.is_empty() => {}
                Ok(report) => status.push_str(&format!(
                    " · {}",
                    i18n.format(
                        "status.activation_failed",
                        &[("error", report.failed.join("; "))],
                    )
                )),
                Err(error) => status.push_str(&format!(
                    " · {}",
                    i18n.format("status.activation_failed", &[("error", error.to_string())])
                )),
            }
        }
        restore_inserted_effects(&mut source, &config, &mut status, &i18n);
        if !config.patchbay_activated {
            restore_effect_connections(&mut source, &patchbay, &mut status, &i18n);
        }
        if let Err(error) = source.refresh() {
            status = format!(
                "{status} · {}",
                i18n.format("status.refresh_failed", &[("error", error)])
            );
        }
        let view = UiGraphState::from_config(&config);
        let app = Rc::new(RefCell::new(Application {
            source,
            commands: pw_graph_command::CommandStack::new(),
            patchbay,
            patchbay_file,
            config: config.clone(),
            config_file,
            config_saved_snapshot: config,
            config_dirty_since: None,
            i18n,
            view,
            snapshot: GraphSnapshot::default(),
            status,
            toast_message: String::new(),
            toast_until: None,
            toast_error: false,
            pending_connection_pin: None,
            effect_draft_id: None,
            effect_draft_enabled: true,
            effect_draft_parameters: BTreeMap::new(),
            debug: args.debug,
            last_refresh: Instant::now(),
            meters: BTreeMap::new(),
            meter_error: None,
            #[cfg(feature = "relay")]
            relay_levels: BTreeMap::new(),
            #[cfg(feature = "relay")]
            relay_connecting: None,
        }));

        let window = MainWindow::new()?;
        {
            let app_for_text = app.clone();
            window.global::<UiI18n>().on_text(move |key| {
                SharedString::from(app_for_text.borrow().i18n.text(key.as_str()))
            });
            let app_for_format = app.clone();
            window.global::<UiI18n>().on_format_one(move |key, value| {
                let value = value.to_string();
                SharedString::from(app_for_format.borrow().i18n.format(
                    key.as_str(),
                    &[
                        ("count", value.clone()),
                        ("path", value.clone()),
                        ("port", value.clone()),
                        ("pin", value),
                    ],
                ))
            });
            let application = app.borrow();
            window
                .global::<UiI18n>()
                .set_version(language_index(&application.config.language));
        }
        {
            let application = app.borrow();
            window.window().set_size(PhysicalSize::new(
                application.config.window_width.max(760.0).round() as u32,
                application.config.window_height.max(520.0).round() as u32,
            ));
            window.set_show_statusbar(application.config.statusbar);
            window.set_show_minimap(application.view.minimap_visible);
            window.set_sort_type(SharedString::from(if application.view.sort_ports_by_name {
                "name"
            } else {
                "id"
            }));
            window.set_sort_order(SharedString::from(
                if application.view.sort_ports_descending {
                    "descending"
                } else {
                    "ascending"
                },
            ));
            window.set_search_text(SharedString::from(application.view.search_query.clone()));
            window.set_media_filter(SharedString::from(application.view.media_filter.as_str()));
            window.set_connect_mode(SharedString::from(application.view.connect_mode.as_str()));
            window.set_pan_x(application.view.pan[0]);
            window.set_pan_y(application.view.pan[1]);
            window.set_zoom(application.view.zoom);
            window.set_show_common_actions(application.config.toolbar);
            window.set_show_patchbay_toolbar(application.config.patchbay_toolbar);
            window.set_repel_overlaps(application.config.repel_overlapping_nodes);
            window.set_connect_through(application.config.connect_through_nodes);
            window.set_thumbnail_view(application.view.thumbnail_mode);
            window.set_language_index(language_index(&application.config.language));
            window.set_meter_policy_index(meter_policy_index(meter_policy));
            window.set_ui_text_scale(application.config.ui_text_scale);
            window.set_panel_text_scale(application.config.panel_text_scale);
            window.set_node_text_scale(application.config.node_text_scale);
            window
                .global::<UiTheme>()
                .set_ui_scale(application.config.ui_text_scale);
            window
                .global::<UiTheme>()
                .set_panel_scale(application.config.panel_text_scale);
            window
                .global::<UiTheme>()
                .set_node_scale(application.config.node_text_scale);
            window.set_patchbay_exclusive(application.config.patchbay_exclusive);
            window.set_patchbay_auto_disconnect(application.config.patchbay_auto_disconnect);
            window.set_patchbay_auto_pin(application.config.patchbay_auto_pin);
            window.set_patchbay_activated(application.config.patchbay_activated);
            window.set_profile_name(SharedString::from(
                application.config.active_patchbay_profile.clone(),
            ));
            window.set_config_path(SharedString::from(
                application.config_file.display().to_string(),
            ));
            window.set_patchbay_path(SharedString::from(
                selected_patchbay_path(&application.config)
                    .display()
                    .to_string(),
            ));
            window.set_profile_options(string_model(profile_options(&application.config)));
            window.set_profile_index(profile_index(&application.config));
            window.set_recent_patchbay_paths(string_model(recent_patchbay_paths(
                &application.config,
            )));
            window.set_relay_device_name(SharedString::from(
                application.config.relay_device_name.clone(),
            ));
            window.set_relay_host_pin(SharedString::from(
                application.config.relay_host_pin.clone(),
            ));
            window.set_relay_host_port_text(SharedString::from(
                application.config.relay_host_port.to_string(),
            ));
            window.set_relay_client_target(SharedString::from(
                application.config.relay_client_target.clone(),
            ));
            window.set_relay_client_pin(SharedString::from(
                application.config.relay_client_pin.clone(),
            ));
            window.set_relay_role_index(relay_role_index(&application.config.relay_role));
            window.set_relay_codec_index(relay_codec_index(&application.config.relay_codec));
            window.set_relay_frame_index(relay_frame_index(application.config.relay_frame_ms));
            window.set_relay_transport_index(relay_transport_index(
                &application.config.relay_transport,
            ));
            window.set_relay_codec_options(string_model([
                application.i18n.text("relay.codec_opus"),
                application.i18n.text("relay.codec_pcm"),
            ]));
            window.set_relay_frame_options(string_model(
                [5, 10, 20, 40, 60]
                    .into_iter()
                    .map(|frame| {
                        application
                            .i18n
                            .format("relay.frame_option", &[("frame", frame.to_string())])
                    })
                    .collect::<Vec<_>>(),
            ));
            window.set_relay_transport_options(string_model([
                application.i18n.text("relay.transport_auto"),
                application.i18n.text("relay.transport_wifi"),
                application.i18n.text("relay.transport_bluetooth_pan"),
                application.i18n.text("relay.transport_lan"),
            ]));
            window.window().set_minimized(args.minimized);
        }

        let nodes = Rc::new(VecModel::default());
        let links = Rc::new(VecModel::default());
        let minimap_nodes = Rc::new(VecModel::default());
        let shortcuts = Rc::new(VecModel::from(shortcut_rows(&app.borrow().i18n, "")));
        window.set_nodes(ModelRc::from(nodes.clone()));
        window.set_links(ModelRc::from(links.clone()));
        window.set_minimap_nodes(ModelRc::from(minimap_nodes.clone()));
        window.set_shortcuts(ModelRc::from(shortcuts.clone()));
        window.set_rules(ModelRc::from(Rc::new(VecModel::from(rule_rows(
            &app.borrow().patchbay,
        )))));
        {
            let application = app.borrow();
            window.set_effects(ModelRc::from(Rc::new(VecModel::from(effect_rows(
                &application.source,
                &application.i18n,
            )))));
            window.set_relay_rows(ModelRc::from(Rc::new(VecModel::from(relay_rows(
                &application,
                &application.i18n,
            )))));
            window.set_effect_options(ModelRc::from(Rc::new(VecModel::from(effect_options(
                &application.source,
            )))));
        }

        let events = Rc::new(RefCell::new(Vec::new()));
        let bridge = Self {
            window,
            app,
            nodes,
            links,
            minimap_nodes,
            shortcuts,
            events,
            geometry: Rc::new(RefCell::new(CanvasGeometry::default())),
            geometry_version: Rc::new(Cell::new(0)),
        };
        bridge.install_callbacks();
        bridge.refresh_meters();
        bridge.sync_models();
        Ok(bridge)
    }

    fn install_callbacks(&self) {
        let events = self.events.clone();
        self.window.on_action(move |action| {
            events
                .borrow_mut()
                .push(UiEvent::Action(action.to_string()));
        });
        install_canvas_callbacks(
            &self.window,
            &self.nodes,
            &self.links,
            &self.geometry,
            &self.events,
        );
    }

    pub(crate) fn run(self) -> Result<(), slint::PlatformError> {
        #[cfg(all(target_os = "linux", feature = "tray"))]
        let tray = {
            let application = self.app.borrow();
            Rc::new(RefCell::new(crate::tray::support::start(
                application.t("tray.show"),
                application.t("tray.hide"),
                application.t("tray.quit"),
            )))
        };
        let timer = Timer::default();
        let weak_window = self.window.as_weak();
        let app = self.app.clone();
        let nodes = self.nodes.clone();
        let links = self.links.clone();
        let minimap_nodes = self.minimap_nodes.clone();
        let shortcuts = self.shortcuts.clone();
        let events = self.events.clone();
        let geometry = self.geometry.clone();
        let geometry_version = self.geometry_version.clone();
        #[cfg(all(target_os = "linux", feature = "tray"))]
        let tray_for_timer = tray.clone();
        timer.start(TimerMode::Repeated, Duration::from_millis(50), move || {
            let Some(window) = weak_window.upgrade() else {
                return;
            };
            #[cfg(all(target_os = "linux", feature = "tray"))]
            crate::tray::support::poll(&window, &tray_for_timer);
            pump(
                &window,
                &app,
                &nodes,
                &links,
                &minimap_nodes,
                &shortcuts,
                &events,
                &geometry,
                &geometry_version,
            );
        });
        let result = self.window.run();
        {
            let mut application = self.app.borrow_mut();
            read_window_state(&self.window, &mut application);
            application.sync_patchbay_connections();
            application.autosave_patchbay();
            save_config(&mut application, false);
            application.source.reset_meters();
            #[cfg(feature = "relay")]
            {
                if application.source.relay_status().host_active {
                    let _ = application.source.relay_stop_host();
                }
                application.source.relay_discovery_stop();
            }
        }
        #[cfg(all(target_os = "linux", feature = "tray"))]
        if let Some(state) = tray.borrow().as_ref() {
            state.shutdown();
        }
        result
    }

    fn sync_models(&self) {
        let mut app = self.app.borrow_mut();
        sync_models(
            &self.window,
            &mut app,
            &self.nodes,
            &self.links,
            &self.minimap_nodes,
            &self.geometry,
            &self.geometry_version,
        );
    }

    fn refresh_meters(&self) {
        let mut app = self.app.borrow_mut();
        refresh_meters(&self.window, &mut app);
    }
}

#[cfg(test)]
mod tests {
    use super::actions::handle_action;
    use super::connections::{
        connect_pin_pair, delete_selected_connections, easy_connect_from_pin, easy_connect_nodes,
        handle_link_requested,
    };
    use super::*;
    use crate::canvas::{self, HIT_NODE, HIT_NODE_BODY};
    use crate::model::ConnectMode;
    use pw_graph_core::Direction;
    use slint::platform::{PointerEventButton, WindowEvent};
    use slint::{LogicalPosition, ModelRc};
    use std::path::PathBuf;

    fn demo_application() -> Application {
        let args = Args {
            demo: true,
            ..Args::default()
        };
        let i18n = I18n::from_language("en");
        let (source, status) = ApplicationDriver::new(&args, MeterPolicy::Disabled, &i18n);
        let config = AppConfig::default();
        let mut view = UiGraphState::from_config(&config);
        let snapshot = view.snapshot(source.graph(), &config);
        Application {
            source,
            commands: pw_graph_command::CommandStack::new(),
            patchbay: Patchbay::new("test"),
            patchbay_file: PathBuf::new(),
            config: config.clone(),
            config_file: PathBuf::new(),
            config_saved_snapshot: config,
            config_dirty_since: None,
            i18n,
            view,
            snapshot,
            status,
            toast_message: String::new(),
            toast_until: None,
            toast_error: false,
            pending_connection_pin: None,
            effect_draft_id: None,
            effect_draft_enabled: true,
            effect_draft_parameters: BTreeMap::new(),
            debug: false,
            last_refresh: Instant::now(),
            meters: BTreeMap::new(),
            meter_error: None,
            #[cfg(feature = "relay")]
            relay_levels: BTreeMap::new(),
            #[cfg(feature = "relay")]
            relay_connecting: None,
        }
    }

    #[test]
    fn advanced_pin_connections_reach_demo_backend_in_both_directions() {
        let mut application = demo_application();
        let output = application.view.ids.port(pw_graph_core::PortId(1)).unwrap();
        let input = application.view.ids.port(pw_graph_core::PortId(3)).unwrap();

        connect_pin_pair(&mut application, output, input);
        assert!(application
            .source
            .graph()
            .links
            .values()
            .any(|link| link.output_port.0 == 1 && link.input_port.0 == 3));
        assert_eq!(application.toast_message, "Connection created");
        assert!(!application.toast_error);

        connect_pin_pair(&mut application, input, output);
        assert_eq!(application.toast_message, "Connection already exists");
        assert!(!application.toast_error);
    }

    #[test]
    fn advanced_connection_rejects_stale_and_same_direction_pins() {
        let mut application = demo_application();
        let output = application.view.ids.port(pw_graph_core::PortId(1)).unwrap();
        let other_output = application.view.ids.port(pw_graph_core::PortId(2)).unwrap();

        handle_link_requested(&mut application, output, output);
        assert_eq!(application.pending_connection_pin, Some(output));
        assert!(!application.toast_error);
        assert!(application
            .toast_message
            .contains("click a destination pin"));

        handle_link_requested(&mut application, output, output);
        assert_eq!(application.pending_connection_pin, None);
        assert_eq!(application.toast_message, "Connection cancelled");

        connect_pin_pair(&mut application, output, 99_999);
        assert!(application.toast_error);
        assert!(application.toast_message.contains("no longer available"));

        connect_pin_pair(&mut application, output, other_output);
        assert!(application.toast_error);
        assert!(application.toast_message.contains("one output pin"));
    }

    #[test]
    fn delete_removes_the_selected_connection() {
        let mut application = demo_application();
        let output = application.view.ids.port(pw_graph_core::PortId(1)).unwrap();
        let input = application.view.ids.port(pw_graph_core::PortId(3)).unwrap();
        connect_pin_pair(&mut application, output, input);
        let link = *application.source.graph().links.keys().next().unwrap();
        application.view.selected_links.insert(link);

        delete_selected_connections(&mut application);

        assert!(application.source.graph().links.is_empty());
        assert!(application.view.selected_links.is_empty());
        assert_eq!(application.toast_message, "Removed 1 connection(s)");
        assert!(!application.toast_error);
    }

    #[test]
    fn easy_connections_create_all_matching_demo_channels() {
        let mut application = demo_application();
        let source = application.view.ids.node(pw_graph_core::NodeId(1)).unwrap();
        let target_position = application
            .snapshot
            .nodes
            .iter()
            .find(|node| node.node_id == pw_graph_core::NodeId(2))
            .map(|node| node.position)
            .unwrap();

        easy_connect_nodes(
            &mut application,
            source,
            target_position[0] + 10.0,
            target_position[1] + 10.0,
            0,
        );

        assert_eq!(application.source.graph().links.len(), 2);
        assert!(application.toast_message.contains("created 2 connection"));
        assert!(channels_are_paired_straight(&application));
    }

    #[test]
    fn easy_drop_accepts_the_visible_pin_margin() {
        let mut application = demo_application();
        let source = application.view.ids.node(pw_graph_core::NodeId(1)).unwrap();
        let target = application
            .snapshot
            .nodes
            .iter()
            .find(|node| node.node_id == pw_graph_core::NodeId(2))
            .unwrap();
        let pin_edge_x = target.position[0] - 6.0;
        let pin_y = target.position[1] + target.height / 2.0;

        easy_connect_nodes(&mut application, source, pin_edge_x, pin_y, 0);

        assert_eq!(application.source.graph().links.len(), 2);
        assert!(application.toast_message.contains("created 2 connection"));
    }

    #[test]
    fn easy_port_drag_connects_when_released_on_the_target_body() {
        let mut application = demo_application();
        // This drop is only reachable in Easy mode, where the pin the drag
        // started on stands for the whole capture channel group.
        application.view.connect_mode = ConnectMode::Easy;
        let source_pin = application.view.ids.port(pw_graph_core::PortId(1)).unwrap();
        let (target_x, target_y) = application
            .snapshot
            .nodes
            .iter()
            .find(|node| node.node_id == pw_graph_core::NodeId(2))
            .map(|node| {
                (
                    node.position[0] + node.width / 2.0,
                    node.position[1] + node.height / 2.0,
                )
            })
            .unwrap();

        easy_connect_from_pin(&mut application, source_pin, target_x, target_y);

        assert_eq!(application.source.graph().links.len(), 2);
        assert!(application.toast_message.contains("created 2 connection"));
        assert!(!application.toast_error);
        assert!(channels_are_paired_straight(&application));
    }

    #[test]
    fn two_pin_clicks_connect_without_holding_the_pointer() {
        let mut application = demo_application();
        let output = application.view.ids.port(pw_graph_core::PortId(1)).unwrap();
        let input = application.view.ids.port(pw_graph_core::PortId(3)).unwrap();

        handle_link_requested(&mut application, output, output);
        assert_eq!(application.pending_connection_pin, Some(output));

        handle_link_requested(&mut application, input, input);

        assert_eq!(application.pending_connection_pin, None);
        assert_eq!(application.source.graph().links.len(), 1);
        assert_eq!(application.toast_message, "Connection created");
        assert!(!application.toast_error);
    }

    #[test]
    fn two_pin_clicks_group_channels_in_easy_mode() {
        let mut application = demo_application();
        application.view.connect_mode = ConnectMode::Easy;
        let output = application.view.ids.port(pw_graph_core::PortId(1)).unwrap();
        let input = application.view.ids.port(pw_graph_core::PortId(3)).unwrap();

        handle_link_requested(&mut application, output, output);
        handle_link_requested(&mut application, input, input);

        assert_eq!(application.pending_connection_pin, None);
        assert_eq!(application.source.graph().links.len(), 2);
        assert!(application.toast_message.contains("created 2 connection"));
        assert!(!application.toast_error);
        assert!(channels_are_paired_straight(&application));
    }

    #[test]
    fn connection_feedback_is_transient() {
        let mut application = demo_application();
        set_connection_feedback(&mut application, "test connection", false);
        assert!(toast_visible(&application));

        application.toast_until = Some(Instant::now() - Duration::from_secs(1));
        assert!(!toast_visible(&application));
    }

    #[test]
    fn shortcut_catalog_matches_the_documented_help_dialog() {
        let i18n = I18n::from_language("en");
        assert_eq!(shortcut_rows(&i18n, "").len(), 22);
        let filtered = shortcut_rows(&i18n, "thumbnail");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].keys.as_str(), "T");
    }

    #[test]
    fn node_volume_track_preserves_unity_and_boost_range() {
        assert!((volume_from_track_position(0.9) - 1.0).abs() < f32::EPSILON);
        assert!((volume_from_track_position(1.0) - 1.5).abs() < f32::EPSILON);
        assert!((volume_from_track_position(0.45) - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn meter_track_uses_dbfs_scale() {
        assert_eq!(meter_fraction(0.0), 0.0);
        assert_eq!(meter_fraction(0.001), 0.0);
        assert!((meter_fraction(0.01) - (1.0 / 3.0)).abs() < 0.001);
        assert_eq!(meter_fraction(1.0), 1.0);
    }

    #[test]
    fn audio_slider_updates_are_coalesced_per_node() {
        let compacted = coalesce_audio_volume_events(vec![
            UiEvent::SetAudioVolume(7, 0.1),
            UiEvent::SetAudioVolume(7, 0.8),
            UiEvent::SetAudioVolume(8, 0.4),
        ]);
        assert_eq!(compacted.len(), 2);
        assert!(
            matches!(compacted[0], UiEvent::SetAudioVolume(7, value) if (value - 0.8).abs() < f32::EPSILON)
        );
        assert!(
            matches!(compacted[1], UiEvent::SetAudioVolume(8, value) if (value - 0.4).abs() < f32::EPSILON)
        );
    }

    #[test]
    fn preference_indices_round_trip_supported_values() {
        for policy in MeterPolicy::ALL {
            assert_eq!(meter_policy_from_index(meter_policy_index(policy)), policy);
        }
        for code in ["en", "es", "fr"] {
            assert_eq!(language_code(language_index(code)), code);
        }
    }

    /// Drives the real window with the real canvas wiring: rows, geometry and
    /// callbacks are produced by the same code the application runs.
    struct CanvasHarness {
        window: MainWindow,
        nodes: Rc<VecModel<NodeRow>>,
        links: Rc<VecModel<LinkRow>>,
        geometry: Rc<RefCell<CanvasGeometry>>,
        events: Rc<RefCell<Vec<UiEvent>>>,
        application: Application,
    }

    /// The graph starts at the right edge of the icon rail.
    const RAIL_WIDTH: f32 = 76.0;

    impl CanvasHarness {
        fn new(connect_mode: ConnectMode) -> Self {
            i_slint_backend_testing::init_no_event_loop();
            let mut application = demo_application();
            application.view.connect_mode = connect_mode;
            // Anchor the viewport so world and screen differ only by the rail.
            application.view.pan = [0.0, 0.0];
            application.view.zoom = 1.0;

            let window = MainWindow::new().unwrap();
            window
                .window()
                .set_size(slint::LogicalSize::new(1400.0, 900.0));
            let nodes = Rc::new(VecModel::default());
            let links = Rc::new(VecModel::default());
            window.set_nodes(ModelRc::from(nodes.clone()));
            window.set_links(ModelRc::from(links.clone()));
            let geometry = Rc::new(RefCell::new(CanvasGeometry::default()));
            let events = Rc::new(RefCell::new(Vec::new()));
            install_canvas_callbacks(&window, &nodes, &links, &geometry, &events);

            let mut harness = Self {
                window,
                nodes,
                links,
                geometry,
                events,
                application,
            };
            harness.sync();
            harness
        }

        fn sync(&mut self) {
            let minimap = Rc::new(VecModel::default());
            let version = Rc::new(Cell::new(0));
            sync_models(
                &self.window,
                &mut self.application,
                &self.nodes,
                &self.links,
                &minimap,
                &self.geometry,
                &version,
            );
        }

        fn screen_of(&self, world: (f32, f32)) -> LogicalPosition {
            let zoom = self.application.view.zoom;
            let pan = self.application.view.pan;
            LogicalPosition::new(
                RAIL_WIDTH + pan[0] + world.0 * zoom,
                pan[1] + world.1 * zoom,
            )
        }

        /// Collapse a card through the same event the card's chevron sends.
        fn collapse(&mut self, node_id: i32) {
            process_event(
                &self.window,
                &mut self.application,
                UiEvent::ToggleCollapse(node_id),
            );
            self.sync();
        }

        /// Zoom the viewport the way the toolbar does, then push it to the UI.
        fn set_zoom(&mut self, zoom: f32) {
            self.application.view.zoom = zoom;
            self.sync();
        }

        fn dispatch(&self, event: WindowEvent) {
            self.window.window().dispatch_event(event);
            slint::platform::update_timers_and_animations();
        }

        fn drag(&self, from: (f32, f32), to: (f32, f32)) {
            self.dispatch(WindowEvent::PointerPressed {
                position: self.screen_of(from),
                button: PointerEventButton::Left,
            });
            self.dispatch(WindowEvent::PointerMoved {
                position: self.screen_of(to),
            });
            self.dispatch(WindowEvent::PointerReleased {
                position: self.screen_of(to),
                button: PointerEventButton::Left,
            });
        }

        fn click(&self, at: (f32, f32)) {
            self.dispatch(WindowEvent::PointerPressed {
                position: self.screen_of(at),
                button: PointerEventButton::Left,
            });
            self.dispatch(WindowEvent::PointerReleased {
                position: self.screen_of(at),
                button: PointerEventButton::Left,
            });
        }

        fn take_events(&self) -> Vec<UiEvent> {
            std::mem::take(&mut *self.events.borrow_mut())
        }

        /// World centre of a pin, exactly where the dot is drawn.
        fn pin(&self, pin_id: i32) -> (f32, f32) {
            let geometry = self.geometry.borrow();
            let pin = geometry.pin(pin_id).expect("pin is cached");
            (pin.x, pin.y)
        }

        /// A point on the card body that carries no widget of its own: below
        /// the header and below the audio block when the card has one.
        fn body_point(&self, card: &NodeRow) -> (f32, f32) {
            let top = canvas::BODY_TOP
                + if card.has_audio_controls {
                    canvas::AUDIO_BLOCK_HEIGHT
                } else {
                    canvas::PORT_LIST_TOP
                };
            (card.x + card.width / 2.0, card.y + top + 8.0)
        }

        /// A point on the curve the canvas actually draws for `link_id`, read
        /// back out of the rendered SVG commands rather than recomputed, so the
        /// test aims at the same pixels the user sees.
        fn point_on_rendered_link(&self, link_id: i32, t: f32) -> (f32, f32) {
            let commands = self.window.invoke_graph_link_path(
                link_id,
                self.window.get_geometry_version(),
                0.0,
                0.0,
            );
            let numbers: Vec<f32> = commands
                .as_str()
                .split_whitespace()
                .filter_map(|token| token.parse::<f32>().ok())
                .collect();
            assert_eq!(numbers.len(), 8, "a cubic path: {commands}");
            let curve = [
                (numbers[0], numbers[1]),
                (numbers[2], numbers[3]),
                (numbers[4], numbers[5]),
                (numbers[6], numbers[7]),
            ];
            let u = 1.0 - t;
            let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
            (
                a * curve[0].0 + b * curve[1].0 + c * curve[2].0 + d * curve[3].0,
                a * curve[0].1 + b * curve[1].1 + c * curve[2].1 + d * curve[3].1,
            )
        }

        /// Create one link through the real pin-drag gesture and render it.
        fn create_link(&mut self) -> i32 {
            let (output, input) = self.connectable_pair();
            self.drag(self.pin(output), self.pin(input));
            for event in self.take_events() {
                process_event(&self.window, &mut self.application, event);
            }
            self.sync();
            let rendered = rows_of(&self.links);
            assert_eq!(rendered.len(), 1, "the new link reaches the render model");
            rendered[0].id
        }

        fn link_row(&self, link_id: i32) -> LinkRow {
            rows_of(&self.links)
                .into_iter()
                .find(|link| link.id == link_id)
                .expect("link is rendered")
        }

        fn node_row(&self, node_id: i32) -> NodeRow {
            rows_of(&self.nodes)
                .into_iter()
                .find(|node| node.id == node_id)
                .expect("node is rendered")
        }

        /// First output pin and first input pin of two different cards.
        fn connectable_pair(&self) -> (i32, i32) {
            let output = self
                .application
                .snapshot
                .nodes
                .iter()
                .find_map(|node| {
                    node.ports
                        .iter()
                        .find(|port| port.direction != Direction::Sink)
                        .map(|port| (node.id, port.pin_id))
                })
                .expect("the demo graph has an output port");
            let input = self
                .application
                .snapshot
                .nodes
                .iter()
                .filter(|node| node.id != output.0)
                .find_map(|node| {
                    node.ports
                        .iter()
                        .find(|port| port.direction == Direction::Sink)
                        .map(|port| port.pin_id)
                })
                .expect("the demo graph has an input port on another card");
            (output.1, input)
        }
    }

    #[test]
    fn dragging_between_two_rendered_pins_requests_that_exact_pair() {
        let harness = CanvasHarness::new(ConnectMode::Advanced);
        let (output, input) = harness.connectable_pair();

        harness.drag(harness.pin(output), harness.pin(input));

        let requested = harness
            .take_events()
            .into_iter()
            .find_map(|event| match event {
                UiEvent::LinkRequested(start, end) => Some((start, end)),
                _ => None,
            });
        assert_eq!(requested, Some((output, input)));
    }

    #[test]
    fn a_pin_drag_released_over_empty_canvas_cancels_in_advanced_mode() {
        let harness = CanvasHarness::new(ConnectMode::Advanced);
        let (output, _) = harness.connectable_pair();

        harness.drag(harness.pin(output), (1150.0, 800.0));

        assert!(harness
            .take_events()
            .iter()
            .any(|event| matches!(event, UiEvent::LinkCancelled)));
    }

    #[test]
    fn easy_mode_accepts_a_pin_drag_that_lands_anywhere_on_the_target_card() {
        let harness = CanvasHarness::new(ConnectMode::Easy);
        let (output, input) = harness.connectable_pair();
        let target = harness.pin(input);
        // Well clear of the pin dot, but still inside the destination card.
        let body = (target.0 + 70.0, target.1 + 6.0);

        harness.drag(harness.pin(output), body);

        let dropped = harness
            .take_events()
            .into_iter()
            .find_map(|event| match event {
                UiEvent::LinkDropped(pin, x, y) => Some((pin, x, y)),
                _ => None,
            });
        let (pin, x, y) = dropped.expect("easy mode routes the drop to the whole-card handler");
        assert_eq!(pin, output);
        assert!((x - body.0).abs() < 1.0 && (y - body.1).abs() < 1.0);
    }

    #[test]
    fn easy_mode_connects_whole_cards_dragged_from_their_body() {
        let harness = CanvasHarness::new(ConnectMode::Easy);
        let (output, input) = harness.connectable_pair();
        let source_node = harness
            .geometry
            .borrow()
            .pin(output)
            .expect("pin is cached")
            .node_id;
        let source = harness.node_row(source_node);
        let from = harness.body_point(&source);

        harness.drag(from, harness.pin(input));

        let connected = harness
            .take_events()
            .into_iter()
            .find_map(|event| match event {
                UiEvent::NodeConnectDropped(id, _, _, target) => Some((id, target)),
                _ => None,
            });
        assert_eq!(connected, Some((source_node, input)));
    }

    #[test]
    fn advanced_mode_moves_the_card_when_its_body_is_dragged() {
        let mut harness = CanvasHarness::new(ConnectMode::Advanced);
        let card = harness.node_row(harness.application.snapshot.nodes[0].id);
        let from = harness.body_point(&card);

        harness.drag(from, (from.0 + 40.0, from.1 + 25.0));

        let moved = harness
            .take_events()
            .into_iter()
            .find_map(|event| match event {
                UiEvent::DragCommitted(id, dx, dy) => Some((id, dx, dy)),
                _ => None,
            });
        let (id, dx, dy) = moved.expect("a body drag moves the card in advanced mode");
        assert_eq!(id, card.id);
        assert!((dx - 40.0).abs() < 0.1 && (dy - 25.0).abs() < 0.1);
        // The rendered row follows immediately, without waiting for a refresh.
        let after = harness.node_row(card.id);
        assert!((after.x - card.x - 40.0).abs() < 0.1);
        assert!((after.y - card.y - 25.0).abs() < 0.1);
        // ...and the cached pins moved with it, so the edges stay attached.
        let pin = harness.application.snapshot.nodes[0]
            .ports
            .first()
            .map(|port| port.pin_id);
        if let Some(pin) = pin {
            let cached = harness.geometry.borrow().pin(pin).expect("pin is cached");
            assert!((cached.x - after.x - canvas::PIN_INSET).abs() < card.width);
        }
        harness.sync();
    }

    #[test]
    fn dragging_the_header_moves_the_whole_selection() {
        let harness = CanvasHarness::new(ConnectMode::Advanced);
        let first = harness.node_row(harness.application.snapshot.nodes[0].id);
        let second = harness.node_row(harness.application.snapshot.nodes[1].id);

        harness.click((first.x + 60.0, first.y + 12.0));
        harness.dispatch(WindowEvent::PointerPressed {
            position: harness.screen_of((second.x + 60.0, second.y + 12.0)),
            button: PointerEventButton::Left,
        });
        harness.dispatch(WindowEvent::PointerMoved {
            position: harness.screen_of((second.x + 60.0 + 30.0, second.y + 12.0 + 15.0)),
        });
        harness.dispatch(WindowEvent::PointerReleased {
            position: harness.screen_of((second.x + 60.0 + 30.0, second.y + 12.0 + 15.0)),
            button: PointerEventButton::Left,
        });

        // Clicking the second card replaced the selection, so only it moves.
        assert!((harness.node_row(second.id).x - second.x - 30.0).abs() < 0.1);
        assert!((harness.node_row(first.id).x - first.x).abs() < 0.1);
    }

    #[test]
    fn clicking_a_card_selects_it_and_clicking_the_background_clears_it() {
        let harness = CanvasHarness::new(ConnectMode::Advanced);
        let card = harness.node_row(harness.application.snapshot.nodes[0].id);

        harness.click((card.x + 60.0, card.y + 12.0));
        assert!(harness.node_row(card.id).selected);
        assert!(harness
            .geometry
            .borrow()
            .node(card.id)
            .is_some_and(|node| node.selected));

        harness.click((1150.0, 820.0));
        assert!(!harness.node_row(card.id).selected);
    }

    #[test]
    fn the_body_mute_button_keeps_its_own_pointer_gesture() {
        let harness = CanvasHarness::new(ConnectMode::Advanced);
        let card = harness
            .application
            .snapshot
            .nodes
            .iter()
            .find(|node| node.has_audio_controls)
            .map(|node| harness.node_row(node.id))
            .expect("the demo graph has a card with audio controls");

        // The mute button sits at the right of the audio block inside the body.
        harness.click((card.x + card.width - 22.0, card.y + canvas::BODY_TOP + 20.0));

        assert!(harness
            .take_events()
            .iter()
            .any(|event| matches!(event, UiEvent::ToggleAudioMute(id) if *id == card.id)));
    }

    #[test]
    fn a_created_link_is_rendered_as_a_curve_between_its_two_pins() {
        let mut harness = CanvasHarness::new(ConnectMode::Advanced);
        let (output, input) = harness.connectable_pair();

        harness.drag(harness.pin(output), harness.pin(input));
        for event in harness.take_events() {
            process_event(&harness.window, &mut harness.application, event);
        }
        harness.sync();

        let rendered = rows_of(&harness.links);
        assert_eq!(rendered.len(), 1, "the new link reaches the render model");
        let commands = harness.window.invoke_graph_link_path(
            rendered[0].id,
            harness.window.get_geometry_version(),
            0.0,
            0.0,
        );
        let start = harness.pin(output);
        let end = harness.pin(input);
        assert!(
            commands.starts_with(&format!("M {:.2} {:.2} C ", start.0, start.1)),
            "the curve starts on the output pin: {commands}"
        );
        assert!(
            commands.ends_with(&format!(" {:.2} {:.2}", end.0, end.1)),
            "the curve ends on the input pin: {commands}"
        );
    }

    #[test]
    fn clicking_between_curve_samples_selects_the_connection() {
        let mut harness = CanvasHarness::new(ConnectMode::Advanced);
        let link = harness.create_link();
        // 0.37 sits between the stops the old sampled hit test measured, which
        // is where a press used to fall through to the background.
        let on_curve = harness.point_on_rendered_link(link, 0.37);

        harness.click(on_curve);

        let selected = harness
            .take_events()
            .into_iter()
            .find_map(|event| match event {
                UiEvent::SelectLink(id, extend) => Some((id, extend)),
                _ => None,
            });
        assert_eq!(selected, Some((link, false)));
        assert!(
            harness.link_row(link).selected,
            "the rendered edge shows as selected"
        );
    }

    #[test]
    fn clicking_a_connection_at_half_zoom_selects_it() {
        let mut harness = CanvasHarness::new(ConnectMode::Advanced);
        let link = harness.create_link();
        harness.set_zoom(0.5);
        let on_curve = harness.point_on_rendered_link(link, 0.37);

        harness.click(on_curve);

        assert!(
            harness
                .take_events()
                .iter()
                .any(|event| matches!(event, UiEvent::SelectLink(id, _) if *id == link)),
            "a connection stays clickable when the canvas is zoomed out"
        );
        assert!(harness.link_row(link).selected);
    }

    /// The header is the move handle in both connect modes; only the blank
    /// card body is an Easy-mode connect surface.
    #[test]
    fn easy_mode_header_drag_still_moves_the_card() {
        let mut harness = CanvasHarness::new(ConnectMode::Easy);
        let card = harness.node_row(harness.application.snapshot.nodes[0].id);
        let header = (card.x + 60.0, card.y + 12.0);

        harness.drag(header, (header.0 + 40.0, header.1 + 25.0));

        let events = harness.take_events();
        let moved = events.iter().find_map(|event| match event {
            UiEvent::DragCommitted(id, dx, dy) => Some((*id, *dx, *dy)),
            _ => None,
        });
        let (id, dx, dy) = moved.expect("the header stays a move handle in easy mode");
        assert_eq!(id, card.id);
        assert!((dx - 40.0).abs() < 0.1 && (dy - 25.0).abs() < 0.1);
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, UiEvent::NodeConnectDropped(..))),
            "the header never starts an easy-connect gesture"
        );
        harness.sync();
    }

    /// A collapsed card draws no pins, so its whole body used to land in the
    /// `HIT_NODE` branch and connected only because Easy mode claimed every
    /// `HIT_NODE`. Now that the header is a move handle again, the geometry has
    /// to mark the body of a pinless card as a connect surface on its own.
    #[test]
    fn easy_mode_connects_a_collapsed_card_from_its_body() {
        let mut harness = CanvasHarness::new(ConnectMode::Easy);
        let (output, input) = harness.connectable_pair();
        let source_node = harness.geometry.borrow().pin(output).unwrap().node_id;
        harness.collapse(source_node);
        let card = harness.node_row(source_node);
        assert!(card.collapsed, "the card is collapsed");
        // Below the header, in the collapsed card's remaining strip.
        let body = (card.x + card.width / 2.0, card.y + card.height - 3.0);

        harness.drag(body, harness.pin(input));

        let events = harness.take_events();
        assert!(
            events.iter().any(
                |event| matches!(event, UiEvent::NodeConnectDropped(id, ..) if *id == source_node)
            ),
            "a collapsed card still connects from its body in easy mode"
        );
    }

    #[test]
    fn advanced_mode_header_drag_moves_the_card() {
        let mut harness = CanvasHarness::new(ConnectMode::Advanced);
        let card = harness.node_row(harness.application.snapshot.nodes[0].id);
        let header = (card.x + 60.0, card.y + 12.0);

        harness.drag(header, (header.0 + 40.0, header.1 + 25.0));

        let moved = harness
            .take_events()
            .into_iter()
            .find_map(|event| match event {
                UiEvent::DragCommitted(id, dx, dy) => Some((id, dx, dy)),
                _ => None,
            });
        let (id, dx, dy) = moved.expect("the header is a move handle in advanced mode");
        assert_eq!(id, card.id);
        assert!((dx - 40.0).abs() < 0.1 && (dy - 25.0).abs() < 0.1);
        harness.sync();
    }

    #[test]
    fn no_connect_backend_still_allows_link_selection() {
        let mut harness = CanvasHarness::new(ConnectMode::Advanced);
        let link = harness.create_link();
        let (output, input) = harness.connectable_pair();
        // What the UI does for a backend whose capabilities report connect and
        // disconnect as unsupported, such as Windows Core Audio.
        harness.window.set_connections_available(false);

        harness.click(harness.point_on_rendered_link(link, 0.37));
        assert!(
            harness
                .take_events()
                .iter()
                .any(|event| matches!(event, UiEvent::SelectLink(id, _) if *id == link)),
            "observed links stay selectable when routing is unsupported"
        );
        assert!(harness.link_row(link).selected);

        let card = harness.node_row(harness.application.snapshot.nodes[0].id);
        harness.click((card.x + 60.0, card.y + 12.0));
        assert!(
            harness
                .take_events()
                .iter()
                .any(|event| matches!(event, UiEvent::SelectNode(id, _) if *id == card.id)),
            "cards stay selectable too"
        );

        harness.drag(harness.pin(output), harness.pin(input));
        let events = harness.take_events();
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, UiEvent::LinkRequested(..))),
            "no routing is attempted"
        );
    }

    #[test]
    fn the_background_grid_is_generated_for_the_visible_canvas() {
        let harness = CanvasHarness::new(ConnectMode::Advanced);

        harness.window.invoke_graph_request_grid();

        let grid = harness.window.get_grid_commands();
        assert!(!grid.is_empty(), "the canvas asks Rust for its grid lines");
        assert!(grid.starts_with("M "));
    }

    #[test]
    fn easy_mode_pin_drag_connects_every_channel_of_the_two_groups() {
        let mut application = demo_application();
        application.view.connect_mode = ConnectMode::Easy;
        // In Easy mode the capture and playback cards each render one grouped
        // pin that stands for both FL and FR.
        let capture = application.view.ids.port(pw_graph_core::PortId(1)).unwrap();
        let playback = application.view.ids.port(pw_graph_core::PortId(3)).unwrap();

        handle_link_requested(&mut application, capture, playback);

        assert_eq!(application.source.graph().links.len(), 2);
        assert!(channels_are_paired_straight(&application));
    }

    #[test]
    fn easy_mode_pin_drag_keeps_the_channels_straight_when_dragged_backwards() {
        let mut application = demo_application();
        application.view.connect_mode = ConnectMode::Easy;
        let capture = application.view.ids.port(pw_graph_core::PortId(1)).unwrap();
        let playback = application.view.ids.port(pw_graph_core::PortId(3)).unwrap();

        handle_link_requested(&mut application, playback, capture);

        assert_eq!(application.source.graph().links.len(), 2);
        assert!(channels_are_paired_straight(&application));
    }

    /// Every link joins two ports whose channel suffix is the same, so left
    /// stays left and right stays right.
    fn channels_are_paired_straight(application: &Application) -> bool {
        let graph = application.source.graph();
        graph.links.values().all(|link| {
            let output = graph.port(link.output_port).map(|port| port.name.clone());
            let input = graph.port(link.input_port).map(|port| port.name.clone());
            match (output, input) {
                (Some(output), Some(input)) => {
                    let suffix =
                        |name: &str| name.rsplit('_').next().unwrap_or_default().to_owned();
                    suffix(&output) == suffix(&input)
                }
                _ => false,
            }
        })
    }

    #[test]
    fn an_easy_mode_pin_drag_on_the_canvas_connects_both_channels() {
        let mut harness = CanvasHarness::new(ConnectMode::Easy);
        let (output, input) = harness.connectable_pair();

        harness.drag(harness.pin(output), harness.pin(input));
        for event in harness.take_events() {
            process_event(&harness.window, &mut harness.application, event);
        }
        harness.sync();

        // One grouped pin at each end, two channels, two links.
        assert_eq!(harness.application.source.graph().links.len(), 2);
        assert!(channels_are_paired_straight(&harness.application));
        assert_eq!(rows_of(&harness.links).len(), 2);
    }

    #[test]
    fn an_easy_mode_card_drag_on_the_canvas_connects_both_channels() {
        let mut harness = CanvasHarness::new(ConnectMode::Easy);
        let (output, input) = harness.connectable_pair();
        let source = harness.node_row(
            harness
                .geometry
                .borrow()
                .pin(output)
                .expect("pin is cached")
                .node_id,
        );

        harness.drag(harness.body_point(&source), harness.pin(input));
        for event in harness.take_events() {
            process_event(&harness.window, &mut harness.application, event);
        }
        harness.sync();

        assert_eq!(harness.application.source.graph().links.len(), 2);
        assert!(channels_are_paired_straight(&harness.application));
    }

    #[test]
    fn toggling_to_easy_mode_enables_body_connect() {
        let mut harness = CanvasHarness::new(ConnectMode::Advanced);
        let (output, input) = harness.connectable_pair();
        let source_node = harness.geometry.borrow().pin(output).unwrap().node_id;
        let source = harness.node_row(source_node);
        let target_node = harness.geometry.borrow().pin(input).unwrap().node_id;
        let target = harness.node_row(target_node);
        let body_point = (source.x + source.width / 2.0, harness.body_point(&source).1);

        let hit = harness
            .geometry
            .borrow()
            .hit_test(body_point.0, body_point.1, 1.0);
        assert_eq!(hit.kind, HIT_NODE, "advanced mode drags the card");

        handle_action(
            &harness.window,
            &mut harness.application,
            "toggle-connect-mode",
        );
        harness.sync();

        let hit = harness
            .geometry
            .borrow()
            .hit_test(body_point.0, body_point.1, 1.0);
        assert_eq!(
            hit.kind, HIT_NODE_BODY,
            "easy mode turns the body into a connect gesture"
        );

        let body = (
            target.x + target.width / 2.0,
            target.y + target.height / 2.0,
        );
        harness.drag(harness.body_point(&source), body);
        for event in harness.take_events() {
            process_event(&harness.window, &mut harness.application, event);
        }
        harness.sync();

        assert_eq!(
            harness.application.source.graph().links.len(),
            2,
            "links created after toggling to easy"
        );
        assert!(channels_are_paired_straight(&harness.application));
    }

    #[test]
    fn pump_path_connects_body_to_body_after_toggling_to_easy() {
        i_slint_backend_testing::init_no_event_loop();
        let application = Rc::new(RefCell::new(demo_application()));
        application.borrow_mut().view.connect_mode = ConnectMode::Advanced;
        application.borrow_mut().view.pan = [0.0, 0.0];
        application.borrow_mut().view.zoom = 1.0;

        let window = MainWindow::new().unwrap();
        window
            .window()
            .set_size(slint::LogicalSize::new(1400.0, 900.0));
        let nodes = Rc::new(VecModel::default());
        let links = Rc::new(VecModel::default());
        window.set_nodes(ModelRc::from(nodes.clone()));
        window.set_links(ModelRc::from(links.clone()));
        let geometry = Rc::new(RefCell::new(CanvasGeometry::default()));
        let events = Rc::new(RefCell::new(Vec::new()));
        install_canvas_callbacks(&window, &nodes, &links, &geometry, &events);
        let minimap = Rc::new(VecModel::default());
        let shortcuts = Rc::new(VecModel::default());
        let version = Rc::new(Cell::new(0));

        let screen_of = |world: (f32, f32)| -> LogicalPosition {
            let application = application.borrow();
            LogicalPosition::new(
                RAIL_WIDTH + application.view.pan[0] + world.0,
                application.view.pan[1] + world.1,
            )
        };
        let body_of = |row: &NodeRow| -> (f32, f32) {
            let top = canvas::BODY_TOP
                + if row.has_audio_controls {
                    canvas::AUDIO_BLOCK_HEIGHT
                } else {
                    canvas::PORT_LIST_TOP
                };
            (row.x + row.width / 2.0, row.y + top + 8.0)
        };

        // First pump establishes advanced geometry, exactly like app startup.
        pump(
            &window,
            &application,
            &nodes,
            &links,
            &minimap,
            &shortcuts,
            &events,
            &geometry,
            &version,
        );

        // Toggle to Easy through the same events queue the toolbar uses.
        events
            .borrow_mut()
            .push(UiEvent::Action("toggle-connect-mode".into()));
        pump(
            &window,
            &application,
            &nodes,
            &links,
            &minimap,
            &shortcuts,
            &events,
            &geometry,
            &version,
        );

        let (output, input) = {
            let application = application.borrow();
            let output = application
                .snapshot
                .nodes
                .iter()
                .find_map(|node| {
                    node.ports
                        .iter()
                        .find(|port| port.direction != Direction::Sink)
                        .map(|port| port.pin_id)
                })
                .expect("the demo graph has an output port");
            let input = application
                .snapshot
                .nodes
                .iter()
                .find_map(|node| {
                    node.ports
                        .iter()
                        .find(|port| port.direction == Direction::Sink)
                        .map(|port| port.pin_id)
                })
                .expect("the demo graph has an input port on another card");
            (output, input)
        };
        let source_node = geometry
            .borrow()
            .pin(output)
            .expect("source pin cached")
            .node_id;
        let target_node = geometry
            .borrow()
            .pin(input)
            .expect("target pin cached")
            .node_id;
        let source = rows_of(&nodes)
            .into_iter()
            .find(|row| row.id == source_node)
            .unwrap();
        let target = rows_of(&nodes)
            .into_iter()
            .find(|row| row.id == target_node)
            .unwrap();
        let body = (
            target.x + target.width / 2.0,
            target.y + target.height / 2.0,
        );

        window.window().dispatch_event(WindowEvent::PointerPressed {
            position: screen_of(body_of(&source)),
            button: PointerEventButton::Left,
        });
        slint::platform::update_timers_and_animations();
        window.window().dispatch_event(WindowEvent::PointerMoved {
            position: screen_of(body),
        });
        slint::platform::update_timers_and_animations();
        window
            .window()
            .dispatch_event(WindowEvent::PointerReleased {
                position: screen_of(body),
                button: PointerEventButton::Left,
            });
        slint::platform::update_timers_and_animations();
        for event in std::mem::take(&mut *events.borrow_mut()) {
            process_event(&window, &mut application.borrow_mut(), event);
        }
        pump(
            &window,
            &application,
            &nodes,
            &links,
            &minimap,
            &shortcuts,
            &events,
            &geometry,
            &version,
        );

        assert_eq!(
            application.borrow().source.graph().links.len(),
            2,
            "links created"
        );
        assert!(channels_are_paired_straight(&application.borrow()));
    }

    #[test]
    fn body_drag_released_on_a_target_pin_still_connects_the_whole_group() {
        let mut harness = CanvasHarness::new(ConnectMode::Easy);
        let (output, input) = harness.connectable_pair();
        let source_node = harness.geometry.borrow().pin(output).unwrap().node_id;
        let source = harness.node_row(source_node);

        // Release the whole-card drag exactly on the target node's rendered
        // pin: the group under it must still pair both stereo channels.
        let drop = harness.pin(input);
        harness.drag(harness.body_point(&source), drop);
        for event in harness.take_events() {
            process_event(&harness.window, &mut harness.application, event);
        }
        harness.sync();

        assert_eq!(
            harness.application.source.graph().links.len(),
            2,
            "drop on a pin fills the whole group"
        );
        assert!(channels_are_paired_straight(&harness.application));
    }
}
