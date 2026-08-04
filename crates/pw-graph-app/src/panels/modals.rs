use super::components::{
    document_button, document_checkbox, document_slider, document_text_input_sized,
};
use super::shared::{fresh_scroll_area, show_centered_dialog, show_close_button};
use crate::app::effects::{available_descriptors, EffectGalleryState};
use crate::app::QpwgraphApp;
use eframe::egui::{self, Color32, RichText, Sense, Stroke, Ui};
use pw_graph_effects::EffectDescriptor;
use pw_graph_ui::UiDocument;

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

fn effect_gallery_card(
    document: &mut UiDocument,
    ui: &mut Ui,
    descriptor: &EffectDescriptor,
    summary: String,
    selected: bool,
) -> bool {
    let width = ui.available_width().max(1.0);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 96.0), Sense::click());
    let visuals = ui.style().interact_selectable(&response, selected);
    let fill = if selected {
        Color32::from_rgb(35, 86, 119)
    } else if response.hovered() {
        Color32::from_rgb(42, 50, 62)
    } else {
        Color32::from_rgb(33, 39, 49)
    };
    let stroke = if selected {
        Stroke::new(1.5_f32, Color32::from_rgb(74, 183, 240))
    } else {
        visuals.bg_stroke
    };
    ui.painter().rect(rect, 7.0, fill, stroke);
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(rect.shrink(10.0))
            .id_salt(("effect-gallery-card", &descriptor.id)),
        |ui| {
            ui.label(RichText::new(&descriptor.name).strong());
            ui.label(
                RichText::new(format!("{} · {}", descriptor.vendor, descriptor.version))
                    .small()
                    .weak(),
            );
            ui.add_space(8.0);
            ui.label(
                RichText::new(summary)
                    .small()
                    .color(Color32::from_rgb(174, 197, 216)),
            );
        },
    );
    document.record_click(
        format!("modals.effects.card.{}", descriptor.id),
        response.clicked(),
    )
}

fn show_effect_initial_settings(
    document: &mut UiDocument,
    ui: &mut Ui,
    descriptor: &EffectDescriptor,
    gallery: &mut EffectGalleryState,
    initial_settings_label: String,
    enabled_label: String,
) {
    egui::CollapsingHeader::new(initial_settings_label)
        .default_open(false)
        .show(ui, |ui| {
            gallery.enabled = document_checkbox(
                document,
                ui,
                "modals.effects.enabled",
                gallery.enabled,
                enabled_label,
                None,
            );
            for parameter in &descriptor.parameters {
                if parameter.unit == "boolean" {
                    let value = gallery
                        .parameters
                        .get(&parameter.id)
                        .copied()
                        .unwrap_or(parameter.default)
                        >= 0.5;
                    let value = document_checkbox(
                        document,
                        ui,
                        &format!("modals.effects.parameters.{}.boolean", parameter.id),
                        value,
                        parameter.name.clone(),
                        None,
                    );
                    gallery
                        .parameters
                        .insert(parameter.id.clone(), if value { 1.0 } else { 0.0 });
                } else {
                    let value = gallery
                        .parameters
                        .get(&parameter.id)
                        .copied()
                        .unwrap_or(parameter.default);
                    let (_, value) = document_slider(
                        document,
                        ui,
                        &format!("modals.effects.parameters.{}", parameter.id),
                        value,
                        parameter.minimum,
                        parameter.maximum,
                        0.0,
                        format!("{} ({})", parameter.name, parameter.unit),
                        true,
                        None,
                    );
                    gallery.parameters.insert(parameter.id.clone(), value);
                }
            }
        });
}

impl QpwgraphApp {
    pub(crate) fn show_effect_gallery_modal(&mut self, ctx: &egui::Context) {
        if self.effect_gallery.is_none() {
            return;
        }
        let descriptors = available_descriptors(self.driver.as_ref());
        let supports_effect_nodes = self.driver.supports_effect_nodes();
        let Some(mut gallery) = self.effect_gallery.take() else {
            return;
        };
        if gallery.effect_id.is_empty()
            || !descriptors
                .iter()
                .any(|descriptor| descriptor.id == gallery.effect_id)
        {
            if let Some(descriptor) = descriptors.first() {
                gallery.select_effect(descriptor);
            }
        }
        let mut cancel = false;
        let mut create = false;
        let initial_settings_label = self.i18n.text("effects.initial_settings");
        let enabled_label = self.i18n.text("effects.enabled");
        let mut document = std::mem::take(&mut self.ui_document);
        let dialog_response = show_centered_dialog(
            &mut document,
            ctx,
            "effects-gallery",
            self.i18n.text("effects.gallery_title"),
            720.0,
            |ui, document| {
                ui.label(RichText::new(self.i18n.text("effects.gallery_hint")).weak());
                if !supports_effect_nodes {
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(self.i18n.text("effects.backend_unavailable"))
                            .color(Color32::from_rgb(239, 169, 82)),
                    );
                }
                ui.add_space(8.0);

                fresh_scroll_area(("effects-gallery-scroll", gallery.scroll_epoch), 400.0).show(
                    ui,
                    |ui| {
                        ui.label(RichText::new(self.i18n.text("effects.choose_effect")).strong());
                        ui.add_space(6.0);
                        if descriptors.is_empty() {
                            ui.label(RichText::new(self.i18n.text("effects.no_available")).weak());
                        } else if ui.available_width() < 440.0 {
                            for descriptor in &descriptors {
                                let selected = gallery.effect_id == descriptor.id;
                                let summary = format!(
                                    "{} · {}",
                                    self.tf(
                                        "effects.parameter_count",
                                        &[("count", descriptor.parameters.len().to_string())],
                                    ),
                                    self.i18n.text("effects.port_flow"),
                                );
                                if effect_gallery_card(document, ui, descriptor, summary, selected)
                                {
                                    gallery.select_effect(descriptor);
                                }
                                ui.add_space(8.0);
                            }
                        } else {
                            ui.columns(2, |columns| {
                                for (index, descriptor) in descriptors.iter().enumerate() {
                                    let column = &mut columns[index % 2];
                                    let selected = gallery.effect_id == descriptor.id;
                                    let summary = format!(
                                        "{} · {}",
                                        self.tf(
                                            "effects.parameter_count",
                                            &[("count", descriptor.parameters.len().to_string())],
                                        ),
                                        self.i18n.text("effects.port_flow"),
                                    );
                                    if effect_gallery_card(
                                        document, column, descriptor, summary, selected,
                                    ) {
                                        gallery.select_effect(descriptor);
                                    }
                                    column.add_space(8.0);
                                }
                            });
                        }

                        let selected_descriptor = descriptors
                            .iter()
                            .find(|descriptor| descriptor.id == gallery.effect_id);
                        if let Some(descriptor) = selected_descriptor {
                            ui.add_space(4.0);
                            ui.separator();
                            ui.add_space(6.0);
                            show_effect_initial_settings(
                                document,
                                ui,
                                descriptor,
                                &mut gallery,
                                initial_settings_label.clone(),
                                enabled_label.clone(),
                            );
                        }
                    },
                );

                ui.add_space(10.0);
                ui.separator();
                ui.horizontal(|ui| {
                    if document_button(
                        document,
                        ui,
                        "modals.effects.cancel",
                        self.i18n.text("effects.cancel"),
                        true,
                    ) {
                        cancel = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if document_button(
                            document,
                            ui,
                            "modals.effects.create_node",
                            self.i18n.text("effects.create_node"),
                            supports_effect_nodes && !gallery.effect_id.is_empty(),
                        ) {
                            create = true;
                        }
                    });
                });
            },
        );
        self.ui_document = document;

        if dialog_response.backdrop_clicked {
            self.effect_gallery = None;
            return;
        }

        if create && self.create_effect_from_gallery(&gallery) {
            return;
        }
        if !cancel {
            self.effect_gallery = Some(gallery);
        }
    }

    pub(crate) fn show_shortcuts_modal(&mut self, ctx: &egui::Context) {
        if !self.show_shortcuts {
            return;
        }
        let mut document = std::mem::take(&mut self.ui_document);
        let dialog_response = show_centered_dialog(
            &mut document,
            ctx,
            "shortcuts",
            self.i18n.text("shortcuts.title"),
            560.0,
            |ui, document| {
                ui.label(RichText::new(self.i18n.text("shortcuts.hint")).weak());
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(self.i18n.text("shortcuts.search")).strong());
                    let clear_width = if self.shortcut_search.is_empty() {
                        0.0
                    } else {
                        ui.spacing().button_padding.x * 2.0 + 42.0
                    };
                    let search_width = (ui.available_width() - clear_width).max(140.0);
                    let search_hint = self.i18n.text("shortcuts.search_hint");
                    let current_search = self.shortcut_search.clone();
                    let (search_response, search_value) = document_text_input_sized(
                        document,
                        ui,
                        "modals.shortcuts.search",
                        &current_search,
                        String::new(),
                        Some(search_hint),
                        Some(search_width),
                    );
                    self.shortcut_search = search_value;
                    if self.shortcut_focus_search
                        || ui.input(|input| {
                            input.modifiers.command && input.key_pressed(egui::Key::F)
                        })
                    {
                        search_response.request_focus();
                        self.shortcut_focus_search = false;
                    }
                    if !self.shortcut_search.is_empty()
                        && document_button(
                            document,
                            ui,
                            "modals.shortcuts.clear_search",
                            self.i18n.text("shortcuts.clear_search"),
                            true,
                        )
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
                        let description = self.i18n.text(entry.description_key);
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
                            ui.label(RichText::new(self.i18n.text("shortcuts.no_results")).weak());
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
                if show_close_button(
                    document,
                    ui,
                    "modals.shortcuts.close",
                    self.i18n.text("shortcuts.close"),
                ) {
                    self.close_shortcuts();
                }
            },
        );
        self.ui_document = document;
        if dialog_response.backdrop_clicked {
            self.close_shortcuts();
        }
    }

    pub(crate) fn show_history_modal(&mut self, ctx: &egui::Context) {
        if !self.show_history {
            return;
        }
        let mut document = std::mem::take(&mut self.ui_document);
        let dialog_response = show_centered_dialog(
            &mut document,
            ctx,
            "history",
            self.i18n.text("history.title"),
            520.0,
            |ui, document| {
                ui.label(RichText::new(self.i18n.text("history.hint")).weak());
                ui.add_space(8.0);
                ui.label(RichText::new(self.i18n.text("history.undoable")).strong());
                let undo_history = self.commands.undo_history();
                if undo_history.is_empty() {
                    ui.label(RichText::new(self.i18n.text("history.empty")).weak());
                } else {
                    for (index, entry) in undo_history.iter().enumerate() {
                        ui.label(format!("{}. {}", index + 1, entry));
                    }
                }
                ui.add_space(8.0);
                ui.label(RichText::new(self.i18n.text("history.redoable")).strong());
                let redo_history = self.commands.redo_history();
                if redo_history.is_empty() {
                    ui.label(RichText::new(self.i18n.text("history.empty")).weak());
                } else {
                    for (index, entry) in redo_history.iter().enumerate() {
                        ui.label(format!("{}. {}", index + 1, entry));
                    }
                }
                ui.add_space(10.0);
                if show_close_button(
                    document,
                    ui,
                    "modals.history.close",
                    self.i18n.text("shortcuts.close"),
                ) {
                    self.show_history = false;
                }
            },
        );
        self.ui_document = document;
        if dialog_response.backdrop_clicked {
            self.show_history = false;
        }
    }
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
