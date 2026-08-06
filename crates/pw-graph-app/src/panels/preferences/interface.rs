//! Interface tab: language, chrome visibility, canvas behavior, metering,
//! and typography.
//!
//! Every entry here is an independent preference, so the tab is a plain
//! top-to-bottom page of titled sections with one setting row each. Nothing
//! on it refers to anything else on it — which is exactly why it is worth
//! keeping apart from the patchbay editor, where everything does.

use super::super::components::{document_button, document_setting_number, document_setting_select};
use super::super::shared::{meter_policy_key, panel_section};
use super::PREFERENCES_SELECT_WIDTH;
use crate::app::QpwgraphApp;
use crate::icons::{icon_label, Icon};
use eframe::egui::{self, RichText, Ui};
use pw_graph_backend::MeterPolicy;
use pw_graph_i18n::Locale;
use pw_graph_ui::{OptionItem, UiDocument};

impl QpwgraphApp {
    pub(super) fn show_preferences_interface_tab(
        &mut self,
        document: &mut UiDocument,
        ui: &mut Ui,
    ) {
        let theme = document.theme().clone();
        self.show_configuration_section(document, ui, &theme);
        self.show_chrome_section(document, ui, &theme);
        self.show_behavior_section(document, ui, &theme);
        panel_section(
            ui,
            self.i18n.text("inspector.audio_metering"),
            &theme,
            |ui| {
                self.show_meter_controls(document, ui);
            },
        );
        self.show_typography_section(document, ui, &theme);
    }

    /// Language, plus the "save now" escape hatch for the config file.
    fn show_configuration_section(
        &mut self,
        document: &mut UiDocument,
        ui: &mut Ui,
        theme: &pw_graph_ui::Theme,
    ) {
        let current_locale = self.i18n.locale();
        // The retained select can hold a stale code if the locale changed from
        // somewhere else; the app's own locale always wins.
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
        panel_section(ui, self.i18n.text("inspector.configuration"), theme, |ui| {
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
    }

    /// Which pieces of application chrome are visible.
    fn show_chrome_section(
        &mut self,
        document: &mut UiDocument,
        ui: &mut Ui,
        theme: &pw_graph_ui::Theme,
    ) {
        panel_section(ui, self.i18n.text("inspector.interface"), theme, |ui| {
            self.config.toolbar = self.preferences_switch(
                document,
                ui,
                "interface.toolbar",
                self.config.toolbar,
                Icon::Toolbar,
                "toolbar_visible",
            );
            self.config.statusbar = self.preferences_switch(
                document,
                ui,
                "interface.statusbar",
                self.config.statusbar,
                Icon::Statusbar,
                "statusbar_visible",
            );
            self.config.patchbay_toolbar = self.preferences_switch(
                document,
                ui,
                "interface.patchbay_toolbar",
                self.config.patchbay_toolbar,
                Icon::Patchbay,
                "patchbay_toolbar_visible",
            );
        });
    }

    /// How the canvas behaves while you drag and connect.
    fn show_behavior_section(
        &mut self,
        document: &mut UiDocument,
        ui: &mut Ui,
        theme: &pw_graph_ui::Theme,
    ) {
        panel_section(ui, self.i18n.text("inspector.behavior"), theme, |ui| {
            self.config.repel_overlapping_nodes = self.preferences_switch(
                document,
                ui,
                "behavior.repel",
                self.config.repel_overlapping_nodes,
                Icon::Repel,
                "repel_overlaps",
            );
            self.config.connect_through_nodes = self.preferences_switch(
                document,
                ui,
                "behavior.connect_through",
                self.config.connect_through_nodes,
                Icon::Connect,
                "connect_through",
            );
            self.canvas.thumbnail_mode = self.preferences_switch(
                document,
                ui,
                "behavior.thumbnail",
                self.canvas.thumbnail_mode,
                Icon::Thumbnail,
                "thumbnail_view",
            );
        });
    }

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

    fn show_typography_section(
        &mut self,
        document: &mut UiDocument,
        ui: &mut Ui,
        theme: &pw_graph_ui::Theme,
    ) {
        panel_section(ui, self.i18n.text("inspector.typography"), theme, |ui| {
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
            self.config.ui_text_scale = self.typography_setting(
                document,
                ui,
                "typography.ui_scale",
                "ui_text_scale",
                self.config.ui_text_scale,
            );
            self.config.panel_text_scale = self.typography_setting(
                document,
                ui,
                "typography.panel_scale",
                "panel_text_scale",
                self.config.panel_text_scale,
            );
            self.config.node_text_scale = self.typography_setting(
                document,
                ui,
                "typography.node_scale",
                "node_text_scale",
                self.config.node_text_scale,
            );
        });
    }

    /// A clamped text-scaling number input. UI, panel and node scale all share
    /// the same bounds and step, so the three rows are built from one helper.
    fn typography_setting(
        &self,
        document: &mut UiDocument,
        ui: &mut Ui,
        id: &str,
        scale_name: &str,
        value: f32,
    ) -> f32 {
        document_setting_number(
            document,
            ui,
            id,
            value,
            0.80,
            2.0,
            0.05,
            self.i18n.text(&format!("inspector.{scale_name}")),
            self.i18n.text(&format!("help.{scale_name}")),
            82.0,
        )
    }
}
