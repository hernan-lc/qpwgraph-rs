//! Sessions section: live relay sessions with per-session disconnect.

use super::super::components::document_button;
use super::super::shared::panel_section;
use crate::app::QpwgraphApp;
use eframe::egui::{RichText, Ui};
use pw_graph_ui::UiDocument;

impl QpwgraphApp {
    pub(super) fn show_relay_sessions_section(&mut self, document: &mut UiDocument, ui: &mut Ui) {
        panel_section(ui, self.i18n.text("relay.sessions"), |ui| {
            let sessions = self.driver.relay_status().sessions;
            if sessions.is_empty() {
                ui.label(RichText::new(self.i18n.text("relay.no_sessions")).weak());
            }
            for session in sessions {
                ui.horizontal(|ui| {
                    ui.label(format!("{} — {}", session.peer.name, session.peer.addr));
                    if document_button(
                        document,
                        ui,
                        &format!("relay.panel.sessions.disconnect.{}", session.id.0),
                        self.i18n.text("relay.disconnect"),
                        true,
                    ) {
                        let mut relay = std::mem::take(&mut self.relay);
                        relay.disconnect(self, session.id);
                        self.relay = relay;
                    }
                });
            }
        });
    }
}
