//! Relay panel: broadcast this device's audio as an emitter, or connect to a
//! relay host as a receiver.
//!
//! The panel is a first-class docked surface (opened from the navigation
//! rail) rather than a Preferences tab, because relaying is an activity you
//! watch — sessions and status change while you work the canvas. One renderer
//! per section lives in its own submodule; all of them drive the shared
//! action layer in [`crate::app::relay`] and persist through `AppConfig`.

mod client;
mod discovery;
mod host;
mod sessions;

use super::components::document_button;
use super::shared::{apply_panel_text_scale, fresh_scroll_area, PANEL_FILL};
use crate::app::QpwgraphApp;
use eframe::egui::{self, Color32, RichText, Ui};
use pw_graph_ui::UiDocument;

const RELAY_PANEL_MIN_WIDTH: f32 = 320.0;
const RELAY_PANEL_DEFAULT_WIDTH: f32 = 380.0;
/// Wide enough for the option labels used by the role/codec/link selects.
pub(super) const RELAY_SELECT_WIDTH: f32 = 260.0;
const PANEL_TITLE_COLOR: Color32 = Color32::from_rgb(205, 216, 230);

impl QpwgraphApp {
    /// Right-docked relay panel, visible while the canvas stays interactive.
    pub(crate) fn show_relay_panel(&mut self, ctx: &egui::Context) {
        if !self.show_relay || self.any_modal_open() {
            return;
        }
        let mut document = std::mem::take(&mut self.ui_document);
        egui::SidePanel::right("relay")
            .resizable(true)
            .min_width(RELAY_PANEL_MIN_WIDTH)
            .default_width(RELAY_PANEL_DEFAULT_WIDTH)
            .frame(egui::Frame::none().fill(PANEL_FILL).inner_margin(8.0))
            .show(ctx, |ui| {
                apply_panel_text_scale(ui, self.config.panel_text_scale);
                self.show_relay_contents(&mut document, ui);
            });
        self.ui_document = document;
    }

    fn show_relay_contents(&mut self, document: &mut UiDocument, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(self.i18n.text("relay.title"))
                    .strong()
                    .color(PANEL_TITLE_COLOR),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if document_button(
                    document,
                    ui,
                    "relay.panel.close",
                    self.i18n.text("relay.panel_close"),
                    true,
                ) {
                    self.show_relay = false;
                }
            });
        });
        ui.separator();
        if !self.driver.relay_available() {
            ui.label(RichText::new(self.i18n.text("relay.unavailable")).weak());
            return;
        }
        fresh_scroll_area("relay-panel-scroll", ui.available_height()).show(ui, |ui| {
            self.show_relay_host_section(document, ui);
            self.show_relay_client_section(document, ui);
            self.show_relay_discovery_section(document, ui);
            self.show_relay_sessions_section(document, ui);
        });
        let message = self.relay.message.clone();
        if !message.is_empty() {
            ui.separator();
            ui.label(RichText::new(message).small().weak());
        }
    }
}
