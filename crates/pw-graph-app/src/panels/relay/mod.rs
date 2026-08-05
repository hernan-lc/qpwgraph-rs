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

use super::shared::{apply_panel_text_scale, fresh_scroll_area, panel_fill};
use crate::app::{QpwgraphApp, RelayPanelTab};
use crate::icons::Icon;
use eframe::egui::{self, RichText, Ui};
use pw_graph_ui::{IconButtonProps, TabItem, TabsProps, Theme, ThemeToken, UiDocument};

const RELAY_PANEL_MIN_WIDTH: f32 = 320.0;
const RELAY_PANEL_DEFAULT_WIDTH: f32 = 380.0;
/// Wide enough for the longest option label the role/codec/link selects show
/// ("Both directions"), and no wider: the panel is a side dock, so every
/// point a control does not need belongs to the label beside it.
pub(super) const RELAY_SELECT_WIDTH: f32 = 168.0;
/// Steppers only ever hold two or three digits.
pub(super) const RELAY_NUMBER_WIDTH: f32 = 92.0;

/// Selection accent from theme (shared by tab strip and stepper).
pub(super) fn accent_color(theme: &Theme) -> egui::Color32 {
    theme.color(ThemeToken::Accent)
}
/// Live-session accent matching the canvas link colour.
pub(super) fn connected_accent_color(theme: &Theme) -> egui::Color32 {
    theme.color(ThemeToken::AccentConnected)
}

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
        let fill = panel_fill(document.theme());
        egui::SidePanel::right("relay")
            .resizable(true)
            .min_width(RELAY_PANEL_MIN_WIDTH)
            .default_width(RELAY_PANEL_DEFAULT_WIDTH)
            .frame(egui::Frame::none().fill(fill).inner_margin(8.0))
            .show(ctx, |ui| {
                apply_panel_text_scale(ui, self.config.panel_text_scale);
                self.show_relay_contents(&mut document, ui);
            });
        self.ui_document = document;
        self.show_relay_qr_modal(ctx);
    }

    fn show_relay_contents(&mut self, document: &mut UiDocument, ui: &mut Ui) {
        // The close affordance is an icon rather than a labelled button: at
        // this width a "Close the relay panel" button consumed more than half
        // the header, and the panel's own title is what the row is for. The
        // label survives as the tooltip.
        let title_color = document.theme_color(ThemeToken::TextPrimary);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(self.i18n.text("relay.title"))
                    .strong()
                    .color(title_color),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if document.icon_button(
                    ui,
                    IconButtonProps::new("relay.panel.close", Icon::Close.source())
                        .tooltip(self.i18n.text("relay.panel_close")),
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
            self.with_relay(|app, relay| relay.start_discovery(app));
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
        let accent = accent_color(document.theme());
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
            .accent(accent),
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
