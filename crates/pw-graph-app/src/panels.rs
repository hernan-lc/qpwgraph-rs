use crate::app::QpwgraphApp;
use crate::icons::{icon_button, icon_checkbox, icon_heading, icon_label, nav_icon_button, Icon};
use egui::Ui;
use pw_graph_command::RenameCommand;
use pw_graph_core::NodeId;
use pw_graph_i18n::Locale;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum AppScreen {
    #[default]
    Graph,
    Patchbay,
    Interface,
    Diagnostics,
}

impl QpwgraphApp {
    pub(crate) fn show_gui_panels(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("navigation")
            .resizable(false)
            .default_width(58.0)
            .show(ctx, |ui| self.show_navigation(ui));

        egui::SidePanel::right("screen_panel")
            .default_width(320.0)
            .show(ctx, |ui| match self.screen {
                AppScreen::Graph => self.show_graph_screen(ui),
                AppScreen::Patchbay => self.show_patchbay_screen(ui),
                AppScreen::Interface => self.show_interface_screen(ui),
                AppScreen::Diagnostics => self.show_diagnostics_screen(ui),
            });
    }

    fn show_navigation(&mut self, ui: &mut Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(8.0);
            icon_label(ui, Icon::Brand, self.t("app.title"));
            ui.separator();
            let screens = [
                (AppScreen::Graph, Icon::Graph, "screen.graph"),
                (AppScreen::Patchbay, Icon::Patchbay, "screen.patchbay"),
                (AppScreen::Interface, Icon::Settings, "screen.interface"),
                (
                    AppScreen::Diagnostics,
                    Icon::Diagnostics,
                    "screen.diagnostics",
                ),
            ];
            for (screen, icon, label) in screens {
                let help_key = match screen {
                    AppScreen::Graph => "help.navigation_graph",
                    AppScreen::Patchbay => "help.navigation_patchbay",
                    AppScreen::Interface => "help.navigation_interface",
                    AppScreen::Diagnostics => "help.navigation_diagnostics",
                };
                if nav_icon_button(
                    ui,
                    label,
                    icon,
                    self.screen == screen,
                    self.t(label),
                    self.t(help_key),
                ) {
                    self.screen = screen;
                }
            }
        });
    }

    fn show_graph_screen(&mut self, ui: &mut Ui) {
        icon_heading(ui, Icon::Graph, self.t("screen.graph"));
        ui.label(self.t("screen.graph_hint"));
        ui.separator();
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
            self.show_rename_control(ui, selected_node);
        }

        ui.separator();
        ui.label(self.t("inspector.layout"));
        let sort_by_name = self.config.sort_type != "id";
        let sort_by_name_before = sort_by_name;
        let mut sort_by_name_choice = sort_by_name;
        let sort_ports_response = egui::ComboBox::from_label(self.t("inspector.sort_ports"))
            .selected_text(if sort_by_name {
                self.t("sort.name")
            } else {
                self.t("sort.id")
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut sort_by_name_choice, true, self.t("sort.name"));
                ui.selectable_value(&mut sort_by_name_choice, false, self.t("sort.id"));
            });
        sort_ports_response
            .response
            .on_hover_text(self.t("help.sort_ports"));
        if sort_by_name_choice != sort_by_name_before {
            self.config.sort_type = if sort_by_name_choice { "name" } else { "id" }.into();
        }
        let descending = self.config.sort_order == "descending";
        let mut descending_choice = descending;
        let sort_order_response = egui::ComboBox::from_label(self.t("inspector.sort_order"))
            .selected_text(if descending {
                self.t("sort.descending")
            } else {
                self.t("sort.ascending")
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut descending_choice, false, self.t("sort.ascending"));
                ui.selectable_value(&mut descending_choice, true, self.t("sort.descending"));
            });
        sort_order_response
            .response
            .on_hover_text(self.t("help.sort_order"));
        if descending_choice != descending {
            self.config.sort_order = if descending_choice {
                "descending"
            } else {
                "ascending"
            }
            .into();
        }
    }

    fn show_rename_control(&mut self, ui: &mut Ui, selected_node: NodeId) {
        let current_name = self
            .driver
            .graph()
            .node(selected_node)
            .map(|node| node.name.clone());
        let Some(current_name) = current_name else {
            return;
        };
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
                Box::new(RenameCommand::new(selected_node, current_name, edited_name)),
                self.driver.as_mut(),
            ) {
                Ok(()) => self.status = self.t("status.renamed"),
                Err(error) => {
                    self.status = self.tf("status.rename_failed", &[("error", error.to_string())])
                }
            }
        }
    }

    fn show_patchbay_screen(&mut self, ui: &mut Ui) {
        icon_heading(ui, Icon::Patchbay, self.t("screen.patchbay"));
        ui.label(self.t("screen.patchbay_hint"));
        ui.separator();
        let exclusive_label = self.t("inspector.exclusive");
        let exclusive_help = self.t("help.exclusive");
        icon_checkbox(
            ui,
            "patchbay.exclusive",
            &mut self.config.patchbay_exclusive,
            Icon::Exclusive,
            exclusive_label,
            exclusive_help,
        );
        let auto_disconnect_label = self.t("inspector.auto_disconnect");
        let auto_disconnect_help = self.t("help.auto_disconnect");
        icon_checkbox(
            ui,
            "patchbay.auto_disconnect",
            &mut self.config.patchbay_auto_disconnect,
            Icon::AutoDisconnect,
            auto_disconnect_label,
            auto_disconnect_help,
        );
        let auto_pin_label = self.t("inspector.auto_pin");
        let auto_pin_help = self.t("help.auto_pin");
        icon_checkbox(
            ui,
            "patchbay.auto_pin",
            &mut self.config.patchbay_auto_pin,
            Icon::Pin,
            auto_pin_label,
            auto_pin_help,
        );
        let patchbay_activated_before = self.config.patchbay_activated;
        let patchbay_activated_label = self.t("inspector.patchbay_activated");
        let patchbay_activated_help = self.t("help.patchbay_activated");
        icon_checkbox(
            ui,
            "patchbay.activated",
            &mut self.config.patchbay_activated,
            Icon::Timer,
            patchbay_activated_label,
            patchbay_activated_help,
        );
        if self.config.patchbay_activated && !patchbay_activated_before {
            self.activate_patchbay();
        }

        ui.separator();
        ui.heading(self.t("inspector.live_links"));
        let links: Vec<_> = self.driver.graph().links.values().cloned().collect();
        for link in links {
            let (output_node, output_port) = self
                .driver
                .graph()
                .port(link.output_port)
                .map(|port| {
                    (
                        self.driver
                            .graph()
                            .node(port.node_id)
                            .map(|node| node.name.clone())
                            .unwrap_or_else(|| port.node_id.to_string()),
                        port.name.clone(),
                    )
                })
                .unwrap_or_else(|| (link.output_port.to_string(), link.output_port.to_string()));
            let (input_node, input_port) = self
                .driver
                .graph()
                .port(link.input_port)
                .map(|port| {
                    (
                        self.driver
                            .graph()
                            .node(port.node_id)
                            .map(|node| node.name.clone())
                            .unwrap_or_else(|| port.node_id.to_string()),
                        port.name.clone(),
                    )
                })
                .unwrap_or_else(|| (link.input_port.to_string(), link.input_port.to_string()));
            let link_summary = self.tf(
                "patchbay.link_summary",
                &[
                    ("output_node", output_node),
                    ("output_port", output_port),
                    ("input_node", input_node),
                    ("input_port", input_port),
                ],
            );
            ui.push_id(("live-link", link.id), |ui| {
                ui.horizontal(|ui| {
                    ui.label(link_summary);
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
                    if icon_button(
                        ui,
                        "disconnect",
                        Icon::Delete,
                        self.t("toolbar.disconnect"),
                        self.t("help.disconnect_link"),
                    ) {
                        self.disconnect(link.id);
                    }
                });
            });
        }
    }

    fn show_interface_screen(&mut self, ui: &mut Ui) {
        icon_heading(ui, Icon::Settings, self.t("screen.interface"));
        ui.label(self.t("screen.interface_hint"));
        ui.separator();
        let current_locale = self.i18n.locale();
        let mut selected_locale = current_locale;
        ui.horizontal(|ui| {
            icon_label(ui, Icon::Language, self.t("language.label"));
            egui::ComboBox::from_label(self.t("language.label"))
                .selected_text(selected_locale.native_name())
                .show_ui(ui, |ui| {
                    for locale in Locale::ALL {
                        ui.selectable_value(&mut selected_locale, locale, locale.native_name());
                    }
                });
        });
        ui.label(self.t("help.language"));
        if icon_button(
            ui,
            "configuration.save",
            Icon::Save,
            self.t("inspector.save_configuration"),
            self.t("help.save_configuration"),
        ) {
            self.save_config_now();
        }
        ui.label(self.tf(
            "inspector.config_path",
            &[("path", self.config_file.display().to_string())],
        ));
        if selected_locale != current_locale {
            self.i18n.set_locale(selected_locale);
            self.config.language = selected_locale.code().to_owned();
            self.status = self.t("status.language_changed");
        }

        ui.separator();
        ui.heading(self.t("inspector.interface"));
        let toolbar_label = self.t("inspector.toolbar_visible");
        let toolbar_help = self.t("help.toolbar_visible");
        icon_checkbox(
            ui,
            "interface.toolbar",
            &mut self.config.toolbar,
            Icon::Toolbar,
            toolbar_label,
            toolbar_help,
        );
        let statusbar_label = self.t("inspector.statusbar_visible");
        let statusbar_help = self.t("help.statusbar_visible");
        icon_checkbox(
            ui,
            "interface.statusbar",
            &mut self.config.statusbar,
            Icon::Statusbar,
            statusbar_label,
            statusbar_help,
        );
        let patchbay_toolbar_label = self.t("inspector.patchbay_toolbar_visible");
        let patchbay_toolbar_help = self.t("help.patchbay_toolbar_visible");
        icon_checkbox(
            ui,
            "interface.patchbay_toolbar",
            &mut self.config.patchbay_toolbar,
            Icon::Patchbay,
            patchbay_toolbar_label,
            patchbay_toolbar_help,
        );
        ui.separator();
        ui.heading(self.t("inspector.behavior"));
        let repel_label = self.t("inspector.repel_overlaps");
        let repel_help = self.t("help.repel_overlaps");
        icon_checkbox(
            ui,
            "behavior.repel",
            &mut self.config.repel_overlapping_nodes,
            Icon::Repel,
            repel_label,
            repel_help,
        );
        let through_label = self.t("inspector.connect_through");
        let through_help = self.t("help.connect_through");
        icon_checkbox(
            ui,
            "behavior.connect_through",
            &mut self.config.connect_through_nodes,
            Icon::Connect,
            through_label,
            through_help,
        );
        let thumbnail_label = self.t("inspector.thumbnail_view");
        let thumbnail_help = self.t("help.thumbnail_view");
        icon_checkbox(
            ui,
            "behavior.thumbnail",
            &mut self.canvas.thumbnail_mode,
            Icon::Thumbnail,
            thumbnail_label,
            thumbnail_help,
        );
    }

    fn show_diagnostics_screen(&mut self, ui: &mut Ui) {
        icon_heading(ui, Icon::Diagnostics, self.t("screen.diagnostics"));
        ui.label(self.t("screen.diagnostics_hint"));
        ui.separator();
        ui.label(self.tf(
            "diagnostics.backend",
            &[("name", self.backend_name.clone())],
        ));
        ui.label(self.tf("diagnostics.status", &[("status", self.status.clone())]));
        ui.label(self.tf(
            "diagnostics.nodes",
            &[("count", self.driver.graph().nodes.len().to_string())],
        ));
        ui.label(self.tf(
            "diagnostics.ports",
            &[("count", self.driver.graph().ports.len().to_string())],
        ));
        ui.label(self.tf(
            "diagnostics.links",
            &[("count", self.driver.graph().links.len().to_string())],
        ));
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
        ui.label(self.tf(
            "inspector.config_path",
            &[("path", self.config_file.display().to_string())],
        ));
    }
}
