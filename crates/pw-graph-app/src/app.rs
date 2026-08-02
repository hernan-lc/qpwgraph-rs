use crate::args::Args;
use crate::backend::CompositeDriver;
use crate::icons::{icon_button, icon_button_enabled, Icon};
use crate::panels::AppScreen;
use eframe::egui;
use pw_graph_alsamidi::AlsaMidiDriver;
use pw_graph_backend::{GraphDriver, InMemoryDriver, PipewireDriver};
use pw_graph_command::{CommandStack, ConnectCommand, DisconnectCommand};
use pw_graph_config::{config_path, AppConfig};
use pw_graph_core::{Graph, LinkId, NodeId};
use pw_graph_i18n::I18n;
use pw_graph_patchbay::Patchbay;
use pw_graph_ui::{CanvasAction, GraphCanvas};
use std::path::PathBuf;

#[cfg(all(target_os = "linux", feature = "tray"))]
use crate::tray::tray_support;

pub(crate) struct QpwgraphApp {
    pub(crate) driver: Box<dyn GraphDriver>,
    pub(crate) commands: CommandStack,
    pub(crate) canvas: GraphCanvas,
    pub(crate) patchbay: Patchbay,
    pub(crate) config: AppConfig,
    pub(crate) config_file: PathBuf,
    pub(crate) patchbay_file: PathBuf,
    pub(crate) status: String,
    pub(crate) debug: bool,
    pub(crate) no_alsa_midi: bool,
    pub(crate) start_minimized: bool,
    pub(crate) i18n: I18n,
    pub(crate) backend_name: String,
    pub(crate) screen: AppScreen,
    pub(crate) rename_node: Option<NodeId>,
    pub(crate) rename_buffer: String,
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
        let patchbay_file = config
            .patchbay_path
            .clone()
            .unwrap_or_else(|| config_file.with_file_name("default.qpwgraph"));
        let mut status = i18n.text("status.backend_unavailable");
        let (mut driver, backend_name): (Box<dyn GraphDriver>, String) = if args.demo {
            status = i18n.text("status.demo_ready");
            (Box::new(InMemoryDriver::demo()), "in-memory".into())
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
                        (
                            Box::new(InMemoryDriver::new(Graph::default())),
                            "none".into(),
                        )
                    }
                }
            } else {
                (
                    Box::new(InMemoryDriver::new(Graph::default())),
                    "none".into(),
                )
            }
        };
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
        canvas.sort_ports_by_name = config.sort_type != "id";
        canvas.sort_ports_descending = config.sort_order == "descending";
        canvas.thumbnail_mode = config.thumbnail_view;
        canvas.repel_overlapping_nodes = config.repel_overlapping_nodes;
        canvas.connect_through_nodes = config.connect_through_nodes;
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
            config,
            config_file,
            patchbay_file,
            status,
            debug: args.debug,
            no_alsa_midi: args.no_alsa_midi,
            start_minimized: args.minimized,
            i18n,
            backend_name,
            screen: AppScreen::default(),
            rename_node: None,
            rename_buffer: String::new(),
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

    fn refresh_graph(&mut self) {
        match self.driver.refresh() {
            Ok(nodes) => {
                self.status = self.tf("status.refreshed", &[("count", nodes.len().to_string())])
            }
            Err(error) => {
                self.status = self.tf("status.refresh_failed", &[("error", error.to_string())])
            }
        }
    }

    fn handle_canvas_actions(&mut self, actions: Vec<CanvasAction>) {
        for action in actions {
            match action {
                CanvasAction::Connect { output, input } => {
                    let command = Box::new(ConnectCommand::new(output, input));
                    match self.commands.execute(command, self.driver.as_mut()) {
                        Ok(()) => {
                            self.patchbay.add_graph_connection(
                                self.driver.graph(),
                                output,
                                input,
                                self.config.patchbay_auto_pin,
                            );
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
                CanvasAction::Disconnect { link } => self.disconnect(link),
                CanvasAction::MoveNode { node, position } => {
                    let _ = self.driver.set_node_position(node, position);
                }
            }
        }
    }

    pub(crate) fn disconnect(&mut self, link: LinkId) {
        let Some(existing) = self.driver.graph().link(link).cloned() else {
            return;
        };
        match self
            .commands
            .execute(Box::new(DisconnectCommand::new(link)), self.driver.as_mut())
        {
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

    fn undo(&mut self) {
        match self.commands.undo(self.driver.as_mut()) {
            Ok(true) => self.status = self.t("status.undo_complete"),
            Ok(false) => self.status = self.t("status.nothing_to_undo"),
            Err(error) => {
                self.status = self.tf("status.undo_failed", &[("error", error.to_string())])
            }
        }
    }

    fn redo(&mut self) {
        match self.commands.redo(self.driver.as_mut()) {
            Ok(true) => self.status = self.t("status.redo_complete"),
            Ok(false) => self.status = self.t("status.nothing_to_redo"),
            Err(error) => {
                self.status = self.tf("status.redo_failed", &[("error", error.to_string())])
            }
        }
    }

    fn save_patchbay(&mut self) {
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

    fn load_patchbay(&mut self) {
        match Patchbay::load_from(&self.patchbay_file) {
            Ok(patchbay) => {
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

    fn snapshot_patchbay(&mut self) {
        self.patchbay
            .snapshot_graph(self.driver.graph(), self.config.patchbay_auto_pin);
        self.status = self.tf(
            "status.snapshot",
            &[("count", self.patchbay.connections.len().to_string())],
        );
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
        self.config.node_positions = self
            .driver
            .graph()
            .nodes
            .iter()
            .map(|(id, node)| (id.0.to_string(), node.position))
            .collect();
        self.config.patchbay_path = Some(self.patchbay_file.clone());
    }

    pub(crate) fn save_config_now(&mut self) {
        self.sync_config();
        match self.config.save_to(&self.config_file) {
            Ok(()) => self.status = self.t("status.config_saved"),
            Err(error) => {
                self.status = self.tf("status.config_save_failed", &[("error", error.to_string())])
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
        #[cfg(all(target_os = "linux", feature = "tray"))]
        self.poll_tray(ctx);
        if self.start_minimized {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            self.start_minimized = false;
        }
        if ctx.input(|input| input.key_pressed(egui::Key::Z) && input.modifiers.command) {
            if ctx.input(|input| input.modifiers.shift) {
                self.redo();
            } else {
                self.undo();
            }
        }

        if let Some(rect) = ctx.input(|input| input.viewport().inner_rect) {
            self.config.window_width = rect.width();
            self.config.window_height = rect.height();
        }

        if self.config.toolbar || self.config.patchbay_toolbar {
            egui::TopBottomPanel::top("action_toolbar").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if self.config.toolbar {
                        if icon_button(
                            ui,
                            "toolbar.refresh",
                            Icon::Refresh,
                            self.t("toolbar.refresh"),
                            self.t("help.refresh"),
                        ) {
                            self.refresh_graph();
                        }
                        if icon_button_enabled(
                            ui,
                            "toolbar.undo",
                            Icon::Undo,
                            self.t("toolbar.undo"),
                            self.t("help.undo"),
                            self.commands.can_undo(),
                        ) {
                            self.undo();
                        }
                        if icon_button_enabled(
                            ui,
                            "toolbar.redo",
                            Icon::Redo,
                            self.t("toolbar.redo"),
                            self.t("help.redo"),
                            self.commands.can_redo(),
                        ) {
                            self.redo();
                        }
                    }
                    if self.config.patchbay_toolbar {
                        if self.config.toolbar {
                            ui.separator();
                        }
                        if icon_button(
                            ui,
                            "toolbar.save",
                            Icon::Save,
                            self.t("toolbar.save_patchbay"),
                            self.t("help.save_patchbay"),
                        ) {
                            self.save_patchbay();
                        }
                        if icon_button(
                            ui,
                            "toolbar.load",
                            Icon::Load,
                            self.t("toolbar.load_patchbay"),
                            self.t("help.load_patchbay"),
                        ) {
                            self.load_patchbay();
                        }
                        if icon_button(
                            ui,
                            "toolbar.snapshot",
                            Icon::Snapshot,
                            self.t("toolbar.snapshot"),
                            self.t("help.snapshot"),
                        ) {
                            self.snapshot_patchbay();
                        }
                        if icon_button(
                            ui,
                            "toolbar.activate",
                            Icon::Activate,
                            self.t("toolbar.activate"),
                            self.t("help.activate"),
                        ) {
                            self.activate_patchbay();
                        }
                    }
                });
            });
        }

        self.show_gui_panels(ctx);

        self.canvas.sort_ports_by_name = self.config.sort_type != "id";
        self.canvas.sort_ports_descending = self.config.sort_order == "descending";
        self.canvas.repel_overlapping_nodes = self.config.repel_overlapping_nodes;
        self.canvas.connect_through_nodes = self.config.connect_through_nodes;
        self.config.thumbnail_view = self.canvas.thumbnail_mode;

        if self.config.statusbar {
            egui::TopBottomPanel::bottom("statusbar").show(ctx, |ui| {
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
            let connect_hint = self.t("canvas.connect_hint");
            let actions = self.canvas.show(ui, self.driver.graph(), &connect_hint);
            self.handle_canvas_actions(actions);
        });
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
        Box::new(move |_creation_context| Ok(Box::new(app))),
    )
}
