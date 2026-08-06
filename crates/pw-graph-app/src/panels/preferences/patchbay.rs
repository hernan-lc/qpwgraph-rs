//! Patchbay tab: activation policy, the file the rules live in, and the
//! rules themselves (in [`super::rules`]).
//!
//! Unlike the Interface tab these three parts are one workflow — the policy
//! decides what activation does, the file decides which rules are loaded, and
//! the list is what gets written back — so they stay in one tab, ordered the
//! way the work runs.

use super::super::components::{
    document_button, document_compact_select, document_text_input_sized,
};
use super::super::shared::panel_section;
use super::{PREFERENCES_PATH_SELECT_WIDTH, PREFERENCES_SELECT_WIDTH};
use crate::app::QpwgraphApp;
use crate::icons::Icon;
use eframe::egui::{RichText, Ui};
use pw_graph_ui::{OptionItem, Theme, UiDocument};

/// Room for the "Save profile" button beside the name field.
const SAVE_PROFILE_BUTTON_WIDTH: f32 = 130.0;

impl QpwgraphApp {
    pub(super) fn show_preferences_patchbay_tab(&mut self, document: &mut UiDocument, ui: &mut Ui) {
        let theme = document.theme().clone();
        self.show_patchbay_rules_section(document, ui, &theme);
    }

    /// What activation is allowed to do to the live graph.
    fn show_patchbay_options_section(
        &mut self,
        document: &mut UiDocument,
        ui: &mut Ui,
        theme: &Theme,
    ) {
        panel_section(
            ui,
            self.i18n.text("inspector.patchbay_options"),
            theme,
            |ui| {
                self.config.patchbay_exclusive = self.preferences_switch(
                    document,
                    ui,
                    "patchbay.exclusive",
                    self.config.patchbay_exclusive,
                    Icon::Exclusive,
                    "exclusive",
                );
                self.config.patchbay_auto_disconnect = self.preferences_switch(
                    document,
                    ui,
                    "patchbay.auto_disconnect",
                    self.config.patchbay_auto_disconnect,
                    Icon::AutoDisconnect,
                    "auto_disconnect",
                );
                self.config.patchbay_auto_pin = self.preferences_switch(
                    document,
                    ui,
                    "patchbay.auto_pin",
                    self.config.patchbay_auto_pin,
                    Icon::Pin,
                    "auto_pin",
                );
                // Turning this on is also a request to activate right now:
                // the setting says "activate on startup", and waiting for the
                // next launch to honour it would read as the switch failing.
                let activated_before = self.config.patchbay_activated;
                self.config.patchbay_activated = self.preferences_switch(
                    document,
                    ui,
                    "patchbay.activated",
                    self.config.patchbay_activated,
                    Icon::Timer,
                    "patchbay_activated",
                );
                if self.config.patchbay_activated && !activated_before {
                    self.activate_patchbay();
                }
            },
        );
    }

    /// Which file the rules are read from and written to, and the named
    /// profiles that point at those files.
    fn show_patchbay_file_section(
        &mut self,
        document: &mut UiDocument,
        ui: &mut Ui,
        theme: &Theme,
    ) {
        panel_section(ui, self.i18n.text("patchbay.file_options"), theme, |ui| {
            self.show_patchbay_profile_picker(document, ui);
            self.show_patchbay_profile_save(document, ui);
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(4.0);
            self.show_patchbay_path_controls(document, ui);
        });
    }

    /// Switching to a saved profile. Previously the select drew its label on
    /// the trailing side, so the page read "[default ▾] Profile" — the one
    /// control here whose label came after it.
    fn show_patchbay_profile_picker(&mut self, document: &mut UiDocument, ui: &mut Ui) {
        let profiles: Vec<_> = self.config.patchbay_profiles.keys().cloned().collect();
        if profiles.is_empty() {
            return;
        }
        let selected_profile = document_compact_select(
            document,
            ui,
            "preferences.patchbay.profile",
            &self.config.active_patchbay_profile,
            self.i18n.text("patchbay.profile"),
            self.i18n.text("patchbay.profile_help"),
            profiles
                .iter()
                .map(|profile| OptionItem::new(profile.clone(), profile.clone())),
            PREFERENCES_SELECT_WIDTH,
        );
        if selected_profile == self.config.active_patchbay_profile {
            return;
        }
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

    /// Naming the current file as a profile. The field is measured against
    /// the button beside it rather than left at egui's default width, which
    /// in this dialog ran the text edit under the button.
    fn show_patchbay_profile_save(&mut self, document: &mut UiDocument, ui: &mut Ui) {
        ui.label(RichText::new(self.i18n.text("patchbay.profile_name")).strong());
        ui.horizontal(|ui| {
            let spacing = ui.spacing().item_spacing.x;
            let field_width =
                (ui.available_width() - SAVE_PROFILE_BUTTON_WIDTH - spacing).max(80.0);
            let (_, profile_name) = document_text_input_sized(
                document,
                ui,
                "preferences.patchbay.profile_name",
                &self.profile_name,
                String::new(),
                Some(self.i18n.text("patchbay.profile_name_help")),
                Some(field_width),
            );
            self.profile_name = profile_name;
            let name = self.profile_name.trim().to_owned();
            if document_button(
                document,
                ui,
                "preferences.patchbay.save_profile",
                self.i18n.text("patchbay.save_profile"),
                !name.is_empty(),
            ) && !name.is_empty()
            {
                self.config.active_patchbay_profile = name.clone();
                self.config
                    .patchbay_profiles
                    .insert(name, self.patchbay_file.clone());
            }
        });
    }

    /// The file itself: where it is now, how to change the directory, and the
    /// recently used files.
    fn show_patchbay_path_controls(&mut self, document: &mut UiDocument, ui: &mut Ui) {
        ui.label(
            RichText::new(self.tf(
                "patchbay.current_path",
                &[("path", self.patchbay_file.display().to_string())],
            ))
            .small()
            .weak(),
        );
        ui.add_space(4.0);
        if document_button(
            document,
            ui,
            "preferences.patchbay.choose_directory",
            self.i18n.text("patchbay.choose_directory"),
            true,
        ) {
            self.choose_patchbay_directory();
        }
        if self.config.recent_patchbay_paths.is_empty() {
            return;
        }
        let current_path = self.patchbay_file.display().to_string();
        let selected_path = document_compact_select(
            document,
            ui,
            "preferences.patchbay.recent_path",
            &current_path,
            self.i18n.text("patchbay.recent_files"),
            self.i18n.text("patchbay.recent_files_help"),
            self.config.recent_patchbay_paths.iter().map(|path| {
                let display = path.display().to_string();
                OptionItem::new(display.clone(), display)
            }),
            PREFERENCES_PATH_SELECT_WIDTH,
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
}
