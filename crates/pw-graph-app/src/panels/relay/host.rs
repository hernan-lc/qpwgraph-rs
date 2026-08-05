//! Emitter section: start a relay host so remote receivers can connect and
//! carry this device's audio.

use super::super::components::{document_button, document_setting_number, document_text_input};
use super::super::shared::panel_section;
use crate::app::QpwgraphApp;
use eframe::egui::{RichText, Ui};
use pw_graph_ui::UiDocument;

impl QpwgraphApp {
    pub(super) fn show_relay_host_section(&mut self, document: &mut UiDocument, ui: &mut Ui) {
        panel_section(ui, self.i18n.text("relay.emitter_section"), |ui| {
            let (_, device_name) = document_text_input(
                document,
                ui,
                "relay.panel.host.device_name",
                &self.config.relay_device_name,
                self.i18n.text("relay.device_name"),
                Some(self.i18n.text("relay.device_name_help")),
            );
            self.config.relay_device_name = device_name;
            let (_, pin) = document_text_input(
                document,
                ui,
                "relay.panel.host.pin",
                &self.config.relay_host_pin,
                self.i18n.text("relay.pin"),
                Some(self.i18n.text("relay.pin_help")),
            );
            self.config.relay_host_pin = pin;
            self.config.relay_host_port = document_setting_number(
                document,
                ui,
                "relay.panel.host.port",
                self.config.relay_host_port as f32,
                0.0,
                65535.0,
                1.0,
                self.i18n.text("relay.port"),
                self.i18n.text("relay.port_help"),
                100.0,
            ) as u16;
            let running = self.driver.relay_status().host_active;
            ui.horizontal(|ui| {
                if document_button(
                    document,
                    ui,
                    "relay.panel.host.toggle",
                    self.i18n.text(if running {
                        "relay.stop_host"
                    } else {
                        "relay.start_host"
                    }),
                    true,
                ) {
                    let mut relay = std::mem::take(&mut self.relay);
                    if running {
                        relay.stop_host(self);
                    } else {
                        relay.start_host(self);
                    }
                    self.relay = relay;
                }
                if running
                    && document_button(
                        document,
                        ui,
                        "relay.panel.host.qr",
                        self.i18n.text("relay.show_qr"),
                        true,
                    )
                {
                    self.relay.show_qr = true;
                }
            });
            self.show_relay_usb_status(ui);
            if running {
                self.show_relay_host_endpoints(ui);
            }
            ui.label(
                RichText::new(self.i18n.text("relay.emitter_hint"))
                    .small()
                    .weak(),
            );
        });
    }

    /// Connection details peers need: the control port, every reachable
    /// `address:port` (the QR's primary endpoint first and highlighted), and
    /// the pairing PIN. Monospace keeps the endpoints readable as one unit.
    fn show_relay_host_endpoints(&self, ui: &mut Ui) {
        let Some(port) = self.driver.relay_status().host_port else {
            return;
        };
        ui.label(self.tf("relay.listening", &[("port", port.to_string())]));
        let links = self.relay.links.clone();
        if links.is_empty() {
            ui.label(
                RichText::new(self.i18n.text("relay.no_links"))
                    .small()
                    .weak(),
            );
        }
        for (index, link) in links.iter().enumerate() {
            let endpoint = format!("{}:{}", link.addr, port);
            if index == 0 {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(endpoint).monospace().strong());
                    ui.label(
                        RichText::new(self.i18n.text("relay.endpoint_primary"))
                            .small()
                            .weak(),
                    );
                });
            } else {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{} · ", link.name)).small().weak());
                    ui.label(RichText::new(endpoint).monospace().weak());
                });
            }
        }
        let pin = self.config.relay_host_pin.trim();
        if !pin.is_empty() {
            ui.label(self.tf("relay.qr_pin", &[("pin", pin.to_owned())]));
        }
    }
}
