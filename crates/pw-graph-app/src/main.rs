use eframe::egui;
use pw_graph_alsamidi::AlsaMidiDriver;
use pw_graph_backend::{GraphDriver, InMemoryDriver, PipewireDriver};
use pw_graph_command::{CommandStack, ConnectCommand, DisconnectCommand};
use pw_graph_config::{config_path, AppConfig};
use pw_graph_core::{Graph, GraphError, Link, LinkId, Node, NodeId, PortId, PortType};
use pw_graph_i18n::{I18n, Locale};
use pw_graph_patchbay::Patchbay;
use pw_graph_ui::{CanvasAction, GraphCanvas};
use std::path::PathBuf;

#[derive(Clone, Debug, Default)]
struct Args {
    minimized: bool,
    debug: bool,
    no_alsa_midi: bool,
    language: Option<String>,
    demo: bool,
}

#[derive(Default)]
struct CompositeDriver {
    pipewire: Option<PipewireDriver>,
    alsa: Option<AlsaMidiDriver>,
    graph: Graph,
}

impl CompositeDriver {
    fn merge_graph(destination: &mut Graph, source: &Graph) -> Result<(), GraphError> {
        for node in source.nodes.values().cloned() {
            destination.add_node(node)?;
        }
        for port in source.ports.values().cloned() {
            destination.add_port(port)?;
        }
        for link in source.links.values().cloned() {
            destination.insert_existing_link(link)?;
        }
        Ok(())
    }
}

impl GraphDriver for CompositeDriver {
    fn refresh(&mut self) -> pw_graph_backend::BackendResult<Vec<Node>> {
        if let Some(driver) = self.pipewire.as_mut() {
            driver.refresh()?;
        }
        if let Some(driver) = self.alsa.as_mut() {
            driver.refresh()?;
        }
        let mut graph = Graph::default();
        if let Some(driver) = self.pipewire.as_ref() {
            Self::merge_graph(&mut graph, driver.graph())?;
        }
        if let Some(driver) = self.alsa.as_ref() {
            Self::merge_graph(&mut graph, driver.graph())?;
        }
        self.graph = graph;
        Ok(self.graph.nodes.values().cloned().collect())
    }

    fn connect(&mut self, src: PortId, dst: PortId) -> pw_graph_backend::BackendResult<Link> {
        let alsa = 1_u64 << 63;
        let link = if src.0 & alsa != 0 && dst.0 & alsa != 0 {
            self.alsa
                .as_mut()
                .ok_or_else(|| {
                    pw_graph_backend::BackendError::Unsupported("ALSA backend is disabled".into())
                })?
                .connect(src, dst)?
        } else if src.0 & alsa == 0 && dst.0 & alsa == 0 {
            self.pipewire
                .as_mut()
                .ok_or_else(|| {
                    pw_graph_backend::BackendError::Unsupported(
                        "PipeWire backend is disabled".into(),
                    )
                })?
                .connect(src, dst)?
        } else {
            return Err(pw_graph_backend::BackendError::Unsupported(
                "connections cannot cross PipeWire and ALSA MIDI backends".into(),
            ));
        };
        self.refresh()?;
        Ok(link)
    }

    fn disconnect(&mut self, link: LinkId) -> pw_graph_backend::BackendResult<Link> {
        let alsa = 1_u64 << 63;
        let existing = self
            .graph
            .link(link)
            .cloned()
            .ok_or(GraphError::MissingLink(link))?;
        if link.0 & alsa != 0 {
            self.alsa
                .as_mut()
                .ok_or_else(|| {
                    pw_graph_backend::BackendError::Unsupported("ALSA backend is disabled".into())
                })?
                .disconnect(link)?;
        } else {
            self.pipewire
                .as_mut()
                .ok_or_else(|| {
                    pw_graph_backend::BackendError::Unsupported(
                        "PipeWire backend is disabled".into(),
                    )
                })?
                .disconnect(link)?;
        }
        self.refresh()?;
        Ok(existing)
    }

    fn rename_node(&mut self, node: NodeId, name: String) -> pw_graph_backend::BackendResult<()> {
        if node.0 & (1_u64 << 63) != 0 {
            self.alsa
                .as_mut()
                .ok_or_else(|| {
                    pw_graph_backend::BackendError::Unsupported("ALSA backend is disabled".into())
                })?
                .rename_node(node, name)
        } else {
            self.pipewire
                .as_mut()
                .ok_or_else(|| {
                    pw_graph_backend::BackendError::Unsupported(
                        "PipeWire backend is disabled".into(),
                    )
                })?
                .rename_node(node, name)
        }
    }

    fn set_node_position(
        &mut self,
        node: NodeId,
        position: [f32; 2],
    ) -> pw_graph_backend::BackendResult<()> {
        if node.0 & (1_u64 << 63) != 0 {
            self.alsa
                .as_mut()
                .ok_or_else(|| {
                    pw_graph_backend::BackendError::Unsupported("ALSA backend is disabled".into())
                })?
                .set_node_position(node, position)?;
        } else {
            self.pipewire
                .as_mut()
                .ok_or_else(|| {
                    pw_graph_backend::BackendError::Unsupported(
                        "PipeWire backend is disabled".into(),
                    )
                })?
                .set_node_position(node, position)?;
        }
        if let Some(node_data) = self.graph.nodes.get_mut(&node) {
            node_data.position = position;
        }
        Ok(())
    }

    fn graph(&self) -> &Graph {
        &self.graph
    }
    fn is_node_type(&self, node_type: pw_graph_core::NodeType) -> bool {
        self.pipewire
            .as_ref()
            .is_some_and(|driver| driver.is_node_type(node_type))
            || self
                .alsa
                .as_ref()
                .is_some_and(|driver| driver.is_node_type(node_type))
    }
    fn is_port_type(&self, port_type: PortType) -> bool {
        self.pipewire
            .as_ref()
            .is_some_and(|driver| driver.is_port_type(port_type))
            || self
                .alsa
                .as_ref()
                .is_some_and(|driver| driver.is_port_type(port_type))
    }
}

fn parse_args() -> Args {
    let mut args = Args::default();
    let system_language = std::env::var("LANG").unwrap_or_default();
    let parser_i18n = I18n::from_language(&system_language);
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "-m" | "--minimized" => args.minimized = true,
            "-d" | "--debug" => args.debug = true,
            "-n" | "--no-alsa-midi" => args.no_alsa_midi = true,
            "--demo" => args.demo = true,
            "--lang" => args.language = arguments.next(),
            value if value.starts_with("--lang=") => {
                args.language = Some(value.trim_start_matches("--lang=").to_owned())
            }
            "-h" | "--help" => {
                println!(
                    "qpwgraph-rs\n\n{}\n  -m, --minimized       {}\n  -d, --debug           {}\n  -n, --no-alsa-midi    {}\n      --lang <LANG>     {}\n      --demo             {}\n",
                    parser_i18n.text("cli.options"),
                    parser_i18n.text("cli.minimized"),
                    parser_i18n.text("cli.debug"),
                    parser_i18n.text("cli.no_alsa"),
                    parser_i18n.text("cli.lang"),
                    parser_i18n.text("cli.demo")
                );
                std::process::exit(0);
            }
            unknown => eprintln!(
                "{}",
                parser_i18n.format("cli.unknown_option", &[("option", unknown.into())])
            ),
        }
    }
    args
}

struct QpwgraphApp {
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
}

impl QpwgraphApp {
    fn new(args: Args) -> Self {
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
            let mut has_pipewire = false;
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
        let mut canvas = GraphCanvas::default();
        canvas.zoom = config.zoom;
        canvas.sort_ports_by_name = config.sort_type != "id";
        canvas.sort_ports_descending = config.sort_order == "descending";
        canvas.thumbnail_mode = config.thumbnail_view;
        Self {
            driver,
            commands: CommandStack::new(),
            canvas,
            patchbay: Patchbay::new("default"),
            config,
            config_file,
            patchbay_file,
            status,
            debug: args.debug,
            no_alsa_midi: args.no_alsa_midi,
            start_minimized: args.minimized,
            i18n,
            backend_name,
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
}

impl eframe::App for QpwgraphApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
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

        if self.config.toolbar {
            egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui.button(self.t("toolbar.refresh")).clicked() {
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
                    if ui
                        .add_enabled(
                            self.commands.can_undo(),
                            egui::Button::new(self.t("toolbar.undo")),
                        )
                        .clicked()
                    {
                        self.undo();
                    }
                    if ui
                        .add_enabled(
                            self.commands.can_redo(),
                            egui::Button::new(self.t("toolbar.redo")),
                        )
                        .clicked()
                    {
                        self.redo();
                    }
                    if self.config.patchbay_toolbar {
                        ui.separator();
                        if ui.button(self.t("toolbar.save_patchbay")).clicked() {
                            self.save_patchbay();
                        }
                        if ui.button(self.t("toolbar.load_patchbay")).clicked() {
                            self.load_patchbay();
                        }
                        if ui.button(self.t("toolbar.activate")).clicked() {
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
                                    self.status = self.tf(
                                        "status.activation_failed",
                                        &[("error", error.to_string())],
                                    )
                                }
                            }
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
                ui.separator();
                let exclusive_label = self.t("inspector.exclusive");
                ui.checkbox(&mut self.config.patchbay_exclusive, exclusive_label);
                let auto_disconnect_label = self.t("inspector.auto_disconnect");
                ui.checkbox(
                    &mut self.config.patchbay_auto_disconnect,
                    auto_disconnect_label,
                );
                let auto_pin_label = self.t("inspector.auto_pin");
                ui.checkbox(&mut self.config.patchbay_auto_pin, auto_pin_label);
                ui.separator();
                ui.heading(self.t("inspector.interface"));
                let toolbar_visible_label = self.t("inspector.toolbar_visible");
                ui.checkbox(&mut self.config.toolbar, toolbar_visible_label);
                let statusbar_visible_label = self.t("inspector.statusbar_visible");
                ui.checkbox(&mut self.config.statusbar, statusbar_visible_label);
                let patchbay_toolbar_visible_label = self.t("inspector.patchbay_toolbar_visible");
                ui.checkbox(
                    &mut self.config.patchbay_toolbar,
                    patchbay_toolbar_visible_label,
                );
                let menubar_visible_label = self.t("inspector.menubar_visible");
                ui.checkbox(&mut self.config.menubar, menubar_visible_label);
                let repel_overlaps_label = self.t("inspector.repel_overlaps");
                ui.checkbox(
                    &mut self.config.repel_overlapping_nodes,
                    repel_overlaps_label,
                );
                let connect_through_label = self.t("inspector.connect_through");
                ui.checkbox(
                    &mut self.config.connect_through_nodes,
                    connect_through_label,
                );
                let thumbnail_label = self.t("inspector.thumbnail_view");
                ui.checkbox(&mut self.canvas.thumbnail_mode, thumbnail_label);
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
                ui.separator();
                egui::ComboBox::from_label(self.t("language.label"))
                    .selected_text(selected_locale.native_name())
                    .show_ui(ui, |ui| {
                        for locale in Locale::ALL {
                            ui.selectable_value(&mut selected_locale, locale, locale.native_name());
                        }
                    });
            });

        if selected_locale != current_locale {
            self.i18n.set_locale(selected_locale);
            self.config.language = selected_locale.code().to_owned();
            self.status = self.t("status.language_changed");
        }

        self.canvas.sort_ports_by_name = self.config.sort_type != "id";
        self.canvas.sort_ports_descending = self.config.sort_order == "descending";
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
        self.config.node_positions = self
            .driver
            .graph()
            .nodes
            .iter()
            .map(|(id, node)| (id.0, node.position))
            .collect();
        self.config.patchbay_path = Some(self.patchbay_file.clone());
        if let Err(error) = self.config.save_to(&self.config_file) {
            eprintln!(
                "{}",
                self.tf("status.config_save_failed", &[("error", error.to_string())])
            );
        }
    }
}

fn main() -> eframe::Result<()> {
    let args = parse_args();
    let app = QpwgraphApp::new(args.clone());
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

#[allow(dead_code)]
fn _keep_port_id_in_public_cli_docs(_: PortId) {}
