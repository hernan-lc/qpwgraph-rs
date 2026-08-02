use super::shared::{fresh_scroll_area, modal_window, show_backdrop, show_close_button};
use crate::app::effects::{
    audio_link_options, available_descriptors, EffectGalleryState, EffectPlacement,
};
use crate::app::QpwgraphApp;
use eframe::egui::{self, Color32, RichText, Sense, Stroke, Ui};
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

fn effect_gallery_card(
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
    response.clicked()
}

fn show_effect_initial_settings(
    ui: &mut Ui,
    descriptor: &EffectDescriptor,
    gallery: &mut EffectGalleryState,
    app: &QpwgraphApp,
) {
    egui::CollapsingHeader::new(app.t("effects.initial_settings"))
        .default_open(false)
        .show(ui, |ui| {
            ui.checkbox(&mut gallery.enabled, app.t("effects.enabled"));
            for parameter in &descriptor.parameters {
                if parameter.unit == "boolean" {
                    let mut value = gallery
                        .parameters
                        .get(&parameter.id)
                        .copied()
                        .unwrap_or(parameter.default)
                        >= 0.5;
                    if ui.checkbox(&mut value, &parameter.name).changed() {
                        gallery
                            .parameters
                            .insert(parameter.id.clone(), if value { 1.0 } else { 0.0 });
                    }
                } else {
                    let mut value = gallery
                        .parameters
                        .get(&parameter.id)
                        .copied()
                        .unwrap_or(parameter.default);
                    if ui
                        .add(
                            egui::Slider::new(&mut value, parameter.minimum..=parameter.maximum)
                                .text(format!("{} ({})", parameter.name, parameter.unit)),
                        )
                        .changed()
                    {
                        gallery.parameters.insert(parameter.id.clone(), value);
                    }
                }
            }
        });
}

impl QpwgraphApp {
    pub(crate) fn show_effect_gallery_modal(&mut self, ctx: &egui::Context) {
        if self.effect_gallery.is_none() {
            return;
        }
        if show_backdrop(ctx, "effects-gallery") {
            self.effect_gallery = None;
            return;
        }

        let descriptors = available_descriptors(self.driver.as_ref());
        let links = audio_link_options(self.driver.as_ref());
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
        if gallery.placement == EffectPlacement::InsertOnLink
            && gallery
                .link_id
                .is_none_or(|link_id| !links.iter().any(|(id, _)| *id == link_id))
        {
            gallery.link_id = links.first().map(|(id, _)| *id);
        }

        let mut cancel = false;
        let mut create = false;
        modal_window("effects-gallery", self.t("effects.gallery_title"), 720.0).show(ctx, |ui| {
            ui.label(RichText::new(self.t("effects.gallery_hint")).weak());
            if !supports_effect_nodes {
                ui.add_space(6.0);
                ui.label(
                    RichText::new(self.t("effects.backend_unavailable"))
                        .color(Color32::from_rgb(239, 169, 82)),
                );
            }
            ui.add_space(8.0);

            fresh_scroll_area(("effects-gallery-scroll", gallery.scroll_epoch), 400.0).show(
                ui,
                |ui| {
                    ui.label(RichText::new(self.t("effects.choose_effect")).strong());
                    ui.add_space(6.0);
                    if descriptors.is_empty() {
                        ui.label(RichText::new(self.t("effects.no_available")).weak());
                    } else if ui.available_width() < 440.0 {
                        for descriptor in &descriptors {
                            let selected = gallery.effect_id == descriptor.id;
                            let summary = format!(
                                "{} · {}",
                                self.tf(
                                    "effects.parameter_count",
                                    &[("count", descriptor.parameters.len().to_string())],
                                ),
                                self.t("effects.port_flow"),
                            );
                            if effect_gallery_card(ui, descriptor, summary, selected) {
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
                                    self.t("effects.port_flow"),
                                );
                                if effect_gallery_card(column, descriptor, summary, selected) {
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
                        ui.add_space(5.0);
                        ui.label(RichText::new(self.t("effects.route_title")).strong());
                        ui.radio_value(
                            &mut gallery.placement,
                            EffectPlacement::NewNode,
                            self.t("effects.create_node_choice"),
                        );
                        ui.label(
                            RichText::new(self.t("effects.create_node_hint"))
                                .small()
                                .weak(),
                        );
                        ui.add_space(4.0);
                        ui.radio_value(
                            &mut gallery.placement,
                            EffectPlacement::InsertOnLink,
                            self.t("effects.insert_link_choice"),
                        );
                        ui.label(
                            RichText::new(self.t("effects.insert_link_hint"))
                                .small()
                                .weak(),
                        );
                        if gallery.placement == EffectPlacement::InsertOnLink {
                            ui.add_space(5.0);
                            if links.is_empty() {
                                gallery.link_id = None;
                                ui.label(RichText::new(self.t("effects.no_audio_links")).weak());
                            } else {
                                let selected_text = gallery
                                    .link_id
                                    .and_then(|id| links.iter().find(|(link_id, _)| *link_id == id))
                                    .map(|(_, label)| label.clone())
                                    .unwrap_or_else(|| self.t("effects.select_link"));
                                egui::ComboBox::from_id_salt("effects-gallery-link")
                                    .selected_text(selected_text)
                                    .width(ui.available_width())
                                    .show_ui(ui, |ui| {
                                        for (id, label) in &links {
                                            ui.selectable_value(
                                                &mut gallery.link_id,
                                                Some(*id),
                                                label,
                                            );
                                        }
                                    });
                            }
                        }

                        ui.add_space(6.0);
                        show_effect_initial_settings(ui, descriptor, &mut gallery, self);
                    }
                },
            );

            ui.add_space(10.0);
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button(self.t("effects.cancel")).clicked() {
                    cancel = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let valid_route =
                        gallery.placement == EffectPlacement::NewNode || gallery.link_id.is_some();
                    let label = self.t(match gallery.placement {
                        EffectPlacement::NewNode => "effects.create_node",
                        EffectPlacement::InsertOnLink => "effects.insert_effect",
                    });
                    if ui
                        .add_enabled(
                            supports_effect_nodes && !gallery.effect_id.is_empty() && valid_route,
                            egui::Button::new(label),
                        )
                        .clicked()
                    {
                        create = true;
                    }
                });
            });
        });

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
