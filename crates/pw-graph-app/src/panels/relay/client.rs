//! Receiver section: connect to a relay host as an emitter of this device's
//! microphone, a sink for the host's playback, or both.

use super::super::components::{
    document_button, document_setting_number, document_setting_select, document_text_input,
};
use super::super::shared::panel_section;
use super::RELAY_SELECT_WIDTH;
use crate::app::QpwgraphApp;
use eframe::egui::{RichText, Ui};
use pw_graph_ui::{OptionItem, UiDocument};

impl QpwgraphApp {
    pub(super) fn show_relay_client_section(&mut self, document: &mut UiDocument, ui: &mut Ui) {
        panel_section(ui, self.i18n.text("relay.receiver_section"), |ui| {
            let (_, target) = document_text_input(
                document,
                ui,
                "relay.panel.client.target",
                &self.config.relay_client_target,
                self.i18n.text("relay.target"),
                Some(self.i18n.text("relay.target_help")),
            );
            self.config.relay_client_target = target;
            let (_, pin) = document_text_input(
                document,
                ui,
                "relay.panel.client.pin",
                &self.config.relay_client_pin,
                self.i18n.text("relay.pin"),
                Some(self.i18n.text("relay.pin_help")),
            );
            self.config.relay_client_pin = pin;
            self.config.relay_role = document_setting_select(
                document,
                ui,
                "relay.panel.client.role",
                &self.config.relay_role,
                None,
                self.i18n.text("relay.role"),
                self.i18n.text("relay.role_help"),
                [
                    OptionItem::new("emit", self.i18n.text("relay.role_emit")),
                    OptionItem::new("receive", self.i18n.text("relay.role_receive")),
                    OptionItem::new("both", self.i18n.text("relay.role_both")),
                ],
                RELAY_SELECT_WIDTH,
            );
            self.config.relay_codec = document_setting_select(
                document,
                ui,
                "relay.panel.client.codec",
                &self.config.relay_codec,
                None,
                self.i18n.text("relay.codec"),
                self.i18n.text("relay.codec_help"),
                [
                    OptionItem::new("opus", "Opus"),
                    OptionItem::new("pcm", "PCM"),
                ],
                RELAY_SELECT_WIDTH,
            );
            self.config.relay_frame_ms = document_setting_number(
                document,
                ui,
                "relay.panel.client.frame_ms",
                self.config.relay_frame_ms as f32,
                5.0,
                60.0,
                5.0,
                self.i18n.text("relay.frame_ms"),
                self.i18n.text("relay.frame_ms_help"),
                100.0,
            ) as u16;
            self.config.relay_transport = document_setting_select(
                document,
                ui,
                "relay.panel.client.transport",
                &self.config.relay_transport,
                None,
                self.i18n.text("relay.transport"),
                self.i18n.text("relay.transport_help"),
                [
                    OptionItem::new("auto", "Auto"),
                    OptionItem::new("usb", "USB"),
                    OptionItem::new("wifi", "Wi-Fi"),
                    OptionItem::new("bluetooth", "Bluetooth PAN"),
                    OptionItem::new("lan", "LAN"),
                ],
                RELAY_SELECT_WIDTH,
            );
            if document_button(
                document,
                ui,
                "relay.panel.client.connect",
                self.i18n.text("relay.connect"),
                true,
            ) {
                let mut relay = std::mem::take(&mut self.relay);
                relay.connect(self);
                self.relay = relay;
            }
            ui.label(
                RichText::new(self.i18n.text("relay.receiver_hint"))
                    .small()
                    .weak(),
            );
        });
    }
}
