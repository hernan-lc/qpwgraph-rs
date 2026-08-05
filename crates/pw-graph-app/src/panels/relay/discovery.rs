//! Discovery section: browse the local network for relay hosts and connect
//! to one without typing its address.

use super::super::components::document_button;
use super::super::shared::panel_section;
use crate::app::QpwgraphApp;
use eframe::egui::{RichText, Ui};
use pw_graph_ui::UiDocument;

impl QpwgraphApp {
    pub(super) fn show_relay_discovery_section(&mut self, document: &mut UiDocument, ui: &mut Ui) {
        panel_section(ui, self.i18n.text("relay.discovery_section"), |ui| {
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
            let peers = self.relay.peers.clone();
            if peers.is_empty() {
                ui.label(
                    RichText::new(self.i18n.text("relay.no_peers"))
                        .small()
                        .weak(),
                );
            }
            for peer in peers {
                ui.horizontal(|ui| {
                    ui.label(format!("{} — {}", peer.name, peer.addr));
                    if document_button(
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
