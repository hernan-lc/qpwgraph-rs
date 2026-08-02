use super::components::{
    modal_checkbox, modal_combo, modal_hint, modal_slider, modal_step_heading,
};
use super::shared::{fresh_scroll_area, modal_window, show_backdrop, show_close_button};
use crate::app::effects::{audio_link_options, default_parameters, EffectWizardState};
use crate::app::QpwgraphApp;
use eframe::egui::{self, RichText, Ui};
use pw_graph_effects::EffectDescriptor;

fn shortcut_row(ui: &mut Ui, keys: &str, description: String) {
    ui.label(RichText::new(keys).strong().monospace());
    ui.label(description);
    ui.end_row();
}

fn shortcut_matches_query(keys: &str, description: &str, query: &str) -> bool {
    query.is_empty()
        || keys.to_lowercase().contains(query)
        || description.to_lowercase().contains(query)
}

struct ShortcutEntry {
    keys: &'static str,
    description_key: &'static str,
}

const SHORTCUT_ENTRIES: &[ShortcutEntry] = &[
    ShortcutEntry {
        keys: "F1",
        description_key: "shortcuts.help",
    },
    ShortcutEntry {
        keys: "Esc",
        description_key: "shortcuts.close_cancel",
    },
    ShortcutEntry {
        keys: "Delete / Backspace",
        description_key: "shortcuts.delete_link",
    },
    ShortcutEntry {
        keys: "Ctrl/Cmd+Z",
        description_key: "shortcuts.undo",
    },
    ShortcutEntry {
        keys: "Ctrl/Cmd+Shift+Z",
        description_key: "shortcuts.redo",
    },
    ShortcutEntry {
        keys: "Ctrl/Cmd+Y",
        description_key: "shortcuts.redo",
    },
    ShortcutEntry {
        keys: "Ctrl/Cmd+S",
        description_key: "shortcuts.save_config",
    },
    ShortcutEntry {
        keys: "Ctrl/Cmd+Shift+S",
        description_key: "shortcuts.save_patchbay",
    },
    ShortcutEntry {
        keys: "Ctrl/Cmd+O",
        description_key: "shortcuts.load_patchbay",
    },
    ShortcutEntry {
        keys: "R",
        description_key: "shortcuts.refresh",
    },
    ShortcutEntry {
        keys: "A",
        description_key: "shortcuts.arrange",
    },
    ShortcutEntry {
        keys: "T",
        description_key: "shortcuts.thumbnail",
    },
    ShortcutEntry {
        keys: "Arrow keys",
        description_key: "shortcuts.pan_keyboard",
    },
    ShortcutEntry {
        keys: "0",
        description_key: "shortcuts.filter_all",
    },
    ShortcutEntry {
        keys: "1",
        description_key: "shortcuts.filter_audio",
    },
    ShortcutEntry {
        keys: "2",
        description_key: "shortcuts.filter_video",
    },
    ShortcutEntry {
        keys: "3",
        description_key: "shortcuts.filter_midi",
    },
    ShortcutEntry {
        keys: "+ / -",
        description_key: "shortcuts.zoom",
    },
    ShortcutEntry {
        keys: "Scroll",
        description_key: "shortcuts.scroll_pan",
    },
    ShortcutEntry {
        keys: "Shift+Scroll",
        description_key: "shortcuts.scroll_pan_horizontal",
    },
    ShortcutEntry {
        keys: "Ctrl/Cmd+Scroll",
        description_key: "shortcuts.scroll_zoom",
    },
];

impl QpwgraphApp {
    pub(crate) fn show_effect_wizard_modal(&mut self, ctx: &egui::Context) {
        if self.effect_wizard.is_none() {
            return;
        }
        if show_backdrop(ctx, "effects-wizard") {
            self.effect_wizard = None;
            return;
        }

        let descriptors = self.driver.effect_descriptors();
        let links = audio_link_options(self.driver.as_ref());
        let Some(mut wizard) = self.effect_wizard.take() else {
            return;
        };
        let mut cancel = false;
        let mut finish = false;
        let title = self.t("effects.wizard_title");

        modal_window("effects-wizard", title, 620.0).show(ctx, |ui| {
            ui.horizontal(|ui| {
                for (index, key) in [
                    "effects.step_effect",
                    "effects.step_link",
                    "effects.step_setup",
                ]
                .into_iter()
                .enumerate()
                {
                    modal_step_heading(ui, index, wizard.step, self.t(key));
                    if index < 2 {
                        ui.label(RichText::new("›").weak());
                    }
                }
            });
            ui.separator();

            match wizard.step {
                0 => show_effect_step(ui, self, &descriptors, &mut wizard),
                1 => show_link_step(ui, self, &links, &mut wizard),
                _ => show_setup_step(ui, self, &descriptors, &links, &mut wizard),
            }

            ui.add_space(12.0);
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button(self.t("effects.cancel")).clicked() {
                    cancel = true;
                }
                if wizard.step > 0
                    && ui
                        .button(self.t("effects.back"))
                        .on_hover_text(self.t("effects.back_help"))
                        .clicked()
                {
                    wizard.step -= 1;
                }
                let can_advance = match wizard.step {
                    0 => !wizard.effect_id.is_empty() && !descriptors.is_empty(),
                    1 => wizard.link_id.is_some() && !links.is_empty(),
                    _ => !wizard.effect_id.is_empty() && wizard.link_id.is_some(),
                };
                if wizard.step < 2 {
                    if ui
                        .add_enabled(can_advance, egui::Button::new(self.t("effects.next")))
                        .clicked()
                    {
                        wizard.step += 1;
                    }
                } else if ui
                    .add_enabled(can_advance, egui::Button::new(self.t("effects.create")))
                    .clicked()
                {
                    finish = true;
                }
            });
        });

        if finish {
            self.finish_effect_wizard(wizard);
        } else if !cancel {
            self.effect_wizard = Some(wizard);
        }
    }

    pub(crate) fn show_shortcuts_modal(&mut self, ctx: &egui::Context) {
        if !self.show_shortcuts {
            return;
        }
        if show_backdrop(ctx, "shortcuts") {
            self.close_shortcuts();
            return;
        }
        modal_window("shortcuts", self.t("shortcuts.title"), 560.0).show(ctx, |ui| {
            ui.label(RichText::new(self.t("shortcuts.hint")).weak());
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(self.t("shortcuts.search")).strong());
                let clear_width = if self.shortcut_search.is_empty() {
                    0.0
                } else {
                    ui.spacing().button_padding.x * 2.0 + 42.0
                };
                let search_width = (ui.available_width() - clear_width).max(140.0);
                let search_hint = self.t("shortcuts.search_hint");
                let search_response = ui.add_sized(
                    [search_width, ui.spacing().interact_size.y],
                    egui::TextEdit::singleline(&mut self.shortcut_search)
                        .id(egui::Id::new("shortcuts-search"))
                        .hint_text(search_hint),
                );
                if self.shortcut_focus_search
                    || ui.input(|input| input.modifiers.command && input.key_pressed(egui::Key::F))
                {
                    search_response.request_focus();
                    self.shortcut_focus_search = false;
                }
                if !self.shortcut_search.is_empty()
                    && ui.small_button(self.t("shortcuts.clear_search")).clicked()
                {
                    self.shortcut_search.clear();
                    self.shortcut_focus_search = true;
                }
            });
            ui.add_space(6.0);

            let query = self.shortcut_search.trim().to_lowercase();
            let matching_entries: Vec<_> = SHORTCUT_ENTRIES
                .iter()
                .filter_map(|entry| {
                    let description = self.t(entry.description_key);
                    shortcut_matches_query(entry.keys, &description, &query)
                        .then_some((entry.keys, description))
                })
                .collect();
            ui.label(
                RichText::new(self.tf(
                    "shortcuts.result_count",
                    &[("count", matching_entries.len().to_string())],
                ))
                .small()
                .weak(),
            );
            fresh_scroll_area(("shortcuts-scroll", self.shortcut_scroll_epoch), 420.0).show(
                ui,
                |ui| {
                    if matching_entries.is_empty() {
                        ui.label(RichText::new(self.t("shortcuts.no_results")).weak());
                    } else {
                        egui::Grid::new("shortcuts-grid")
                            .num_columns(2)
                            .spacing(egui::vec2(18.0, 7.0))
                            .show(ui, |ui| {
                                for (keys, description) in matching_entries {
                                    shortcut_row(ui, keys, description);
                                }
                            });
                    }
                },
            );
            ui.add_space(10.0);
            if show_close_button(ui, self.t("shortcuts.close")) {
                self.close_shortcuts();
            }
        });
    }

    pub(crate) fn show_history_modal(&mut self, ctx: &egui::Context) {
        if !self.show_history {
            return;
        }
        if show_backdrop(ctx, "history") {
            self.show_history = false;
            return;
        }
        modal_window("history", self.t("history.title"), 520.0).show(ctx, |ui| {
            ui.label(RichText::new(self.t("history.hint")).weak());
            ui.add_space(8.0);
            ui.label(RichText::new(self.t("history.undoable")).strong());
            let undo_history = self.commands.undo_history();
            if undo_history.is_empty() {
                ui.label(RichText::new(self.t("history.empty")).weak());
            } else {
                for (index, entry) in undo_history.iter().enumerate() {
                    ui.label(format!("{}. {}", index + 1, entry));
                }
            }
            ui.add_space(8.0);
            ui.label(RichText::new(self.t("history.redoable")).strong());
            let redo_history = self.commands.redo_history();
            if redo_history.is_empty() {
                ui.label(RichText::new(self.t("history.empty")).weak());
            } else {
                for (index, entry) in redo_history.iter().enumerate() {
                    ui.label(format!("{}. {}", index + 1, entry));
                }
            }
            ui.add_space(10.0);
            if show_close_button(ui, self.t("shortcuts.close")) {
                self.show_history = false;
            }
        });
    }
}

fn show_effect_step(
    ui: &mut Ui,
    app: &QpwgraphApp,
    descriptors: &[EffectDescriptor],
    wizard: &mut EffectWizardState,
) {
    ui.label(RichText::new(app.t("effects.choose_effect")).strong());
    modal_hint(ui, app.t("effects.choose_effect_hint"));
    ui.add_space(8.0);

    let current_name = descriptors
        .iter()
        .find(|descriptor| descriptor.id == wizard.effect_id)
        .map(|descriptor| descriptor.name.clone())
        .unwrap_or_else(|| app.t("effects.no_available"));
    let previous_id = wizard.effect_id.clone();
    modal_combo(
        ui,
        "effects-wizard-effect-select",
        app.t("effects.effect_label"),
        current_name,
        &mut wizard.effect_id,
        descriptors.iter().map(|descriptor| {
            (
                descriptor.id.clone(),
                format!("{} — {}", descriptor.name, descriptor.vendor),
            )
        }),
    );
    if previous_id != wizard.effect_id {
        if let Some(descriptor) = descriptors.iter().find(|d| d.id == wizard.effect_id) {
            wizard.parameters = default_parameters(descriptor);
        }
    }
    if let Some(descriptor) = descriptors.iter().find(|d| d.id == wizard.effect_id) {
        ui.add_space(10.0);
        ui.label(format!(
            "{} {} · {}",
            descriptor.name, descriptor.version, descriptor.vendor
        ));
        ui.label(
            RichText::new(app.tf(
                "effects.parameter_count",
                &[("count", descriptor.parameters.len().to_string())],
            ))
            .weak(),
        );
    }
}

fn show_link_step(
    ui: &mut Ui,
    app: &QpwgraphApp,
    links: &[(pw_graph_core::LinkId, String)],
    wizard: &mut EffectWizardState,
) {
    ui.label(RichText::new(app.t("effects.choose_link")).strong());
    modal_hint(ui, app.t("effects.choose_link_hint"));
    ui.add_space(8.0);
    if links.is_empty() {
        ui.label(RichText::new(app.t("effects.no_audio_links")).weak());
        wizard.link_id = None;
        return;
    }
    if wizard
        .link_id
        .is_none_or(|link_id| !links.iter().any(|(id, _)| *id == link_id))
    {
        wizard.link_id = links.first().map(|(id, _)| *id);
    }
    let selected_text = wizard
        .link_id
        .and_then(|link_id| links.iter().find(|(id, _)| *id == link_id))
        .map(|(_, label)| label.clone())
        .unwrap_or_else(|| app.t("effects.select_link"));
    modal_combo(
        ui,
        "effects-wizard-link-select",
        app.t("effects.link_label"),
        selected_text,
        &mut wizard.link_id,
        links
            .iter()
            .map(|(link_id, label)| (Some(*link_id), label.clone())),
    );
}

fn show_setup_step(
    ui: &mut Ui,
    app: &QpwgraphApp,
    descriptors: &[EffectDescriptor],
    links: &[(pw_graph_core::LinkId, String)],
    wizard: &mut EffectWizardState,
) {
    ui.label(RichText::new(app.t("effects.setup")).strong());
    modal_hint(ui, app.t("effects.setup_hint"));
    ui.add_space(8.0);
    modal_checkbox(
        ui,
        "effects-enabled",
        &mut wizard.enabled,
        app.t("effects.enabled"),
    );

    if let Some(descriptor) = descriptors.iter().find(|d| d.id == wizard.effect_id) {
        for parameter in &descriptor.parameters {
            if parameter.id == pw_graph_effects::NOISE_GATE_BYPASS {
                let mut bypass = wizard
                    .parameters
                    .get(&parameter.id)
                    .copied()
                    .unwrap_or(parameter.default)
                    >= 0.5;
                if modal_checkbox(
                    ui,
                    ("effects-parameter", &parameter.id),
                    &mut bypass,
                    parameter.name.clone(),
                ) {
                    wizard
                        .parameters
                        .insert(parameter.id.clone(), if bypass { 1.0 } else { 0.0 });
                }
                continue;
            }
            let mut value = wizard
                .parameters
                .get(&parameter.id)
                .copied()
                .unwrap_or(parameter.default);
            if modal_slider(
                ui,
                ("effects-parameter", &parameter.id),
                &mut value,
                parameter.minimum,
                parameter.maximum,
                parameter.name.clone(),
                &parameter.unit,
            ) {
                wizard.parameters.insert(parameter.id.clone(), value);
            }
        }
    }

    ui.separator();
    ui.label(RichText::new(app.t("effects.review")).strong());
    let selected_link = wizard
        .link_id
        .and_then(|link_id| links.iter().find(|(id, _)| *id == link_id))
        .map(|(_, label)| label.clone())
        .unwrap_or_else(|| app.t("effects.select_link"));
    ui.label(selected_link);
}

#[cfg(test)]
mod tests {
    use super::shortcut_matches_query;

    #[test]
    fn shortcut_search_matches_translated_descriptions() {
        assert!(shortcut_matches_query(
            "Ctrl/Cmd+Z",
            "Deshacer el último cambio",
            "deshacer"
        ));
        assert!(!shortcut_matches_query(
            "Ctrl/Cmd+Z",
            "Deshacer el último cambio",
            "volumen"
        ));
    }
}
