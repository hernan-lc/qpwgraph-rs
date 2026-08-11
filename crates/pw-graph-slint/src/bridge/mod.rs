use crate::args::Args;
use crate::canvas::CanvasGeometry;
use crate::model::{GraphSnapshot, UiGraphState};
use crate::source::ReadOnlyGraphSource;
use pw_graph_backend::MeterPolicy;
use pw_graph_config::{config_path, AppConfig};
use pw_graph_i18n::I18n;
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
mod persist;
mod relay;
mod utils;
use app::*;
use callbacks::*;
use config::*;
use effects::*;
use events::*;
use meters::*;
use models::*;
use persist::*;
use relay::*;
use utils::*;

slint::include_modules!();

pub(crate) struct UiBridge {
    window: MainWindow,
    app: Rc<RefCell<PreviewApp>>,
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
        let (mut source, mut status) = ReadOnlyGraphSource::new(&args, meter_policy);
        restore_configured_effects(&mut source, &config, &mut status);
        if !config.effects.is_empty() {
            if let Err(error) = source.refresh() {
                status = format!("{status} · Could not restore effects: {error}");
            }
        }
        let view = UiGraphState::from_config(&config);
        let state_file = config_file.with_file_name("slint-state.toml");
        let persisted_state = load_slint_state(&state_file);
        let audio_controls = restore_audio_controls(&mut source, &persisted_state);
        let app = Rc::new(RefCell::new(PreviewApp {
            source,
            config: config.clone(),
            config_file,
            config_saved_snapshot: config,
            config_dirty_since: None,
            state_file,
            state_saved_snapshot: persisted_state,
            state_dirty_since: None,
            i18n,
            view,
            snapshot: GraphSnapshot::default(),
            status,
            toast_message: String::new(),
            toast_until: None,
            toast_error: false,
            pending_connection_pin: None,
            debug: args.debug,
            last_refresh: Instant::now(),
            meters: BTreeMap::new(),
            meter_error: None,
            audio_controls,
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
            let preview = app.borrow();
            window
                .global::<UiI18n>()
                .set_version(language_index(&preview.config.language));
        }
        {
            let preview = app.borrow();
            window.window().set_size(PhysicalSize::new(
                preview.config.window_width.max(760.0).round() as u32,
                preview.config.window_height.max(520.0).round() as u32,
            ));
            window.set_show_statusbar(preview.config.statusbar);
            window.set_show_minimap(preview.view.minimap_visible);
            window.set_search_text(SharedString::from(preview.view.search_query.clone()));
            window.set_media_filter(SharedString::from(preview.view.media_filter.as_str()));
            window.set_connect_mode(SharedString::from(preview.view.connect_mode.as_str()));
            window.set_pan_x(preview.view.pan[0]);
            window.set_pan_y(preview.view.pan[1]);
            window.set_zoom(preview.view.zoom);
            window.set_show_common_actions(preview.config.toolbar);
            window.set_show_patchbay_toolbar(preview.config.patchbay_toolbar);
            window.set_repel_overlaps(preview.config.repel_overlapping_nodes);
            window.set_connect_through(preview.config.connect_through_nodes);
            window.set_thumbnail_view(preview.view.thumbnail_mode);
            window.set_language_index(language_index(&preview.config.language));
            window.set_meter_policy_index(meter_policy_index(meter_policy));
            window.set_ui_text_scale(preview.config.ui_text_scale);
            window.set_panel_text_scale(preview.config.panel_text_scale);
            window.set_node_text_scale(preview.config.node_text_scale);
            window.set_patchbay_exclusive(preview.config.patchbay_exclusive);
            window.set_patchbay_auto_disconnect(preview.config.patchbay_auto_disconnect);
            window.set_patchbay_auto_pin(preview.config.patchbay_auto_pin);
            window.set_patchbay_activated(preview.config.patchbay_activated);
            window.set_profile_name(SharedString::from(
                preview.config.active_patchbay_profile.clone(),
            ));
            window.set_config_path(SharedString::from(
                preview.config_file.display().to_string(),
            ));
            window.set_patchbay_path(SharedString::from(
                selected_patchbay_path(&preview.config)
                    .display()
                    .to_string(),
            ));
            window.set_relay_device_name(SharedString::from(
                preview.config.relay_device_name.clone(),
            ));
            window.set_relay_host_pin(SharedString::from(preview.config.relay_host_pin.clone()));
            window.set_relay_host_port_text(SharedString::from(
                preview.config.relay_host_port.to_string(),
            ));
            window.set_relay_client_target(SharedString::from(
                preview.config.relay_client_target.clone(),
            ));
            window
                .set_relay_client_pin(SharedString::from(preview.config.relay_client_pin.clone()));
            window.set_relay_role_index(relay_role_index(&preview.config.relay_role));
            window.set_relay_codec_index(relay_codec_index(&preview.config.relay_codec));
            window.set_relay_frame_index(relay_frame_index(preview.config.relay_frame_ms));
            window
                .set_relay_transport_index(relay_transport_index(&preview.config.relay_transport));
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
            &app.borrow().config,
        )))));
        {
            let preview = app.borrow();
            window.set_effects(ModelRc::from(Rc::new(VecModel::from(effect_rows(
                &preview.source,
            )))));
            window.set_relay_rows(ModelRc::from(Rc::new(VecModel::from(relay_rows(&preview)))));
            window.set_effect_options(ModelRc::from(Rc::new(VecModel::from(effect_options(
                &preview.source,
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
        timer.start(TimerMode::Repeated, Duration::from_millis(50), move || {
            let Some(window) = weak_window.upgrade() else {
                return;
            };
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
            let mut preview = self.app.borrow_mut();
            read_window_state(&self.window, &mut preview);
            save_config(&mut preview, false);
            save_slint_state(&mut preview, false);
            preview.source.reset_meters();
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

    fn demo_preview() -> PreviewApp {
        let args = Args {
            demo: true,
            ..Args::default()
        };
        let (source, status) = ReadOnlyGraphSource::new(&args, MeterPolicy::Disabled);
        let config = AppConfig::default();
        let mut view = UiGraphState::from_config(&config);
        let snapshot = view.snapshot(source.graph(), &config);
        PreviewApp {
            source,
            config: config.clone(),
            config_file: PathBuf::new(),
            config_saved_snapshot: config,
            config_dirty_since: None,
            state_file: PathBuf::new(),
            state_saved_snapshot: PersistedSlintState::default(),
            state_dirty_since: None,
            i18n: I18n::from_language("en"),
            view,
            snapshot,
            status,
            toast_message: String::new(),
            toast_until: None,
            toast_error: false,
            pending_connection_pin: None,
            debug: false,
            last_refresh: Instant::now(),
            meters: BTreeMap::new(),
            meter_error: None,
            audio_controls: BTreeMap::new(),
            #[cfg(feature = "relay")]
            relay_levels: BTreeMap::new(),
            #[cfg(feature = "relay")]
            relay_connecting: None,
        }
    }

    #[test]
    fn advanced_pin_connections_reach_demo_backend_in_both_directions() {
        let mut preview = demo_preview();
        let output = preview.view.ids.port(pw_graph_core::PortId(1)).unwrap();
        let input = preview.view.ids.port(pw_graph_core::PortId(3)).unwrap();

        connect_pin_pair(&mut preview, output, input);
        assert!(preview
            .source
            .graph()
            .links
            .values()
            .any(|link| link.output_port.0 == 1 && link.input_port.0 == 3));
        assert_eq!(preview.toast_message, "Connection created");
        assert!(!preview.toast_error);

        connect_pin_pair(&mut preview, input, output);
        assert_eq!(preview.toast_message, "Connection already exists");
        assert!(!preview.toast_error);
    }

    #[test]
    fn advanced_connection_rejects_stale_and_same_direction_pins() {
        let mut preview = demo_preview();
        let output = preview.view.ids.port(pw_graph_core::PortId(1)).unwrap();
        let other_output = preview.view.ids.port(pw_graph_core::PortId(2)).unwrap();

        handle_link_requested(&mut preview, output, output);
        assert_eq!(preview.pending_connection_pin, Some(output));
        assert!(!preview.toast_error);
        assert!(preview.toast_message.contains("click a destination pin"));

        handle_link_requested(&mut preview, output, output);
        assert_eq!(preview.pending_connection_pin, None);
        assert_eq!(preview.toast_message, "Connection cancelled");

        connect_pin_pair(&mut preview, output, 99_999);
        assert!(preview.toast_error);
        assert!(preview.toast_message.contains("no longer available"));

        connect_pin_pair(&mut preview, output, other_output);
        assert!(preview.toast_error);
        assert!(preview.toast_message.contains("one output pin"));
    }

    #[test]
    fn delete_removes_the_selected_connection() {
        let mut preview = demo_preview();
        let output = preview.view.ids.port(pw_graph_core::PortId(1)).unwrap();
        let input = preview.view.ids.port(pw_graph_core::PortId(3)).unwrap();
        connect_pin_pair(&mut preview, output, input);
        let link = *preview.source.graph().links.keys().next().unwrap();
        preview.view.selected_links.insert(link);

        delete_selected_connections(&mut preview);

        assert!(preview.source.graph().links.is_empty());
        assert!(preview.view.selected_links.is_empty());
        assert_eq!(preview.toast_message, "Removed 1 connection(s)");
        assert!(!preview.toast_error);
    }

    #[test]
    fn easy_connections_create_all_matching_demo_channels() {
        let mut preview = demo_preview();
        let source = preview.view.ids.node(pw_graph_core::NodeId(1)).unwrap();
        let target_position = preview
            .snapshot
            .nodes
            .iter()
            .find(|node| node.node_id == pw_graph_core::NodeId(2))
            .map(|node| node.position)
            .unwrap();

        easy_connect_nodes(
            &mut preview,
            source,
            target_position[0] + 10.0,
            target_position[1] + 10.0,
            0,
        );

        assert_eq!(preview.source.graph().links.len(), 2);
        assert!(preview.toast_message.contains("created 2 connection"));
        assert!(channels_are_paired_straight(&preview));
    }

    #[test]
    fn easy_drop_accepts_the_visible_pin_margin() {
        let mut preview = demo_preview();
        let source = preview.view.ids.node(pw_graph_core::NodeId(1)).unwrap();
        let target = preview
            .snapshot
            .nodes
            .iter()
            .find(|node| node.node_id == pw_graph_core::NodeId(2))
            .unwrap();
        let pin_edge_x = target.position[0] - 6.0;
        let pin_y = target.position[1] + target.height / 2.0;

        easy_connect_nodes(&mut preview, source, pin_edge_x, pin_y, 0);

        assert_eq!(preview.source.graph().links.len(), 2);
        assert!(preview.toast_message.contains("created 2 connection"));
    }

    #[test]
    fn easy_port_drag_connects_when_released_on_the_target_body() {
        let mut preview = demo_preview();
        // This drop is only reachable in Easy mode, where the pin the drag
        // started on stands for the whole capture channel group.
        preview.view.connect_mode = ConnectMode::Easy;
        let source_pin = preview.view.ids.port(pw_graph_core::PortId(1)).unwrap();
        let (target_x, target_y) = preview
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

        easy_connect_from_pin(&mut preview, source_pin, target_x, target_y);

        assert_eq!(preview.source.graph().links.len(), 2);
        assert!(preview.toast_message.contains("created 2 connection"));
        assert!(!preview.toast_error);
        assert!(channels_are_paired_straight(&preview));
    }

    #[test]
    fn two_pin_clicks_connect_without_holding_the_pointer() {
        let mut preview = demo_preview();
        let output = preview.view.ids.port(pw_graph_core::PortId(1)).unwrap();
        let input = preview.view.ids.port(pw_graph_core::PortId(3)).unwrap();

        handle_link_requested(&mut preview, output, output);
        assert_eq!(preview.pending_connection_pin, Some(output));

        handle_link_requested(&mut preview, input, input);

        assert_eq!(preview.pending_connection_pin, None);
        assert_eq!(preview.source.graph().links.len(), 1);
        assert_eq!(preview.toast_message, "Connection created");
        assert!(!preview.toast_error);
    }

    #[test]
    fn two_pin_clicks_group_channels_in_easy_mode() {
        let mut preview = demo_preview();
        preview.view.connect_mode = ConnectMode::Easy;
        let output = preview.view.ids.port(pw_graph_core::PortId(1)).unwrap();
        let input = preview.view.ids.port(pw_graph_core::PortId(3)).unwrap();

        handle_link_requested(&mut preview, output, output);
        handle_link_requested(&mut preview, input, input);

        assert_eq!(preview.pending_connection_pin, None);
        assert_eq!(preview.source.graph().links.len(), 2);
        assert!(preview.toast_message.contains("created 2 connection"));
        assert!(!preview.toast_error);
        assert!(channels_are_paired_straight(&preview));
    }

    #[test]
    fn connection_feedback_is_transient() {
        let mut preview = demo_preview();
        set_connection_feedback(&mut preview, "test connection", false);
        assert!(toast_visible(&preview));

        preview.toast_until = Some(Instant::now() - Duration::from_secs(1));
        assert!(!toast_visible(&preview));
    }

    #[test]
    fn shortcut_catalog_matches_the_egui_help_dialog() {
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

    #[test]
    fn slint_audio_state_round_trips_stable_node_keys() {
        let mut state = PersistedSlintState::default();
        state.audio_controls.insert(
            "PipeWire:Audio Capture".into(),
            PreviewAudioControl {
                volume_position: 0.42,
                muted: true,
            },
        );

        let encoded = toml::to_string_pretty(&state).unwrap();
        let decoded: PersistedSlintState = toml::from_str(&encoded).unwrap();

        assert_eq!(decoded, state);
    }

    /// Drives the real window with the real canvas wiring: rows, geometry and
    /// callbacks are produced by the same code the application runs.
    struct CanvasHarness {
        window: MainWindow,
        nodes: Rc<VecModel<NodeRow>>,
        links: Rc<VecModel<LinkRow>>,
        geometry: Rc<RefCell<CanvasGeometry>>,
        events: Rc<RefCell<Vec<UiEvent>>>,
        preview: PreviewApp,
    }

    /// The graph starts at the right edge of the icon rail.
    const RAIL_WIDTH: f32 = 76.0;

    impl CanvasHarness {
        fn new(connect_mode: ConnectMode) -> Self {
            i_slint_backend_testing::init_no_event_loop();
            let mut preview = demo_preview();
            preview.view.connect_mode = connect_mode;
            // Anchor the viewport so world and screen differ only by the rail.
            preview.view.pan = [0.0, 0.0];
            preview.view.zoom = 1.0;

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
                preview,
            };
            harness.sync();
            harness
        }

        fn sync(&mut self) {
            let minimap = Rc::new(VecModel::default());
            let version = Rc::new(Cell::new(0));
            sync_models(
                &self.window,
                &mut self.preview,
                &self.nodes,
                &self.links,
                &minimap,
                &self.geometry,
                &version,
            );
        }

        fn screen_of(&self, world: (f32, f32)) -> LogicalPosition {
            LogicalPosition::new(RAIL_WIDTH + world.0, world.1)
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

        fn node_row(&self, node_id: i32) -> NodeRow {
            rows_of(&self.nodes)
                .into_iter()
                .find(|node| node.id == node_id)
                .expect("node is rendered")
        }

        /// First output pin and first input pin of two different cards.
        fn connectable_pair(&self) -> (i32, i32) {
            let output = self
                .preview
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
                .preview
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
        let card = harness.node_row(harness.preview.snapshot.nodes[0].id);
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
        let pin = harness.preview.snapshot.nodes[0]
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
        let first = harness.node_row(harness.preview.snapshot.nodes[0].id);
        let second = harness.node_row(harness.preview.snapshot.nodes[1].id);

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
        let card = harness.node_row(harness.preview.snapshot.nodes[0].id);

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
            .preview
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
            process_event(&harness.window, &mut harness.preview, event);
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
    fn the_background_grid_is_generated_for_the_visible_canvas() {
        let harness = CanvasHarness::new(ConnectMode::Advanced);

        harness.window.invoke_graph_request_grid();

        let grid = harness.window.get_grid_commands();
        assert!(!grid.is_empty(), "the canvas asks Rust for its grid lines");
        assert!(grid.starts_with("M "));
    }

    #[test]
    fn easy_mode_pin_drag_connects_every_channel_of_the_two_groups() {
        let mut preview = demo_preview();
        preview.view.connect_mode = ConnectMode::Easy;
        // In Easy mode the capture and playback cards each render one grouped
        // pin that stands for both FL and FR.
        let capture = preview.view.ids.port(pw_graph_core::PortId(1)).unwrap();
        let playback = preview.view.ids.port(pw_graph_core::PortId(3)).unwrap();

        handle_link_requested(&mut preview, capture, playback);

        assert_eq!(preview.source.graph().links.len(), 2);
        assert!(channels_are_paired_straight(&preview));
    }

    #[test]
    fn easy_mode_pin_drag_keeps_the_channels_straight_when_dragged_backwards() {
        let mut preview = demo_preview();
        preview.view.connect_mode = ConnectMode::Easy;
        let capture = preview.view.ids.port(pw_graph_core::PortId(1)).unwrap();
        let playback = preview.view.ids.port(pw_graph_core::PortId(3)).unwrap();

        handle_link_requested(&mut preview, playback, capture);

        assert_eq!(preview.source.graph().links.len(), 2);
        assert!(channels_are_paired_straight(&preview));
    }

    /// Every link joins two ports whose channel suffix is the same, so left
    /// stays left and right stays right.
    fn channels_are_paired_straight(preview: &PreviewApp) -> bool {
        let graph = preview.source.graph();
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
            process_event(&harness.window, &mut harness.preview, event);
        }
        harness.sync();

        // One grouped pin at each end, two channels, two links.
        assert_eq!(harness.preview.source.graph().links.len(), 2);
        assert!(channels_are_paired_straight(&harness.preview));
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
            process_event(&harness.window, &mut harness.preview, event);
        }
        harness.sync();

        assert_eq!(harness.preview.source.graph().links.len(), 2);
        assert!(channels_are_paired_straight(&harness.preview));
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
            .hit_test(body_point.0, body_point.1);
        assert_eq!(hit.kind, HIT_NODE, "advanced mode drags the card");

        handle_action(&harness.window, &mut harness.preview, "toggle-connect-mode");
        harness.sync();

        let hit = harness
            .geometry
            .borrow()
            .hit_test(body_point.0, body_point.1);
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
            process_event(&harness.window, &mut harness.preview, event);
        }
        harness.sync();

        assert_eq!(
            harness.preview.source.graph().links.len(),
            2,
            "links created after toggling to easy"
        );
        assert!(channels_are_paired_straight(&harness.preview));
    }

    #[test]
    fn pump_path_connects_body_to_body_after_toggling_to_easy() {
        i_slint_backend_testing::init_no_event_loop();
        let preview = Rc::new(RefCell::new(demo_preview()));
        preview.borrow_mut().view.connect_mode = ConnectMode::Advanced;
        preview.borrow_mut().view.pan = [0.0, 0.0];
        preview.borrow_mut().view.zoom = 1.0;

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
            let preview = preview.borrow();
            LogicalPosition::new(
                RAIL_WIDTH + preview.view.pan[0] + world.0,
                preview.view.pan[1] + world.1,
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
            &window, &preview, &nodes, &links, &minimap, &shortcuts, &events, &geometry, &version,
        );

        // Toggle to Easy through the same events queue the toolbar uses.
        events
            .borrow_mut()
            .push(UiEvent::Action("toggle-connect-mode".into()));
        pump(
            &window, &preview, &nodes, &links, &minimap, &shortcuts, &events, &geometry, &version,
        );

        let (output, input) = {
            let preview = preview.borrow();
            let output = preview
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
            let input = preview
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
            process_event(&window, &mut preview.borrow_mut(), event);
        }
        pump(
            &window, &preview, &nodes, &links, &minimap, &shortcuts, &events, &geometry, &version,
        );

        assert_eq!(
            preview.borrow().source.graph().links.len(),
            2,
            "links created"
        );
        assert!(channels_are_paired_straight(&preview.borrow()));
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
            process_event(&harness.window, &mut harness.preview, event);
        }
        harness.sync();

        assert_eq!(
            harness.preview.source.graph().links.len(),
            2,
            "drop on a pin fills the whole group"
        );
        assert!(channels_are_paired_straight(&harness.preview));
    }
}
