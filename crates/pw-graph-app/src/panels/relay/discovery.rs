//! Discovery tab: browse the local network (mDNS plus direct USB probing)
//! for relay hosts and connect to one without typing its address.

use super::super::components::{document_button, document_text_input};
use super::super::shared::panel_section;
use crate::app::QpwgraphApp;
use eframe::egui::{RichText, Ui};
use pw_graph_backend::RelayDeviceKind;
use pw_graph_ui::UiDocument;
use std::collections::BTreeSet;
use std::net::SocketAddr;

impl QpwgraphApp {
    pub(super) fn show_relay_discovery_section(&mut self, document: &mut UiDocument, ui: &mut Ui) {
        panel_section(ui, self.i18n.text("relay.discovery_section"), |ui| {
            self.show_relay_usb_status(ui);
            if document_button(
                document,
                ui,
                "relay.panel.discovery.toggle",
                self.i18n.text(if self.relay.discovery_active {
                    "relay.stop_discovery"
                } else {
                    "relay.discover"
                }),
                true,
            ) {
                let mut relay = std::mem::take(&mut self.relay);
                relay.toggle_discovery(self);
                self.relay = relay;
            }
            if self.relay.discovery_active {
                self.show_relay_searching_indicator(ui);
            }
            ui.label(
                RichText::new(self.i18n.text("relay.discovery_help"))
                    .small()
                    .weak(),
            );
            ui.separator();
            self.show_relay_peer_list(document, ui);
            ui.separator();
            self.show_relay_quick_connect(document, ui);
        });
    }

    /// Animated "searching" line so an empty peer list still reads as live.
    fn show_relay_searching_indicator(&self, ui: &mut Ui) {
        let time = ui.ctx().input(|input| input.time);
        let dots = ".".repeat(1 + (time * 1.5) as usize % 3);
        ui.label(
            RichText::new(format!("{}{dots}", self.i18n.text("relay.discovery_searching")))
                .small()
                .strong(),
        );
    }

    /// One row per discovered host with its endpoint and a connect action.
    fn show_relay_peer_list(&mut self, document: &mut UiDocument, ui: &mut Ui) {
        let connected: BTreeSet<SocketAddr> = self
            .driver
            .relay_status()
            .sessions
            .iter()
            .map(|session| session.peer.addr)
            .collect();
        let peers = self.relay.peers.clone();
        if peers.is_empty() {
            ui.label(
                RichText::new(self.i18n.text("relay.no_peers"))
                    .small()
                    .weak(),
            );
            return;
        }
        for peer in peers {
            let is_connected = connected.contains(&peer.addr);
            ui.horizontal(|ui| {
                ui.label(RichText::new(peer.name.clone()).strong());
                ui.label(
                    RichText::new(self.relay_peer_kind_label(peer.kind))
                        .small()
                        .weak(),
                );
            });
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.label(RichText::new(peer.addr.to_string()).monospace().weak());
                if is_connected {
                    ui.label(
                        RichText::new(self.i18n.text("relay.peer_connected"))
                            .small()
                            .weak(),
                    );
                } else if document_button(
                    document,
                    ui,
                    &format!("relay.panel.discovery.connect.{}", peer.addr),
                    self.i18n.text("relay.connect"),
                    true,
                ) {
                    self.config.relay_client_target = peer.addr.to_string();
                    let mut relay = std::mem::take(&mut self.relay);
                    relay.connect(self);
                    self.relay = relay;
                }
            });
        }
    }

    /// Manual fallback on the same tab: `host:port` or a pasted QR payload,
    /// for networks where mDNS does not cross (some USB tethers, guest LANs).
    fn show_relay_quick_connect(&mut self, document: &mut UiDocument, ui: &mut Ui) {
        ui.label(RichText::new(self.i18n.text("relay.discovery_manual")).strong());
        let (_, target) = document_text_input(
            document,
            ui,
            "relay.panel.discovery.quick",
            &self.relay.quick_target,
            self.i18n.text("relay.target"),
            Some(self.i18n.text("relay.quick_target_help")),
        );
        self.relay.quick_target = target;
        let can_connect = !self.relay.quick_target.trim().is_empty();
        if document_button(
            document,
            ui,
            "relay.panel.discovery.quick.connect",
            self.i18n.text("relay.connect"),
            can_connect,
        ) {
            let target = self.relay.quick_target.clone();
            let mut relay = std::mem::take(&mut self.relay);
            relay.connect_target(self, &target);
            relay.quick_target.clear();
            self.relay = relay;
        }
    }

    fn relay_peer_kind_label(&self, kind: RelayDeviceKind) -> String {
        let key = match kind {
            RelayDeviceKind::Android => "relay.peer_kind_android",
            RelayDeviceKind::Linux => "relay.peer_kind_linux",
            RelayDeviceKind::Other => "relay.peer_kind_other",
        };
        self.i18n.text(key)
    }
}