//! Relay panel: broadcast this device's audio as an emitter, or connect to a
//! relay host as a receiver.
//!
//! The panel is a first-class docked surface (opened from the navigation
//! rail) rather than a Preferences tab, because relaying is an activity you
//! watch — sessions and status change while you work the canvas. The body is
//! split into tabs (emitter, receiver, discovery, sessions); one renderer per
//! tab lives in its own submodule, and all of them drive the shared action
//! layer in [`crate::app::relay`] and persist through `AppConfig`.

mod client;
mod discovery;
mod host;
mod qr;
mod sessions;

use super::components::{document_button, document_selectable_label};
use super::shared::{apply_panel_text_scale, fresh_scroll_area, PANEL_FILL};
use crate::app::{QpwgraphApp, RelayPanelTab};
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
        self.show_relay_qr_modal(ctx);
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
        // Browsing is cheap and expected on this tab, so opening it starts
        // discovery automatically; a failed start is not retried until the
        // user toggles it by hand.
        if self.relay.tab == RelayPanelTab::Discover
            && !self.relay.discovery_active
            && !self.relay.discovery_failed
        {
            let mut relay = std::mem::take(&mut self.relay);
            relay.start_discovery(self);
            self.relay = relay;
        }
        self.show_relay_tab_bar(document, ui);
        ui.separator();
        fresh_scroll_area(
            ("relay-panel-scroll", self.relay.tab),
            ui.available_height(),
        )
        .show(ui, |ui| match self.relay.tab {
            RelayPanelTab::Emitter => self.show_relay_host_section(document, ui),
            RelayPanelTab::Receiver => self.show_relay_client_section(document, ui),
            RelayPanelTab::Discover => self.show_relay_discovery_section(document, ui),
            RelayPanelTab::Sessions => self.show_relay_sessions_section(document, ui),
        });
        let message = self.relay.message.clone();
        if !message.is_empty() {
            ui.separator();
            ui.label(RichText::new(message).small().weak());
        }
    }

    /// One tab per relay activity, mirroring the Preferences tab bar.
    fn show_relay_tab_bar(&mut self, document: &mut UiDocument, ui: &mut Ui) {
        let session_count = self.driver.relay_status().sessions.len();
        let tabs = [
            (
                RelayPanelTab::Emitter,
                "relay.tab_emitter",
                "relay.panel.tab.emitter",
            ),
            (
                RelayPanelTab::Receiver,
                "relay.tab_receiver",
                "relay.panel.tab.receiver",
            ),
            (
                RelayPanelTab::Discover,
                "relay.tab_discover",
                "relay.panel.tab.discover",
            ),
            (
                RelayPanelTab::Sessions,
                "relay.tab_sessions",
                "relay.panel.tab.sessions",
            ),
        ];
        ui.horizontal(|ui| {
            for (tab, label_key, tab_id) in tabs {
                let mut label = self.i18n.text(label_key);
                if tab == RelayPanelTab::Sessions && session_count > 0 {
                    label = format!("{label} ({session_count})");
                }
                if document_selectable_label(
                    document,
                    ui,
                    tab_id,
                    self.relay.tab == tab,
                    &label,
                    label.clone(),
                ) && self.relay.tab != tab
                {
                    self.relay.tab = tab;
                }
            }
        });
    }

    /// Status line for an auto-detected USB tether: USB is preferred by the
    /// `Auto` transport policy, so the panel only reports the link.
    pub(super) fn show_relay_usb_status(&self, ui: &mut Ui) {
        if let Some(link) = &self.relay.usb_link {
            ui.label(
                RichText::new(self.tf(
                    "relay.usb_detected",
                    &[("name", link.name.clone()), ("addr", link.addr.to_string())],
                ))
                .small()
                .weak(),
            );
        }
    }
}
