use super::components::{
    document_button, document_checkbox, document_select_sized, document_selectable_label,
    document_setting_number, document_setting_select, document_setting_switch, document_text_input,
};
use super::shared::{
    apply_panel_text_scale, fresh_scroll_area, meter_policy_key, panel_section,
    show_centered_dialog, show_close_button,
};
use crate::app::QpwgraphApp;
use crate::icons::{icon_label, Icon};
use eframe::egui::{self, RichText, Ui};
use pw_graph_backend::MeterPolicy;
use pw_graph_i18n::Locale;
use pw_graph_ui::{OptionItem, UiDocument};

const PREFERENCES_DIALOG_WIDTH: f32 = 780.0;
const PREFERENCES_SCROLL_MAX_HEIGHT: f32 = 600.0;
const PREFERENCES_SELECT_WIDTH: f32 = 260.0;
const PREFERENCES_PATH_SELECT_WIDTH: f32 = 520.0;

/// Tabs inside the Preferences modal, which holds settings you configure once
/// rather than watch while working the canvas.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub(crate) enum PreferencesTab {
    #[default]
    Interface,
    Patchbay,
}

impl QpwgraphApp {
    fn show_meter_controls(&mut self, document: &mut UiDocument, ui: &mut Ui) {
        let current = MeterPolicy::parse(&self.config.audio_meters);
        const METER_POLICY_ID: &str = "preferences.meters.policy";
        if document.text(METER_POLICY_ID) != Some(current.as_str()) {
            document.set_value(METER_POLICY_ID, current.as_str());
        }
        let options = MeterPolicy::ALL.into_iter().map(|policy| {
            OptionItem::new(policy.as_str(), self.i18n.text(meter_policy_key(policy)))
        });
        let selected = document_setting_select(
            document,
            ui,
            METER_POLICY_ID,
            current.as_str(),
            None,
            self.i18n.text("inspector.audio_metering_policy"),
            self.i18n.text("help.audio_metering_policy"),
            options,
            PREFERENCES_SELECT_WIDTH,
        );
        let selected = MeterPolicy::parse(&selected);
        if selected != current {
            self.config.audio_meters = selected.as_str().to_owned();
        }

        egui::Frame::group(ui.style())
            .inner_margin(8.0)
            .show(ui, |ui| {
                ui.label(
                    RichText::new(self.i18n.text(match selected {
                        MeterPolicy::Disabled => "meters.off_hint",
                        MeterPolicy::OnDemand => "meters.on_demand_hint",
                        MeterPolicy::Always => "meters.always_hint",
                    }))
                    .small()
                    .weak(),
                );
            });

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if document_button(
                document,
                ui,
                "meters.reset",
                self.i18n.text("inspector.audio_reset"),
                true,
            ) {
                self.reset_audio_config();
            }
        });

        ui.add_space(2.0);
    }

    pub(crate) fn show_preferences_modal(&mut self, ctx: &egui::Context) {
        if !self.show_preferences {
            return;
        }
        let viewport = ctx.screen_rect();
        let dialog_width = (viewport.width() - 48.0)
            .clamp(320.0, PREFERENCES_DIALOG_WIDTH)
            .max(1.0);
        let scroll_max_height = (viewport.height() - 240.0)
            .clamp(220.0, PREFERENCES_SCROLL_MAX_HEIGHT)
            .max(1.0);
        let mut document = std::mem::take(&mut self.ui_document);
        let dialog_response = show_centered_dialog(
            &mut document,
            ctx,
            "preferences",
            self.i18n.text("preferences.title"),
            dialog_width,
            |ui, document| {
                apply_panel_text_scale(ui, self.config.panel_text_scale);
                ui.horizontal(|ui| {
                    let tabs = [
                        (PreferencesTab::Interface, "screen.interface"),
                        (PreferencesTab::Patchbay, "inspector.patchbay_options"),
                    ];
                    for (tab, label_key) in tabs {
                        let tab_id = match tab {
                            PreferencesTab::Interface => "preferences.tab.interface",
                            PreferencesTab::Patchbay => "preferences.tab.patchbay",
                        };
                        if document_selectable_label(
                            document,
                            ui,
                            tab_id,
                            self.preferences_tab == tab,
                            &self.i18n.text(label_key),
                            self.i18n.text(label_key),
                        ) && self.preferences_tab != tab
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
                fresh_scroll_area(scroll_id, scroll_max_height)
                    .auto_shrink([false, false])
                    .show(ui, |ui| match self.preferences_tab {
                        PreferencesTab::Interface => {
                            self.show_preferences_interface_tab(document, ui)
                        }
                        PreferencesTab::Patchbay => {
                            self.show_preferences_patchbay_tab(document, ui)
                        }
                    });
                ui.add_space(8.0);
                if show_close_button(
                    document,
                    ui,
                    "preferences.close",
                    self.i18n.text("shortcuts.close"),
                ) {
                    self.show_preferences = false;
                }
            },
        );
        self.ui_document = document;
        if dialog_response.backdrop_clicked {
            self.show_preferences = false;
        }
    }

    fn show_preferences_patchbay_tab(&mut self, document: &mut UiDocument, ui: &mut Ui) {
        panel_section(ui, self.i18n.text("inspector.patchbay_options"), |ui| {
            let exclusive_label = self.i18n.text("inspector.exclusive");
            let exclusive_help = self.i18n.text("help.exclusive");
            self.config.patchbay_exclusive = document_setting_switch(
                document,
                ui,
                "patchbay.exclusive",
                self.config.patchbay_exclusive,
                Icon::Exclusive,
                exclusive_label,
                exclusive_help,
            );
            let auto_disconnect_label = self.i18n.text("inspector.auto_disconnect");
            let auto_disconnect_help = self.i18n.text("help.auto_disconnect");
            self.config.patchbay_auto_disconnect = document_setting_switch(
                document,
                ui,
                "patchbay.auto_disconnect",
                self.config.patchbay_auto_disconnect,
                Icon::AutoDisconnect,
                auto_disconnect_label,
                auto_disconnect_help,
            );
            let auto_pin_label = self.i18n.text("inspector.auto_pin");
            let auto_pin_help = self.i18n.text("help.auto_pin");
            self.config.patchbay_auto_pin = document_setting_switch(
                document,
                ui,
                "patchbay.auto_pin",
                self.config.patchbay_auto_pin,
                Icon::Pin,
                auto_pin_label,
                auto_pin_help,
            );
            let patchbay_activated_before = self.config.patchbay_activated;
            let patchbay_activated_label = self.i18n.text("inspector.patchbay_activated");
            let patchbay_activated_help = self.i18n.text("help.patchbay_activated");
            self.config.patchbay_activated = document_setting_switch(
                document,
                ui,
                "patchbay.activated",
                self.config.patchbay_activated,
                Icon::Timer,
                patchbay_activated_label,
                patchbay_activated_help,
            );
            if self.config.patchbay_activated && !patchbay_activated_before {
                self.activate_patchbay();
            }
        });

        let current_path = self.patchbay_file.display().to_string();
        let choose_directory = self.i18n.text("patchbay.choose_directory");
        let recent_label = self.i18n.text("patchbay.recent_files");
        let profile_label = self.i18n.text("patchbay.profile");
        let save_profile_label = self.i18n.text("patchbay.save_profile");
        panel_section(ui, self.i18n.text("patchbay.file_options"), |ui| {
            ui.horizontal(|ui| {
                ui.label(profile_label.clone());
                let (_, profile_name) = document_text_input(
                    document,
                    ui,
                    "preferences.patchbay.profile_name",
                    &self.profile_name,
                    String::new(),
                    None,
                );
                self.profile_name = profile_name;
                if document_button(
                    document,
                    ui,
                    "preferences.patchbay.save_profile",
                    save_profile_label.clone(),
                    true,
                ) {
                    let name = self.profile_name.trim().to_owned();
                    if !name.is_empty() {
                        self.config.active_patchbay_profile = name.clone();
                        self.config
                            .patchbay_profiles
                            .insert(name, self.patchbay_file.clone());
                    }
                }
            });
            let profiles: Vec<_> = self.config.patchbay_profiles.keys().cloned().collect();
            if !profiles.is_empty() {
                let selected_profile = document_select_sized(
                    document,
                    ui,
                    "preferences.patchbay.profile",
                    &self.config.active_patchbay_profile,
                    profile_label.clone(),
                    profiles
                        .iter()
                        .map(|profile| OptionItem::new(profile.clone(), profile.clone())),
                    Some(PREFERENCES_SELECT_WIDTH),
                );
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
            if document_button(
                document,
                ui,
                "preferences.patchbay.choose_directory",
                choose_directory.clone(),
                true,
            ) {
                self.choose_patchbay_directory();
            }
            if !self.config.recent_patchbay_paths.is_empty() {
                let current_path = self.patchbay_file.display().to_string();
                let selected_path = document_select_sized(
                    document,
                    ui,
                    "preferences.patchbay.recent_path",
                    &current_path,
                    recent_label.clone(),
                    self.config.recent_patchbay_paths.iter().map(|path| {
                        let display = path.display().to_string();
                        OptionItem::new(display.clone(), display)
                    }),
                    Some(PREFERENCES_PATH_SELECT_WIDTH),
                );
                if let Some(selected) = self
                    .config
                    .recent_patchbay_paths
                    .iter()
                    .find(|path| path.display().to_string() == selected_path)
                    .cloned()
                {
                    if selected != self.patchbay_file {
                        self.use_recent_patchbay(selected);
                    }
                }
            }
        });

        let remove_rule = self.i18n.text("patchbay.remove_rule");
        let output_label = self.i18n.text("patchbay.output");
        let input_label = self.i18n.text("patchbay.input");
        let pinned_label = self.i18n.text("inspector.pinned");
        panel_section(ui, self.i18n.text("patchbay.connections"), |ui| {
            if self.patchbay.connections.is_empty() {
                ui.label(RichText::new(self.i18n.text("patchbay.no_connections")).weak());
            }
            let mut remove_index = None;
            for index in 0..self.patchbay.connections.len() {
                let (
                    output_node_current,
                    output_name_current,
                    input_node_current,
                    input_name_current,
                    pinned_current,
                ) = {
                    let connection = &self.patchbay.connections[index];
                    (
                        connection.output_node.clone(),
                        connection.output_name.clone(),
                        connection.input_node.clone(),
                        connection.input_name.clone(),
                        connection.pinned,
                    )
                };
                let mut output_node = output_node_current.clone();
                let mut output_name = output_name_current.clone();
                let mut input_node = input_node_current.clone();
                let mut input_name = input_name_current.clone();
                let mut pinned = pinned_current;
                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(output_label.clone());
                        let (_, value) = document_text_input(
                            document,
                            ui,
                            &format!("preferences.patchbay.connections.{index}.output_node"),
                            &output_node,
                            String::new(),
                            None,
                        );
                        output_node = value;
                        ui.label("/");
                        let (_, value) = document_text_input(
                            document,
                            ui,
                            &format!("preferences.patchbay.connections.{index}.output_name"),
                            &output_name,
                            String::new(),
                            None,
                        );
                        output_name = value;
                    });
                    ui.horizontal(|ui| {
                        ui.label(input_label.clone());
                        let (_, value) = document_text_input(
                            document,
                            ui,
                            &format!("preferences.patchbay.connections.{index}.input_node"),
                            &input_node,
                            String::new(),
                            None,
                        );
                        input_node = value;
                        ui.label("/");
                        let (_, value) = document_text_input(
                            document,
                            ui,
                            &format!("preferences.patchbay.connections.{index}.input_name"),
                            &input_name,
                            String::new(),
                            None,
                        );
                        input_name = value;
                    });
                    ui.horizontal(|ui| {
                        pinned = document_checkbox(
                            document,
                            ui,
                            &format!("preferences.patchbay.connections.{index}.pinned"),
                            pinned,
                            pinned_label.clone(),
                            None,
                        );
                        if document_button(
                            document,
                            ui,
                            &format!("preferences.patchbay.connections.{index}.remove"),
                            remove_rule.clone(),
                            true,
                        ) {
                            remove_index = Some(index);
                        }
                    });
                });
                if let Some(connection) = self.patchbay.connections.get_mut(index) {
                    connection.output_node = output_node;
                    connection.output_name = output_name;
                    connection.input_node = input_node;
                    connection.input_name = input_name;
                    connection.pinned = pinned;
                }
            }
            if let Some(index) = remove_index {
                self.patchbay.connections.remove(index);
            }
        });
    }

    fn show_preferences_interface_tab(&mut self, document: &mut UiDocument, ui: &mut Ui) {
        let current_locale = self.i18n.locale();
        let selected_locale_code = document
            .text("preferences.configuration.language")
            .unwrap_or(current_locale.code())
            .to_owned();
        let selected_locale_code = if selected_locale_code != current_locale.code() {
            current_locale.code().to_owned()
        } else {
            selected_locale_code
        };
        let mut selected_locale = current_locale;
        panel_section(ui, self.i18n.text("inspector.configuration"), |ui| {
            ui.label(
                RichText::new(self.i18n.text("help.configuration"))
                    .small()
                    .weak(),
            );
            ui.add_space(4.0);
            let selected = document_setting_select(
                document,
                ui,
                "preferences.configuration.language",
                &selected_locale_code,
                Some(Icon::Language),
                self.i18n.text("language.label"),
                self.i18n.text("help.language"),
                Locale::ALL
                    .into_iter()
                    .map(|locale| OptionItem::new(locale.code(), locale.native_name())),
                PREFERENCES_SELECT_WIDTH,
            );
            selected_locale = Locale::parse(&selected);

            ui.horizontal(|ui| {
                ui.set_min_width(ui.available_width());
                icon_label(ui, Icon::Save, self.i18n.text("help.save_configuration"));
                ui.add_space(8.0);
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new(self.i18n.text("inspector.save_configuration")).strong(),
                    );
                    ui.label(
                        RichText::new(self.tf(
                            "inspector.config_path",
                            &[("path", self.config_file.display().to_string())],
                        ))
                        .small()
                        .weak(),
                    );
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if document_button(
                        document,
                        ui,
                        "configuration.save",
                        self.i18n.text("shortcuts.save_config"),
                        true,
                    ) {
                        self.save_config_now();
                    }
                });
            });
        });
        if selected_locale != current_locale {
            self.i18n.set_locale(selected_locale);
            self.config.language = selected_locale.code().to_owned();
            self.status = self.i18n.text("status.language_changed");
        }

        panel_section(ui, self.i18n.text("inspector.interface"), |ui| {
            let toolbar_label = self.i18n.text("inspector.toolbar_visible");
            let toolbar_help = self.i18n.text("help.toolbar_visible");
            self.config.toolbar = document_setting_switch(
                document,
                ui,
                "interface.toolbar",
                self.config.toolbar,
                Icon::Toolbar,
                toolbar_label,
                toolbar_help,
            );
            let statusbar_label = self.i18n.text("inspector.statusbar_visible");
            let statusbar_help = self.i18n.text("help.statusbar_visible");
            self.config.statusbar = document_setting_switch(
                document,
                ui,
                "interface.statusbar",
                self.config.statusbar,
                Icon::Statusbar,
                statusbar_label,
                statusbar_help,
            );
            let patchbay_toolbar_label = self.i18n.text("inspector.patchbay_toolbar_visible");
            let patchbay_toolbar_help = self.i18n.text("help.patchbay_toolbar_visible");
            self.config.patchbay_toolbar = document_setting_switch(
                document,
                ui,
                "interface.patchbay_toolbar",
                self.config.patchbay_toolbar,
                Icon::Patchbay,
                patchbay_toolbar_label,
                patchbay_toolbar_help,
            );
        });

        panel_section(ui, self.i18n.text("inspector.behavior"), |ui| {
            let repel_label = self.i18n.text("inspector.repel_overlaps");
            let repel_help = self.i18n.text("help.repel_overlaps");
            self.config.repel_overlapping_nodes = document_setting_switch(
                document,
                ui,
                "behavior.repel",
                self.config.repel_overlapping_nodes,
                Icon::Repel,
                repel_label,
                repel_help,
            );
            let through_label = self.i18n.text("inspector.connect_through");
            let through_help = self.i18n.text("help.connect_through");
            self.config.connect_through_nodes = document_setting_switch(
                document,
                ui,
                "behavior.connect_through",
                self.config.connect_through_nodes,
                Icon::Connect,
                through_label,
                through_help,
            );
            let thumbnail_label = self.i18n.text("inspector.thumbnail_view");
            let thumbnail_help = self.i18n.text("help.thumbnail_view");
            self.canvas.thumbnail_mode = document_setting_switch(
                document,
                ui,
                "behavior.thumbnail",
                self.canvas.thumbnail_mode,
                Icon::Thumbnail,
                thumbnail_label,
                thumbnail_help,
            );
        });

        panel_section(ui, self.i18n.text("inspector.audio_metering"), |ui| {
            self.show_meter_controls(document, ui);
        });

        panel_section(ui, self.i18n.text("inspector.typography"), |ui| {
            ui.label(
                RichText::new(self.i18n.text("help.typography_controls"))
                    .small()
                    .weak(),
            );
            ui.add_space(4.0);
            if document_button(
                document,
                ui,
                "typography.recommended",
                self.i18n.text("inspector.typography_recommended"),
                true,
            ) {
                self.config.ui_text_scale = 1.10;
                self.config.panel_text_scale = 1.20;
                self.config.node_text_scale = 1.15;
            }
            self.config.ui_text_scale = document_setting_number(
                document,
                ui,
                "typography.ui_scale",
                self.config.ui_text_scale,
                0.80,
                2.0,
                0.05,
                self.i18n.text("inspector.ui_text_scale"),
                self.i18n.text("help.ui_text_scale"),
                82.0,
            );
            self.config.panel_text_scale = document_setting_number(
                document,
                ui,
                "typography.panel_scale",
                self.config.panel_text_scale,
                0.80,
                2.0,
                0.05,
                self.i18n.text("inspector.panel_text_scale"),
                self.i18n.text("help.panel_text_scale"),
                82.0,
            );
            self.config.node_text_scale = document_setting_number(
                document,
                ui,
                "typography.node_scale",
                self.config.node_text_scale,
                0.80,
                2.0,
                0.05,
                self.i18n.text("inspector.node_text_scale"),
                self.i18n.text("help.node_text_scale"),
                82.0,
            );
        });
    }
}
