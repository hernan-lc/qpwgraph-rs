use super::components::modal_combo;
use super::shared::{
    apply_panel_text_scale, fresh_scroll_area, full_panel_window, meter_policy_key, panel_section,
    preferences_rect, scale_slider, show_backdrop_rect, show_close_button,
};
use crate::app::QpwgraphApp;
use crate::icons::{icon_button, icon_checkbox, icon_label, Icon};
use eframe::egui::{self, RichText, Ui};
use pw_graph_backend::MeterPolicy;
use pw_graph_i18n::Locale;

/// Tabs inside the Preferences modal, which holds settings you configure once
/// rather than watch while working the canvas.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub(crate) enum PreferencesTab {
    #[default]
    Interface,
    Patchbay,
}

impl QpwgraphApp {
    fn show_meter_controls(&mut self, ui: &mut Ui) {
        let current = MeterPolicy::parse(&self.config.audio_meters);
        let mut selected = current;
        if icon_button(
            ui,
            "meters.reset",
            Icon::Refresh,
            self.t("inspector.audio_reset"),
            self.t("help.audio_reset"),
        ) {
            self.reset_audio_config();
        }
        modal_combo(
            ui,
            "meters.policy",
            self.t("inspector.audio_metering_policy"),
            self.t(meter_policy_key(current)),
            &mut selected,
            MeterPolicy::ALL
                .into_iter()
                .map(|policy| (policy, self.t(meter_policy_key(policy)))),
        );
        ui.label(
            RichText::new(self.t("help.audio_metering_policy"))
                .small()
                .weak(),
        );
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
    }

    pub(crate) fn show_preferences_modal(&mut self, ctx: &egui::Context) {
        if !self.show_preferences {
            return;
        }
        let rect = preferences_rect(ctx);
        if show_backdrop_rect(ctx, "preferences", rect) {
            self.show_preferences = false;
            return;
        }
        full_panel_window("preferences", self.t("preferences.title"), rect).show(ctx, |ui| {
            apply_panel_text_scale(ui, self.config.panel_text_scale);
            ui.horizontal(|ui| {
                for (tab, label_key) in [
                    (PreferencesTab::Interface, "screen.interface"),
                    (PreferencesTab::Patchbay, "inspector.patchbay_options"),
                ] {
                    if ui
                        .selectable_label(self.preferences_tab == tab, self.t(label_key))
                        .clicked()
                        && self.preferences_tab != tab
                    {
                        self.preferences_tab = tab;
                        self.preferences_scroll_epoch =
                            self.preferences_scroll_epoch.wrapping_add(1);
                    }
                }
            });
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(6.0);
            let scroll_id = (
                "preferences-scroll",
                self.preferences_tab,
                self.preferences_scroll_epoch,
            );
            fresh_scroll_area(scroll_id, ui.available_height().max(0.0))
                .auto_shrink([false, false])
                .show(ui, |ui| match self.preferences_tab {
                    PreferencesTab::Interface => self.show_preferences_interface_tab(ui),
                    PreferencesTab::Patchbay => self.show_preferences_patchbay_tab(ui),
                });
            ui.add_space(8.0);
            if show_close_button(ui, self.t("shortcuts.close")) {
                self.show_preferences = false;
            }
        });
    }

    fn show_preferences_patchbay_tab(&mut self, ui: &mut Ui) {
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

        let current_path = self.patchbay_file.display().to_string();
        let choose_directory = self.t("patchbay.choose_directory");
        let recent_label = self.t("patchbay.recent_files");
        let profile_label = self.t("patchbay.profile");
        let save_profile_label = self.t("patchbay.save_profile");
        panel_section(ui, self.t("patchbay.file_options"), |ui| {
            ui.horizontal(|ui| {
                ui.label(profile_label.clone());
                ui.text_edit_singleline(&mut self.profile_name);
                if ui.button(save_profile_label.clone()).clicked() {
                    let name = self.profile_name.trim().to_owned();
                    if !name.is_empty() {
                        self.config.active_patchbay_profile = name.clone();
                        self.config
                            .patchbay_profiles
                            .insert(name, self.patchbay_file.clone());
                    }
                }
            });
            let mut selected_profile = self.config.active_patchbay_profile.clone();
            let profiles: Vec<_> = self.config.patchbay_profiles.keys().cloned().collect();
            if !profiles.is_empty() {
                egui::ComboBox::from_id_salt("patchbay-profile-list")
                    .selected_text(selected_profile.clone())
                    .show_ui(ui, |ui| {
                        for profile in &profiles {
                            ui.selectable_value(&mut selected_profile, profile.clone(), profile);
                        }
                    });
                if selected_profile != self.config.active_patchbay_profile {
                    self.config.active_patchbay_profile = selected_profile.clone();
                    self.profile_name = selected_profile.clone();
                    if let Some(path) = self
                        .config
                        .patchbay_profiles
                        .get(&selected_profile)
                        .cloned()
                    {
                        self.select_patchbay_path(path);
                        let _ = self.load_patchbay_from_current();
                    }
                }
            }
            ui.label(self.tf("patchbay.current_path", &[("path", current_path.clone())]));
            if ui.button(choose_directory.clone()).clicked() {
                self.choose_patchbay_directory();
            }
            if !self.config.recent_patchbay_paths.is_empty() {
                let mut selected = self.patchbay_file.clone();
                egui::ComboBox::from_label(recent_label.clone())
                    .selected_text(selected.display().to_string())
                    .show_ui(ui, |ui| {
                        for path in &self.config.recent_patchbay_paths {
                            ui.selectable_value(
                                &mut selected,
                                path.clone(),
                                path.display().to_string(),
                            );
                        }
                    });
                if selected != self.patchbay_file {
                    self.use_recent_patchbay(selected);
                }
            }
        });

        let remove_rule = self.t("patchbay.remove_rule");
        let output_label = self.t("patchbay.output");
        let input_label = self.t("patchbay.input");
        let pinned_label = self.t("inspector.pinned");
        panel_section(ui, self.t("patchbay.connections"), |ui| {
            if self.patchbay.connections.is_empty() {
                ui.label(RichText::new(self.t("patchbay.no_connections")).weak());
            }
            let mut remove_index = None;
            for (index, connection) in self.patchbay.connections.iter_mut().enumerate() {
                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(output_label.clone());
                        ui.text_edit_singleline(&mut connection.output_node);
                        ui.label("/");
                        ui.text_edit_singleline(&mut connection.output_name);
                    });
                    ui.horizontal(|ui| {
                        ui.label(input_label.clone());
                        ui.text_edit_singleline(&mut connection.input_node);
                        ui.label("/");
                        ui.text_edit_singleline(&mut connection.input_name);
                    });
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut connection.pinned, pinned_label.clone());
                        if ui.button(remove_rule.clone()).clicked() {
                            remove_index = Some(index);
                        }
                    });
                });
            }
            if let Some(index) = remove_index {
                self.patchbay.connections.remove(index);
            }
        });
    }

    fn show_preferences_interface_tab(&mut self, ui: &mut Ui) {
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

        panel_section(ui, self.t("inspector.audio_metering"), |ui| {
            self.show_meter_controls(ui);
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
}
