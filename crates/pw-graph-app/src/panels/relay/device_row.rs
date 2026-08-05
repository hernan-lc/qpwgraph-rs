//! One device row in the Connections list.
//!
//! The row is deliberately shaped like a Bluetooth or Wi-Fi entry: a kind
//! icon, the device name over its endpoint, and a single trailing action
//! whose meaning follows the row's state (connect / cancel / disconnect).
//! Connected rows additionally carry a live level bar and expand in place to
//! show what the session negotiated, so nothing about a connection requires
//! leaving the list.

use super::super::components::{document_button, document_icon_button};
use crate::app::{QpwgraphApp, RelayDeviceRow, RelayDeviceState};
use crate::icons::Icon;
use eframe::egui::{self, Color32, RichText, Sense, Ui, Vec2};
use pw_graph_backend::{RelayCodecKind, RelayDeviceKind};
use pw_graph_ui::UiDocument;

/// Accent used for the connected group; matches the canvas link colour so
/// "carrying audio" reads the same in the panel as it does on the graph.
const CONNECTED_ACCENT: Color32 = Color32::from_rgb(96, 190, 130);
const ROW_BACKGROUND: Color32 = Color32::from_rgb(38, 44, 54);
const CONNECTED_BACKGROUND: Color32 = Color32::from_rgb(34, 52, 44);
const LEVEL_BAR_SIZE: Vec2 = Vec2::new(52.0, 6.0);
const ROW_ROUNDING: f32 = 6.0;

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
        let fill = if connected {
            CONNECTED_BACKGROUND
        } else {
            ROW_BACKGROUND
        };
        let mut action = RelayRowAction::None;
        egui::Frame::none()
            .fill(fill)
            .rounding(ROW_ROUNDING)
            .inner_margin(egui::Margin::symmetric(8.0, 6.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    self.paint_relay_device_glyph(ui, row);
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
                        action = self.show_relay_row_action(document, ui, row);
                    });
                });
                if connected && self.relay_row_expanded(row) {
                    ui.add_space(4.0);
                    self.show_relay_row_details(ui, row);
                }
            });
        action
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
                if document_button(
                    document,
                    ui,
                    &format!("relay.panel.device.cancel.{}", row.addr),
                    self.i18n.text("shortcuts.close"),
                    true,
                ) {
                    return RelayRowAction::CancelConnect;
                }
                ui.add(egui::Spinner::new().size(14.0));
                ui.label(
                    RichText::new(self.i18n.text("relay.state_connecting"))
                        .small()
                        .weak(),
                );
            }
            RelayDeviceState::Connected(session) => {
                if document_icon_button(
                    document,
                    ui,
                    &format!("relay.panel.device.disconnect.{}", session.0),
                    Icon::AutoDisconnect,
                    self.i18n.text("relay.disconnect"),
                    self.i18n.text("relay.disconnect_tip"),
                ) {
                    return RelayRowAction::Disconnect;
                }
                if document_icon_button(
                    document,
                    ui,
                    &format!("relay.panel.device.details.{}", session.0),
                    Icon::Settings,
                    self.i18n.text("relay.details_tip"),
                    self.i18n.text("relay.details_tip"),
                ) {
                    return RelayRowAction::ToggleDetails;
                }
                self.paint_relay_level_bar(ui, self.relay.levels.get(&session.0).copied());
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
        ui.label(
            RichText::new(self.tf("relay.detail_codec", &[("codec", codec.to_owned())]))
                .small()
                .weak(),
        );
        let direction = match (session.sending, session.receiving) {
            (true, true) => self.i18n.text("relay.role_both"),
            (true, false) => self.i18n.text("relay.role_emit"),
            (false, true) => self.i18n.text("relay.role_receive"),
            (false, false) => "—".to_owned(),
        };
        ui.label(
            RichText::new(self.tf("relay.detail_direction", &[("direction", direction)]))
                .small()
                .weak(),
        );
        ui.label(
            RichText::new(self.tf(
                "relay.detail_frame",
                &[("frame", self.config.relay_frame_ms.to_string())],
            ))
            .small()
            .weak(),
        );
    }

    /// Device kind badge. The kinds are drawn rather than themed with icon
    /// assets because they are glyph-sized and need to tint with the row's
    /// state.
    fn paint_relay_device_glyph(&self, ui: &mut Ui, row: &RelayDeviceRow) {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::hover());
        let painter = ui.painter();
        let color = if row.is_connected() {
            CONNECTED_ACCENT
        } else {
            ui.visuals().weak_text_color()
        };
        let stroke = egui::Stroke::new(1.4_f32, color);
        match row.kind {
            RelayDeviceKind::Android => {
                // Portrait slab with a speaker slit: a phone.
                let body = egui::Rect::from_center_size(rect.center(), Vec2::new(11.0, 17.0));
                painter.rect_stroke(body, 2.0, stroke);
                painter.line_segment(
                    [
                        egui::pos2(body.center().x - 2.5, body.top() + 3.0),
                        egui::pos2(body.center().x + 2.5, body.top() + 3.0),
                    ],
                    stroke,
                );
            }
            RelayDeviceKind::Linux => {
                // Screen over a base line: a desktop or laptop.
                let screen = egui::Rect::from_center_size(
                    rect.center() - Vec2::new(0.0, 2.0),
                    Vec2::new(16.0, 11.0),
                );
                painter.rect_stroke(screen, 2.0, stroke);
                painter.line_segment(
                    [
                        egui::pos2(rect.left() + 1.0, screen.bottom() + 4.0),
                        egui::pos2(rect.right() - 1.0, screen.bottom() + 4.0),
                    ],
                    stroke,
                );
            }
            RelayDeviceKind::Other => {
                painter.circle_stroke(rect.center(), 7.0, stroke);
            }
        }
    }

    /// Live incoming level for a connected session. An absent reading (a
    /// send-only session, or one that has not reported yet) draws the empty
    /// track rather than nothing, so rows keep a stable width.
    fn paint_relay_level_bar(&self, ui: &mut Ui, rms: Option<f32>) {
        let (rect, _) = ui.allocate_exact_size(LEVEL_BAR_SIZE, Sense::hover());
        let painter = ui.painter();
        painter.rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
        let Some(rms) = rms else {
            return;
        };
        // RMS is small for ordinary speech; a square-root curve keeps the
        // bar legible instead of pinned near zero.
        let level = rms.clamp(0.0, 1.0).sqrt();
        if level <= f32::EPSILON {
            return;
        }
        let filled =
            egui::Rect::from_min_size(rect.min, Vec2::new(rect.width() * level, rect.height()));
        painter.rect_filled(filled, 3.0, CONNECTED_ACCENT);
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
