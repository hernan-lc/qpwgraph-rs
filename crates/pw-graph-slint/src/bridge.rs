use crate::args::Args;
use crate::model::{
    node_layout_key, node_type_color, node_type_label, port_type_color, ConnectMode, GraphSnapshot,
    LinkView, MediaFilter, MeterReading, MeterState, NodeView, UiGraphState,
};
use crate::source::ReadOnlyGraphSource;
use pw_graph_backend::MeterPolicy;
use pw_graph_config::{config_path, AppConfig};
use pw_graph_i18n::I18n;
use pw_graph_patchbay::Patchbay;
use serde::{Deserialize, Serialize};
use slint::{
    Color, ComponentHandle, ModelRc, PhysicalSize, SharedString, Timer, TimerMode, VecModel,
};
use slint_node_editor::{GraphLogic, MovableNode, NodeEditorController, NodeEditorSetup};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
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
    SetAudioVolume(i32, f32),
    ToggleAudioMute(i32),
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
struct PreviewAudioControl {
    volume_position: f32,
    muted: bool,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default)]
struct PersistedSlintState {
    audio_controls: BTreeMap<String, PreviewAudioControl>,
}

impl Default for PreviewAudioControl {
    fn default() -> Self {
        Self {
            volume_position: 0.9,
            muted: false,
        }
    }
}

struct PreviewApp {
    source: ReadOnlyGraphSource,
    config: AppConfig,
    config_file: PathBuf,
    config_saved_snapshot: AppConfig,
    config_dirty_since: Option<Instant>,
    state_file: PathBuf,
    state_saved_snapshot: PersistedSlintState,
    state_dirty_since: Option<Instant>,
    i18n: I18n,
    view: UiGraphState,
    snapshot: GraphSnapshot,
    status: String,
    debug: bool,
    last_refresh: Instant,
    meters: BTreeMap<pw_graph_core::NodeId, MeterReading>,
    meter_error: Option<String>,
    audio_controls: BTreeMap<pw_graph_core::NodeId, PreviewAudioControl>,
}

pub(crate) struct UiBridge {
    window: MainWindow,
    app: Rc<RefCell<PreviewApp>>,
    nodes: Rc<VecModel<NodeRow>>,
    links: Rc<VecModel<LinkData>>,
    minimap_nodes: Rc<VecModel<MinimapNode>>,
    shortcuts: Rc<VecModel<ShortcutRow>>,
    events: Rc<RefCell<Vec<UiEvent>>>,
    controller: Rc<NodeEditorController>,
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
        let (mut source, status) = ReadOnlyGraphSource::new(&args, meter_policy);
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
            debug: args.debug,
            last_refresh: Instant::now(),
            meters: BTreeMap::new(),
            meter_error: None,
            audio_controls,
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
            shortcuts,
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
        let events = self.events.clone();
        self.window.on_graph_audio_volume_changed(move |id, value| {
            events.borrow_mut().push(UiEvent::SetAudioVolume(id, value));
        });
        let events = self.events.clone();
        self.window.on_graph_audio_mute_toggled(move |id| {
            events.borrow_mut().push(UiEvent::ToggleAudioMute(id));
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
        let shortcuts = self.shortcuts.clone();
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
                &shortcuts,
                &events,
                &controller,
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
    shortcuts: &Rc<VecModel<ShortcutRow>>,
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
    restore_missing_audio_controls(&mut preview);
    refresh_meters(window, &mut preview);
    autosave_config(&mut preview);
    autosave_slint_state(&mut preview);
    shortcuts.set_vec(shortcut_rows(
        &preview.i18n,
        window.get_shortcut_search().as_str(),
    ));
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
            preview.status = "Node expansion changed; configuration will be saved".into();
        }
        UiEvent::DragCommitted(id, dx, dy) => {
            preview.view.move_selected(id, dx, dy, &preview.snapshot);
            preview.status = "Node arrangement changed; configuration will be saved".into();
        }
        UiEvent::SetAudioVolume(id, position) => {
            if let Some(node_id) = preview.view.ids.node_id(id) {
                let position = position.clamp(0.0, 1.0);
                let volume = volume_from_track_position(position);
                match preview.source.set_node_volume(node_id, volume) {
                    Ok(()) => {
                        preview
                            .audio_controls
                            .entry(node_id)
                            .or_default()
                            .volume_position = position;
                        preview.status = format!("Node volume: {:.0}%", volume * 100.0);
                    }
                    Err(error) => preview.status = format!("Could not change node volume: {error}"),
                }
            }
        }
        UiEvent::ToggleAudioMute(id) => {
            if let Some(node_id) = preview.view.ids.node_id(id) {
                let muted = !preview
                    .audio_controls
                    .get(&node_id)
                    .copied()
                    .unwrap_or_default()
                    .muted;
                match preview.source.set_node_mute(node_id, muted) {
                    Ok(()) => {
                        preview.audio_controls.entry(node_id).or_default().muted = muted;
                        preview.status = if muted {
                            "Node muted".into()
                        } else {
                            "Node unmuted".into()
                        };
                    }
                    Err(error) => preview.status = format!("Could not change node mute: {error}"),
                }
            }
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
        "toggle-thumbnail" => {
            preview.view.thumbnail_mode = !preview.view.thumbnail_mode;
            preview.status = "Thumbnail view changed locally".into();
        }
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
            preview.status = "Nodes arranged; configuration will be saved".into();
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
        "reset-audio" => {
            preview.source.reset_meters();
            preview.meters.clear();
            preview.status = "Audio monitoring helpers were reset".into();
        }
        "escape" => {
            close_modals(window);
            window.set_show_relay(false);
            window.set_show_qr(false);
        }
        "save-config" => save_config(preview, true),
        "undo"
        | "redo"
        | "save-patchbay"
        | "load-patchbay"
        | "delete-selection"
        | "activate-patchbay"
        | "save-profile"
        | "choose-patchbay-directory"
        | "add-rule"
        | "create-effect"
        | "inspect-effect"
        | "relay-connect"
        | "relay-host-start" => {
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
    preview.view.thumbnail_mode = window.get_thumbnail_view();
    preview.view.node_text_scale = window.get_node_text_scale().clamp(0.8, 2.0);
    preview.config.statusbar = window.get_show_statusbar();
    preview.config.toolbar = window.get_show_common_actions();
    preview.config.patchbay_toolbar = window.get_show_patchbay_toolbar();
    preview.config.repel_overlapping_nodes = window.get_repel_overlaps();
    preview.config.connect_through_nodes = window.get_connect_through();
    preview.config.thumbnail_view = preview.view.thumbnail_mode;
    preview.config.ui_text_scale = window.get_ui_text_scale().clamp(0.8, 2.0);
    preview.config.panel_text_scale = window.get_panel_text_scale().clamp(0.8, 2.0);
    preview.config.node_text_scale = preview.view.node_text_scale;
    preview.config.patchbay_exclusive = window.get_patchbay_exclusive();
    preview.config.patchbay_auto_disconnect = window.get_patchbay_auto_disconnect();
    preview.config.patchbay_auto_pin = window.get_patchbay_auto_pin();
    preview.config.patchbay_activated = window.get_patchbay_activated();
    preview.config.language = language_code(window.get_language_index()).into();
    preview.config.window_width = window.get_width_().max(760.0);
    preview.config.window_height = window.get_height_().max(520.0);

    let meter_policy = meter_policy_from_index(window.get_meter_policy_index());
    if meter_policy != preview.source.meter_policy() {
        preview.config.audio_meters = meter_policy.as_str().into();
        if let Err(error) = preview.source.set_meter_policy(meter_policy) {
            preview.status = format!("Could not change audio metering policy: {error}");
        } else {
            preview.meters.clear();
            preview.status = format!(
                "Audio metering is {} for this preview",
                meter_policy.as_str()
            );
        }
    }
}

fn sync_config(preview: &mut PreviewApp) {
    preview.config.zoom = preview.view.zoom;
    preview.config.thumbnail_view = preview.view.thumbnail_mode;
    preview.config.minimap_visible = preview.view.minimap_visible;
    preview.config.connect_mode = preview.view.connect_mode.as_str().into();
    preview.config.media_filter = preview.view.media_filter.as_str().into();
    preview.config.graph_search = preview.view.search_query.clone();
    preview.config.node_text_scale = preview.view.node_text_scale;
    preview
        .view
        .write_to_config(preview.source.graph(), &mut preview.config);
}

fn autosave_config(preview: &mut PreviewApp) {
    sync_config(preview);
    if preview.config == preview.config_saved_snapshot {
        preview.config_dirty_since = None;
        return;
    }
    let dirty_since = preview.config_dirty_since.get_or_insert_with(Instant::now);
    if dirty_since.elapsed() >= Duration::from_millis(500) {
        save_config(preview, false);
    }
}

fn save_config(preview: &mut PreviewApp, report_success: bool) {
    sync_config(preview);
    match preview.config.save_to(&preview.config_file) {
        Ok(()) => {
            preview.config_saved_snapshot = preview.config.clone();
            preview.config_dirty_since = None;
            if report_success {
                preview.status =
                    format!("Configuration saved to {}", preview.config_file.display());
            }
        }
        Err(error) => {
            preview.status = format!("Could not save configuration: {error}");
            preview.config_dirty_since = Some(Instant::now());
        }
    }
}

fn current_slint_state(preview: &PreviewApp) -> PersistedSlintState {
    let mut audio_controls = preview.state_saved_snapshot.audio_controls.clone();
    audio_controls.extend(
        preview
            .audio_controls
            .iter()
            .filter_map(|(node_id, control)| {
                preview
                    .source
                    .graph()
                    .node(*node_id)
                    .map(|node| (node_layout_key(node), *control))
            }),
    );
    PersistedSlintState { audio_controls }
}

fn load_slint_state(path: &std::path::Path) -> PersistedSlintState {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|contents| toml::from_str(&contents).ok())
        .unwrap_or_default()
}

fn restore_audio_controls(
    source: &mut ReadOnlyGraphSource,
    state: &PersistedSlintState,
) -> BTreeMap<pw_graph_core::NodeId, PreviewAudioControl> {
    let controls: Vec<_> = source
        .graph()
        .nodes
        .values()
        .filter_map(|node| {
            state
                .audio_controls
                .get(&node_layout_key(node))
                .copied()
                .map(|control| (node.id, control))
        })
        .collect();
    let mut restored = BTreeMap::new();
    for (node_id, control) in controls {
        let volume = volume_from_track_position(control.volume_position);
        if source.set_node_volume(node_id, volume).is_ok()
            && source.set_node_mute(node_id, control.muted).is_ok()
        {
            restored.insert(node_id, control);
        }
    }
    restored
}

fn restore_missing_audio_controls(preview: &mut PreviewApp) {
    let missing_keys: BTreeSet<_> = preview
        .source
        .graph()
        .nodes
        .values()
        .filter(|node| !preview.audio_controls.contains_key(&node.id))
        .map(node_layout_key)
        .collect();
    let missing: PersistedSlintState = PersistedSlintState {
        audio_controls: preview
            .state_saved_snapshot
            .audio_controls
            .iter()
            .filter(|(key, _)| missing_keys.contains(*key))
            .map(|(key, control)| (key.clone(), *control))
            .collect(),
    };
    let restored = restore_audio_controls(&mut preview.source, &missing);
    for (node_id, control) in restored {
        preview.audio_controls.entry(node_id).or_insert(control);
    }
}

fn autosave_slint_state(preview: &mut PreviewApp) {
    let state = current_slint_state(preview);
    if state == preview.state_saved_snapshot {
        preview.state_dirty_since = None;
        return;
    }
    let dirty_since = preview.state_dirty_since.get_or_insert_with(Instant::now);
    if dirty_since.elapsed() >= Duration::from_millis(500) {
        save_slint_state(preview, false);
    }
}

fn save_slint_state(preview: &mut PreviewApp, report_success: bool) {
    let state = current_slint_state(preview);
    let result = toml::to_string_pretty(&state)
        .map_err(|error| error.to_string())
        .and_then(|contents| {
            if let Some(parent) = preview.state_file.parent() {
                std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            std::fs::write(&preview.state_file, contents).map_err(|error| error.to_string())
        });
    match result {
        Ok(()) => {
            preview.state_saved_snapshot = state;
            preview.state_dirty_since = None;
            if report_success {
                preview.status = format!("Slint state saved to {}", preview.state_file.display());
            }
        }
        Err(error) => {
            preview.status = format!("Could not save Slint state: {error}");
            preview.state_dirty_since = Some(Instant::now());
        }
    }
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
    nodes.set_vec(
        snapshot
            .nodes
            .iter()
            .map(|node| {
                node_row(
                    node,
                    preview
                        .audio_controls
                        .get(&node.node_id)
                        .copied()
                        .unwrap_or_default(),
                )
            })
            .collect::<Vec<_>>(),
    );
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
    window.set_thumbnail_view(preview.view.thumbnail_mode);
    window.set_show_common_actions(preview.config.toolbar);
    window.set_show_patchbay_toolbar(preview.config.patchbay_toolbar);
    window.set_repel_overlaps(preview.config.repel_overlapping_nodes);
    window.set_connect_through(preview.config.connect_through_nodes);
    window.set_language_index(language_index(&preview.config.language));
    window.set_meter_policy_index(meter_policy_index(preview.source.meter_policy()));
    window.set_ui_text_scale(preview.config.ui_text_scale);
    window.set_panel_text_scale(preview.config.panel_text_scale);
    window.set_node_text_scale(preview.view.node_text_scale);
    window.set_patchbay_exclusive(preview.config.patchbay_exclusive);
    window.set_patchbay_auto_disconnect(preview.config.patchbay_auto_disconnect);
    window.set_patchbay_auto_pin(preview.config.patchbay_auto_pin);
    window.set_patchbay_activated(preview.config.patchbay_activated);
    window.set_zoom(preview.view.zoom);
    window.set_pan_x(preview.view.pan[0]);
    window.set_pan_y(preview.view.pan[1]);
    preview.snapshot = snapshot;
}

fn node_row(node: &NodeView, audio: PreviewAudioControl) -> NodeRow {
    NodeRow {
        id: node.id,
        node_title: SharedString::from(node.title.clone()),
        node_subtitle: SharedString::from(node_type_label(node.node_type)),
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
                .or_else(|| node.ports.first().map(|port| port_type_color(port.port_type)))
                .unwrap_or_else(|| node_type_color(node.node_type)),
        ),
        has_audio_controls: node.has_audio_controls,
        meter_rms: node.meter.rms,
        meter_peak: node.meter.peak,
        meter_available: matches!(node.meter.state, MeterState::Live | MeterState::Demo),
        meter_label: SharedString::from(node.meter.state.label()),
        audio_volume_position: audio.volume_position,
        audio_muted: audio.muted,
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

fn meter_policy_index(policy: MeterPolicy) -> i32 {
    match policy {
        MeterPolicy::Disabled => 0,
        MeterPolicy::OnDemand => 1,
        MeterPolicy::Always => 2,
    }
}

fn meter_policy_from_index(index: i32) -> MeterPolicy {
    match index {
        0 => MeterPolicy::Disabled,
        2 => MeterPolicy::Always,
        _ => MeterPolicy::OnDemand,
    }
}

fn language_index(language: &str) -> i32 {
    match language.trim().to_ascii_lowercase().as_str() {
        "es" | "es-es" => 1,
        "fr" | "fr-fr" => 2,
        _ => 0,
    }
}

fn language_code(index: i32) -> &'static str {
    match index {
        1 => "es",
        2 => "fr",
        _ => "en",
    }
}

fn volume_from_track_position(position: f32) -> f32 {
    const UNITY_TRACK_POSITION: f32 = 0.9;
    const MAX_VOLUME: f32 = 1.5;
    let position = position.clamp(0.0, 1.0);
    if position <= UNITY_TRACK_POSITION {
        position / UNITY_TRACK_POSITION
    } else {
        1.0 + (position - UNITY_TRACK_POSITION) / (1.0 - UNITY_TRACK_POSITION) * (MAX_VOLUME - 1.0)
    }
}

fn shortcut_rows(i18n: &I18n, query: &str) -> Vec<ShortcutRow> {
    const ENTRIES: [(&str, &str); 21] = [
        ("F1", "shortcuts.help"),
        ("Esc", "shortcuts.close_cancel"),
        ("Delete / Backspace", "shortcuts.delete_link"),
        ("Ctrl/Cmd+Z", "shortcuts.undo"),
        ("Ctrl/Cmd+Shift+Z", "shortcuts.redo"),
        ("Ctrl/Cmd+Y", "shortcuts.redo"),
        ("Ctrl/Cmd+S", "shortcuts.save_config"),
        ("Ctrl/Cmd+Shift+S", "shortcuts.save_patchbay"),
        ("Ctrl/Cmd+O", "shortcuts.load_patchbay"),
        ("R", "shortcuts.refresh"),
        ("A", "shortcuts.arrange"),
        ("T", "shortcuts.thumbnail"),
        ("Arrow keys", "shortcuts.pan_keyboard"),
        ("0", "shortcuts.filter_all"),
        ("1", "shortcuts.filter_audio"),
        ("2", "shortcuts.filter_video"),
        ("3", "shortcuts.filter_midi"),
        ("+ / -", "shortcuts.zoom"),
        ("Scroll", "shortcuts.scroll_pan"),
        ("Shift+Scroll", "shortcuts.scroll_pan_horizontal"),
        ("Ctrl/Cmd+Scroll", "shortcuts.scroll_zoom"),
    ];
    let query = query.trim().to_ascii_lowercase();
    ENTRIES
        .into_iter()
        .filter_map(|(keys, key)| {
            let description = i18n.text(key);
            (query.is_empty()
                || keys.to_ascii_lowercase().contains(&query)
                || description.to_ascii_lowercase().contains(&query))
            .then(|| ShortcutRow {
                keys: SharedString::from(keys),
                description: SharedString::from(description),
            })
        })
        .collect()
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
    let path = selected_patchbay_path(config);
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

fn selected_patchbay_path(config: &AppConfig) -> std::path::PathBuf {
    let default_file = config_path("qpwgraph-rs").with_file_name("default.qpwgraph");
    config
        .patchbay_profiles
        .get(&config.active_patchbay_profile)
        .cloned()
        .or_else(|| config.patchbay_path.clone())
        .unwrap_or(default_file)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcut_catalog_matches_the_egui_help_dialog() {
        let i18n = I18n::from_language("en");
        assert_eq!(shortcut_rows(&i18n, "").len(), 21);
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
}
