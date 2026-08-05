use super::super::shared::show_centered_dialog;
use super::super::shared::show_close_button;
use crate::app::QpwgraphApp;
use eframe::egui::{self, Color32, RichText};

impl QpwgraphApp {
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
                ui.label(
                    RichText::new(self.i18n.text("history.hint"))
                        .color(Color32::from_rgb(180, 195, 215)),
                );
                ui.add_space(8.0);
                ui.label(
                    RichText::new(self.i18n.text("history.undoable"))
                        .strong()
                        .color(Color32::from_rgb(240, 244, 250)),
                );
                let undo_history = self.commands.undo_history();
                if undo_history.is_empty() {
                    ui.label(
                        RichText::new(self.i18n.text("history.empty"))
                            .color(Color32::from_rgb(180, 195, 215)),
                    );
                } else {
                    for (index, entry) in undo_history.iter().enumerate() {
                        ui.label(
                            RichText::new(format!("{}. {}", index + 1, entry))
                                .color(Color32::from_rgb(215, 225, 238)),
                        );
                    }
                }
                ui.add_space(8.0);
                ui.label(
                    RichText::new(self.i18n.text("history.redoable"))
                        .strong()
                        .color(Color32::from_rgb(240, 244, 250)),
                );
                let redo_history = self.commands.redo_history();
                if redo_history.is_empty() {
                    ui.label(
                        RichText::new(self.i18n.text("history.empty"))
                            .color(Color32::from_rgb(180, 195, 215)),
                    );
                } else {
                    for (index, entry) in redo_history.iter().enumerate() {
                        ui.label(
                            RichText::new(format!("{}. {}", index + 1, entry))
                                .color(Color32::from_rgb(215, 225, 238)),
                        );
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
