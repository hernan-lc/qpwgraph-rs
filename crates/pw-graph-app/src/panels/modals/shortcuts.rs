use super::super::components::{document_button, document_text_input_sized};
use super::super::shared::{fresh_scroll_area, show_close_button};
use crate::app::QpwgraphApp;
use eframe::egui::{self, Color32, RichText, Ui};
use pw_graph_ui::ThemeToken;

fn shortcut_row(ui: &mut Ui, keys: &str, description: String, primary: Color32, secondary: Color32) {
    ui.label(RichText::new(keys).strong().monospace().color(primary));
    ui.label(RichText::new(description).color(secondary));
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
    pub(crate) fn show_shortcuts_modal(&mut self, ctx: &egui::Context) {
        if !self.show_shortcuts {
            return;
        }
        if self.run_dialog(ctx, "shortcuts", self.i18n.text("shortcuts.title"), 560.0, |app, ui, document| {
            let primary = document.theme_color(ThemeToken::TextPrimary);
            let secondary = document.theme_color(ThemeToken::TextSecondary);
            let weak = document.theme_color(ThemeToken::TextWeak);
            ui.label(
                RichText::new(app.i18n.text("shortcuts.hint")).color(weak),
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(app.i18n.text("shortcuts.search"))
                        .strong()
                        .color(primary),
                );
                let clear_width = if app.shortcut_search.is_empty() {
                    0.0
                } else {
                    ui.spacing().button_padding.x * 2.0 + 42.0
                };
                let search_width = (ui.available_width() - clear_width).max(140.0);
                let search_hint = app.i18n.text("shortcuts.search_hint");
                let current_search = app.shortcut_search.clone();
                let (search_response, search_value) = document_text_input_sized(
                    document,
                    ui,
                    "modals.shortcuts.search",
                    &current_search,
                    String::new(),
                    Some(search_hint),
                    Some(search_width),
                );
                app.shortcut_search = search_value;
                if app.shortcut_focus_search
                    || ui.input(|input| {
                        input.modifiers.command && input.key_pressed(egui::Key::F)
                    })
                {
                    search_response.request_focus();
                    app.shortcut_focus_search = false;
                }
                if !app.shortcut_search.is_empty()
                    && document_button(
                        document,
                        ui,
                        "modals.shortcuts.clear_search",
                        app.i18n.text("shortcuts.clear_search"),
                        true,
                    )
                {
                    app.shortcut_search.clear();
                    app.shortcut_focus_search = true;
                }
            });
            ui.add_space(6.0);

            let query = app.shortcut_search.trim().to_lowercase();
            let matching_entries: Vec<_> = SHORTCUT_ENTRIES
                .iter()
                .filter_map(|entry| {
                    let description = app.i18n.text(entry.description_key);
                    shortcut_matches_query(entry.keys, &description, &query)
                        .then_some((entry.keys, description))
                })
                .collect();
            ui.label(
                RichText::new(app.tf(
                    "shortcuts.result_count",
                    &[("count", matching_entries.len().to_string())],
                ))
                .small()
                .color(weak),
            );
            fresh_scroll_area(("shortcuts-scroll", app.shortcut_scroll_epoch), 420.0).show(ui, |ui| {
                if matching_entries.is_empty() {
                    ui.label(
                        RichText::new(app.i18n.text("shortcuts.no_results")).color(weak),
                    );
                } else {
                    egui::Grid::new("shortcuts-grid")
                        .num_columns(2)
                        .spacing(egui::vec2(18.0, 7.0))
                        .show(ui, |ui| {
                            for (keys, description) in matching_entries {
                                shortcut_row(ui, keys, description, primary, secondary);
                            }
                        });
                }
            });
            ui.add_space(10.0);
            if show_close_button(
                document,
                ui,
                "modals.shortcuts.close",
                app.i18n.text("shortcuts.close"),
            ) {
                app.close_shortcuts();
            }
        }) {
            self.close_shortcuts();
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
