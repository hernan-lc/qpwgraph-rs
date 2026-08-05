//! Discovery tab: browse the local network (mDNS plus direct USB probing)
//! for relay hosts and connect to one without typing its address.

use super::super::components::document_button;
use super::super::shared::panel_section;
use crate::app::QpwgraphApp;
use eframe::egui::{RichText, Ui};
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
            }
            for peer in peers {
                let is_connected = connected.contains(&peer.addr);
                ui.horizontal(|ui| {
                    ui.label(format!("{} — {}", peer.name, peer.addr));
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
        });
    }
}
