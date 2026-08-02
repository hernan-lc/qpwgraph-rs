use crate::app::QpwgraphApp;
use crate::icons::{icon_button, icon_checkbox, icon_heading, icon_label, nav_icon_button, Icon};
use egui::{Color32, RichText, Stroke, Ui};
use pw_graph_backend::MeterPolicy;
use pw_graph_command::RenameCommand;
use pw_graph_core::{NodeId, PortId};
use pw_graph_i18n::Locale;

const PANEL_FILL: Color32 = Color32::from_rgb(25, 29, 36);
const SECTION_FILL: Color32 = Color32::from_rgb(30, 35, 43);
const SECTION_STROKE: Color32 = Color32::from_rgb(59, 70, 84);

fn apply_panel_text_scale(ui: &mut Ui, scale: f32) {
    let scale = scale.clamp(0.80, 2.0);
    for font_id in ui.style_mut().text_styles.values_mut() {
        font_id.size *= scale;
    }
    ui.spacing_mut().item_spacing = egui::vec2(8.0, 7.0);
    ui.spacing_mut().button_padding = egui::vec2(8.0, 5.0);
}

fn panel_header(ui: &mut Ui, icon: Icon, title: String, hint: String) {
    ui.add_space(4.0);
    icon_heading(ui, icon, title);
    ui.add_space(1.0);
    ui.label(RichText::new(hint).weak());
    ui.add_space(5.0);
    ui.separator();
    ui.add_space(4.0);
}

fn panel_section(ui: &mut Ui, title: String, contents: impl FnOnce(&mut Ui)) {
    egui::Frame::group(ui.style())
        .fill(SECTION_FILL)
        .stroke(Stroke::new(1.0_f32, SECTION_STROKE))
        .inner_margin(9.0)
        .show(ui, |ui| {
            ui.label(
                RichText::new(title)
                    .strong()
                    .color(Color32::from_rgb(205, 216, 230)),
            );
            ui.add_space(5.0);
            contents(ui);
        });
    ui.add_space(8.0);
}

fn stat_card(ui: &mut Ui, label: String, value: String) {
    egui::Frame::none()
        .fill(Color32::from_rgb(36, 43, 53))
        .rounding(5.0)
        .inner_margin(egui::Margin::symmetric(9.0, 6.0))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new(value).strong().size(18.0));
                ui.label(RichText::new(label).small().weak());
            });
        });
}

fn scale_slider(ui: &mut Ui, id: &str, value: &mut f32, label: String, help: String) {
    ui.push_id(("text-scale", id), |ui| {
        ui.horizontal(|ui| {
            ui.label(label);
            let response = ui.add(
                egui::Slider::new(value, 0.80..=2.0)
                    .step_by(0.05)
                    .show_value(false),
            );
            response.on_hover_text(help);
            ui.label(RichText::new(format!("{:.0}%", *value * 100.0)).weak());
        });
    });
}

fn level_db(value: f32) -> f32 {
    (20.0 * value.max(0.000001).log10()).clamp(-120.0, 0.0)
}

fn meter_policy_key(policy: MeterPolicy) -> &'static str {
    match policy {
        MeterPolicy::Disabled => "meters.off",
        MeterPolicy::OnDemand => "meters.on_demand",
        MeterPolicy::Always => "meters.always",
    }
}

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
            .default_width(64.0)
            .frame(egui::Frame::none().fill(PANEL_FILL).inner_margin(6.0))
            .show(ctx, |ui| {
                apply_panel_text_scale(ui, self.config.panel_text_scale);
                self.show_navigation(ui)
            });

        egui::SidePanel::right("screen_panel")
            .default_width(370.0)
            .width_range(310.0..=520.0)
            .frame(egui::Frame::none().fill(PANEL_FILL).inner_margin(10.0))
            .show(ctx, |ui| {
                apply_panel_text_scale(ui, self.config.panel_text_scale);
                egui::ScrollArea::vertical()
                    .id_salt("inspector-scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| match self.screen {
                        AppScreen::Graph => self.show_graph_screen(ui),
                        AppScreen::Patchbay => self.show_patchbay_screen(ui),
                        AppScreen::Interface => self.show_interface_screen(ui),
                        AppScreen::Diagnostics => self.show_diagnostics_screen(ui),
                    });
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
        panel_header(
            ui,
            Icon::Graph,
            self.t("screen.graph"),
            self.t("screen.graph_hint"),
        );
        let node_count = self.driver.graph().nodes.len().to_string();
        let port_count = self.driver.graph().ports.len().to_string();
        let link_count = self.driver.graph().links.len().to_string();
        panel_section(ui, self.t("inspector.overview"), |ui| {
            ui.horizontal_wrapped(|ui| {
                stat_card(ui, self.t("inspector.nodes_short"), node_count);
                stat_card(ui, self.t("inspector.ports_short"), port_count);
                stat_card(ui, self.t("inspector.links_short"), link_count);
            });
        });

        panel_section(ui, self.t("inspector.audio_metering"), |ui| {
            self.show_meter_controls(ui);
        });

        if let Some(pinned_port) = self.canvas.pinned_meter {
            panel_section(ui, self.t("inspector.audio_monitor"), |ui| {
                self.show_meter_monitor(ui, pinned_port);
            });
        }

        if let Some(selected_node) = self.canvas.selected_node {
            panel_section(ui, self.t("inspector.rename"), |ui| {
                self.show_rename_control(ui, selected_node);
            });
        }

        panel_section(ui, self.t("inspector.layout"), |ui| {
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
        });
    }

    fn show_meter_controls(&mut self, ui: &mut Ui) {
        let current = MeterPolicy::parse(&self.config.audio_meters);
        let mut selected = current;
        let response = egui::ComboBox::from_label(self.t("inspector.audio_metering_policy"))
            .selected_text(self.t(meter_policy_key(current)))
            .show_ui(ui, |ui| {
                for policy in MeterPolicy::ALL {
                    let label = self.t(meter_policy_key(policy));
                    ui.selectable_value(&mut selected, policy, label);
                }
            });
        response
            .response
            .on_hover_text(self.t("help.audio_metering_policy"));
        if selected != current {
            self.config.audio_meters = selected.as_str().to_owned();
        }
        ui.label(
            RichText::new(self.t(match selected {
                MeterPolicy::Disabled => "meters.off_hint",
                MeterPolicy::OnDemand => "meters.on_demand_hint",
                MeterPolicy::Always => "meters.always_hint",
            }))
            .small()
            .weak(),
        );
        ui.add_space(2.0);
        if icon_button(
            ui,
            "meters.reset",
            Icon::Refresh,
            self.t("inspector.audio_reset"),
            self.t("help.audio_reset"),
        ) {
            self.reset_audio_config();
        }
    }

    fn show_meter_monitor(&mut self, ui: &mut Ui, port_id: PortId) {
        let Some(port) = self.driver.graph().port(port_id) else {
            self.canvas.pinned_meter = None;
            return;
        };
        let node_id = port.node_id;
        let port_name = port.name.clone();
        let node_name = self
            .driver
            .graph()
            .node(node_id)
            .map(|node| node.name.clone())
            .unwrap_or_else(|| node_id.to_string());
        ui.label(RichText::new(format!("{node_name} / {port_name}")).strong());
        ui.label(RichText::new(self.t("canvas.audio_meter_node")).weak());
        if let Some(reading) = self.canvas.meters.get(&node_id).copied() {
            if reading.available {
                let stale = reading.age_ms > 750;
                ui.add(
                    egui::ProgressBar::new(reading.rms.clamp(0.0, 1.0))
                        .desired_width(ui.available_width())
                        .text(format!(
                            "{}  {:.1} dB",
                            self.t("canvas.audio_meter_rms"),
                            level_db(reading.rms)
                        )),
                );
                let peak_hold = self.canvas.meter_peak_hold(node_id, reading.peak);
                ui.add(
                    egui::ProgressBar::new(peak_hold.clamp(0.0, 1.0))
                        .desired_width(ui.available_width())
                        .text(format!(
                            "{}  {:.1} dB",
                            self.t("canvas.audio_meter_peak_hold"),
                            level_db(peak_hold)
                        )),
                );
                ui.label(
                    RichText::new(if stale {
                        self.t("canvas.audio_meter_stale")
                    } else {
                        self.t("canvas.audio_meter_live")
                    })
                    .weak(),
                );
                ui.label(
                    RichText::new(self.tf(
                        "canvas.audio_meter_age",
                        &[("age", reading.age_ms.to_string())],
                    ))
                    .small()
                    .weak(),
                );
            } else {
                ui.label(RichText::new(self.t("canvas.audio_meter_unavailable")).weak());
            }
        } else {
            ui.label(RichText::new(self.t("canvas.audio_meter_unavailable")).weak());
        }
        if ui
            .button(self.t("inspector.audio_monitor_unpin"))
            .on_hover_text(self.t("help.audio_monitor_unpin"))
            .clicked()
        {
            self.canvas.pinned_meter = None;
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
        ui.label(RichText::new(self.t("inspector.selected_node")).weak());
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
        panel_header(
            ui,
            Icon::Patchbay,
            self.t("screen.patchbay"),
            self.t("screen.patchbay_hint"),
        );
        panel_section(ui, self.t("inspector.patchbay_options"), |ui| {
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
        });

        panel_section(ui, self.t("inspector.live_links"), |ui| {
            let links: Vec<_> = self.driver.graph().links.values().cloned().collect();
            if links.is_empty() {
                ui.label(RichText::new(self.t("inspector.no_live_links")).weak());
            }
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
                    .unwrap_or_else(|| {
                        (link.output_port.to_string(), link.output_port.to_string())
                    });
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
                        let row_height = ui.spacing().interact_size.y;
                        let spacing = ui.spacing().item_spacing.x;
                        let pinned_width = 78.0;
                        let delete_width = 34.0;
                        let label_width =
                            (ui.available_width() - pinned_width - delete_width - spacing * 2.0)
                                .max(48.0);
                        let summary_response = ui.add_sized(
                            [label_width, row_height],
                            egui::Label::new(RichText::new(link_summary.clone()).weak()).truncate(),
                        );
                        summary_response.on_hover_ui(|ui| {
                            ui.set_max_width(520.0);
                            ui.add(egui::Label::new(link_summary.clone()).wrap());
                        });
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
                            .add_sized(
                                [pinned_width, row_height],
                                egui::Checkbox::new(&mut pinned, self.t("inspector.pinned")),
                            )
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
        });
    }

    fn show_interface_screen(&mut self, ui: &mut Ui) {
        panel_header(
            ui,
            Icon::Settings,
            self.t("screen.interface"),
            self.t("screen.interface_hint"),
        );
        let current_locale = self.i18n.locale();
        let mut selected_locale = current_locale;
        panel_section(ui, self.t("inspector.configuration"), |ui| {
            ui.horizontal(|ui| {
                icon_label(ui, Icon::Language, self.t("language.label"));
                let response = egui::ComboBox::from_label(self.t("language.label"))
                    .selected_text(selected_locale.native_name())
                    .show_ui(ui, |ui| {
                        for locale in Locale::ALL {
                            ui.selectable_value(&mut selected_locale, locale, locale.native_name());
                        }
                    });
                response.response.on_hover_text(self.t("help.language"));
            });
            ui.add_space(2.0);
            if icon_button(
                ui,
                "configuration.save",
                Icon::Save,
                self.t("inspector.save_configuration"),
                self.t("help.save_configuration"),
            ) {
                self.save_config_now();
            }
            ui.label(
                RichText::new(self.tf(
                    "inspector.config_path",
                    &[("path", self.config_file.display().to_string())],
                ))
                .small()
                .weak(),
            );
        });
        if selected_locale != current_locale {
            self.i18n.set_locale(selected_locale);
            self.config.language = selected_locale.code().to_owned();
            self.status = self.t("status.language_changed");
        }

        panel_section(ui, self.t("inspector.interface"), |ui| {
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
        });

        panel_section(ui, self.t("inspector.behavior"), |ui| {
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
        });

        panel_section(ui, self.t("inspector.typography"), |ui| {
            if ui
                .small_button(self.t("inspector.typography_recommended"))
                .on_hover_text(self.t("help.typography_recommended"))
                .clicked()
            {
                self.config.ui_text_scale = 1.10;
                self.config.panel_text_scale = 1.20;
                self.config.node_text_scale = 1.15;
            }
            let ui_text_label = self.t("inspector.ui_text_scale");
            let ui_text_help = self.t("help.ui_text_scale");
            scale_slider(
                ui,
                "ui",
                &mut self.config.ui_text_scale,
                ui_text_label,
                ui_text_help,
            );
            let panel_text_label = self.t("inspector.panel_text_scale");
            let panel_text_help = self.t("help.panel_text_scale");
            scale_slider(
                ui,
                "panels",
                &mut self.config.panel_text_scale,
                panel_text_label,
                panel_text_help,
            );
            let node_text_label = self.t("inspector.node_text_scale");
            let node_text_help = self.t("help.node_text_scale");
            scale_slider(
                ui,
                "nodes",
                &mut self.config.node_text_scale,
                node_text_label,
                node_text_help,
            );
        });
    }

    fn show_diagnostics_screen(&mut self, ui: &mut Ui) {
        panel_header(
            ui,
            Icon::Diagnostics,
            self.t("screen.diagnostics"),
            self.t("screen.diagnostics_hint"),
        );
        panel_section(ui, self.t("inspector.runtime"), |ui| {
            ui.label(self.tf(
                "diagnostics.backend",
                &[("name", self.backend_name.clone())],
            ));
            ui.label(self.tf("diagnostics.status", &[("status", self.status.clone())]));
            ui.horizontal_wrapped(|ui| {
                stat_card(
                    ui,
                    self.t("inspector.nodes_short"),
                    self.driver.graph().nodes.len().to_string(),
                );
                stat_card(
                    ui,
                    self.t("inspector.ports_short"),
                    self.driver.graph().ports.len().to_string(),
                );
                stat_card(
                    ui,
                    self.t("inspector.links_short"),
                    self.driver.graph().links.len().to_string(),
                );
            });
        });
        panel_section(ui, self.t("inspector.port_colors"), |ui| {
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
        panel_section(ui, self.t("inspector.configuration"), |ui| {
            ui.label(
                RichText::new(self.tf(
                    "inspector.config_path",
                    &[("path", self.config_file.display().to_string())],
                ))
                .small()
                .weak(),
            );
        });
    }
}
