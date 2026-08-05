//! Advanced connection settings, formerly the Receiver tab.
//!
//! Role, codec, frame duration, and preferred link apply to the next
//! connection this device makes. They were a whole tab before, which put
//! four rarely-touched selects in front of the common "pick a device and
//! connect" path; they now live in a disclosure under the device list.
//!
//! Inside the disclosure each option is one line — label, then value — with
//! its explanation on hover. The section as a whole is introduced once by
//! `relay.advanced_help`; repeating a paragraph under every select would make
//! four settings taller than the device list they belong to.

use super::super::components::{document_compact_number, document_compact_select};
use super::{RELAY_NUMBER_WIDTH, RELAY_SELECT_WIDTH};
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
            ui.add_space(4.0);
            self.config.relay_role = document_compact_select(
                document,
                ui,
                "relay.panel.client.role",
                &self.config.relay_role,
                self.i18n.text("relay.role"),
                self.i18n.text("relay.role_help"),
                [
                    OptionItem::new("emit", self.i18n.text("relay.role_emit")),
                    OptionItem::new("receive", self.i18n.text("relay.role_receive")),
                    OptionItem::new("both", self.i18n.text("relay.role_both")),
                ],
                RELAY_SELECT_WIDTH,
            );
            self.config.relay_codec = document_compact_select(
                document,
                ui,
                "relay.panel.client.codec",
                &self.config.relay_codec,
                self.i18n.text("relay.codec"),
                self.i18n.text("relay.codec_help"),
                [
                    OptionItem::new("opus", "Opus"),
                    OptionItem::new("pcm", "PCM"),
                ],
                RELAY_SELECT_WIDTH,
            );
            self.config.relay_frame_ms = document_compact_number(
                document,
                ui,
                "relay.panel.client.frame_ms",
                self.config.relay_frame_ms as f32,
                5.0,
                60.0,
                5.0,
                self.i18n.text("relay.frame_ms"),
                self.i18n.text("relay.frame_ms_help"),
                RELAY_NUMBER_WIDTH,
            ) as u16;
            self.config.relay_transport = document_compact_select(
                document,
                ui,
                "relay.panel.client.transport",
                &self.config.relay_transport,
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
