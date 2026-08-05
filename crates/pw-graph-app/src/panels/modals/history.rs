use super::super::shared::show_close_button;
use crate::app::QpwgraphApp;
use eframe::egui::{self, RichText, Ui};
use pw_graph_ui::{ThemeToken, UiDocument};

/// One numbered list of history entries: a heading followed by either the
/// "empty" note or the entries themselves. Used for both the undo and redo
/// lists so they cannot drift apart.
fn history_block(
    ui: &mut Ui,
    document: &UiDocument,
    title: &str,
    empty: &str,
    entries: &[String],
) {
    ui.label(
        RichText::new(title)
            .strong()
            .color(document.theme_color(ThemeToken::TextPrimary)),
    );
    if entries.is_empty() {
        ui.label(
            RichText::new(empty).color(document.theme_color(ThemeToken::TextWeak)),
        );
    } else {
        let secondary = document.theme_color(ThemeToken::TextSecondary);
        for (index, entry) in entries.iter().enumerate() {
            ui.label(RichText::new(format!("{}. {}", index + 1, entry)).color(secondary));
        }
    }
}

impl QpwgraphApp {
    pub(crate) fn show_history_modal(&mut self, ctx: &egui::Context) {
        if !self.show_history {
            return;
        }
        if self.run_dialog(ctx, "history", self.i18n.text("history.title"), 520.0, |app, ui, document| {
            ui.label(
                RichText::new(app.i18n.text("history.hint"))
                    .color(document.theme_color(ThemeToken::TextWeak)),
            );
            ui.add_space(8.0);
            history_block(
                ui,
                document,
                &app.i18n.text("history.undoable"),
                &app.i18n.text("history.empty"),
                &app.commands.undo_history(),
            );
            ui.add_space(8.0);
            history_block(
                ui,
                document,
                &app.i18n.text("history.redoable"),
                &app.i18n.text("history.empty"),
                &app.commands.redo_history(),
            );
            ui.add_space(10.0);
            if show_close_button(
                document,
                ui,
                "modals.history.close",
                app.i18n.text("shortcuts.close"),
            ) {
                app.show_history = false;
            }
        }) {
            self.show_history = false;
        }
    }
}
