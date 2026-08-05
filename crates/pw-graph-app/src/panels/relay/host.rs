//! Host tab: start a relay host so remote receivers can connect and carry
//! this device's audio.
//!
//! Hosting is a short setup with a real order to it — identify the device,
//! choose where it listens, then hand the address to a peer — and the last
//! part is meaningless until the host is actually running. The tab therefore
//! leads with a [`StepsProps`] header rather than a flat form: the stepper is
//! the honest shape for a task whose later parts are not yet reachable, which
//! a tab strip (where everything is always available) would misrepresent.

use super::super::components::{document_button, document_setting_number, document_text_input};
use super::super::shared::panel_section;
use super::{accent_color, connected_accent_color};
use crate::app::QpwgraphApp;
use crate::icons::Icon;
use eframe::egui::{RichText, Ui};
use pw_graph_ui::{
    BadgeProps, CardProps, IconButtonProps, StepItem, StepsProps, ThemeToken, UiDocument,
};

impl QpwgraphApp {
    pub(super) fn show_relay_host_section(&mut self, document: &mut UiDocument, ui: &mut Ui) {
        let theme = document.theme().clone();
        panel_section(ui, self.i18n.text("relay.emitter_section"), &theme, |ui| {
            let running = self.driver.relay_status().host_active;
            self.show_relay_host_steps(document, ui, running);
            ui.add_space(6.0);
            self.show_relay_host_identity(document, ui);
            self.show_relay_host_network(document, ui, running);
            if running {
                self.show_relay_host_share(document, ui);
            }
            ui.label(
                RichText::new(self.i18n.text("relay.emitter_hint"))
                    .small()
                    .color(document.theme_color(ThemeToken::TextWeak)),
            );
        });
    }

    /// Progress header. The identity step is complete once the device has a
    /// name and PIN, the network step once the host is listening, and sharing
    /// is only reachable after that.
    fn show_relay_host_steps(&mut self, document: &mut UiDocument, ui: &mut Ui, running: bool) {
        let identified = !self.config.relay_device_name.trim().is_empty()
            && !self.config.relay_host_pin.trim().is_empty();
        let current = if running {
            2
        } else if identified {
            1
        } else {
            0
        };
        document.steps(
            ui,
            StepsProps::new(
                "relay.panel.host.steps",
                [
                    StepItem::new(self.i18n.text("relay.step_identity")).done(identified),
                    StepItem::new(self.i18n.text("relay.step_network")).done(running),
                    StepItem::new(self.i18n.text("relay.step_share")),
                ],
            )
            .current(current)
            .navigable(false)
            .accent(if running {
                connected_accent_color(document.theme())
            } else {
                accent_color(document.theme())
            }),
        );
    }

    /// Step 1: how peers will recognise and authenticate this device.
    fn show_relay_host_identity(&mut self, document: &mut UiDocument, ui: &mut Ui) {
        document.card(
            ui,
            CardProps::new("relay.panel.host.identity_card"),
            |ui, document| {
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
            },
        );
    }

    /// Step 2: the listening port and the start/stop action.
    fn show_relay_host_network(&mut self, document: &mut UiDocument, ui: &mut Ui, running: bool) {
        document.card(
            ui,
            CardProps::new("relay.panel.host.network_card")
                .accent_option(running.then_some(connected_accent_color(document.theme()))),
            |ui, document| {
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
                    if running {
                        if let Some(port) = self.driver.relay_status().host_port {
                            document.badge(
                                ui,
                                BadgeProps::new(
                                    "relay.panel.host.port_badge",
                                    self.tf("relay.listening", &[("port", port.to_string())]),
                                )
                                .color(connected_accent_color(document.theme())),
                            );
                        }
                    }
                });
                self.show_relay_usb_status(ui);
            },
        );
    }

    /// Step 3: everything a peer needs to reach this host.
    fn show_relay_host_share(&mut self, document: &mut UiDocument, ui: &mut Ui) {
        document.card(
            ui,
            CardProps::new("relay.panel.host.share_card"),
            |ui, document| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(self.i18n.text("relay.step_share"))
                            .strong()
                            .color(document.theme_color(ThemeToken::TextPrimary)),
                    );
                    if document.icon_button(
                        ui,
                        IconButtonProps::new("relay.panel.host.qr", Icon::QrCode.source())
                            .tooltip(self.i18n.text("relay.show_qr")),
                    ) {
                        self.relay.show_qr = true;
                    }
                });
                self.show_relay_host_endpoints(document, ui);
            },
        );
    }

    /// Connection details peers need: every reachable `address:port` (the
    /// QR's primary endpoint first and highlighted) and the pairing PIN.
    /// Monospace keeps the endpoints readable as one unit.
    fn show_relay_host_endpoints(&self, document: &UiDocument, ui: &mut Ui) {
        let Some(port) = self.driver.relay_status().host_port else {
            return;
        };
        let weak = document.theme_color(ThemeToken::TextWeak);
        let primary = document.theme_color(ThemeToken::TextPrimary);
        let secondary = document.theme_color(ThemeToken::TextSecondary);
        let links = if self.relay.links.is_empty() {
            self.driver.relay_local_links()
        } else {
            self.relay.links.clone()
        };
        if links.is_empty() {
            ui.label(
                RichText::new(self.i18n.text("relay.no_links"))
                    .small()
                    .color(weak),
            );
        }
        for (index, link) in links.iter().enumerate() {
            let endpoint = format!("{}:{}", link.addr, port);
            if index == 0 {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(endpoint).monospace().strong().color(primary));
                    ui.label(
                        RichText::new(self.i18n.text("relay.endpoint_primary"))
                            .small()
                            .color(weak),
                    );
                });
            } else {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("{} · ", link.name))
                            .small()
                            .color(weak),
                    );
                    ui.label(RichText::new(endpoint).monospace().color(secondary));
                });
            }
        }
        let pin = self.config.relay_host_pin.trim();
        if !pin.is_empty() {
            ui.label(
                RichText::new(self.tf("relay.qr_pin", &[("pin", pin.to_owned())])).color(secondary),
            );
        }
    }
}
