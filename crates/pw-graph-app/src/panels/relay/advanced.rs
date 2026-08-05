//! Advanced connection settings, formerly the Receiver tab.
//!
//! Role, codec, frame duration, and preferred link apply to the next
//! connection this device makes. They were a whole tab before, which put
//! four rarely-touched selects in front of the common "pick a device and
//! connect" path; they now live in a disclosure under the device list.

use super::super::components::{document_setting_number, document_setting_select};
use super::RELAY_SELECT_WIDTH;
use crate::app::QpwgraphApp;
use eframe::egui::{RichText, Ui};
use pw_graph_ui::{DisclosureProps, OptionItem, UiDocument};

impl QpwgraphApp {
    pub(super) fn show_relay_advanced_settings(&mut self, document: &mut UiDocument, ui: &mut Ui) {
        let props = DisclosureProps::new("relay.panel.advanced", self.i18n.text("relay.advanced"))
            .summary(self.relay_advanced_summary());
        document.disclosure(ui, props, |ui, document| {
            ui.label(
                RichText::new(self.i18n.text("relay.advanced_help"))
                    .small()
                    .weak(),
            );
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
                    OptionItem::new("wifi", "Wi-Fi"),
                    OptionItem::new("bluetooth", "Bluetooth PAN"),
                    OptionItem::new("lan", "LAN"),
                ],
                RELAY_SELECT_WIDTH,
            );
        });
    }

    /// One-line summary shown on the collapsed header, so the settings that
    /// matter are visible without opening the section.
    fn relay_advanced_summary(&self) -> String {
        let codec = if self.config.relay_codec.eq_ignore_ascii_case("pcm") {
            "PCM"
        } else {
            "Opus"
        };
        format!("{codec} · {} ms", self.config.relay_frame_ms)
    }
}
