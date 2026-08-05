//! Connections tab: one list holding both discovered hosts and live
//! sessions, replacing the former Discover and Sessions tabs.
//!
//! Discovery runs for as long as the panel is open, so the list behaves like
//! a system Bluetooth or Wi-Fi pane: it fills in by itself, a refresh action
//! restarts the scan, and connecting or disconnecting happens on the row
//! without changing tabs. Manual entry and the codec/role settings sit in
//! disclosures below, out of the way of the common path.

use super::super::components::{document_button, document_setting_text};
use super::super::shared::text_colors;
use super::connected_accent_color;
use super::device_row::RelayRowAction;
use crate::app::{QpwgraphApp, RelayDeviceRow, RelayDeviceState};
use crate::icons::Icon;
use eframe::egui::{self, RichText, Ui};
use pw_graph_ui::{BadgeProps, DisclosureProps, IconButtonProps, UiDocument};

/// A list separator, not a title. Device names are already `strong`, so the
/// group labels above them are set small and coloured instead of bold —
/// otherwise the section headings outweigh the rows they organise.
fn group_heading(ui: &mut Ui, text: String, color: egui::Color32) {
    ui.label(
        RichText::new(text.to_uppercase())
            .small()
            .strong()
            .color(color),
    );
}

impl QpwgraphApp {
    /// The tab is laid out directly on the panel surface rather than inside a
    /// titled section frame. The window title already says "Audio relay" and
    /// the tab already says "Connections", so a third box labelled "Devices"
    /// only spent width — which is the one thing a 380 pt side dock cannot
    /// spare. Structure comes from the device cards and the two disclosures.
    pub(super) fn show_relay_connections_section(
        &mut self,
        document: &mut UiDocument,
        ui: &mut Ui,
    ) {
        self.show_relay_connections_header(document, ui);
        self.show_relay_usb_status(ui);
        ui.add_space(6.0);
        self.show_relay_device_groups(document, ui);
        ui.add_space(10.0);
        ui.separator();
        self.show_relay_manual_entry(document, ui);
        self.show_relay_advanced_settings(document, ui);
    }

    /// Scan state plus the refresh action, mirroring the header of a system
    /// device pane.
    fn show_relay_connections_header(&mut self, document: &mut UiDocument, ui: &mut Ui) {
        ui.horizontal(|ui| {
            if self.relay.discovery_active {
                document.activity(
                    ui,
                    "relay.panel.connections.searching",
                    self.i18n.text("relay.discovery_searching"),
                );
            } else {
                ui.label(
                    RichText::new(self.i18n.text("relay.discovery_stopped"))
                        .small()
                        .weak(),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if document.icon_button(
                    ui,
                    IconButtonProps::new("relay.panel.connections.refresh", Icon::Refresh.source())
                        .tooltip(self.i18n.text("relay.refresh_help")),
                ) {
                    self.with_relay(|app, relay| relay.refresh(app));
                }
            });
        });
    }

    /// Connected devices first, then everything else — the ordering every
    /// system pairing UI uses, because the connected set is what the user
    /// acts on most.
    fn show_relay_device_groups(&mut self, document: &mut UiDocument, ui: &mut Ui) {
        let rows = self.relay.device_rows(self);
        let (connected, available): (Vec<RelayDeviceRow>, Vec<RelayDeviceRow>) =
            rows.into_iter().partition(RelayDeviceRow::is_connected);

        if !connected.is_empty() {
            let accent = connected_accent_color(document.theme());
            ui.horizontal(|ui| {
                group_heading(ui, self.i18n.text("relay.group_connected"), accent);
                document.badge(
                    ui,
                    BadgeProps::new(
                        "relay.panel.connections.connected_count",
                        connected.len().to_string(),
                    )
                    .color(accent),
                );
            });
            for row in &connected {
                self.show_relay_row_with_actions(document, ui, row);
            }
            ui.add_space(8.0);
        }

        let heading_color = text_colors(document.theme()).weak;
        group_heading(ui, self.i18n.text("relay.group_available"), heading_color);
        if available.is_empty() {
            self.show_relay_empty_state(ui);
        }
        for row in &available {
            self.show_relay_row_with_actions(document, ui, row);
        }
    }

    /// Nothing found yet. This is the only place the discovery explanation
    /// appears: it answers "why is my phone not here?", which stops being a
    /// question the moment a row shows up, and printing it permanently under
    /// a populated list is a paragraph of noise on every frame.
    fn show_relay_empty_state(&self, ui: &mut Ui) {
        ui.add_space(2.0);
        ui.label(
            RichText::new(self.i18n.text("relay.no_peers"))
                .small()
                .weak(),
        );
        ui.add_space(2.0);
        ui.label(
            RichText::new(self.i18n.text("relay.discovery_help"))
                .small()
                .weak(),
        );
    }

    fn show_relay_row_with_actions(
        &mut self,
        document: &mut UiDocument,
        ui: &mut Ui,
        row: &RelayDeviceRow,
    ) {
        match self.show_relay_device_row(document, ui, row) {
            RelayRowAction::None => {}
            RelayRowAction::Connect => {
                self.config.relay_client_target = row.addr.to_string();
                self.with_relay(|app, relay| relay.connect(app));
            }
            RelayRowAction::CancelConnect => self.relay.cancel_connect(),
            RelayRowAction::Disconnect => {
                if let RelayDeviceState::Connected(session) = row.state {
                    self.with_relay(|app, relay| relay.disconnect(app, session));
                }
            }
            RelayRowAction::ToggleDetails => {
                if let RelayDeviceState::Connected(session) = row.state {
                    self.relay.expanded = if self.relay.expanded == Some(session.0) {
                        None
                    } else {
                        Some(session.0)
                    };
                }
            }
        }
    }

    /// Fallback for networks where mDNS does not reach: a typed `host:port`
    /// or a pasted QR payload. Collapsed by default so the discovered list
    /// stays the primary path.
    fn show_relay_manual_entry(&mut self, document: &mut UiDocument, ui: &mut Ui) {
        let props = DisclosureProps::new(
            "relay.panel.connections.manual",
            self.i18n.text("relay.add_manually"),
        );
        document.disclosure(ui, props, |ui, document| {
            self.relay.quick_target = document_setting_text(
                document,
                ui,
                "relay.panel.connections.quick",
                &self.relay.quick_target,
                self.i18n.text("relay.target"),
                self.i18n.text("relay.quick_target_help"),
            );
            self.config.relay_client_pin = document_setting_text(
                document,
                ui,
                "relay.panel.connections.pin",
                &self.config.relay_client_pin,
                self.i18n.text("relay.pin"),
                self.i18n.text("relay.pin_help"),
            );
            let can_connect = !self.relay.quick_target.trim().is_empty();
            if document_button(
                document,
                ui,
                "relay.panel.connections.quick.connect",
                self.i18n.text("relay.connect"),
                can_connect,
            ) {
                let target = self.relay.quick_target.clone();
                self.with_relay(|app, relay| {
                    relay.connect_target(app, &target);
                    relay.quick_target.clear();
                });
            }
        });
    }
}
