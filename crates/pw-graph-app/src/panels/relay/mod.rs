//! Relay panel: broadcast this device's audio as an emitter, or connect to a
//! relay host as a receiver.
//!
//! The panel is a first-class docked surface (opened from the navigation
//! rail) rather than a Preferences tab, because relaying is an activity you
//! watch — sessions and status change while you work the canvas.
//!
//! The body has two tabs. **Connections** is a single device list in the
//! shape of a system Bluetooth or Wi-Fi pane: discovered hosts and live
//! sessions are the same rows in different states, with manual entry and the
//! connection settings tucked into disclosures beneath. **Host** covers the
//! separate activity of broadcasting this device. Each tab's renderer lives
//! in its own submodule and drives the shared action layer in
//! [`crate::app::relay`], persisting through `AppConfig`.

mod advanced;
mod connections;
mod device_row;
mod host;
mod qr;

use super::components::document_button;
use super::shared::{apply_panel_text_scale, fresh_scroll_area, PANEL_FILL};
use crate::app::{QpwgraphApp, RelayPanelTab};
use crate::icons::Icon;
use eframe::egui::{self, Color32, RichText, Ui};
use pw_graph_ui::{TabItem, TabsProps, UiDocument};

const RELAY_PANEL_MIN_WIDTH: f32 = 320.0;
const RELAY_PANEL_DEFAULT_WIDTH: f32 = 380.0;
/// Wide enough for the option labels used by the role/codec/link selects.
pub(super) const RELAY_SELECT_WIDTH: f32 = 260.0;
const PANEL_TITLE_COLOR: Color32 = Color32::from_rgb(205, 216, 230);
/// Selection accent shared by the tab strip and the stepper, so "this is the
/// active thing" reads the same throughout the panel.
pub(super) const ACCENT: Color32 = Color32::from_rgb(96, 165, 250);
/// Live-session accent, matching the canvas link colour.
pub(super) const CONNECTED_ACCENT: Color32 = Color32::from_rgb(96, 190, 130);

/// Tab identity in the document. The enum stays the app's source of truth;
/// these map it onto the stable string values the tab strip retains.
fn relay_tab_value(tab: RelayPanelTab) -> &'static str {
    match tab {
        RelayPanelTab::Connections => "connections",
        RelayPanelTab::Host => "host",
    }
}

fn relay_tab_from_value(value: &str) -> RelayPanelTab {
    match value {
        "host" => RelayPanelTab::Host,
        _ => RelayPanelTab::Connections,
    }
}

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
        // Browsing is cheap and is what the panel is for, so having it open
        // keeps discovery running regardless of tab — the device list must
        // already be populated when the user switches back to it. A failed
        // start is not retried until the user presses refresh.
        if !self.relay.discovery_active && !self.relay.discovery_failed {
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
            RelayPanelTab::Connections => self.show_relay_connections_section(document, ui),
            RelayPanelTab::Host => self.show_relay_host_section(document, ui),
        });
        let message = self.relay.message.clone();
        if !message.is_empty() {
            ui.separator();
            ui.label(RichText::new(message).small().weak());
        }
    }

    /// Two tabs: the device list, and hosting. The session count rides on the
    /// Connections label as a badge, so it stays visible from the Host tab.
    fn show_relay_tab_bar(&mut self, document: &mut UiDocument, ui: &mut Ui) {
        let session_count = self.driver.relay_status().sessions.len();
        let selected = document.tabs(
            ui,
            TabsProps::new(
                "relay.panel.tabs",
                [
                    TabItem::new("connections", self.i18n.text("relay.tab_connections"))
                        .icon(Icon::Connect.source())
                        .badge_count(session_count),
                    TabItem::new("host", self.i18n.text("relay.tab_host"))
                        .icon(Icon::Relay.source()),
                ],
            )
            .selected(relay_tab_value(self.relay.tab))
            .accent(ACCENT),
        );
        self.relay.tab = relay_tab_from_value(&selected);
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
