use eframe::egui;
use pw_graph_backend::{GraphDriver, InMemoryDriver};
use pw_graph_command::{CommandStack, ConnectCommand, DisconnectCommand};
use pw_graph_config::{config_path, AppConfig};
use pw_graph_core::{LinkId, PortId};
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
            "--lang" => args.language = arguments.next(),
            value if value.starts_with("--lang=") => {
                args.language = Some(value.trim_start_matches("--lang=").to_owned())
            }
            "-h" | "--help" => {
                println!(
                    "qpwgraph-rs\n\n{}\n  -m, --minimized       {}\n  -d, --debug           {}\n  -n, --no-alsa-midi    {}\n      --lang <LANG>     {}\n",
                    parser_i18n.text("cli.options"),
                    parser_i18n.text("cli.minimized"),
                    parser_i18n.text("cli.debug"),
                    parser_i18n.text("cli.no_alsa"),
                    parser_i18n.text("cli.lang")
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
    driver: InMemoryDriver,
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
            .unwrap_or_else(|| config_file.with_file_name("default.qpwgraph.json"));
        let mut canvas = GraphCanvas::default();
        canvas.zoom = config.zoom;
        Self {
            driver: InMemoryDriver::demo(),
            commands: CommandStack::new(),
            canvas,
            patchbay: Patchbay::new("default"),
            config,
            config_file,
            patchbay_file,
            status: i18n.text("status.demo_ready"),
            debug: args.debug,
            no_alsa_midi: args.no_alsa_midi,
            start_minimized: args.minimized,
            i18n,
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
                    match self.commands.execute(command, &mut self.driver) {
                        Ok(()) => {
                            self.patchbay.add_connection(
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
                            self.status = self.tf(
                                "status.connect_failed",
                                &[("error", error.to_string())],
                            )
                        }
                    }
                }
                CanvasAction::Disconnect { link } => self.disconnect(link),
            }
        }
    }

    fn disconnect(&mut self, link: LinkId) {
        let Some(existing) = self.driver.graph().link(link).cloned() else {
            return;
        };
        match self
            .commands
            .execute(Box::new(DisconnectCommand::new(link)), &mut self.driver)
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
        match self.commands.undo(&mut self.driver) {
            Ok(true) => self.status = self.t("status.undo_complete"),
            Ok(false) => self.status = self.t("status.nothing_to_undo"),
            Err(error) => {
                self.status = self.tf("status.undo_failed", &[("error", error.to_string())])
            }
        }
    }

    fn redo(&mut self) {
        match self.commands.redo(&mut self.driver) {
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
                ui.separator();
                if ui.button(self.t("toolbar.save_patchbay")).clicked() {
                    self.save_patchbay();
                }
                if ui.button(self.t("toolbar.load_patchbay")).clicked() {
                    self.load_patchbay();
                }
                if ui.button(self.t("toolbar.activate")).clicked() {
                    match self.patchbay.activate(
                        &mut self.driver,
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
                            self.status = self
                                .tf("status.activation_failed", &[("error", error.to_string())])
                        }
                    }
                }
            });
        });

        egui::SidePanel::right("inspector")
            .default_width(250.0)
            .show(ctx, |ui| {
                ui.heading("Graph");
                ui.label(format!("{} nodes", self.driver.graph().nodes.len()));
                ui.label(format!("{} ports", self.driver.graph().ports.len()));
                ui.label(format!("{} links", self.driver.graph().links.len()));
                ui.separator();
                ui.checkbox(&mut self.config.patchbay_exclusive, "Exclusive activation");
                ui.checkbox(
                    &mut self.config.patchbay_auto_disconnect,
                    "Auto-disconnect sinks",
                );
                ui.checkbox(&mut self.config.patchbay_auto_pin, "Auto-pin new links");
                ui.separator();
                ui.heading("Live links");
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
                ui.label("Port colors");
                ui.colored_label(egui::Color32::from_rgb(87, 199, 133), "Audio");
                ui.colored_label(egui::Color32::from_rgb(78, 157, 230), "Video");
                ui.colored_label(egui::Color32::from_rgb(227, 93, 106), "PW/JACK MIDI");
                ui.colored_label(egui::Color32::from_rgb(169, 121, 209), "ALSA MIDI");
            });

        egui::TopBottomPanel::bottom("statusbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(&self.status);
                if self.debug {
                    ui.separator();
                    ui.monospace(format!(
                        "backend=in-memory alsa_midi={}",
                        !self.no_alsa_midi
                    ));
                }
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            let actions = self.canvas.show(ui, self.driver.graph());
            self.handle_canvas_actions(actions);
        });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.config.zoom = self.canvas.zoom;
        self.config.patchbay_path = Some(self.patchbay_file.clone());
        if let Err(error) = self.config.save_to(&self.config_file) {
            eprintln!("could not save config: {error}");
        }
    }
}

fn main() -> eframe::Result<()> {
    let args = parse_args();
    let app = QpwgraphApp::new(args.clone());
    let viewport = egui::ViewportBuilder::default()
        .with_inner_size([app.config.window_width, app.config.window_height]);
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "qpwgraph-rs",
        options,
        Box::new(move |_creation_context| Ok(Box::new(app))),
    )
}

#[allow(dead_code)]
fn _keep_port_id_in_public_cli_docs(_: PortId) {}
