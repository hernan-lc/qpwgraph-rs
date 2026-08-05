//! One device row in the Connections list.
//!
//! The row is deliberately shaped like a Bluetooth or Wi-Fi entry: a kind
//! icon, the device name over its endpoint, and a single trailing action
//! whose meaning follows the row's state (connect / cancel / disconnect).
//! Connected rows additionally carry a live level bar and expand in place to
//! show what the session negotiated, so nothing about a connection requires
//! leaving the list.
//!
//! Everything here is assembled from `pw-graph-ui` components — a card for
//! the surface, a meter for the level, icon buttons for the actions — so the
//! row inherits the shared styling instead of restating it locally.

use super::super::components::document_button;
use super::CONNECTED_ACCENT;
use crate::app::{QpwgraphApp, RelayDeviceRow, RelayDeviceState};
use crate::icons::Icon;
use eframe::egui::{self, Color32, RichText, Ui};
use pw_graph_backend::{RelayCodecKind, RelayDeviceKind};
use pw_graph_ui::{icon_image, CardProps, IconButtonProps, MeterProps, UiDocument};

const ROW_BACKGROUND: Color32 = Color32::from_rgb(38, 44, 54);
const CONNECTED_BACKGROUND: Color32 = Color32::from_rgb(34, 52, 44);
const DEVICE_ICON_SIZE: f32 = 20.0;

/// What the user asked the row to do, resolved by the caller so this module
/// stays free of engine access.
pub(super) enum RelayRowAction {
    None,
    Connect,
    CancelConnect,
    Disconnect,
    ToggleDetails,
}

impl QpwgraphApp {
    pub(super) fn show_relay_device_row(
        &self,
        document: &mut UiDocument,
        ui: &mut Ui,
        row: &RelayDeviceRow,
    ) -> RelayRowAction {
        let connected = row.is_connected();
        let expanded = self.relay_row_expanded(row);
        let card = CardProps::new(format!("relay.panel.device.card.{}", row.addr))
            .fill(if connected {
                CONNECTED_BACKGROUND
            } else {
                ROW_BACKGROUND
            })
            .accent_option(connected.then_some(CONNECTED_ACCENT));
        document
            .card(ui, card, |ui, document| {
                let action = ui
                    .horizontal(|ui| {
                        let tint = if connected {
                            CONNECTED_ACCENT
                        } else {
                            ui.visuals().weak_text_color()
                        };
                        ui.add(icon_image(
                            &relay_device_icon(row.kind).source(),
                            DEVICE_ICON_SIZE,
                            tint,
                        ));
                        ui.add_space(6.0);
                        ui.vertical(|ui| {
                            ui.label(RichText::new(row.name.clone()).strong());
                            ui.label(
                                RichText::new(format!(
                                    "{} · {}",
                                    self.relay_peer_kind_label(row.kind),
                                    row.addr
                                ))
                                .small()
                                .weak(),
                            );
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            self.show_relay_row_action(document, ui, row)
                        })
                        .inner
                    })
                    .inner;
                if expanded {
                    self.show_relay_row_details(ui, row);
                }
                action
            })
            .unwrap_or(RelayRowAction::None)
    }

    /// Trailing controls. Connected rows lead with the level bar so the eye
    /// lands on "is audio flowing?" before the destructive action.
    fn show_relay_row_action(
        &self,
        document: &mut UiDocument,
        ui: &mut Ui,
        row: &RelayDeviceRow,
    ) -> RelayRowAction {
        match row.state {
            RelayDeviceState::Available => {
                if document_button(
                    document,
                    ui,
                    &format!("relay.panel.device.connect.{}", row.addr),
                    self.i18n.text("relay.connect"),
                    true,
                ) {
                    return RelayRowAction::Connect;
                }
            }
            RelayDeviceState::Connecting => {
                if document.icon_button(
                    ui,
                    IconButtonProps::new(
                        format!("relay.panel.device.cancel.{}", row.addr),
                        Icon::Close.source(),
                    )
                    .tooltip(self.i18n.text("relay.cancel_connect")),
                ) {
                    return RelayRowAction::CancelConnect;
                }
                document.activity(
                    ui,
                    format!("relay.panel.device.connecting.{}", row.addr),
                    self.i18n.text("relay.state_connecting"),
                );
            }
            RelayDeviceState::Connected(session) => {
                if document.icon_button(
                    ui,
                    IconButtonProps::new(
                        format!("relay.panel.device.disconnect.{}", session.0),
                        Icon::AutoDisconnect.source(),
                    )
                    .tooltip(self.i18n.text("relay.disconnect_tip")),
                ) {
                    return RelayRowAction::Disconnect;
                }
                if document.icon_button(
                    ui,
                    IconButtonProps::new(
                        format!("relay.panel.device.details.{}", session.0),
                        Icon::Settings.source(),
                    )
                    .tooltip(self.i18n.text("relay.details_tip")),
                ) {
                    return RelayRowAction::ToggleDetails;
                }
                document.meter(
                    ui,
                    MeterProps::new(format!("relay.panel.device.level.{}", session.0))
                        .level_option(self.relay.levels.get(&session.0).copied())
                        .color(CONNECTED_ACCENT)
                        .tooltip(self.i18n.text("relay.level_tip")),
                );
            }
        }
        RelayRowAction::None
    }

    fn relay_row_expanded(&self, row: &RelayDeviceRow) -> bool {
        match row.state {
            RelayDeviceState::Connected(session) => self.relay.expanded == Some(session.0),
            _ => false,
        }
    }

    /// What the session actually negotiated. Shown on demand because it is
    /// diagnostic detail, not something to scan past on every row.
    fn show_relay_row_details(&self, ui: &mut Ui, row: &RelayDeviceRow) {
        let Some(session) = &row.session else {
            return;
        };
        ui.separator();
        let codec = match session.codec {
            RelayCodecKind::Opus => "Opus",
            RelayCodecKind::Pcm => "PCM",
        };
        let direction = match (session.sending, session.receiving) {
            (true, true) => self.i18n.text("relay.role_both"),
            (true, false) => self.i18n.text("relay.role_emit"),
            (false, true) => self.i18n.text("relay.role_receive"),
            (false, false) => "—".to_owned(),
        };
        for line in [
            self.tf("relay.detail_codec", &[("codec", codec.to_owned())]),
            self.tf("relay.detail_direction", &[("direction", direction)]),
            self.tf(
                "relay.detail_frame",
                &[("frame", self.config.relay_frame_ms.to_string())],
            ),
        ] {
            ui.label(RichText::new(line).small().weak());
        }
    }

    pub(super) fn relay_peer_kind_label(&self, kind: RelayDeviceKind) -> String {
        let key = match kind {
            RelayDeviceKind::Android => "relay.peer_kind_android",
            RelayDeviceKind::Linux => "relay.peer_kind_linux",
            RelayDeviceKind::Other => "relay.peer_kind_other",
        };
        self.i18n.text(key)
    }
}

/// Device-kind artwork. Real SVGs rather than painted primitives, so the
/// badge carries the same weight and tinting as every other icon in the app.
fn relay_device_icon(kind: RelayDeviceKind) -> Icon {
    match kind {
        RelayDeviceKind::Android => Icon::DevicePhone,
        RelayDeviceKind::Linux => Icon::DeviceDesktop,
        RelayDeviceKind::Other => Icon::DeviceGeneric,
    }
}
