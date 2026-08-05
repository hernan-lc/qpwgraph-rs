//! Connections tab: one list holding both discovered hosts and live
//! sessions, replacing the former Discover and Sessions tabs.
//!
//! Discovery runs for as long as the panel is open, so the list behaves like
//! a system Bluetooth or Wi-Fi pane: it fills in by itself, a refresh action
//! restarts the scan, and connecting or disconnecting happens on the row
//! without changing tabs. Manual entry and the codec/role settings sit in
//! disclosures below, out of the way of the common path.

use super::super::components::{document_button, document_text_input};
use super::super::shared::panel_section;
use super::device_row::RelayRowAction;
use super::CONNECTED_ACCENT;
use crate::app::{QpwgraphApp, RelayDeviceRow, RelayDeviceState};
use crate::icons::Icon;
use eframe::egui::{self, RichText, Ui};
use pw_graph_ui::{BadgeProps, DisclosureProps, IconButtonProps, UiDocument};

impl QpwgraphApp {
    pub(super) fn show_relay_connections_section(
        &mut self,
        document: &mut UiDocument,
        ui: &mut Ui,
    ) {
        panel_section(ui, self.i18n.text("relay.connections_section"), |ui| {
            self.show_relay_connections_header(document, ui);
            self.show_relay_usb_status(ui);
            ui.add_space(4.0);
            self.show_relay_device_groups(document, ui);
            ui.add_space(6.0);
            self.show_relay_manual_entry(document, ui);
            self.show_relay_advanced_settings(document, ui);
        });
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
                    let mut relay = std::mem::take(&mut self.relay);
                    relay.refresh(self);
                    self.relay = relay;
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
            ui.horizontal(|ui| {
                ui.label(RichText::new(self.i18n.text("relay.group_connected")).strong());
                document.badge(
                    ui,
                    BadgeProps::new(
                        "relay.panel.connections.connected_count",
                        connected.len().to_string(),
                    )
                    .color(CONNECTED_ACCENT),
                );
            });
            for row in &connected {
                self.show_relay_row_with_actions(document, ui, row);
            }
            ui.add_space(6.0);
        }

        ui.label(RichText::new(self.i18n.text("relay.group_available")).strong());
        if available.is_empty() {
            ui.label(
                RichText::new(self.i18n.text("relay.no_peers"))
                    .small()
                    .weak(),
            );
        }
        for row in &available {
            self.show_relay_row_with_actions(document, ui, row);
        }
        ui.add_space(4.0);
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
                let mut relay = std::mem::take(&mut self.relay);
                relay.connect(self);
                self.relay = relay;
            }
            RelayRowAction::CancelConnect => self.relay.cancel_connect(),
            RelayRowAction::Disconnect => {
                if let RelayDeviceState::Connected(session) = row.state {
                    let mut relay = std::mem::take(&mut self.relay);
                    relay.disconnect(self, session);
                    self.relay = relay;
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
            let (_, target) = document_text_input(
                document,
                ui,
                "relay.panel.connections.quick",
                &self.relay.quick_target,
                self.i18n.text("relay.target"),
                Some(self.i18n.text("relay.quick_target_help")),
            );
            self.relay.quick_target = target;
            let (_, pin) = document_text_input(
                document,
                ui,
                "relay.panel.connections.pin",
                &self.config.relay_client_pin,
                self.i18n.text("relay.pin"),
                Some(self.i18n.text("relay.pin_help")),
            );
            self.config.relay_client_pin = pin;
            let can_connect = !self.relay.quick_target.trim().is_empty();
            if document_button(
                document,
                ui,
                "relay.panel.connections.quick.connect",
                self.i18n.text("relay.connect"),
                can_connect,
            ) {
                let target = self.relay.quick_target.clone();
                let mut relay = std::mem::take(&mut self.relay);
                relay.connect_target(self, &target);
                relay.quick_target.clear();
                self.relay = relay;
            }
        });
    }
}
