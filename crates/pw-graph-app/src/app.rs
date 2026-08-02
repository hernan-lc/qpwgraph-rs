use eframe::egui;
use pw_graph_backend::{GraphDriver, InMemoryDriver};
use pw_graph_command::{CommandStack, ConnectCommand, DisconnectCommand, RenameCommand};
use pw_graph_config::{config_path, AppConfig};
use pw_graph_core::{GraphError, Link, LinkId, NodeId, PortId};
use pw_graph_i18n::I18n;
use pw_graph_patchbay::Patchbay;
use pw_graph_ui::{CanvasAction, GraphCanvas};
use crate::args::Args;
use crate::backend::CompositeDriver;
use std::path::PathBuf;

#[cfg(all(target_os = "linux", feature = "tray"))]
use crate::tray::tray_support;









pub(crate) struct QpwgraphApp {
    driver: Box<dyn GraphDriver>,
    commands: CommandStack,
    canvas: GraphCanvas,
    patchbay: Patchbay,
    config: AppConfig,
    config_file: PathBuf,
    patchbay_file: PathBuf,
    status: String,
    debug: bool,
    no_alsa_midi: bool,
    start_minimized: bool,
    i18n: I18n,
    backend_name: String,
    rename_node: Option<NodeId>,
    rename_buffer: String,
    #[cfg(all(target_os = "linux", feature = "tray"))]
    tray: Option<tray_support::State>,
}

fn icon_button(
    ui: &mut egui::Ui,
    id: &str,
    icon: &str,
    label: String,
    explanation: String,
) -> bool {
    ui.push_id(("action", id), |ui| {
        ui.button(format!("{icon} {label}"))
            .on_hover_text(explanation)
            .clicked()
    })
    .inner
}

fn icon_checkbox(
    ui: &mut egui::Ui,
    id: &str,
    value: &mut bool,
    icon: &str,
    label: String,
    explanation: String,
) -> bool {
    ui.push_id(("configuration", id), |ui| {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(icon).color(egui::Color32::LIGHT_BLUE));
            let response = ui.checkbox(value, label);
            let changed = response.changed();
            response.on_hover_text(explanation);
            changed
        })
        .inner
    })
    .inner
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
        let mut status = i18n.text("status.demo_ready");
        let (mut driver, backend_name): (Box<dyn GraphDriver>, String) = if args.demo {
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
                        (Box::new(InMemoryDriver::demo()), "in-memory".into())
                    }
                }
            } else {
                (Box::new(InMemoryDriver::demo()), "in-memory".into())
            }
        };
        for (node_id, position) in &config.node_positions {
            let _ = driver.set_node_position(NodeId(*node_id), *position);
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
            rename_node: None,
            rename_buffer: String::new(),
            #[cfg(all(target_os = "linux", feature = "tray"))]
            tray,
        }
    }

    fn t(&self, key: &str) -> String {
        self.i18n.text(key)
    }

    fn tf(&self, key: &str, variables: &[(&str, String)]) -> String {
        self.i18n.format(key, variables)
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

    fn disconnect(&mut self, link: LinkId) {
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

    fn activate_patchbay(&mut self) {
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
            .map(|(id, node)| (id.0, node.position))
            .collect();
        self.config.patchbay_path = Some(self.patchbay_file.clone());
    }

    fn save_config_now(&mut self) {
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

        if self.config.menubar {
            egui::TopBottomPanel::top("menubar").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if icon_button(
                        ui,
                        "menubar.refresh",
                        "⟳",
                        self.t("toolbar.refresh"),
                        self.t("help.refresh"),
                    ) {
                        match self.driver.refresh() {
                            Ok(nodes) => {
                                self.status = self
                                    .tf("status.refreshed", &[("count", nodes.len().to_string())])
                            }
                            Err(error) => {
                                self.status = self
                                    .tf("status.refresh_failed", &[("error", error.to_string())])
                            }
                        }
                    }
                    if icon_button(
                        ui,
                        "menubar.save",
                        "▣",
                        self.t("toolbar.save_patchbay"),
                        self.t("help.save_patchbay"),
                    ) {
                        self.save_patchbay();
                    }
                    if icon_button(
                        ui,
                        "menubar.load",
                        "□",
                        self.t("toolbar.load_patchbay"),
                        self.t("help.load_patchbay"),
                    ) {
                        self.load_patchbay();
                    }
                    if icon_button(
                        ui,
                        "menubar.activate",
                        "▶",
                        self.t("toolbar.activate"),
                        self.t("help.activate"),
                    ) {
                        self.activate_patchbay();
                    }
                });
            });
        }

        if self.config.toolbar {
            egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if icon_button(
                        ui,
                        "toolbar.refresh",
                        "⟳",
                        self.t("toolbar.refresh"),
                        self.t("help.refresh"),
                    ) {
                        match self.driver.refresh() {
                            Ok(nodes) => {
                                self.status = self
                                    .tf("status.refreshed", &[("count", nodes.len().to_string())])
                            }
                            Err(error) => {
                                self.status = self
                                    .tf("status.refresh_failed", &[("error", error.to_string())])
                            }
                        }
                    }
                    let undo_response = ui
                        .add_enabled(
                            self.commands.can_undo(),
                            egui::Button::new(format!("↶ {}", self.t("toolbar.undo"))),
                        )
                        .on_hover_text(self.t("help.undo"));
                    if undo_response.clicked() {
                        self.undo();
                    }
                    let redo_response = ui
                        .add_enabled(
                            self.commands.can_redo(),
                            egui::Button::new(format!("↷ {}", self.t("toolbar.redo"))),
                        )
                        .on_hover_text(self.t("help.redo"));
                    if redo_response.clicked() {
                        self.redo();
                    }
                    if self.config.patchbay_toolbar {
                        ui.separator();
                        if icon_button(
                            ui,
                            "toolbar.save",
                            "▣",
                            self.t("toolbar.save_patchbay"),
                            self.t("help.save_patchbay"),
                        ) {
                            self.save_patchbay();
                        }
                        if icon_button(
                            ui,
                            "toolbar.load",
                            "□",
                            self.t("toolbar.load_patchbay"),
                            self.t("help.load_patchbay"),
                        ) {
                            self.load_patchbay();
                        }
                        if icon_button(
                            ui,
                            "toolbar.snapshot",
                            "◉",
                            self.t("toolbar.snapshot"),
                            self.t("help.snapshot"),
                        ) {
                            self.snapshot_patchbay();
                        }
                        if icon_button(
                            ui,
                            "toolbar.activate",
                            "▶",
                            self.t("toolbar.activate"),
                            self.t("help.activate"),
                        ) {
                            self.activate_patchbay();
                        }
                    }
                });
            });
        }

        let current_locale = self.i18n.locale();
        let mut selected_locale = current_locale;
        egui::SidePanel::right("inspector")
            .default_width(250.0)
            .show(ctx, |ui| {
                ui.heading(self.t("inspector.graph"));
                ui.label(self.tf(
                    "inspector.nodes",
                    &[("count", self.driver.graph().nodes.len().to_string())],
                ));
                ui.label(self.tf(
                    "inspector.ports",
                    &[("count", self.driver.graph().ports.len().to_string())],
                ));
                ui.label(self.tf(
                    "inspector.links",
                    &[("count", self.driver.graph().links.len().to_string())],
                ));
                if let Some(selected_node) = self.canvas.selected_node {
                    let current_name = self
                        .driver
                        .graph()
                        .node(selected_node)
                        .map(|node| node.name.clone());
                    if let Some(current_name) = current_name {
                        ui.separator();
                        ui.label(self.t("inspector.selected_node"));
                        if self.rename_node != Some(selected_node) {
                            self.rename_node = Some(selected_node);
                            self.rename_buffer = current_name.clone();
                        }
                        let response = ui.text_edit_singleline(&mut self.rename_buffer);
                        if response.lost_focus()
                            && ui.input(|input| input.key_pressed(egui::Key::Enter))
                            && self.rename_buffer != current_name
                            && !self.rename_buffer.trim().is_empty()
                        {
                            let edited_name = self.rename_buffer.clone();
                            match self.commands.execute(
                                Box::new(RenameCommand::new(
                                    selected_node,
                                    current_name,
                                    edited_name,
                                )),
                                self.driver.as_mut(),
                            ) {
                                Ok(()) => self.status = self.t("status.renamed"),
                                Err(error) => {
                                    self.status = self
                                        .tf("status.rename_failed", &[("error", error.to_string())])
                                }
                            }
                        }
                    }
                }
                ui.separator();
                ui.heading(format!("⚙ {}", self.t("inspector.configuration")));
                ui.label(self.t("inspector.configuration_hint"))
                    .on_hover_text(self.t("help.configuration"));
                ui.horizontal(|ui| {
                    ui.label("🌐");
                    egui::ComboBox::from_label(self.t("language.label"))
                        .selected_text(selected_locale.native_name())
                        .show_ui(ui, |ui| {
                            for locale in Locale::ALL {
                                ui.selectable_value(
                                    &mut selected_locale,
                                    locale,
                                    locale.native_name(),
                                );
                            }
                        });
                });
                ui.label(self.t("help.language"));
                if icon_button(
                    ui,
                    "configuration.save",
                    "▣",
                    self.t("inspector.save_configuration"),
                    self.t("help.save_configuration"),
                ) {
                    self.save_config_now();
                }
                ui.label(self.tf(
                    "inspector.config_path",
                    &[("path", self.config_file.display().to_string())],
                ));
                ui.separator();
                ui.label(self.t("inspector.patchbay_options"));
                let exclusive_label = self.t("inspector.exclusive");
                let exclusive_help = self.t("help.exclusive");
                let auto_disconnect_label = self.t("inspector.auto_disconnect");
                let auto_disconnect_help = self.t("help.auto_disconnect");
                let auto_pin_label = self.t("inspector.auto_pin");
                let auto_pin_help = self.t("help.auto_pin");
                let patchbay_activated_label = self.t("inspector.patchbay_activated");
                let patchbay_activated_help = self.t("help.patchbay_activated");
                icon_checkbox(
                    ui,
                    "patchbay.exclusive",
                    &mut self.config.patchbay_exclusive,
                    "◇",
                    exclusive_label,
                    exclusive_help,
                );
                icon_checkbox(
                    ui,
                    "patchbay.auto_disconnect",
                    &mut self.config.patchbay_auto_disconnect,
                    "⇄",
                    auto_disconnect_label,
                    auto_disconnect_help,
                );
                icon_checkbox(
                    ui,
                    "patchbay.auto_pin",
                    &mut self.config.patchbay_auto_pin,
                    "⚑",
                    auto_pin_label,
                    auto_pin_help,
                );
                let patchbay_activated_before = self.config.patchbay_activated;
                icon_checkbox(
                    ui,
                    "patchbay.activated",
                    &mut self.config.patchbay_activated,
                    "⏱",
                    patchbay_activated_label,
                    patchbay_activated_help,
                );
                if self.config.patchbay_activated && !patchbay_activated_before {
                    self.activate_patchbay();
                }
                ui.separator();
                ui.heading(self.t("inspector.interface"));
                let toolbar_visible_label = self.t("inspector.toolbar_visible");
                let toolbar_visible_help = self.t("help.toolbar_visible");
                let statusbar_visible_label = self.t("inspector.statusbar_visible");
                let statusbar_visible_help = self.t("help.statusbar_visible");
                let patchbay_toolbar_visible_label = self.t("inspector.patchbay_toolbar_visible");
                let patchbay_toolbar_visible_help = self.t("help.patchbay_toolbar_visible");
                let menubar_visible_label = self.t("inspector.menubar_visible");
                let menubar_visible_help = self.t("help.menubar_visible");
                icon_checkbox(
                    ui,
                    "interface.toolbar",
                    &mut self.config.toolbar,
                    "▤",
                    toolbar_visible_label,
                    toolbar_visible_help,
                );
                icon_checkbox(
                    ui,
                    "interface.statusbar",
                    &mut self.config.statusbar,
                    "▥",
                    statusbar_visible_label,
                    statusbar_visible_help,
                );
                icon_checkbox(
                    ui,
                    "interface.patchbay_toolbar",
                    &mut self.config.patchbay_toolbar,
                    "▦",
                    patchbay_toolbar_visible_label,
                    patchbay_toolbar_visible_help,
                );
                icon_checkbox(
                    ui,
                    "interface.menubar",
                    &mut self.config.menubar,
                    "☰",
                    menubar_visible_label,
                    menubar_visible_help,
                );
                ui.separator();
                ui.label(self.t("inspector.behavior"));
                let repel_overlaps_label = self.t("inspector.repel_overlaps");
                let repel_overlaps_help = self.t("help.repel_overlaps");
                let connect_through_label = self.t("inspector.connect_through");
                let connect_through_help = self.t("help.connect_through");
                let thumbnail_label = self.t("inspector.thumbnail_view");
                let thumbnail_help = self.t("help.thumbnail_view");
                icon_checkbox(
                    ui,
                    "behavior.repel",
                    &mut self.config.repel_overlapping_nodes,
                    "✣",
                    repel_overlaps_label,
                    repel_overlaps_help,
                );
                icon_checkbox(
                    ui,
                    "behavior.connect_through",
                    &mut self.config.connect_through_nodes,
                    "↔",
                    connect_through_label,
                    connect_through_help,
                );
                icon_checkbox(
                    ui,
                    "behavior.thumbnail",
                    &mut self.canvas.thumbnail_mode,
                    "▧",
                    thumbnail_label,
                    thumbnail_help,
                );
                let sort_by_name = self.config.sort_type != "id";
                let sort_by_name_before = sort_by_name;
                let mut sort_by_name_choice = sort_by_name;
                egui::ComboBox::from_label(self.t("inspector.sort_ports"))
                    .selected_text(if sort_by_name {
                        self.t("sort.name")
                    } else {
                        self.t("sort.id")
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut sort_by_name_choice, true, self.t("sort.name"));
                        ui.selectable_value(&mut sort_by_name_choice, false, self.t("sort.id"));
                    });
                if sort_by_name_choice != sort_by_name_before {
                    self.config.sort_type = if sort_by_name_choice { "name" } else { "id" }.into();
                }
                let descending = self.config.sort_order == "descending";
                let mut descending_choice = descending;
                egui::ComboBox::from_label(self.t("inspector.sort_order"))
                    .selected_text(if descending {
                        self.t("sort.descending")
                    } else {
                        self.t("sort.ascending")
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut descending_choice,
                            false,
                            self.t("sort.ascending"),
                        );
                        ui.selectable_value(
                            &mut descending_choice,
                            true,
                            self.t("sort.descending"),
                        );
                    });
                if descending_choice != descending {
                    self.config.sort_order = if descending_choice {
                        "descending"
                    } else {
                        "ascending"
                    }
                    .into();
                }
                ui.separator();
                ui.heading(self.t("inspector.live_links"));
                let links: Vec<_> = self.driver.graph().links.values().cloned().collect();
                for link in links {
                    ui.horizontal(|ui| {
                        ui.label(format!("{} → {}", link.output_port, link.input_port));
                        let mut pinned = self
                            .patchbay
                            .connections
                            .iter()
                            .find(|connection| {
                                connection.output_port == link.output_port
                                    && connection.input_port == link.input_port
                            })
                            .is_some_and(|connection| connection.pinned);
                        if ui
                            .checkbox(&mut pinned, self.t("inspector.pinned"))
                            .changed()
                        {
                            if let Some(connection) =
                                self.patchbay.connections.iter_mut().find(|connection| {
                                    connection.output_port == link.output_port
                                        && connection.input_port == link.input_port
                                })
                            {
                                connection.pinned = pinned;
                            } else {
                                self.patchbay.add_graph_connection(
                                    self.driver.graph(),
                                    link.output_port,
                                    link.input_port,
                                    pinned,
                                );
                            }
                        }
                        if ui.small_button("×").clicked() {
                            self.disconnect(link.id);
                        }
                    });
                }
                ui.separator();
                ui.label(self.t("inspector.port_colors"));
                ui.colored_label(egui::Color32::from_rgb(87, 199, 133), self.t("port.audio"));
                ui.colored_label(egui::Color32::from_rgb(78, 157, 230), self.t("port.video"));
                ui.colored_label(
                    egui::Color32::from_rgb(227, 93, 106),
                    self.t("port.pw_midi"),
                );
                ui.colored_label(
                    egui::Color32::from_rgb(169, 121, 209),
                    self.t("port.alsa_midi"),
                );
            });

        if selected_locale != current_locale {
            self.i18n.set_locale(selected_locale);
            self.config.language = selected_locale.code().to_owned();
            self.status = self.t("status.language_changed");
        }

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

