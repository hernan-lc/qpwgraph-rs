use crate::args::Args;
use crate::model::{
    node_type_color, node_type_label, port_type_color, ConnectMode, GraphSnapshot, LinkView,
    MediaFilter, MeterReading, MeterState, NodeView, UiGraphState,
};
use crate::source::ReadOnlyGraphSource;
use pw_graph_backend::MeterPolicy;
use pw_graph_config::{config_path, AppConfig};
use pw_graph_i18n::I18n;
use pw_graph_patchbay::Patchbay;
use slint::{
    Color, ComponentHandle, ModelRc, PhysicalSize, SharedString, Timer, TimerMode, VecModel,
};
use slint_node_editor::{GraphLogic, MovableNode, NodeEditorController, NodeEditorSetup};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

slint::include_modules!();

impl MovableNode for NodeRow {
    fn id(&self) -> i32 {
        self.id
    }

    fn x(&self) -> f32 {
        self.x
    }

    fn y(&self) -> f32 {
        self.y
    }

    fn selected(&self) -> bool {
        self.selected
    }

    fn set_x(&mut self, x: f32) {
        self.x = x;
    }

    fn set_y(&mut self, y: f32) {
        self.y = y;
    }
}

enum UiEvent {
    Action(String),
    SelectNode(i32, bool),
    SelectLink(i32, bool),
    ClearSelection,
    SelectBox(f32, f32, f32, f32, bool),
    LinkRequested(i32, i32),
    LinkCancelled,
    ToggleCollapse(i32),
    DragCommitted(i32, f32, f32),
}

struct PreviewApp {
    source: ReadOnlyGraphSource,
    config: AppConfig,
    i18n: I18n,
    view: UiGraphState,
    snapshot: GraphSnapshot,
    status: String,
    debug: bool,
    last_refresh: Instant,
    meters: BTreeMap<pw_graph_core::NodeId, MeterReading>,
    meter_error: Option<String>,
}

pub(crate) struct UiBridge {
    window: MainWindow,
    app: Rc<RefCell<PreviewApp>>,
    nodes: Rc<VecModel<NodeRow>>,
    links: Rc<VecModel<LinkData>>,
    minimap_nodes: Rc<VecModel<MinimapNode>>,
    events: Rc<RefCell<Vec<UiEvent>>>,
    controller: Rc<NodeEditorController>,
}

impl UiBridge {
    pub(crate) fn new(args: Args) -> Result<Self, slint::PlatformError> {
        let config_file = config_path("qpwgraph-rs");
        let config = AppConfig::load_from(config_file).unwrap_or_default();
        let language = args
            .language
            .clone()
            .unwrap_or_else(|| config.language.clone());
        let i18n = I18n::from_language(&language);
        let meter_policy = MeterPolicy::parse(&config.audio_meters);
        let (source, status) = ReadOnlyGraphSource::new(&args, meter_policy);
        let view = UiGraphState::from_config(&config);
        let app = Rc::new(RefCell::new(PreviewApp {
            source,
            config,
            i18n,
            view,
            snapshot: GraphSnapshot::default(),
            status,
            debug: args.debug,
            last_refresh: Instant::now(),
            meters: BTreeMap::new(),
            meter_error: None,
        }));

        let window = MainWindow::new()?;
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
            window.window().set_minimized(args.minimized);
        }

        let nodes = Rc::new(VecModel::default());
        let links = Rc::new(VecModel::default());
        let minimap_nodes = Rc::new(VecModel::default());
        window.set_nodes(ModelRc::from(nodes.clone()));
        window.set_links(ModelRc::from(links.clone()));
        window.set_minimap_nodes(ModelRc::from(minimap_nodes.clone()));
        window.set_rules(ModelRc::from(Rc::new(VecModel::from(rule_rows(
            &app.borrow().config,
        )))));
        window.set_effects(ModelRc::from(Rc::new(VecModel::from(effect_rows(
            &app.borrow().config,
        )))));
        window.set_relay_rows(ModelRc::from(Rc::new(VecModel::from(relay_rows(
            &app.borrow().config,
        )))));

        let events = Rc::new(RefCell::new(Vec::new()));
        let setup = NodeEditorSetup::new({
            let nodes = nodes.clone();
            let events = events.clone();
            move |dragged, delta_x, delta_y| {
                GraphLogic::commit_drag(&nodes, dragged, delta_x, delta_y);
                events
                    .borrow_mut()
                    .push(UiEvent::DragCommitted(dragged, delta_x, delta_y));
            }
        });
        let controller = setup.controller().clone();
        slint_node_editor::wire_node_editor!(window, setup);

        let bridge = Self {
            window,
            app,
            nodes,
            links,
            minimap_nodes,
            events,
            controller,
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
        let events = self.events.clone();
        self.window.on_graph_node_selected(move |id, shift| {
            events.borrow_mut().push(UiEvent::SelectNode(id, shift));
        });
        let events = self.events.clone();
        self.window.on_graph_link_selected(move |id, shift| {
            events.borrow_mut().push(UiEvent::SelectLink(id, shift));
        });
        let events = self.events.clone();
        self.window.on_graph_selection_cleared(move || {
            events.borrow_mut().push(UiEvent::ClearSelection);
        });
        let events = self.events.clone();
        self.window
            .on_graph_box_selected(move |x, y, width, height, shift| {
                events
                    .borrow_mut()
                    .push(UiEvent::SelectBox(x, y, width, height, shift));
            });
        let events = self.events.clone();
        self.window.on_graph_link_requested(move |start, end| {
            events.borrow_mut().push(UiEvent::LinkRequested(start, end));
        });
        let events = self.events.clone();
        self.window.on_graph_link_cancelled(move || {
            events.borrow_mut().push(UiEvent::LinkCancelled);
        });
        let controller = self.controller.clone();
        self.window.on_graph_compute_pin_at(move |x, y| {
            controller.cache().borrow().find_pin_at(x, y, 10.0)
        });
        let controller = self.controller.clone();
        self.window.on_graph_compute_link_at(move |x, y| {
            controller.find_link_at_world(x, y, 8.0, 50.0, 20)
        });
        let controller = self.controller.clone();
        let weak_window = self.window.as_weak();
        self.window.on_graph_request_grid(move || {
            if let Some(window) = weak_window.upgrade() {
                window.set_grid_commands(controller.generate_grid(
                    window.get_width_(),
                    window.get_height_(),
                    window.get_pan_x(),
                    window.get_pan_y(),
                ));
            }
        });

        let controller = self.controller.clone();
        self.window
            .global::<NodeEditorInternalCallbacks>()
            .on_start_node_drag(move |node_id, _, _, _| {
                controller.handle_node_drag_started(node_id)
            });
        let events = self.events.clone();
        self.window
            .global::<NodeEditorInternalCallbacks>()
            .on_double_click_node(move |node_id| {
                events.borrow_mut().push(UiEvent::ToggleCollapse(node_id));
            });
    }

    pub(crate) fn run(self) -> Result<(), slint::PlatformError> {
        let timer = Timer::default();
        let weak_window = self.window.as_weak();
        let app = self.app.clone();
        let nodes = self.nodes.clone();
        let links = self.links.clone();
        let minimap_nodes = self.minimap_nodes.clone();
        let events = self.events.clone();
        let controller = self.controller.clone();
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
                &events,
                &controller,
            );
        });
        let result = self.window.run();
        self.app.borrow_mut().source.reset_meters();
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
            &self.controller,
        );
    }

    fn refresh_meters(&self) {
        let mut app = self.app.borrow_mut();
        refresh_meters(&self.window, &mut app);
    }
}

fn pump(
    window: &MainWindow,
    app: &Rc<RefCell<PreviewApp>>,
    nodes: &Rc<VecModel<NodeRow>>,
    links: &Rc<VecModel<LinkData>>,
    minimap_nodes: &Rc<VecModel<MinimapNode>>,
    events: &Rc<RefCell<Vec<UiEvent>>>,
    controller: &Rc<NodeEditorController>,
) {
    let pending = std::mem::take(&mut *events.borrow_mut());
    let mut preview = app.borrow_mut();
    read_window_state(window, &mut preview);
    for event in pending {
        process_event(window, &mut preview, event);
    }
    if preview.source.graph_dirty() || preview.last_refresh.elapsed() >= Duration::from_millis(500)
    {
        if let Err(error) = preview.source.refresh() {
            preview.status = format!("Could not refresh graph: {error}");
        } else {
            preview.last_refresh = Instant::now();
        }
    }
    refresh_meters(window, &mut preview);
    sync_models(
        window,
        &mut preview,
        nodes,
        links,
        minimap_nodes,
        controller,
    );
}

fn refresh_meters(window: &MainWindow, preview: &mut PreviewApp) {
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

fn process_event(window: &MainWindow, preview: &mut PreviewApp, event: UiEvent) {
    match event {
        UiEvent::Action(action) => handle_action(window, preview, &action),
        UiEvent::SelectNode(id, shift) => preview.view.select_node(id, shift),
        UiEvent::SelectLink(id, shift) => preview.view.select_link(id, shift),
        UiEvent::ClearSelection => preview.view.clear_selection(),
        UiEvent::SelectBox(x, y, width, height, shift) => {
            preview
                .view
                .select_box(&preview.snapshot, x, y, width, height, shift)
        }
        UiEvent::LinkRequested(start, end) => {
            preview.status = format!(
                "Read-only preview: connection request {start} → {end} was not sent to PipeWire"
            );
        }
        UiEvent::LinkCancelled => {
            preview.status = "Connection preview cancelled".into();
        }
        UiEvent::ToggleCollapse(id) => {
            preview.view.toggle_local_collapse(id, &preview.snapshot);
            preview.status = "Node expansion changed locally; it will not be saved".into();
        }
        UiEvent::DragCommitted(id, dx, dy) => {
            preview.view.move_selected(id, dx, dy, &preview.snapshot);
            preview.status = "Node arrangement changed locally; it will not be saved".into();
        }
    }
}

fn handle_action(window: &MainWindow, preview: &mut PreviewApp, action: &str) {
    match action {
        "refresh" => match preview.source.refresh() {
            Ok(()) => {
                preview.last_refresh = Instant::now();
                preview.status = preview
                    .i18n
                    .text("status.refreshed")
                    .replace("{count}", &preview.source.graph().nodes.len().to_string());
            }
            Err(error) => preview.status = format!("Could not refresh graph: {error}"),
        },
        "zoom-in" => preview.view.zoom = (preview.view.zoom * 1.1).clamp(0.35, 2.5),
        "zoom-out" => preview.view.zoom = (preview.view.zoom / 1.1).clamp(0.35, 2.5),
        "toggle-minimap" => preview.view.minimap_visible = !preview.view.minimap_visible,
        "toggle-connect-mode" => {
            preview.view.connect_mode = match preview.view.connect_mode {
                ConnectMode::Advanced => ConnectMode::Easy,
                ConnectMode::Easy => ConnectMode::Advanced,
            };
            preview.status = format!(
                "{} connection mode is active locally",
                if preview.view.connect_mode == ConnectMode::Easy {
                    "Easy"
                } else {
                    "Advanced"
                }
            );
        }
        "filter-all" => preview.view.media_filter = MediaFilter::All,
        "filter-audio" => preview.view.media_filter = MediaFilter::Audio,
        "filter-video" => preview.view.media_filter = MediaFilter::Video,
        "filter-midi" => preview.view.media_filter = MediaFilter::Midi,
        "cycle-filter" => {
            preview.view.media_filter = match preview.view.media_filter {
                MediaFilter::All => MediaFilter::Audio,
                MediaFilter::Audio => MediaFilter::Video,
                MediaFilter::Video => MediaFilter::Midi,
                MediaFilter::Midi => MediaFilter::All,
            }
        }
        "arrange" => {
            let positions = preview.source.graph().default_node_positions();
            for (node_id, position) in positions {
                if let Some(ui_id) = preview.view.ids.node(node_id) {
                    preview
                        .view
                        .set_local_position(ui_id, position[0], position[1]);
                }
            }
            preview.status = "Nodes arranged locally; no layout was persisted".into();
        }
        "preferences" => toggle_overlay(window, Overlay::Preferences),
        "history" => toggle_overlay(window, Overlay::History),
        "shortcuts" => toggle_overlay(window, Overlay::Shortcuts),
        "effects" => toggle_overlay(window, Overlay::Effects),
        "relay" => {
            window.set_show_relay(!window.get_show_relay());
            close_modals(window);
        }
        "relay-show-qr" => window.set_show_qr(true),
        "close-qr" => window.set_show_qr(false),
        "toggle-statusbar" => window.set_show_statusbar(!window.get_show_statusbar()),
        "escape" => {
            close_modals(window);
            window.set_show_relay(false);
            window.set_show_qr(false);
        }
        "undo" | "redo" | "save-patchbay" | "activate-patchbay" | "add-rule" | "create-effect"
        | "inspect-effect" | "relay-connect" | "relay-host-start" => {
            preview.status = format!(
                "Read-only preview: {} is not available",
                action.replace('-', " ")
            );
        }
        _ => {
            preview.status = format!("Read-only preview: {action} is not available");
        }
    }
    if preview.debug {
        eprintln!("[qpwgraph-slint] {}", preview.status);
    }
}

#[derive(Clone, Copy)]
enum Overlay {
    Preferences,
    History,
    Shortcuts,
    Effects,
}

fn toggle_overlay(window: &MainWindow, overlay: Overlay) {
    let currently_open = match overlay {
        Overlay::Preferences => window.get_show_preferences(),
        Overlay::History => window.get_show_history(),
        Overlay::Shortcuts => window.get_show_shortcuts(),
        Overlay::Effects => window.get_show_effects(),
    };
    close_modals(window);
    match overlay {
        Overlay::Preferences => window.set_show_preferences(!currently_open),
        Overlay::History => window.set_show_history(!currently_open),
        Overlay::Shortcuts => window.set_show_shortcuts(!currently_open),
        Overlay::Effects => window.set_show_effects(!currently_open),
    }
}

fn close_modals(window: &MainWindow) {
    window.set_show_preferences(false);
    window.set_show_history(false);
    window.set_show_shortcuts(false);
    window.set_show_effects(false);
}

fn read_window_state(window: &MainWindow, preview: &mut PreviewApp) {
    preview.view.zoom = window.get_zoom().clamp(0.35, 2.5);
    preview.view.pan = [window.get_pan_x(), window.get_pan_y()];
    preview.view.search_query = window.get_search_text().to_string();
}

fn sync_models(
    window: &MainWindow,
    preview: &mut PreviewApp,
    nodes: &Rc<VecModel<NodeRow>>,
    links: &Rc<VecModel<LinkData>>,
    minimap_nodes: &Rc<VecModel<MinimapNode>>,
    controller: &Rc<NodeEditorController>,
) {
    let snapshot = preview.view.snapshot_with_meters(
        preview.source.graph(),
        &preview.config,
        &preview.meters,
        meter_fallback(&preview.source),
    );
    nodes.set_vec(snapshot.nodes.iter().map(node_row).collect::<Vec<_>>());
    links.set_vec(snapshot.links.iter().map(link_row).collect::<Vec<_>>());
    minimap_nodes.set_vec(
        snapshot
            .nodes
            .iter()
            .map(|node| MinimapNode {
                id: node.id,
                x: node.position[0],
                y: node.position[1],
                width: node.width,
                height: node.height,
                color: color(
                    node.appearance
                        .color
                        .unwrap_or_else(|| node_type_color(node.node_type)),
                ),
            })
            .collect::<Vec<_>>(),
    );
    controller.clear_links();
    for link in &snapshot.links {
        controller.register_link(link.id, link.start_pin_id, link.end_pin_id);
    }
    let (node_count, port_count, link_count) = preview.view.visible_counts(&snapshot);
    window.set_status(SharedString::from(preview.status.clone()));
    window.set_backend(SharedString::from(preview.source.backend_name()));
    window.set_graph_counts(SharedString::from(format!(
        "{node_count} nodes · {port_count} ports · {link_count} links"
    )));
    window.set_show_minimap(preview.view.minimap_visible);
    window.set_media_filter(SharedString::from(preview.view.media_filter.as_str()));
    window.set_connect_mode(SharedString::from(preview.view.connect_mode.as_str()));
    window.set_zoom(preview.view.zoom);
    window.set_pan_x(preview.view.pan[0]);
    window.set_pan_y(preview.view.pan[1]);
    preview.snapshot = snapshot;
}

fn node_row(node: &NodeView) -> NodeRow {
    NodeRow {
        id: node.id,
        title: SharedString::from(node.title.clone()),
        subtitle: SharedString::from(node_type_label(node.node_type)),
        x: node.position[0],
        y: node.position[1],
        width: node.width,
        height: node.height,
        selected: node.selected,
        collapsed: node.collapsed,
        thumbnail: node.thumbnail,
        font_scale: node.font_scale,
        accent: color(
            node.appearance
                .color
                .unwrap_or_else(|| node_type_color(node.node_type)),
        ),
        has_audio_controls: node.has_audio_controls,
        meter_rms: node.meter.rms,
        meter_peak: node.meter.peak,
        meter_available: matches!(node.meter.state, MeterState::Live | MeterState::Demo),
        meter_label: SharedString::from(node.meter.state.label()),
        ports: ModelRc::from(Rc::new(VecModel::from(
            node.ports
                .iter()
                .enumerate()
                .map(|(index, port)| PortRow {
                    id: port.pin_id,
                    label: SharedString::from(port.label.clone()),
                    direction: if port.direction == pw_graph_core::Direction::Sink {
                        0
                    } else {
                        1
                    },
                    color: color(port_type_color(port.port_type)),
                    y: index as f32 * 25.0,
                })
                .collect::<Vec<_>>(),
        ))),
    }
}

fn meter_fallback(source: &ReadOnlyGraphSource) -> MeterState {
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

fn link_row(link: &LinkView) -> LinkData {
    LinkData {
        id: link.id,
        start_pin_id: link.start_pin_id,
        end_pin_id: link.end_pin_id,
        color: color(link.color),
        selected: link.selected,
        line_width: 2.0,
        status: -1,
    }
}

fn rule_rows(config: &AppConfig) -> Vec<RuleRow> {
    let config_file = config_path("qpwgraph-rs");
    let default_file = config_file.with_file_name("default.qpwgraph");
    let path = config
        .patchbay_profiles
        .get(&config.active_patchbay_profile)
        .cloned()
        .or_else(|| config.patchbay_path.clone())
        .unwrap_or(default_file);
    Patchbay::load_from(path)
        .map(|patchbay| {
            patchbay
                .connections
                .into_iter()
                .map(|rule| RuleRow {
                    output: SharedString::from(format!(
                        "{} · {}",
                        rule.output_node, rule.output_name
                    )),
                    input: SharedString::from(format!("{} · {}", rule.input_node, rule.input_name)),
                    pinned: rule.pinned,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn effect_rows(config: &AppConfig) -> Vec<EffectRow> {
    if config.effects.is_empty() {
        return vec![
            EffectRow {
                name: "Adaptive noise reduction".into(),
                vendor: "qpwgraph-rs".into(),
                description: "Reduce stationary background noise".into(),
                enabled: true,
            },
            EffectRow {
                name: "Noise gate".into(),
                vendor: "qpwgraph-rs".into(),
                description: "Attenuate audio below a configured threshold".into(),
                enabled: true,
            },
        ];
    }
    config
        .effects
        .iter()
        .map(|effect| EffectRow {
            name: SharedString::from(effect.instance.effect_id.clone()),
            vendor: SharedString::from("Configured effect"),
            description: SharedString::from(effect.instance.instance_id.clone()),
            enabled: effect.instance.enabled,
        })
        .collect()
}

fn relay_rows(config: &AppConfig) -> Vec<RelayRow> {
    if config.relay_client_target.trim().is_empty() {
        return vec![RelayRow {
            name: "No peer selected".into(),
            address: "Use the working app to discover peers".into(),
            state: "idle".into(),
            level: 0.0,
        }];
    }
    vec![RelayRow {
        name: SharedString::from(config.relay_device_name.clone()),
        address: SharedString::from(config.relay_client_target.clone()),
        state: "configured".into(),
        level: 0.0,
    }]
}

fn color(rgba: [u8; 4]) -> Color {
    Color::from_argb_u8(rgba[3], rgba[0], rgba[1], rgba[2])
}
