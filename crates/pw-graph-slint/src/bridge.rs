use crate::args::Args;
use crate::model::{
    node_layout_key, node_type_color, port_type_color, ConnectMode, GraphSnapshot, LinkView,
    MediaFilter, MeterReading, MeterState, NodeView, UiGraphState,
};
use crate::source::ReadOnlyGraphSource;
#[cfg(feature = "relay")]
use pw_graph_backend::{
    relay_build_qr_payload, relay_parse_qr_payload, relay_qr, RelayCodecKind, RelayEvent,
    RelayHostRequest, RelayRoles, RelaySessionId, RelayTransportPreference,
};
use pw_graph_backend::{EffectInsertRequest, EffectNodeRequest, MeterPolicy};
use pw_graph_config::{config_path, AppConfig, PersistedEffect};
use pw_graph_core::Direction;
use pw_graph_i18n::I18n;
use pw_graph_patchbay::Patchbay;
use serde::{Deserialize, Serialize};
use slint::{
    Color, ComponentHandle, Image, Model, ModelRc, PhysicalSize, SharedString, Timer, TimerMode,
    VecModel,
};
#[cfg(feature = "relay")]
use slint::{Rgba8Pixel, SharedPixelBuffer};
use slint_node_editor::{GraphLogic, MovableNode, NodeEditorController, NodeEditorSetup};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
#[cfg(feature = "relay")]
use std::net::ToSocketAddrs;
use std::path::PathBuf;
use std::rc::Rc;
#[cfg(feature = "relay")]
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
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
    EasyConnect(i32, f32, f32, i32),
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
    toast_message: String,
    toast_until: Option<Instant>,
    toast_error: bool,
    debug: bool,
    last_refresh: Instant,
    meters: BTreeMap<pw_graph_core::NodeId, MeterReading>,
    meter_error: Option<String>,
    audio_controls: BTreeMap<pw_graph_core::NodeId, PreviewAudioControl>,
    #[cfg(feature = "relay")]
    relay_levels: BTreeMap<u64, f32>,
    #[cfg(feature = "relay")]
    relay_connecting: Option<String>,
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
            window
                .global::<UiI18n>()
                .on_format_one(move |key, value| {
                    let value = value.to_string();
                    SharedString::from(
                        app_for_format
                            .borrow()
                            .i18n
                            .format(
                                key.as_str(),
                                &[
                                    ("count", value.clone()),
                                    ("path", value.clone()),
                                    ("port", value.clone()),
                                    ("pin", value),
                                ],
                            ),
                    )
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
        let nodes = self.nodes.clone();
        let links = self.links.clone();
        self.window.on_graph_node_selected(move |id, shift| {
            project_node_selection(&nodes, &links, id, shift);
            events.borrow_mut().push(UiEvent::SelectNode(id, shift));
        });
        let events = self.events.clone();
        let nodes = self.nodes.clone();
        let links = self.links.clone();
        self.window.on_graph_link_selected(move |id, shift| {
            project_link_selection(&nodes, &links, id, shift);
            events.borrow_mut().push(UiEvent::SelectLink(id, shift));
        });
        let events = self.events.clone();
        let nodes = self.nodes.clone();
        let links = self.links.clone();
        self.window.on_graph_selection_cleared(move || {
            clear_model_selection(&nodes, &links);
            events.borrow_mut().push(UiEvent::ClearSelection);
        });
        let events = self.events.clone();
        let nodes = self.nodes.clone();
        let links = self.links.clone();
        let controller = self.controller.clone();
        self.window
            .on_graph_box_selected(move |x, y, width, height, shift| {
                project_box_selection(&nodes, &links, &controller, x, y, width, height, shift);
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
        self.window.on_graph_easy_connect(move |id, x, y, target_pin| {
            events
                .borrow_mut()
                .push(UiEvent::EasyConnect(id, x, y, target_pin));
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
        let events = self.events.clone();
        self.window.on_graph_node_collapse_toggled(move |node_id| {
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

#[allow(clippy::too_many_arguments)]
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
    let pending = coalesce_audio_volume_events(std::mem::take(&mut *events.borrow_mut()));
    let mut preview = app.borrow_mut();
    read_window_state(window, &mut preview);
    for event in pending {
        process_event(window, &mut preview, event);
    }
    poll_relay_events(&mut preview);
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

fn project_node_selection(
    nodes: &Rc<VecModel<NodeRow>>,
    links: &Rc<VecModel<LinkData>>,
    node_id: i32,
    shift: bool,
) {
    if !shift {
        slint_node_editor::selection::clear_selection(links, |link| &mut link.selected);
    }
    slint_node_editor::selection::apply_click(
        nodes,
        |node| node.id,
        |node| &mut node.selected,
        node_id,
        shift,
    );
}

fn project_link_selection(
    nodes: &Rc<VecModel<NodeRow>>,
    links: &Rc<VecModel<LinkData>>,
    link_id: i32,
    shift: bool,
) {
    if !shift {
        slint_node_editor::selection::clear_selection(nodes, |node| &mut node.selected);
    }
    slint_node_editor::selection::apply_click(
        links,
        |link| link.id,
        |link| &mut link.selected,
        link_id,
        shift,
    );
}

fn clear_model_selection(nodes: &Rc<VecModel<NodeRow>>, links: &Rc<VecModel<LinkData>>) {
    slint_node_editor::selection::clear_selection(nodes, |node| &mut node.selected);
    slint_node_editor::selection::clear_selection(links, |link| &mut link.selected);
}

#[allow(clippy::too_many_arguments)]
fn project_box_selection(
    nodes: &Rc<VecModel<NodeRow>>,
    links: &Rc<VecModel<LinkData>>,
    controller: &NodeEditorController,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    shift: bool,
) {
    let (node_hits, link_hits) = {
        let cache = controller.cache();
        let cache = cache.borrow();
        let rows = (0..links.row_count())
            .filter_map(|index| links.row_data(index))
            .map(|link| (link.id, link.start_pin_id, link.end_pin_id));
        (
            cache.nodes_in_selection_box(x, y, width, height),
            cache.links_in_selection_box(x, y, width, height, rows),
        )
    };

    if !shift {
        clear_model_selection(nodes, links);
    }
    slint_node_editor::selection::apply_box(
        nodes,
        |node| node.id,
        |node| &mut node.selected,
        node_hits,
        shift,
    );
    slint_node_editor::selection::apply_box(
        links,
        |link| link.id,
        |link| &mut link.selected,
        link_hits,
        shift,
    );
}

fn coalesce_audio_volume_events(pending: Vec<UiEvent>) -> Vec<UiEvent> {
    let mut compacted = Vec::with_capacity(pending.len());
    let mut volume_indices = BTreeMap::<i32, usize>::new();
    for event in pending {
        match event {
            UiEvent::SetAudioVolume(id, position) => {
                if let Some(index) = volume_indices.get(&id).copied() {
                    compacted[index] = UiEvent::SetAudioVolume(id, position);
                } else {
                    volume_indices.insert(id, compacted.len());
                    compacted.push(UiEvent::SetAudioVolume(id, position));
                }
            }
            event => compacted.push(event),
        }
    }
    compacted
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

const CONNECTION_TOAST_DURATION: Duration = Duration::from_secs(4);

fn set_connection_feedback(preview: &mut PreviewApp, message: impl Into<String>, error: bool) {
    let message = message.into();
    preview.status = message.clone();
    preview.toast_message = message;
    preview.toast_error = error;
    preview.toast_until = Some(Instant::now() + CONNECTION_TOAST_DURATION);
}

fn toast_visible(preview: &PreviewApp) -> bool {
    preview
        .toast_until
        .is_some_and(|deadline| Instant::now() < deadline)
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
        UiEvent::LinkRequested(start, end) => connect_pin_pair(preview, start, end),
        UiEvent::LinkCancelled => {
            set_connection_feedback(preview, "Connection preview cancelled", false);
        }
        UiEvent::EasyConnect(source, x, y, target_pin) => {
            easy_connect_nodes(preview, source, x, y, target_pin)
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

fn connect_pin_pair(preview: &mut PreviewApp, start_id: i32, end_id: i32) {
    let Some(start_port) = preview.view.ids.port_id(start_id) else {
        set_connection_feedback(
            preview,
            format!("Connection failed: source pin {start_id} is no longer available"),
            true,
        );
        return;
    };
    let Some(end_port) = preview.view.ids.port_id(end_id) else {
        set_connection_feedback(
            preview,
            format!("Connection failed: destination pin {end_id} is no longer available"),
            true,
        );
        return;
    };

    let (output, input) = {
        let graph = preview.source.graph();
        let Some(start) = graph.port(start_port) else {
            set_connection_feedback(preview, "Connection failed: source pin disappeared", true);
            return;
        };
        let Some(end) = graph.port(end_port) else {
            set_connection_feedback(
                preview,
                "Connection failed: destination pin disappeared",
                true,
            );
            return;
        };
        match (start.direction, end.direction) {
            (Direction::Source, Direction::Sink) => (start_port, end_port),
            (Direction::Sink, Direction::Source) => (end_port, start_port),
            _ => {
                set_connection_feedback(
                    preview,
                    "Connection failed: connect one output pin to one input pin",
                    true,
                );
                return;
            }
        }
    };

    let Some((output, input)) = ({
        let graph = preview.source.graph();
        graph.port_key(output).zip(graph.port_key(input))
    }) else {
        set_connection_feedback(
            preview,
            "Connection failed: pin identity is unavailable",
            true,
        );
        return;
    };

    match preview.source.connect_by_key_if_missing(&output, &input) {
        Ok(created) => match refresh_connection_graph(preview) {
            Ok(()) => {
                let message = if created {
                    "Connection created"
                } else {
                    "Connection already exists"
                };
                set_connection_feedback(preview, message, false);
            }
            Err(error) => set_connection_feedback(
                preview,
                format!("Connection succeeded, but graph refresh failed: {error}"),
                true,
            ),
        },
        Err(error) => set_connection_feedback(preview, format!("Connection failed: {error}"), true),
    }
}

fn refresh_connection_graph(preview: &mut PreviewApp) -> Result<(), String> {
    preview.source.refresh()?;
    preview.last_refresh = Instant::now();
    Ok(())
}

fn easy_connect_nodes(
    preview: &mut PreviewApp,
    source_id: i32,
    x: f32,
    y: f32,
    target_pin_id: i32,
) {
    let Some(source_node) = preview.view.ids.node_id(source_id) else {
        set_connection_feedback(preview, "Easy connect source is no longer available", true);
        return;
    };
    // Prefer the actual rendered pin under the release. This is authoritative
    // even when a transformed/captured TouchArea reports imperfect card-local
    // coordinates at the edge of another node.
    let target_from_pin = preview
        .view
        .ids
        .port_id(target_pin_id)
        .and_then(|port| preview.source.graph().port(port))
        .map(|port| port.node_id)
        .filter(|node| *node != source_node);
    let Some(target_node) = target_from_pin.or_else(|| {
        // A card-body drop has no pin identity. Keep coordinate hit-testing as
        // its fallback, including the small margin occupied by edge pins.
        preview
            .view
            .node_at(&preview.snapshot, x, y, source_node)
            .or_else(|| {
                preview
                    .view
                    .node_at_with_margin(&preview.snapshot, x, y, source_node, 12.0)
            })
    })
    else {
        set_connection_feedback(
            preview,
            "Easy connect cancelled: drop onto another node",
            true,
        );
        return;
    };
    easy_connect_node_pair(preview, source_node, target_node);
}

fn easy_connect_node_pair(
    preview: &mut PreviewApp,
    source_node: pw_graph_core::NodeId,
    target_node: pw_graph_core::NodeId,
) {
    let port_keys = {
        let graph = preview.source.graph();
        preview
            .view
            .matching_port_pairs(graph, source_node, target_node)
            .into_iter()
            .filter_map(|(output, input)| Some((graph.port_key(output)?, graph.port_key(input)?)))
            .collect::<Vec<_>>()
    };
    if port_keys.is_empty() {
        set_connection_feedback(
            preview,
            "Easy connect found no compatible output/input ports",
            true,
        );
        return;
    }

    let mut connected = 0usize;
    let mut already_connected = 0usize;
    for (output, input) in port_keys {
        match preview.source.connect_by_key_if_missing(&output, &input) {
            Ok(true) => connected += 1,
            Ok(false) => already_connected += 1,
            Err(error) => {
                let _ = refresh_connection_graph(preview);
                set_connection_feedback(
                    preview,
                    format!("Easy connect created {connected} connection(s), then failed: {error}"),
                    true,
                );
                return;
            }
        }
    }
    match refresh_connection_graph(preview) {
        Ok(()) => {
            let message = if connected == 0 {
                format!("Easy connect: {already_connected} connection(s) already exist")
            } else if already_connected == 0 {
                format!("Easy connect created {connected} connection(s)")
            } else {
                format!(
                    "Easy connect created {connected} connection(s); {already_connected} already exist"
                )
            };
            set_connection_feedback(preview, message, false);
        }
        Err(error) => {
            set_connection_feedback(
                preview,
                format!("Easy connect succeeded, but graph refresh failed: {error}"),
                true,
            );
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
            let show = !window.get_show_relay();
            window.set_show_relay(show);
            close_modals(window);
            if show {
                start_relay_discovery(preview);
            } else {
                stop_relay_discovery(preview);
            }
        }
        "relay-show-qr" => {
            if relay_qr_payload(preview).is_some() {
                window.set_show_qr(true);
            } else {
                preview.status = "Start the relay host before showing its QR code".into();
            }
        }
        "close-qr" => window.set_show_qr(false),
        "relay-connect-configured" => connect_relay(preview, None),
        "relay-connect" => connect_relay(preview, None),
        "relay-host-toggle" => {
            if relay_host_active(preview) {
                stop_relay_host(preview);
            } else {
                start_relay_host(preview);
            }
        }
        "relay-host-start" => start_relay_host(preview),
        "relay-host-stop" => stop_relay_host(preview),
        _ if action == "effect-create" || action == "create-effect" => {
            create_effect(window, preview);
        }
        _ if action == "effect-inspect" || action == "inspect-effect" => {
            inspect_effect(preview, None);
        }
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
            stop_relay_discovery(preview);
        }
        "save-config" => save_config(preview, true),
        "delete-selection" => delete_selected_connections(preview),
        "undo"
        | "redo"
        | "save-patchbay"
        | "load-patchbay"
        | "activate-patchbay"
        | "save-profile"
        | "choose-patchbay-directory"
        | "add-rule" => {
            preview.status = format!(
                "Read-only preview: {} is not available",
                action.replace('-', " ")
            );
        }
        _ if action.strip_prefix("effect-toggle:").is_some() => {
            let instance_id = action.strip_prefix("effect-toggle:").unwrap_or_default();
            toggle_effect(preview, instance_id);
        }
        _ if action.strip_prefix("effect-parameter:").is_some() => {
            let details = action.strip_prefix("effect-parameter:").unwrap_or_default();
            set_effect_parameter(preview, details);
        }
        _ if action.strip_prefix("effect-remove:").is_some() => {
            let instance_id = action.strip_prefix("effect-remove:").unwrap_or_default();
            remove_effect(preview, instance_id);
        }
        _ if action.strip_prefix("effect-inspect:").is_some() => {
            let instance_id = action.strip_prefix("effect-inspect:").unwrap_or_default();
            inspect_effect(preview, Some(instance_id));
        }
        _ if action.strip_prefix("relay-connect:").is_some() => {
            let target = action.strip_prefix("relay-connect:").unwrap_or_default();
            connect_relay(preview, Some(target));
        }
        _ if action.strip_prefix("relay-disconnect:").is_some() => {
            let session = action
                .strip_prefix("relay-disconnect:")
                .and_then(|value| value.parse::<u64>().ok());
            disconnect_relay(preview, session);
        }
        _ => {
            preview.status = format!("Read-only preview: {action} is not available");
        }
    }
    if preview.debug {
        eprintln!("[qpwgraph-slint] {}", preview.status);
    }
}

fn delete_selected_connections(preview: &mut PreviewApp) {
    let keys = {
        let graph = preview.source.graph();
        preview
            .view
            .selected_links
            .iter()
            .filter_map(|id| {
                let link = graph.link(*id)?;
                Some((
                    graph.port_key(link.output_port)?,
                    graph.port_key(link.input_port)?,
                ))
            })
            .collect::<Vec<_>>()
    };
    if keys.is_empty() {
        set_connection_feedback(preview, "Select a connection before deleting", true);
        return;
    }

    let mut removed = 0usize;
    for (output, input) in keys {
        match preview.source.disconnect_by_key_if_present(&output, &input) {
            Ok(true) => removed += 1,
            Ok(false) => {}
            Err(error) => {
                let _ = refresh_connection_graph(preview);
                set_connection_feedback(
                    preview,
                    format!("Removed {removed} connection(s), then failed: {error}"),
                    true,
                );
                return;
            }
        }
    }
    preview.view.clear_selection();
    match refresh_connection_graph(preview) {
        Ok(()) => set_connection_feedback(
            preview,
            format!("Removed {removed} connection(s)"),
            false,
        ),
        Err(error) => set_connection_feedback(
            preview,
            format!("Connection removed, but graph refresh failed: {error}"),
            true,
        ),
    }
}

fn restore_configured_effects(
    source: &mut ReadOnlyGraphSource,
    config: &AppConfig,
    status: &mut String,
) {
    if config.effects.is_empty() {
        return;
    }
    if !source.supports_effect_nodes() {
        *status = format!(
            "{status} · {} saved effect(s) could not be restored: effect processing is unavailable",
            config.effects.len()
        );
        return;
    }

    for saved in config.effects.iter().cloned() {
        let result = match (&saved.source, &saved.destination) {
            (Some(source_port), Some(destination_port)) => source
                .connect_by_key_if_missing(source_port, destination_port)
                .map(|_| ())
                .and_then(|_| {
                    source.insert_effect(EffectInsertRequest {
                        instance_id: saved.instance.instance_id.clone(),
                        effect_id: saved.instance.effect_id.clone(),
                        module_path: saved.instance.module_path.clone(),
                        source: source_port.clone(),
                        destination: destination_port.clone(),
                        enabled: saved.instance.enabled,
                        parameters: saved.instance.parameters.clone(),
                        position: saved.position,
                    })
                }),
            (None, None) => source.create_effect_node(EffectNodeRequest {
                instance_id: saved.instance.instance_id.clone(),
                effect_id: saved.instance.effect_id.clone(),
                module_path: saved.instance.module_path.clone(),
                enabled: saved.instance.enabled,
                parameters: saved.instance.parameters.clone(),
                position: saved.position,
            }),
            _ => Err("effect routing is incomplete".into()),
        };
        if let Err(error) = result {
            *status = format!("{status} · Could not restore effect: {error}");
        }
    }
}

fn create_effect(window: &MainWindow, preview: &mut PreviewApp) {
    if !preview.source.supports_effect_nodes() {
        preview.status = "Effect processing is not available for this backend".into();
        return;
    }
    let descriptors = preview.source.effect_descriptors();
    let Some(descriptor) = descriptors
        .get(window.get_effect_selection_index().max(0) as usize)
        .or_else(|| descriptors.first())
    else {
        preview.status = "No effects are available".into();
        return;
    };

    let instance_id = unique_effect_id(preview);
    let parameters = descriptor
        .parameters
        .iter()
        .map(|parameter| (parameter.id.clone(), parameter.default))
        .collect();
    let request = EffectNodeRequest {
        instance_id,
        effect_id: descriptor.id.clone(),
        module_path: None,
        enabled: true,
        parameters,
        position: preferred_effect_position(preview),
    };
    match preview.source.create_effect_node(request) {
        Ok(instance) => {
            let name = descriptor.name.clone();
            persist_effect(preview, instance);
            match preview.source.refresh() {
                Ok(()) => preview.last_refresh = Instant::now(),
                Err(error) => {
                    preview.status = format!("Effect created, but graph refresh failed: {error}");
                    return;
                }
            }
            preview.status = format!("Effect created: {name}");
        }
        Err(error) => preview.status = format!("Could not create effect: {error}"),
    }
}

fn toggle_effect(preview: &mut PreviewApp, instance_id: &str) {
    let Some(instance) = preview
        .source
        .effect_instances()
        .into_iter()
        .find(|instance| instance.config.instance_id == instance_id)
    else {
        preview.status = format!("Effect instance not found: {instance_id}");
        return;
    };
    let enabled = !instance.config.enabled;
    match preview.source.set_effect_enabled(instance_id, enabled) {
        Ok(()) => {
            if let Some(saved) = preview
                .config
                .effects
                .iter_mut()
                .find(|effect| effect.instance.instance_id == instance_id)
            {
                saved.instance.enabled = enabled;
            }
            preview.status = format!(
                "Effect {}: {}",
                instance_id,
                if enabled { "enabled" } else { "bypassed" }
            );
        }
        Err(error) => preview.status = format!("Could not change effect state: {error}"),
    }
}

fn set_effect_parameter(preview: &mut PreviewApp, details: &str) {
    let Some((details, value)) = details.rsplit_once(':') else {
        preview.status = "Invalid effect parameter action".into();
        return;
    };
    let Some((instance_id, parameter)) = details.rsplit_once(':') else {
        preview.status = "Invalid effect parameter action".into();
        return;
    };
    let Ok(value) = value.parse::<f32>() else {
        preview.status = "Invalid effect parameter value".into();
        return;
    };
    match preview
        .source
        .set_effect_parameter(instance_id, parameter, value)
    {
        Ok(()) => {
            if let Some(saved) = preview
                .config
                .effects
                .iter_mut()
                .find(|effect| effect.instance.instance_id == instance_id)
            {
                saved
                    .instance
                    .parameters
                    .insert(parameter.to_owned(), value);
            }
            preview.status = format!("{instance_id} · {parameter} = {value:.2}");
        }
        Err(error) => preview.status = format!("Could not change effect parameter: {error}"),
    }
}

fn remove_effect(preview: &mut PreviewApp, instance_id: &str) {
    match preview.source.remove_effect(instance_id) {
        Ok(()) => {
            preview
                .config
                .effects
                .retain(|effect| effect.instance.instance_id != instance_id);
            if let Err(error) = preview.source.refresh() {
                preview.status = format!("Effect removed, but graph refresh failed: {error}");
            } else {
                preview.last_refresh = Instant::now();
                preview.status = format!("Effect removed: {instance_id}");
            }
        }
        Err(error) => preview.status = format!("Could not remove effect: {error}"),
    }
}

fn inspect_effect(preview: &mut PreviewApp, instance_id: Option<&str>) {
    let instance = match instance_id {
        Some(instance_id) => preview
            .source
            .effect_instances()
            .into_iter()
            .find(|instance| instance.config.instance_id == instance_id),
        None => preview.source.effect_instances().into_iter().next(),
    };
    let Some(instance) = instance else {
        preview.status = "No effect instance is available".into();
        return;
    };
    let descriptor = preview
        .source
        .effect_descriptors()
        .into_iter()
        .find(|descriptor| descriptor.id == instance.config.effect_id);
    let name = descriptor
        .as_ref()
        .map(|descriptor| descriptor.name.as_str())
        .unwrap_or(instance.config.effect_id.as_str());
    let parameters = instance
        .config
        .parameters
        .iter()
        .map(|(id, value)| format!("{id}={value:.2}"))
        .collect::<Vec<_>>()
        .join(", ");
    preview.status = if parameters.is_empty() {
        format!("{name} · {}", instance.config.instance_id)
    } else {
        format!("{name} · {} · {parameters}", instance.config.instance_id)
    };
}

fn persist_effect(preview: &mut PreviewApp, instance: pw_graph_backend::EffectInstance) {
    let position = preview
        .source
        .graph()
        .node(instance.node_id)
        .map(|node| node.position)
        .unwrap_or([260.0, 180.0]);
    preview
        .config
        .effects
        .retain(|effect| effect.instance.instance_id != instance.config.instance_id);
    preview.config.effects.push(PersistedEffect {
        instance: instance.config,
        source: instance.source,
        destination: instance.destination,
        position,
    });
}

fn preferred_effect_position(preview: &PreviewApp) -> [f32; 2] {
    let rightmost = preview
        .source
        .graph()
        .nodes
        .values()
        .map(|node| node.position[0])
        .fold(0.0_f32, f32::max);
    [rightmost + 290.0, 180.0]
}

fn unique_effect_id(preview: &PreviewApp) -> String {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    loop {
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let id = format!("slint-effect-{sequence}");
        if !preview
            .source
            .effect_instances()
            .iter()
            .any(|effect| effect.config.instance_id == id)
            && !preview
                .config
                .effects
                .iter()
                .any(|effect| effect.instance.instance_id == id)
        {
            return id;
        }
    }
}

fn start_relay_discovery(preview: &mut PreviewApp) {
    #[cfg(feature = "relay")]
    {
        if let Err(error) = preview.source.relay_discovery_start() {
            preview.status = format!("Relay discovery unavailable: {error}");
        }
    }
    #[cfg(not(feature = "relay"))]
    {
        preview.status = "Relay support is not enabled in this build".into();
    }
}

fn stop_relay_discovery(preview: &mut PreviewApp) {
    #[cfg(feature = "relay")]
    preview.source.relay_discovery_stop();
    #[cfg(not(feature = "relay"))]
    let _ = preview;
}

fn relay_host_active(preview: &PreviewApp) -> bool {
    #[cfg(feature = "relay")]
    {
        return preview.source.relay_status().host_active;
    }
    #[cfg(not(feature = "relay"))]
    {
        let _ = preview;
        false
    }
}

fn relay_nodes_visible(preview: &PreviewApp) -> bool {
    #[cfg(feature = "relay")]
    {
        let status = preview.source.relay_status();
        return status.host_active
            || !status.sessions.is_empty()
            || preview.relay_connecting.is_some();
    }
    #[cfg(not(feature = "relay"))]
    {
        let _ = preview;
        false
    }
}

fn start_relay_host(preview: &mut PreviewApp) {
    #[cfg(feature = "relay")]
    {
        let request = RelayHostRequest {
            device_name: preview.config.relay_device_name.trim().to_owned(),
            pin: preview.config.relay_host_pin.trim().to_owned(),
            port: preview.config.relay_host_port,
            codec: relay_codec(&preview.config.relay_codec),
            frame_ms: preview.config.relay_frame_ms.clamp(5, 60),
            transport: relay_transport(&preview.config.relay_transport),
        };
        match preview.source.relay_start_host(request) {
            Ok(port) => preview.status = format!("Relay host started on port {port}"),
            Err(error) => preview.status = format!("Could not start relay host: {error}"),
        }
    }
    #[cfg(not(feature = "relay"))]
    {
        preview.status = "Relay support is not enabled in this build".into();
    }
}

fn stop_relay_host(preview: &mut PreviewApp) {
    #[cfg(feature = "relay")]
    {
        match preview.source.relay_stop_host() {
            Ok(()) => preview.status = "Relay host stopped".into(),
            Err(error) => preview.status = format!("Could not stop relay host: {error}"),
        }
    }
    #[cfg(not(feature = "relay"))]
    {
        preview.status = "Relay support is not enabled in this build".into();
    }
}

fn connect_relay(preview: &mut PreviewApp, requested_target: Option<&str>) {
    #[cfg(feature = "relay")]
    {
        let raw_target = requested_target
            .map(str::to_owned)
            .unwrap_or_else(|| preview.config.relay_client_target.clone());
        let raw_target = raw_target.trim().to_owned();
        if raw_target.is_empty() {
            preview.status = "Enter a relay address before connecting".into();
            return;
        }
        let target_text = match relay_parse_qr_payload(&raw_target) {
            Some(payload) => {
                preview.config.relay_client_target = payload.target.clone();
                if let Some(pin) = payload.pin {
                    preview.config.relay_client_pin = pin;
                }
                payload.target
            }
            None => {
                if requested_target.is_some() {
                    preview.config.relay_client_target = raw_target.clone();
                }
                raw_target
            }
        };
        let target = match target_text
            .to_socket_addrs()
            .ok()
            .and_then(|mut addrs| addrs.next())
        {
            Some(target) => target,
            None => {
                preview.status = format!("Invalid relay target: {target_text}");
                return;
            }
        };
        match preview.source.relay_connect(
            target,
            preview.config.relay_client_pin.trim(),
            relay_roles(&preview.config.relay_role),
        ) {
            Ok(()) => {
                preview.relay_connecting = Some(target.to_string());
                preview.status = format!("Connecting to relay peer {target}");
            }
            Err(error) => preview.status = format!("Could not connect to relay peer: {error}"),
        }
    }
    #[cfg(not(feature = "relay"))]
    {
        let _ = requested_target;
        preview.status = "Relay support is not enabled in this build".into();
    }
}

fn disconnect_relay(preview: &mut PreviewApp, session: Option<u64>) {
    #[cfg(feature = "relay")]
    {
        let Some(session) = session else {
            preview.status = "Invalid relay session".into();
            return;
        };
        match preview.source.relay_disconnect(RelaySessionId(session)) {
            Ok(()) => preview.status = "Disconnecting relay peer".into(),
            Err(error) => preview.status = format!("Could not disconnect relay peer: {error}"),
        }
    }
    #[cfg(not(feature = "relay"))]
    {
        let _ = session;
        preview.status = "Relay support is not enabled in this build".into();
    }
}

#[cfg(feature = "relay")]
fn poll_relay_events(preview: &mut PreviewApp) {
    for event in preview.source.relay_events() {
        match event {
            RelayEvent::HostStarted { port } => {
                preview.status = format!("Relay host started on port {port}");
            }
            RelayEvent::HostStopped => preview.status = "Relay host stopped".into(),
            RelayEvent::PeerDiscovered { peer } => {
                preview.status = format!("Relay peer discovered: {}", peer.name);
            }
            RelayEvent::PeerLost { peer } => {
                preview.status = format!("Relay peer lost: {}", peer.name);
            }
            RelayEvent::SessionEstablished { peer, .. } => {
                preview.relay_connecting = None;
                preview.status = format!("Relay connected: {}", peer.name);
            }
            RelayEvent::SessionLost { id, reason } => {
                preview.relay_levels.remove(&id.0);
                preview.status = format!("Relay session lost: {reason}");
            }
            RelayEvent::AudioLevel { id, rms } => {
                preview.relay_levels.insert(id.0, rms.clamp(0.0, 1.0));
            }
            RelayEvent::Error { message } => {
                preview.relay_connecting = None;
                preview.status = format!("Relay error: {message}");
            }
        }
    }
}

#[cfg(not(feature = "relay"))]
fn poll_relay_events(_preview: &mut PreviewApp) {}

#[cfg(feature = "relay")]
fn relay_roles(value: &str) -> RelayRoles {
    match value {
        "emit" => RelayRoles::emit_only(),
        "receive" => RelayRoles::receive_only(),
        _ => RelayRoles::both(),
    }
}

#[cfg(feature = "relay")]
fn relay_codec(value: &str) -> RelayCodecKind {
    if value.eq_ignore_ascii_case("pcm") {
        RelayCodecKind::Pcm
    } else {
        RelayCodecKind::Opus
    }
}

#[cfg(feature = "relay")]
fn relay_transport(value: &str) -> RelayTransportPreference {
    RelayTransportPreference::from_str(value).unwrap_or_default()
}

#[cfg(feature = "relay")]
fn relay_qr_payload(preview: &PreviewApp) -> Option<String> {
    let port = preview.source.relay_status().host_port?;
    let link = preview.source.relay_local_links().into_iter().next()?;
    Some(relay_build_qr_payload(
        link.addr,
        port,
        preview.config.relay_host_pin.trim(),
    ))
}

#[cfg(not(feature = "relay"))]
fn relay_qr_payload(_preview: &PreviewApp) -> Option<String> {
    None
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
    let language = language_code(window.get_language_index());
    if preview.config.language != language {
        preview.config.language = language.into();
        preview.i18n = I18n::from_language(language);
        preview.status = preview.i18n.text("status.language_changed");
    }
    preview.config.window_width = window.get_width_().max(760.0);
    preview.config.window_height = window.get_height_().max(520.0);
    preview.config.relay_device_name = window.get_relay_device_name().to_string();
    preview.config.relay_host_pin = window.get_relay_host_pin().to_string();
    preview.config.relay_host_port = window
        .get_relay_host_port_text()
        .trim()
        .parse::<u16>()
        .unwrap_or(preview.config.relay_host_port);
    preview.config.relay_client_target = window.get_relay_client_target().to_string();
    preview.config.relay_client_pin = window.get_relay_client_pin().to_string();
    preview.config.relay_role = relay_role_from_index(window.get_relay_role_index()).into();
    preview.config.relay_codec = relay_codec_from_index(window.get_relay_codec_index()).into();
    preview.config.relay_frame_ms = relay_frame_from_index(window.get_relay_frame_index());
    preview.config.relay_transport =
        relay_transport_from_index(window.get_relay_transport_index()).into();

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
    preview.view.relay_nodes_visible = relay_nodes_visible(preview);
    let snapshot = preview.view.snapshot_with_meters(
        preview.source.graph(),
        &preview.config,
        &preview.meters,
        meter_fallback(&preview.source),
    );
    let node_rows = snapshot
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
                &preview.i18n,
            )
        })
        .collect::<Vec<_>>();
    sync_node_rows(window, nodes, node_rows);
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
    window.set_toast_message(SharedString::from(preview.toast_message.clone()));
    window.set_toast_visible(toast_visible(preview));
    window.set_toast_error(preview.toast_error);
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
    window
        .global::<UiI18n>()
        .set_version(language_index(&preview.config.language));
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
    window.set_relay_device_name(SharedString::from(preview.config.relay_device_name.clone()));
    window.set_relay_host_pin(SharedString::from(preview.config.relay_host_pin.clone()));
    window.set_relay_host_port_text(SharedString::from(
        preview.config.relay_host_port.to_string(),
    ));
    window.set_relay_client_target(SharedString::from(
        preview.config.relay_client_target.clone(),
    ));
    window.set_relay_client_pin(SharedString::from(preview.config.relay_client_pin.clone()));
    window.set_relay_role_index(relay_role_index(&preview.config.relay_role));
    window.set_relay_codec_index(relay_codec_index(&preview.config.relay_codec));
    window.set_relay_frame_index(relay_frame_index(preview.config.relay_frame_ms));
    window.set_relay_transport_index(relay_transport_index(&preview.config.relay_transport));
    window.set_effects(ModelRc::from(Rc::new(VecModel::from(effect_rows(
        &preview.source,
    )))));
    window.set_effect_options(ModelRc::from(Rc::new(VecModel::from(effect_options(
        &preview.source,
    )))));
    window.set_effects_available(preview.source.supports_effect_nodes());
    window.set_relay_rows(ModelRc::from(Rc::new(VecModel::from(relay_rows(preview)))));
    #[cfg(feature = "relay")]
    {
        let relay_status = preview.source.relay_status();
        window.set_relay_available(preview.source.relay_available());
        window.set_relay_host_active(relay_status.host_active);
        window.set_relay_host_endpoint(SharedString::from(relay_host_endpoint(
            preview,
            relay_status.host_port,
        )));
        let payload = relay_qr_payload(preview).unwrap_or_default();
        window.set_relay_qr_payload(SharedString::from(payload.clone()));
        window.set_relay_qr_image(qr_image(&payload));
    }
    #[cfg(not(feature = "relay"))]
    {
        window.set_relay_available(false);
        window.set_relay_host_active(false);
        window.set_relay_host_endpoint(SharedString::new());
        window.set_relay_qr_payload(SharedString::new());
        window.set_relay_qr_image(Image::default());
    }
    preview.snapshot = snapshot;
}

/// Replacing a Slint model invalidates its repeated component instances. Update
/// stable rows in place so the 50ms refresh timer cannot cancel pointer capture
/// between mouse-down and release. Defer structural changes during a drag.
fn sync_node_rows(window: &MainWindow, nodes: &VecModel<NodeRow>, rows: Vec<NodeRow>) {
    let stable_shape = nodes.row_count() == rows.len()
        && rows.iter().enumerate().all(|(index, row)| {
            nodes
                .row_data(index)
                .is_some_and(|current| current.id == row.id)
        });
    if stable_shape {
        for (index, row) in rows.into_iter().enumerate() {
            nodes.set_row_data(index, row);
        }
    } else if !window.get_graph_node_dragging() {
        nodes.set_vec(rows);
    }
}

fn node_row(node: &NodeView, audio: PreviewAudioControl, i18n: &I18n) -> NodeRow {
    NodeRow {
        id: node.id,
        node_title: SharedString::from(node.title.clone()),
        node_subtitle: SharedString::from(localized_node_type(i18n, node.node_type)),
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
                .or_else(|| {
                    node.ports
                        .first()
                        .map(|port| port_type_color(port.port_type))
                })
                .unwrap_or_else(|| node_type_color(node.node_type)),
        ),
        has_audio_controls: node.has_audio_controls,
        meter_rms: node.meter.rms,
        meter_peak: node.meter.peak,
        meter_peak_position: meter_fraction(node.meter.peak),
        meter_available: matches!(node.meter.state, MeterState::Live | MeterState::Demo),
        meter_label: SharedString::from(localized_meter_label(i18n, node.meter.state)),
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

fn localized_node_type(i18n: &I18n, node_type: pw_graph_core::NodeType) -> String {
    let key = match node_type {
        pw_graph_core::NodeType::PipeWire => "canvas.node_type_pipewire",
        pw_graph_core::NodeType::Effect => "canvas.node_type_effect",
        pw_graph_core::NodeType::AlsaMidi => "canvas.node_type_alsa_midi",
        pw_graph_core::NodeType::Unknown => "canvas.node_type_unknown",
    };
    i18n.text(key)
}

fn localized_meter_label(i18n: &I18n, state: MeterState) -> String {
    let key = match state {
        MeterState::Unavailable => "canvas.unknown",
        MeterState::Disabled => "meters.off",
        MeterState::Waiting => "canvas.audio_meter_starting",
        MeterState::Live => "canvas.audio_meter_live",
        MeterState::Demo => "canvas.audio_meter_demo",
    };
    i18n.text(key)
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

/// Match the canvas meter scale: audio levels are shown over a -60 dBFS to
/// 0 dBFS range, not as a linear 0.0–1.0 amplitude fraction.
fn meter_fraction(value: f32) -> f32 {
    let value = value.clamp(0.000001, 1.0);
    ((20.0 * value.log10() + 60.0) / 60.0).clamp(0.0, 1.0)
}

fn shortcut_rows(i18n: &I18n, query: &str) -> Vec<ShortcutRow> {
    const ENTRIES: [(&str, &str); 22] = [
        ("F1", "shortcuts.help"),
        ("Esc", "shortcuts.close_cancel"),
        ("Delete / Backspace", "shortcuts.delete_link"),
        ("Ctrl/Cmd+Z", "shortcuts.undo"),
        ("Ctrl/Cmd+Shift+Z", "shortcuts.redo"),
        ("Ctrl/Cmd+Y", "shortcuts.redo"),
        ("Ctrl/Cmd+S", "shortcuts.save_config"),
        ("Ctrl/Cmd+Shift+S", "shortcuts.save_patchbay"),
        ("Ctrl/Cmd+O", "shortcuts.load_patchbay"),
        ("Ctrl/Cmd+F", "shortcuts.search_hint"),
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

fn effect_rows(source: &ReadOnlyGraphSource) -> Vec<EffectRow> {
    let descriptors = source.effect_descriptors();
    let mut instances = source.effect_instances();
    instances.sort_by(|a, b| a.config.instance_id.cmp(&b.config.instance_id));
    instances
        .into_iter()
        .map(|instance| {
            let descriptor = descriptors
                .iter()
                .find(|descriptor| descriptor.id == instance.config.effect_id);
            let name = descriptor
                .map(|descriptor| descriptor.name.clone())
                .unwrap_or_else(|| instance.config.effect_id.clone());
            let vendor = descriptor
                .map(|descriptor| descriptor.vendor.clone())
                .unwrap_or_else(|| "Unknown effect provider".into());
            let description = instance.config.instance_id.clone();
            let vendor = match instance.error {
                Some(error) => format!("{vendor} · error: {error}"),
                None => vendor,
            };
            let parameter = descriptor.and_then(|descriptor| descriptor.parameters.first());
            EffectRow {
                name: SharedString::from(name),
                vendor: SharedString::from(vendor),
                description: SharedString::from(description),
                enabled: instance.config.enabled,
                has_parameter: parameter.is_some(),
                parameter_id: SharedString::from(
                    parameter
                        .map(|parameter| parameter.id.clone())
                        .unwrap_or_default(),
                ),
                parameter_label: SharedString::from(
                    parameter
                        .map(|parameter| parameter.name.clone())
                        .unwrap_or_default(),
                ),
                parameter_minimum: parameter.map(|parameter| parameter.minimum).unwrap_or(0.0),
                parameter_maximum: parameter.map(|parameter| parameter.maximum).unwrap_or(1.0),
                parameter_value: parameter
                    .map(|parameter| {
                        instance
                            .config
                            .parameters
                            .get(&parameter.id)
                            .copied()
                            .unwrap_or(parameter.default)
                    })
                    .unwrap_or_default(),
            }
        })
        .collect()
}

fn effect_options(source: &ReadOnlyGraphSource) -> Vec<SharedString> {
    source
        .effect_descriptors()
        .into_iter()
        .map(|descriptor| SharedString::from(descriptor.name))
        .collect()
}

fn relay_rows(preview: &PreviewApp) -> Vec<RelayRow> {
    #[cfg(not(feature = "relay"))]
    let _ = preview;
    #[cfg(feature = "relay")]
    {
        let status = preview.source.relay_status();
        let mut rows = Vec::new();
        let mut connected = BTreeSet::new();
        for session in status.sessions {
            let address = session.peer.addr.to_string();
            connected.insert(address.clone());
            let direction = match (session.sending, session.receiving) {
                (true, true) => "send + receive",
                (true, false) => "send",
                (false, true) => "receive",
                (false, false) => "connected",
            };
            rows.push(RelayRow {
                id: SharedString::from(session.id.0.to_string()),
                name: SharedString::from(session.peer.name),
                address: SharedString::from(address),
                state: SharedString::from(format!("connected · {direction}")),
                level: preview
                    .relay_levels
                    .get(&session.id.0)
                    .copied()
                    .unwrap_or_default(),
            });
        }
        let connecting = preview.relay_connecting.as_deref();
        let mut peers = preview.source.relay_peers();
        peers.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.addr.cmp(&b.addr)));
        for peer in peers {
            let address = peer.addr.to_string();
            if connected.contains(&address) {
                continue;
            }
            let state = if connecting == Some(address.as_str()) {
                "connecting"
            } else {
                "available"
            };
            rows.push(RelayRow {
                id: SharedString::from(address.clone()),
                name: SharedString::from(peer.name),
                address: SharedString::from(address),
                state: SharedString::from(state),
                level: 0.0,
            });
        }
        if let Some(target) = connecting {
            if !rows.iter().any(|row| row.address == target) {
                rows.push(RelayRow {
                    id: SharedString::from(target),
                    name: SharedString::from(target),
                    address: SharedString::from(target),
                    state: "connecting".into(),
                    level: 0.0,
                });
            }
        }
        if rows.is_empty() && !preview.config.relay_client_target.trim().is_empty() {
            rows.push(RelayRow {
                id: SharedString::from(preview.config.relay_client_target.clone()),
                name: "Configured peer".into(),
                address: SharedString::from(preview.config.relay_client_target.clone()),
                state: "configured".into(),
                level: 0.0,
            });
        }
        if rows.is_empty() {
            rows.push(RelayRow {
                id: SharedString::new(),
                name: "No relay peers discovered".into(),
                address: "Open discovery or enter an address above".into(),
                state: "idle".into(),
                level: 0.0,
            });
        }
        return rows;
    }
    #[cfg(not(feature = "relay"))]
    {
        vec![RelayRow {
            id: SharedString::new(),
            name: "Relay support not compiled".into(),
            address: "Build with the relay feature to connect peers".into(),
            state: "unavailable".into(),
            level: 0.0,
        }]
    }
}

fn relay_role_index(value: &str) -> i32 {
    match value {
        "emit" => 0,
        "receive" => 1,
        _ => 2,
    }
}

fn relay_role_from_index(index: i32) -> &'static str {
    match index {
        0 => "emit",
        1 => "receive",
        _ => "both",
    }
}

fn relay_codec_index(value: &str) -> i32 {
    if value.eq_ignore_ascii_case("pcm") {
        1
    } else {
        0
    }
}

fn relay_codec_from_index(index: i32) -> &'static str {
    if index == 1 {
        "pcm"
    } else {
        "opus"
    }
}

fn relay_frame_index(frame_ms: u16) -> i32 {
    match frame_ms {
        0..=5 => 0,
        6..=15 => 1,
        16..=30 => 2,
        31..=50 => 3,
        _ => 4,
    }
}

fn relay_frame_from_index(index: i32) -> u16 {
    match index {
        1 => 10,
        2 => 20,
        3 => 40,
        4 => 60,
        _ => 5,
    }
}

fn relay_transport_index(value: &str) -> i32 {
    match value {
        "wifi" => 1,
        "bluetooth" => 2,
        "lan" => 3,
        _ => 0,
    }
}

fn relay_transport_from_index(index: i32) -> &'static str {
    match index {
        1 => "wifi",
        2 => "bluetooth",
        3 => "lan",
        _ => "auto",
    }
}

#[cfg(feature = "relay")]
fn relay_host_endpoint(preview: &PreviewApp, port: Option<u16>) -> String {
    let Some(port) = port else {
        return String::new();
    };
    preview
        .source
        .relay_local_links()
        .into_iter()
        .next()
        .map(|link| format!("{}:{port}", link.addr))
        .unwrap_or_else(|| format!("0.0.0.0:{port}"))
}

#[cfg(feature = "relay")]
fn qr_image(payload: &str) -> Image {
    let Some(scale) = relay_qr::module_scale_for(payload, 236) else {
        return Image::default();
    };
    let Some(bitmap) = relay_qr::render(payload, scale, relay_qr::DEFAULT_QUIET_MODULES) else {
        return Image::default();
    };
    let pixels: Vec<Rgba8Pixel> = bitmap
        .dark
        .into_iter()
        .map(|dark| {
            if dark {
                Rgba8Pixel {
                    r: 0,
                    g: 0,
                    b: 0,
                    a: 255,
                }
            } else {
                Rgba8Pixel {
                    r: 255,
                    g: 255,
                    b: 255,
                    a: 255,
                }
            }
        })
        .collect();
    let mut buffer =
        SharedPixelBuffer::<Rgba8Pixel>::new(bitmap.width as u32, bitmap.height as u32);
    buffer.make_mut_slice().copy_from_slice(&pixels);
    Image::from_rgba8(buffer)
}

fn color(rgba: [u8; 4]) -> Color {
    Color::from_argb_u8(rgba[3], rgba[0], rgba[1], rgba[2])
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::platform::{PointerEventButton, WindowEvent};
    use slint::{LogicalPosition, ModelRc};

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
    fn connection_feedback_is_transient() {
        let mut preview = demo_preview();
        set_connection_feedback(&mut preview, "test connection", false);
        assert!(toast_visible(&preview));

        preview.toast_until = Some(Instant::now() - Duration::from_secs(1));
        assert!(!toast_visible(&preview));
    }

    #[test]
    fn rendered_pin_drag_reports_the_visible_output_to_input_pair() {
        i_slint_backend_testing::init_no_event_loop();

        let window = MainWindow::new().unwrap();
        window
            .window()
            .set_size(slint::LogicalSize::new(1100.0, 760.0));
        let source_ports = Rc::new(VecModel::from(vec![PortRow {
            id: 101,
            label: "output".into(),
            direction: 1,
            color: Color::from_rgb_u8(87, 199, 133),
            y: 0.0,
        }]));
        let target_ports = Rc::new(VecModel::from(vec![PortRow {
            id: 202,
            label: "input".into(),
            direction: 0,
            color: Color::from_rgb_u8(87, 199, 133),
            y: 0.0,
        }]));
        let nodes = Rc::new(VecModel::from(vec![
            NodeRow {
                id: 7,
                node_title: "Source".into(),
                node_subtitle: "PipeWire node".into(),
                x: 100.0,
                y: 100.0,
                width: 280.0,
                height: 100.0,
                selected: false,
                collapsed: false,
                thumbnail: false,
                font_scale: 1.0,
                accent: Color::from_rgb_u8(87, 199, 133),
                has_audio_controls: false,
                meter_rms: 0.0,
                meter_peak: 0.0,
                meter_peak_position: 0.0,
                meter_available: false,
                meter_label: "N/A".into(),
                audio_volume_position: 0.9,
                audio_muted: false,
                ports: ModelRc::from(source_ports),
            },
            NodeRow {
                id: 8,
                node_title: "Target".into(),
                node_subtitle: "PipeWire node".into(),
                x: 500.0,
                y: 100.0,
                width: 280.0,
                height: 100.0,
                selected: false,
                collapsed: false,
                thumbnail: false,
                font_scale: 1.0,
                accent: Color::from_rgb_u8(87, 199, 133),
                has_audio_controls: false,
                meter_rms: 0.0,
                meter_peak: 0.0,
                meter_peak_position: 0.0,
                meter_available: false,
                meter_label: "N/A".into(),
                audio_volume_position: 0.9,
                audio_muted: false,
                ports: ModelRc::from(target_ports),
            },
        ]));
        window.set_nodes(ModelRc::from(nodes));

        let setup = NodeEditorSetup::new(|_, _, _| {});
        let controller = setup.controller().clone();
        slint_node_editor::wire_node_editor!(window, setup);
        window.on_graph_compute_pin_at({
            let controller = controller.clone();
            move |x, y| controller.cache().borrow().find_pin_at(x, y, 10.0)
        });
        let link_result = Rc::new(RefCell::new(None));
        window.on_graph_link_requested({
            let link_result = link_result.clone();
            move |start, end| *link_result.borrow_mut() = Some((start, end))
        });
        // Seed the same world-space geometry that the Pin reporting callbacks
        // provide once the render loop has laid out the rows.
        controller.handle_node_rect(7, 100.0, 100.0, 280.0, 100.0);
        controller.handle_node_rect(8, 500.0, 100.0, 280.0, 100.0);
        controller.handle_pin_position(101, 7, 2, 270.5, 56.5);
        controller.handle_pin_position(202, 8, 1, 9.5, 56.5);

        let dispatch = |event| {
            window.window().dispatch_event(event);
            slint::platform::update_timers_and_animations();
        };
        // Workspace starts at x=76 and the editor pan is (24, 24). These are
        // the visible centers after including the body and port-row offsets.
        dispatch(WindowEvent::PointerPressed {
            position: LogicalPosition::new(470.5, 180.5),
            button: PointerEventButton::Left,
        });
        dispatch(WindowEvent::PointerMoved {
            position: LogicalPosition::new(609.5, 180.5),
        });
        dispatch(WindowEvent::PointerReleased {
            position: LogicalPosition::new(609.5, 180.5),
            button: PointerEventButton::Left,
        });

        assert_eq!(*link_result.borrow(), Some((101, 202)));

        // Easy mode groups channels but must retain standard patch-cable
        // interaction: the pin owns one continuous press-drag-release gesture.
        let easy_result = Rc::new(RefCell::new(None));
        window.on_graph_easy_connect({
            let easy_result = easy_result.clone();
            move |source, _, _, target_pin| {
                *easy_result.borrow_mut() = Some((source, target_pin))
            }
        });
        window.set_connect_mode("easy".into());
        *link_result.borrow_mut() = None;
        dispatch(WindowEvent::PointerPressed {
            position: LogicalPosition::new(470.5, 180.5),
            button: PointerEventButton::Left,
        });
        dispatch(WindowEvent::PointerMoved {
            position: LogicalPosition::new(609.5, 180.5),
        });
        dispatch(WindowEvent::PointerReleased {
            position: LogicalPosition::new(609.5, 180.5),
            button: PointerEventButton::Left,
        });
        assert_eq!(*link_result.borrow(), Some((101, 202)));
        assert_eq!(*easy_result.borrow(), None);

        let cancelled = Rc::new(RefCell::new(false));
        window.on_graph_link_cancelled({
            let cancelled = cancelled.clone();
            move || *cancelled.borrow_mut() = true
        });
        dispatch(WindowEvent::PointerPressed {
            position: LogicalPosition::new(470.5, 180.5),
            button: PointerEventButton::Left,
        });
        dispatch(WindowEvent::PointerMoved {
            position: LogicalPosition::new(800.0, 300.0),
        });
        dispatch(WindowEvent::PointerReleased {
            position: LogicalPosition::new(800.0, 300.0),
            button: PointerEventButton::Left,
        });
        assert!(*cancelled.borrow());
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

    #[test]
    fn node_header_drag_and_body_mute_button_receive_pointer_events() {
        i_slint_backend_testing::init_no_event_loop();

        let window = MainWindow::new().unwrap();
        window
            .window()
            .set_size(slint::LogicalSize::new(1100.0, 760.0));
        let nodes = Rc::new(VecModel::from(vec![
            NodeRow {
                id: 7,
                node_title: "Test audio node".into(),
                node_subtitle: "PipeWire node".into(),
                x: 100.0,
                y: 100.0,
                width: 280.0,
                height: 110.0,
                selected: false,
                collapsed: false,
                thumbnail: false,
                font_scale: 1.0,
                accent: Color::from_rgb_u8(87, 199, 133),
                has_audio_controls: true,
                meter_rms: 0.0,
                meter_peak: 0.0,
                meter_peak_position: 0.0,
                meter_available: false,
                meter_label: "N/A".into(),
                audio_volume_position: 0.9,
                audio_muted: false,
                ports: ModelRc::default(),
            },
            NodeRow {
                id: 8,
                node_title: "Test effect node".into(),
                node_subtitle: "Effect node".into(),
                x: 500.0,
                y: 100.0,
                width: 280.0,
                height: 110.0,
                selected: false,
                collapsed: false,
                thumbnail: false,
                font_scale: 1.0,
                accent: Color::from_rgb_u8(82, 117, 176),
                has_audio_controls: false,
                meter_rms: 0.0,
                meter_peak: 0.0,
                meter_peak_position: 0.0,
                meter_available: false,
                meter_label: "N/A".into(),
                audio_volume_position: 0.9,
                audio_muted: false,
                ports: ModelRc::default(),
            },
        ]));
        window.set_nodes(ModelRc::from(nodes.clone()));

        let drag_result = Rc::new(RefCell::new(None));
        window
            .global::<NodeEditorInternalCallbacks>()
            .on_end_node_drag({
                let drag_result = drag_result.clone();
                let nodes = nodes.clone();
                move |node_id, dx, dy| {
                    GraphLogic::commit_drag(&nodes, node_id, dx, dy);
                    *drag_result.borrow_mut() = Some((node_id, dx, dy));
                }
            });
        let muted_node = Rc::new(RefCell::new(None));
        window.on_graph_audio_mute_toggled({
            let muted_node = muted_node.clone();
            move |node_id| *muted_node.borrow_mut() = Some(node_id)
        });

        let dispatch = |event| {
            window.window().dispatch_event(event);
            slint::platform::update_timers_and_animations();
        };

        // Workspace starts at x=76; editor pan is (24, 24). The node header
        // therefore starts at screen position (200, 124).
        dispatch(WindowEvent::PointerPressed {
            position: LogicalPosition::new(240.0, 140.0),
            button: PointerEventButton::Left,
        });

        // The refresh timer may run after mouse-down but before the pointer
        // crosses the drag threshold. Updating the stable row must retain the
        // component instance and its pointer capture.
        let mut pressed_refresh = nodes.row_data(0).unwrap();
        pressed_refresh.meter_peak = 0.25;
        sync_node_rows(
            &window,
            &nodes,
            vec![pressed_refresh, nodes.row_data(1).unwrap()],
        );
        assert_eq!(nodes.row_data(0).unwrap().meter_peak, 0.25);

        dispatch(WindowEvent::PointerMoved {
            position: LogicalPosition::new(270.0, 160.0),
        });
        assert!(window.get_graph_node_dragging());

        // It may run again during the drag; another in-place update must also
        // preserve the gesture through release.
        let mut stale_refresh = nodes.row_data(0).unwrap();
        stale_refresh.meter_peak = 0.5;
        sync_node_rows(
            &window,
            &nodes,
            vec![stale_refresh, nodes.row_data(1).unwrap()],
        );
        assert_eq!(nodes.row_data(0).unwrap().meter_peak, 0.5);

        dispatch(WindowEvent::PointerReleased {
            position: LogicalPosition::new(270.0, 160.0),
            button: PointerEventButton::Left,
        });

        let (node_id, dx, dy) = drag_result.borrow().expect("header drag must finish");
        assert_eq!(node_id, 7);
        assert!((dx - 30.0).abs() < 0.1);
        assert!((dy - 20.0).abs() < 0.1);
        assert!(!window.get_graph_node_dragging());
        assert!((nodes.row_data(0).unwrap().x - 130.0).abs() < 0.1);
        assert!((nodes.row_data(0).unwrap().y - 120.0).abs() < 0.1);

        // The mute button is in the node body. It must not be captured by the
        // inherited drag area after the node has visually moved by (30, 20).
        dispatch(WindowEvent::PointerPressed {
            position: LogicalPosition::new(485.0, 205.0),
            button: PointerEventButton::Left,
        });
        dispatch(WindowEvent::PointerReleased {
            position: LogicalPosition::new(485.0, 205.0),
            button: PointerEventButton::Left,
        });
        assert_eq!(*muted_node.borrow(), Some(7));

        let easy_drop = Rc::new(RefCell::new(None));
        window.on_graph_easy_connect({
            let easy_drop = easy_drop.clone();
            move |node_id, x, y, _| *easy_drop.borrow_mut() = Some((node_id, x, y))
        });
        window.set_connect_mode("easy".into());
        dispatch(WindowEvent::PointerPressed {
            position: LogicalPosition::new(250.0, 235.0),
            button: PointerEventButton::Left,
        });
        dispatch(WindowEvent::PointerMoved {
            position: LogicalPosition::new(620.0, 190.0),
        });
        dispatch(WindowEvent::PointerReleased {
            position: LogicalPosition::new(620.0, 190.0),
            button: PointerEventButton::Left,
        });
        let (source, x, y) = easy_drop.borrow().expect("easy-mode body drag must finish");
        assert_eq!(source, 7);
        assert!((x - 520.0).abs() < 0.1);
        assert!((y - 166.0).abs() < 0.1);

        // The body drop callback is world-space even when the editor is panned
        // and zoomed. This guards the hit-test path used by the live connector.
        window.set_pan_x(130.0);
        window.set_pan_y(70.0);
        window.set_zoom(1.5);
        dispatch(WindowEvent::PointerPressed {
            position: LogicalPosition::new(431.0, 310.0),
            button: PointerEventButton::Left,
        });
        dispatch(WindowEvent::PointerMoved {
            position: LogicalPosition::new(986.0, 310.0),
        });
        dispatch(WindowEvent::PointerReleased {
            position: LogicalPosition::new(986.0, 310.0),
            button: PointerEventButton::Left,
        });
        let (source, x, y) = easy_drop
            .borrow()
            .expect("panned and zoomed easy-mode drag must finish");
        assert_eq!(source, 7);
        assert!((x - 520.0).abs() < 1.0);
        assert!((y - 166.0).abs() < 1.0);
    }
}
