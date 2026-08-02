use crate::args::Args;
use crate::backend::CompositeDriver;
use crate::panels::PreferencesTab;
use eframe::egui;
#[cfg(feature = "alsa")]
use pw_graph_alsamidi::AlsaMidiDriver;
#[cfg(feature = "pipewire")]
use pw_graph_backend::PipewireDriver;
use pw_graph_backend::{DemoDriver, GraphDriver, MeterPolicy};
use pw_graph_command::{
    CommandStack, ConnectCommand, ConnectManyCommand, DisconnectAllCommand, DisconnectCommand,
    DisconnectManyCommand, MoveNodesCommand,
};
use pw_graph_config::{config_path, AppConfig};
use pw_graph_core::{Graph, LinkId, NodeId};
use pw_graph_i18n::I18n;
use pw_graph_patchbay::Patchbay;
use pw_graph_ui::{CanvasAction, ConnectMode, GraphCanvas, MediaFilter, MeterReading};
use rfd::FileDialog;
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[cfg(all(target_os = "linux", feature = "tray"))]
use crate::tray::tray_support;

pub(crate) struct QpwgraphApp {
    pub(crate) driver: Box<dyn GraphDriver>,
    pub(crate) commands: CommandStack,
    pub(crate) canvas: GraphCanvas,
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
    #[cfg(all(target_os = "linux", feature = "tray"))]
    pub(crate) tray: Option<tray_support::State>,
}

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
        let (mut driver, backend_name): (Box<dyn GraphDriver>, String) = if args.demo {
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
        for (node_id, position) in &config.node_positions {
            if let Ok(node_id) = node_id.parse::<u64>() {
                let _ = driver.set_node_position(NodeId(node_id), *position);
            }
        }
        let patchbay = Patchbay::load_from(&patchbay_file).unwrap_or_else(|_| {
            Patchbay::new(
                patchbay_file
                    .file_stem()
                    .and_then(|value| value.to_str())
                    .unwrap_or("default"),
            )
        });
        if config.patchbay_activated {
            match patchbay.activate(
                driver.as_mut(),
                config.patchbay_exclusive,
                config.patchbay_auto_disconnect,
            ) {
                Ok(report) => {
                    status = i18n.format(
                        "status.activated",
                        &[
                            ("connected", report.connected.to_string()),
                            ("present", report.already_present.to_string()),
                            ("disconnected", report.disconnected.to_string()),
                        ],
                    );
                }
                Err(error) => {
                    status =
                        i18n.format("status.activation_failed", &[("error", error.to_string())]);
                }
            }
        }
        let mut canvas = GraphCanvas::default();
        canvas.zoom = config.zoom;
        canvas.node_text_scale = config.node_text_scale;
        canvas.sort_ports_by_name = config.sort_type != "id";
        canvas.sort_ports_descending = config.sort_order == "descending";
        canvas.thumbnail_mode = config.thumbnail_view;
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
        Self {
            driver,
            commands: CommandStack::new(),
            canvas,
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
            shortcut_search: String::new(),
            shortcut_focus_search: false,
            shortcut_scroll_epoch: 0,
            show_preferences: false,
            preferences_tab: PreferencesTab::default(),
            preferences_scroll_epoch: 0,
            profile_name,
            last_meter_refresh: Instant::now() - Duration::from_secs(1),
            last_graph_refresh: Instant::now(),
            meter_policy,
            #[cfg(all(target_os = "linux", feature = "tray"))]
            tray,
        }
    }

    pub(crate) fn t(&self, key: &str) -> String {
        self.i18n.text(key)
    }

    pub(crate) fn tf(&self, key: &str, variables: &[(&str, String)]) -> String {
        self.i18n.format(key, variables)
    }

    fn apply_ui_text_scale(&self, ctx: &egui::Context) {
        let scale = self.config.ui_text_scale.clamp(0.80, 2.0);
        let default_text_styles = egui::Style::default().text_styles;
        ctx.style_mut(|style| {
            for (text_style, font_id) in &default_text_styles {
                let mut scaled_font = font_id.clone();
                scaled_font.size *= scale;
                style.text_styles.insert(text_style.clone(), scaled_font);
            }
        });
    }

    pub(crate) fn refresh_graph(&mut self) {
        match self.driver.refresh() {
            Ok(nodes) => {
                self.last_graph_refresh = Instant::now();
                self.status = self.tf("status.refreshed", &[("count", nodes.len().to_string())])
            }
            Err(error) => {
                self.status = self.tf("status.refresh_failed", &[("error", error.to_string())])
            }
        }
    }

    fn refresh_graph_if_dirty(&mut self) {
        if self.last_graph_refresh.elapsed() < Duration::from_millis(100)
            || !self.driver.graph_dirty()
        {
            return;
        }
        match self.driver.refresh() {
            Ok(_) => self.last_graph_refresh = Instant::now(),
            Err(error) => {
                // Keep the dirty bit eligible for the next frame. A failed
                // retry must not hide a short-lived PipeWire transition for
                // another half second.
                self.status = self.tf("status.refresh_failed", &[("error", error.to_string())]);
            }
        }
    }

    pub(crate) fn any_modal_open(&self) -> bool {
        self.show_shortcuts || self.show_preferences
    }

    pub(crate) fn toggle_shortcuts(&mut self) {
        if self.show_shortcuts {
            self.show_shortcuts = false;
            self.shortcut_focus_search = false;
            return;
        }
        self.show_shortcuts = true;
        self.show_preferences = false;
        self.shortcut_search.clear();
        self.shortcut_focus_search = true;
        self.shortcut_scroll_epoch = self.shortcut_scroll_epoch.wrapping_add(1);
    }

    pub(crate) fn close_shortcuts(&mut self) {
        self.show_shortcuts = false;
        self.shortcut_focus_search = false;
    }

    fn text_input_focused(&self, ctx: &egui::Context) -> bool {
        let shortcut_search_id = self
            .show_shortcuts
            .then_some(egui::Id::new("shortcuts-search"));
        ctx.wants_keyboard_input()
            || ctx.memory(|memory| {
                memory
                    .focused()
                    .is_some_and(|focused| Some(focused) == shortcut_search_id)
            })
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let f1_pressed = ctx.input(|input| input.key_pressed(egui::Key::F1));
        if f1_pressed {
            self.toggle_shortcuts();
            return;
        }

        if self.any_modal_open() {
            if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
                self.close_shortcuts();
                self.show_preferences = false;
            }
            return;
        }

        if self.text_input_focused(ctx) {
            return;
        }

        let (command, shift, undo, redo, save, load) = ctx.input(|input| {
            (
                input.modifiers.command,
                input.modifiers.shift,
                input.key_pressed(egui::Key::Z),
                input.key_pressed(egui::Key::Y),
                input.key_pressed(egui::Key::S),
                input.key_pressed(egui::Key::O),
            )
        });
        if command && undo {
            if shift {
                self.redo();
            } else {
                self.undo();
            }
            return;
        }
        if command && redo {
            self.redo();
            return;
        }
        if command && save {
            if shift {
                self.save_patchbay();
            } else {
                self.save_config_now();
            }
            return;
        }
        if command && load {
            self.load_patchbay();
            return;
        }
        if ctx.input(|input| input.key_pressed(egui::Key::R)) {
            self.refresh_graph();
        }
        if ctx.input(|input| input.key_pressed(egui::Key::A)) {
            self.arrange_nodes();
        }
        if ctx.input(|input| input.key_pressed(egui::Key::T)) {
            self.canvas.thumbnail_mode = !self.canvas.thumbnail_mode;
        }
        if ctx.input(|input| input.key_pressed(egui::Key::Num0)) {
            self.set_media_filter(MediaFilter::All);
        } else if ctx.input(|input| input.key_pressed(egui::Key::Num1)) {
            self.set_media_filter(MediaFilter::Audio);
        } else if ctx.input(|input| input.key_pressed(egui::Key::Num2)) {
            self.set_media_filter(MediaFilter::Video);
        } else if ctx.input(|input| input.key_pressed(egui::Key::Num3)) {
            self.set_media_filter(MediaFilter::Midi);
        }
        if ctx.input(|input| input.key_pressed(egui::Key::Plus)) {
            self.canvas.zoom = (self.canvas.zoom * 1.1).clamp(0.35, 2.5);
        }
        if ctx.input(|input| input.key_pressed(egui::Key::Minus)) {
            self.canvas.zoom = (self.canvas.zoom / 1.1).clamp(0.35, 2.5);
        }
    }

    fn set_media_filter(&mut self, filter: MediaFilter) {
        self.canvas.media_filter = filter;
        self.config.media_filter = filter.as_str().into();
    }

    /// Push a metering-policy change from the panel down to the driver.
    fn sync_meter_policy(&mut self) {
        let policy = MeterPolicy::parse(&self.config.audio_meters);
        if policy == self.meter_policy {
            return;
        }
        self.meter_policy = policy;
        self.canvas.metering_disabled = policy == MeterPolicy::Disabled;
        if let Err(error) = self.driver.set_meter_policy(policy) {
            self.status = self.tf(
                "status.meter_policy_failed",
                &[("error", error.to_string())],
            );
        }
    }

    /// Tell the driver which nodes the user is actually looking at. Under the
    /// on-demand policy this is the only thing that opens a metering stream.
    fn request_visible_meters(&mut self) {
        if self.meter_policy != MeterPolicy::OnDemand {
            return;
        }
        let requested = self.canvas.requested_meter_nodes(self.driver.graph());
        let _ = self.driver.request_meters(&requested);
    }

    /// Release every metering stream so the daemon can return the nodes it had
    /// resumed to their configured state.
    pub(crate) fn reset_audio_config(&mut self) {
        self.canvas.pinned_meter = None;
        self.canvas.meters.clear();
        self.canvas.port_meters.clear();
        match self.driver.reset_audio_config() {
            Ok(()) => self.status = self.t("status.audio_reset"),
            Err(error) => {
                self.status = self.tf("status.audio_reset_failed", &[("error", error.to_string())])
            }
        }
    }

    fn refresh_audio_meters(&mut self) {
        let readings = match self.driver.audio_meters() {
            Ok(readings) => readings,
            Err(_) => {
                self.canvas.meters.clear();
                return;
            }
        };
        self.canvas.meters.clear();
        self.canvas.port_meters.clear();
        for meter in readings.into_iter() {
            let reading = MeterReading {
                rms: meter.rms,
                peak: meter.peak,
                age_ms: meter.age_ms,
                available: meter.available,
            };
            self.canvas.meters.insert(meter.node_id, reading);
            if let Some(port_id) = meter.port_id {
                self.canvas.port_meters.insert(port_id, reading);
            }
        }
        if self
            .canvas
            .pinned_meter
            .is_some_and(|port| self.driver.graph().port(port).is_none())
        {
            self.canvas.pinned_meter = None;
        }
    }

    fn handle_canvas_actions(&mut self, actions: Vec<CanvasAction>) {
        for action in actions {
            match action {
                CanvasAction::Connect { output, input } => {
                    let stable_pair = self
                        .driver
                        .graph()
                        .port_key(output)
                        .zip(self.driver.graph().port_key(input));
                    let command = Box::new(ConnectCommand::new(output, input));
                    match self.commands.execute(command, self.driver.as_mut()) {
                        Ok(()) => {
                            if let Some((output_key, input_key)) = stable_pair {
                                if let (Some(output), Some(input)) = (
                                    self.driver.graph().resolve_port_key(&output_key),
                                    self.driver.graph().resolve_port_key(&input_key),
                                ) {
                                    self.patchbay.add_graph_connection(
                                        self.driver.graph(),
                                        output,
                                        input,
                                        self.config.patchbay_auto_pin,
                                    );
                                }
                            }
                            self.status = self.tf(
                                "status.connected",
                                &[("output", output.to_string()), ("input", input.to_string())],
                            );
                        }
                        Err(error) => {
                            self.status =
                                self.tf("status.connect_failed", &[("error", error.to_string())])
                        }
                    }
                }
                CanvasAction::ConnectMany { pairs } => self.connect_many(pairs),
                CanvasAction::Disconnect { link } => self.disconnect(link),
                CanvasAction::DisconnectMany { links } => self.disconnect_many(links),
                CanvasAction::DisconnectNode { node } => self.disconnect_node(node),
                CanvasAction::ArrangeNodes { nodes } => self.arrange_selected_nodes(nodes),
                CanvasAction::MoveNode { node, position } => {
                    let _ = self.driver.set_node_position(node, position);
                }
                CanvasAction::CommitNodeMove { before, after } => {
                    let _ = self.commands.execute(
                        Box::new(MoveNodesCommand::new(before, after)),
                        self.driver.as_mut(),
                    );
                }
            }
        }
    }

    fn connect_many(&mut self, pairs: Vec<(pw_graph_core::PortId, pw_graph_core::PortId)>) {
        if pairs.is_empty() {
            return;
        }
        let count = pairs.len();
        let stable_pairs: Vec<_> = pairs
            .iter()
            .filter_map(|(output, input)| {
                self.driver
                    .graph()
                    .port_key(*output)
                    .zip(self.driver.graph().port_key(*input))
            })
            .collect();
        match self.commands.execute(
            Box::new(ConnectManyCommand::new(pairs.clone())),
            self.driver.as_mut(),
        ) {
            Ok(()) => {
                for (output_key, input_key) in stable_pairs {
                    if let (Some(output), Some(input)) = (
                        self.driver.graph().resolve_port_key(&output_key),
                        self.driver.graph().resolve_port_key(&input_key),
                    ) {
                        self.patchbay.add_graph_connection(
                            self.driver.graph(),
                            output,
                            input,
                            self.config.patchbay_auto_pin,
                        );
                    }
                }
                self.status = self.tf("status.connected_many", &[("count", count.to_string())]);
            }
            Err(error) => {
                self.status = self.tf("status.connect_failed", &[("error", error.to_string())]);
            }
        }
    }

    fn disconnect_node(&mut self, node: NodeId) {
        let links: Vec<_> = self
            .driver
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
            .collect();
        let ids: Vec<_> = links.iter().map(|link| link.id).collect();
        if ids.is_empty() {
            return;
        }
        let count = ids.len();
        match self.commands.execute(
            Box::new(DisconnectManyCommand::from_links(
                self.driver.graph(),
                links.clone(),
            )),
            self.driver.as_mut(),
        ) {
            Ok(()) => {
                for link in links {
                    self.patchbay
                        .remove_connection(link.output_port, link.input_port);
                }
                self.status = self.tf("status.disconnected_all", &[("count", count.to_string())]);
            }
            Err(error) => {
                self.status = self.tf("status.disconnect_failed", &[("error", error.to_string())]);
            }
        }
    }

    fn arrange_selected_nodes(&mut self, nodes: Vec<NodeId>) {
        let defaults = self.driver.graph().default_node_positions();
        let before: Vec<_> = nodes
            .iter()
            .filter_map(|node| {
                self.driver
                    .graph()
                    .node(*node)
                    .map(|item| (*node, item.position))
            })
            .collect();
        let after: Vec<_> = before
            .iter()
            .map(|(node, current)| (*node, defaults.get(node).copied().unwrap_or(*current)))
            .collect();
        if before == after {
            return;
        }
        if self
            .commands
            .execute(
                Box::new(MoveNodesCommand::new(before, after)),
                self.driver.as_mut(),
            )
            .is_ok()
        {
            self.status = self.tf("status.arranged", &[("count", nodes.len().to_string())]);
        }
    }

    pub(crate) fn disconnect(&mut self, link: LinkId) {
        let Some(existing) = self.driver.graph().link(link).cloned() else {
            return;
        };
        match self.commands.execute(
            Box::new(DisconnectCommand::from_link(
                self.driver.graph(),
                existing.clone(),
            )),
            self.driver.as_mut(),
        ) {
            Ok(()) => {
                self.patchbay
                    .remove_connection(existing.output_port, existing.input_port);
                self.status = self.tf("status.disconnected", &[("link", link.to_string())]);
            }
            Err(error) => {
                self.status = self.tf("status.disconnect_failed", &[("error", error.to_string())])
            }
        }
    }

    pub(crate) fn disconnect_many(&mut self, link_ids: Vec<LinkId>) {
        if link_ids.is_empty() {
            return;
        }
        let links: Vec<_> = link_ids
            .iter()
            .filter_map(|link_id| self.driver.graph().link(*link_id).cloned())
            .collect();
        if links.is_empty() {
            return;
        }
        let count = links.len();
        match self.commands.execute(
            Box::new(DisconnectManyCommand::from_links(
                self.driver.graph(),
                links.clone(),
            )),
            self.driver.as_mut(),
        ) {
            Ok(()) => {
                for link in links {
                    self.patchbay
                        .remove_connection(link.output_port, link.input_port);
                }
                self.canvas.clear_selected_link();
                self.status = self.tf("status.disconnected_all", &[("count", count.to_string())]);
            }
            Err(error) => {
                self.status = self.tf("status.disconnect_failed", &[("error", error.to_string())]);
            }
        }
    }

    pub(crate) fn disconnect_all(&mut self) {
        let count = self.driver.graph().links.len();
        if count == 0 {
            self.status = self.t("status.no_links");
            return;
        }

        match self
            .commands
            .execute(Box::new(DisconnectAllCommand::new()), self.driver.as_mut())
        {
            Ok(()) => {
                self.patchbay.connections.clear();
                self.canvas.clear_selected_link();
                self.status = self.tf("status.disconnected_all", &[("count", count.to_string())]);
            }
            Err(error) => {
                self.status = self.tf("status.disconnect_failed", &[("error", error.to_string())])
            }
        }
    }

    pub(crate) fn undo(&mut self) {
        match self.commands.undo(self.driver.as_mut()) {
            Ok(true) => self.status = self.t("status.undo_complete"),
            Ok(false) => self.status = self.t("status.nothing_to_undo"),
            Err(error) => {
                self.status = self.tf("status.undo_failed", &[("error", error.to_string())])
            }
        }
    }

    pub(crate) fn redo(&mut self) {
        match self.commands.redo(self.driver.as_mut()) {
            Ok(true) => self.status = self.t("status.redo_complete"),
            Ok(false) => self.status = self.t("status.nothing_to_redo"),
            Err(error) => {
                self.status = self.tf("status.redo_failed", &[("error", error.to_string())])
            }
        }
    }

    pub(crate) fn save_patchbay(&mut self) {
        let directory = self
            .config
            .patchbay_dir
            .clone()
            .or_else(|| self.patchbay_file.parent().map(PathBuf::from));
        let selected = FileDialog::new()
            .set_directory(directory.unwrap_or_else(|| PathBuf::from(".")))
            .set_file_name(
                self.patchbay_file
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("default.qpwgraph"),
            )
            .add_filter("Patchbay", &["qpwgraph", "xml", "json"])
            .save_file();
        let Some(path) = selected else {
            return;
        };
        self.select_patchbay_path(path);
        match self.patchbay.save_to(&self.patchbay_file) {
            Ok(()) => {
                self.status = self.tf(
                    "status.saved_patchbay",
                    &[("path", self.patchbay_file.display().to_string())],
                )
            }
            Err(error) => {
                self.status = self.tf(
                    "status.patchbay_save_failed",
                    &[("error", error.to_string())],
                )
            }
        }
    }

    pub(crate) fn load_patchbay(&mut self) {
        let directory = self
            .config
            .patchbay_dir
            .clone()
            .or_else(|| self.patchbay_file.parent().map(PathBuf::from));
        let selected = FileDialog::new()
            .set_directory(directory.unwrap_or_else(|| PathBuf::from(".")))
            .add_filter("Patchbay", &["qpwgraph", "xml", "json"])
            .pick_file();
        let Some(path) = selected else {
            return;
        };
        match Patchbay::load_from(&path) {
            Ok(patchbay) => {
                self.select_patchbay_path(path);
                self.patchbay = patchbay;
                self.status = self.tf(
                    "status.loaded",
                    &[("path", self.patchbay_file.display().to_string())],
                );
            }
            Err(error) => {
                self.status = self.tf(
                    "status.patchbay_load_failed",
                    &[("error", error.to_string())],
                )
            }
        }
    }

    pub(crate) fn select_patchbay_path(&mut self, path: PathBuf) {
        self.patchbay_file = path.clone();
        self.config.patchbay_dir = path.parent().map(PathBuf::from);
        self.config
            .recent_patchbay_paths
            .retain(|item| item != &path);
        self.config.recent_patchbay_paths.insert(0, path.clone());
        self.config.recent_patchbay_paths.truncate(8);
        self.config.patchbay_path = Some(path);
        self.config.patchbay_profiles.insert(
            self.config.active_patchbay_profile.clone(),
            self.patchbay_file.clone(),
        );
    }

    pub(crate) fn choose_patchbay_directory(&mut self) {
        let initial = self
            .config
            .patchbay_dir
            .clone()
            .or_else(|| self.patchbay_file.parent().map(PathBuf::from));
        if let Some(path) = FileDialog::new()
            .set_directory(initial.unwrap_or_else(|| PathBuf::from(".")))
            .pick_folder()
        {
            self.config.patchbay_dir = Some(path);
        }
    }

    pub(crate) fn use_recent_patchbay(&mut self, path: PathBuf) {
        if Patchbay::load_from(&path).is_ok() {
            self.select_patchbay_path(path);
            let _ = self.load_patchbay_from_current();
        }
    }

    pub(crate) fn load_patchbay_from_current(&mut self) -> Result<(), String> {
        match Patchbay::load_from(&self.patchbay_file) {
            Ok(patchbay) => {
                self.patchbay = patchbay;
                self.status = self.tf(
                    "status.loaded",
                    &[("path", self.patchbay_file.display().to_string())],
                );
                Ok(())
            }
            Err(error) => Err(error.to_string()),
        }
    }

    pub(crate) fn activate_patchbay(&mut self) {
        match self.patchbay.activate(
            self.driver.as_mut(),
            self.config.patchbay_exclusive,
            self.config.patchbay_auto_disconnect,
        ) {
            Ok(report) => {
                self.status = self.tf(
                    "status.activated",
                    &[
                        ("connected", report.connected.to_string()),
                        ("present", report.already_present.to_string()),
                        ("disconnected", report.disconnected.to_string()),
                    ],
                )
            }
            Err(error) => {
                self.status = self.tf("status.activation_failed", &[("error", error.to_string())])
            }
        }
    }

    pub(crate) fn snapshot_patchbay(&mut self) {
        self.patchbay
            .snapshot_graph(self.driver.graph(), self.config.patchbay_auto_pin);
        self.status = self.tf(
            "status.snapshot",
            &[("count", self.patchbay.connections.len().to_string())],
        );
    }

    pub(crate) fn arrange_nodes(&mut self) {
        let defaults = self.driver.graph().default_node_positions();
        let before: Vec<_> = self
            .driver
            .graph()
            .nodes
            .values()
            .map(|node| (node.id, node.position))
            .collect();
        let after: Vec<_> = before
            .iter()
            .map(|(node, position)| (*node, defaults.get(node).copied().unwrap_or(*position)))
            .collect();
        if before != after
            && self
                .commands
                .execute(
                    Box::new(MoveNodesCommand::new(before, after)),
                    self.driver.as_mut(),
                )
                .is_ok()
        {
            self.status = self.tf(
                "status.arranged",
                &[("count", self.driver.graph().nodes.len().to_string())],
            );
        }
    }

    fn sync_config(&mut self) {
        self.config.zoom = self.canvas.zoom;
        self.config.sort_type = if self.canvas.sort_ports_by_name {
            "name".into()
        } else {
            "id".into()
        };
        self.config.sort_order = if self.canvas.sort_ports_descending {
            "descending".into()
        } else {
            "ascending".into()
        };
        self.config.thumbnail_view = self.canvas.thumbnail_mode;
        self.config.connect_mode = self.canvas.connect_mode.as_str().into();
        self.config.media_filter = self.canvas.media_filter.as_str().into();
        self.config.graph_search = self.canvas.search_query.clone();
        self.config.node_positions = self
            .driver
            .graph()
            .nodes
            .iter()
            .map(|(id, node)| (id.0.to_string(), node.position))
            .collect();
        self.config.patchbay_path = Some(self.patchbay_file.clone());
        self.config.patchbay_profiles.insert(
            self.config.active_patchbay_profile.clone(),
            self.patchbay_file.clone(),
        );
    }

    pub(crate) fn save_config_now(&mut self) {
        self.sync_config();
        match self.config.save_to(&self.config_file) {
            Ok(()) => {
                self.config_saved_snapshot = self.config.clone();
                self.config_dirty_since = None;
                self.status = self.t("status.config_saved");
            }
            Err(error) => {
                self.status = self.tf("status.config_save_failed", &[("error", error.to_string())])
            }
        }
    }

    fn autosave_config(&mut self) {
        self.sync_config();
        if self.config == self.config_saved_snapshot {
            self.config_dirty_since = None;
            return;
        }
        let dirty_since = self.config_dirty_since.get_or_insert_with(Instant::now);
        if dirty_since.elapsed() < Duration::from_millis(500) {
            return;
        }
        match self.config.save_to(&self.config_file) {
            Ok(()) => {
                self.config_saved_snapshot = self.config.clone();
                self.config_dirty_since = None;
            }
            Err(error) => {
                self.config_dirty_since = Some(Instant::now());
                self.status = self.tf("status.config_save_failed", &[("error", error.to_string())]);
            }
        }
    }

    #[cfg(all(target_os = "linux", feature = "tray"))]
    fn poll_tray(&mut self, ctx: &egui::Context) {
        let Some(tray) = self.tray.as_ref() else {
            return;
        };
        while let Ok(command) = tray.receiver.try_recv() {
            match command {
                tray_support::Command::Show => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                tray_support::Command::Hide => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                }
                tray_support::Command::Quit => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
    }
}

impl eframe::App for QpwgraphApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.apply_ui_text_scale(ctx);
        self.sync_meter_policy();
        self.refresh_graph_if_dirty();
        if self.last_meter_refresh.elapsed() >= Duration::from_millis(50) {
            self.refresh_audio_meters();
            self.last_meter_refresh = Instant::now();
        }
        ctx.request_repaint_after(Duration::from_millis(50));
        #[cfg(all(target_os = "linux", feature = "tray"))]
        self.poll_tray(ctx);
        if self.start_minimized {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            self.start_minimized = false;
        }
        self.handle_shortcuts(ctx);

        if let Some(rect) = ctx.input(|input| input.viewport().inner_rect) {
            self.config.window_width = rect.width();
            self.config.window_height = rect.height();
        }

        self.show_gui_panels(ctx);

        self.canvas.media_filter = MediaFilter::parse(&self.config.media_filter);
        self.canvas.search_query = self.config.graph_search.clone();
        self.canvas.sort_ports_by_name = self.config.sort_type != "id";
        self.canvas.sort_ports_descending = self.config.sort_order == "descending";
        self.canvas.node_text_scale = self.config.node_text_scale;
        self.canvas.repel_overlapping_nodes = self.config.repel_overlapping_nodes;
        self.canvas.connect_through_nodes = self.config.connect_through_nodes;
        self.config.thumbnail_view = self.canvas.thumbnail_mode;
        self.config.connect_mode = self.canvas.connect_mode.as_str().into();

        if self.config.statusbar {
            egui::TopBottomPanel::bottom("statusbar")
                .frame(
                    egui::Frame::none()
                        .fill(egui::Color32::from_rgb(29, 33, 40))
                        .inner_margin(egui::Margin::symmetric(8.0, 4.0)),
                )
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(&self.status);
                        if self.debug {
                            ui.separator();
                            ui.monospace(self.i18n.format(
                                "debug.backend",
                                &[
                                    ("backend", self.backend_name.clone()),
                                    ("enabled", (!self.no_alsa_midi).to_string()),
                                ],
                            ));
                        }
                    });
                });
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            let actions = if self.any_modal_open() {
                Vec::new()
            } else {
                self.canvas.show_with_keyboard_shortcuts(
                    ui,
                    self.driver.graph(),
                    &self.i18n,
                    !self.text_input_focused(ctx),
                )
            };
            self.handle_canvas_actions(actions);
        });

        // Runs after the canvas so the request reflects what this frame drew.
        self.request_visible_meters();
        self.show_shortcuts_modal(ctx);
        self.show_preferences_modal(ctx);
        self.autosave_config();
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        #[cfg(all(target_os = "linux", feature = "tray"))]
        if let Some(tray) = self.tray.as_ref() {
            tray.shutdown();
        }
        self.sync_config();
        if let Err(error) = self.config.save_to(&self.config_file) {
            eprintln!(
                "{}",
                self.tf("status.config_save_failed", &[("error", error.to_string())])
            );
        }
    }
}

pub(crate) fn run(args: Args) -> eframe::Result<()> {
    let app = QpwgraphApp::new(args);
    let window_title = app.i18n.text("app.title");
    let viewport = egui::ViewportBuilder::default()
        .with_inner_size([app.config.window_width, app.config.window_height]);
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        &window_title,
        options,
        Box::new(move |creation_context| {
            egui_extras::install_image_loaders(&creation_context.egui_ctx);
            Ok(Box::new(app))
        }),
    )
}
