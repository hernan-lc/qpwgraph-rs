//! QR modal: renders the running host's connection payload (address, port,
//! and PIN) as a QR code so phones can scan it instead of typing.

use super::super::shared::{show_centered_dialog, show_close_button};
use crate::app::{QpwgraphApp, RelayUiState};
use eframe::egui::{self, Color32, ColorImage, RichText, TextureOptions, Ui};
use pw_graph_ui::UiDocument;

const QR_DIALOG_WIDTH: f32 = 380.0;
const QR_MODULE_SCALE: usize = 6;
const QR_QUIET_MODULES: usize = 4;
const QR_DISPLAY_SIZE: f32 = 260.0;

impl QpwgraphApp {
    /// Modal with the host QR, opened from the Emitter tab.
    pub(crate) fn show_relay_qr_modal(&mut self, ctx: &egui::Context) {
        if !self.relay.show_qr || !self.show_relay {
            return;
        }
        let payload = RelayUiState::qr_payload(self);
        self.ensure_relay_qr_texture(ctx, payload.as_deref());
        let mut document = std::mem::take(&mut self.ui_document);
        let response = show_centered_dialog(
            &mut document,
            ctx,
            "relay-qr",
            self.i18n.text("relay.qr_title"),
            QR_DIALOG_WIDTH,
            |ui, document| self.show_relay_qr_contents(payload.as_deref(), document, ui),
        );
        self.ui_document = document;
        if response.backdrop_clicked {
            self.relay.show_qr = false;
        }
    }

    /// Rebuild the QR texture only when the payload changes.
    fn ensure_relay_qr_texture(&mut self, ctx: &egui::Context, payload: Option<&str>) {
        let Some(payload) = payload else {
            self.relay.qr_texture = None;
            self.relay.qr_text.clear();
            return;
        };
        if self.relay.qr_text == payload && self.relay.qr_texture.is_some() {
            return;
        }
        let Some(image) = render_qr_image(payload) else {
            self.relay.qr_texture = None;
            return;
        };
        let texture = ctx.load_texture("relay-qr", image, TextureOptions::NEAREST);
        self.relay.qr_text = payload.to_owned();
        self.relay.qr_texture = Some(texture);
    }

    fn show_relay_qr_contents(
        &mut self,
        payload: Option<&str>,
        document: &mut UiDocument,
        ui: &mut Ui,
    ) {
        ui.vertical_centered(|ui| {
            let texture_ready = payload.is_some() && self.relay.qr_texture.is_some();
            if texture_ready {
                if let Some(texture) = &self.relay.qr_texture {
                    ui.add(egui::Image::new(egui::load::SizedTexture::new(
                        texture.id(),
                        egui::vec2(QR_DISPLAY_SIZE, QR_DISPLAY_SIZE),
                    )));
                }
                ui.add_space(8.0);
                if let Some(payload) = payload {
                    // Human-readable fallback: `host:port` without scheme/PIN.
                    let target = payload
                        .trim_start_matches("qpw-relay://")
                        .split('?')
                        .next()
                        .unwrap_or_default();
                    ui.label(RichText::new(target).strong());
                }
            } else {
                ui.label(RichText::new(self.i18n.text("relay.qr_unavailable")).weak());
            }
            ui.add_space(4.0);
            ui.label(
                RichText::new(self.i18n.text("relay.qr_hint"))
                    .small()
                    .weak(),
            );
        });
        ui.add_space(6.0);
        if show_close_button(
            document,
            ui,
            "relay.qr.close",
            self.i18n.text("shortcuts.close"),
        ) {
            self.relay.show_qr = false;
        }
    }
}

/// Rasterize a payload into a black-on-white image with a quiet zone.
fn render_qr_image(text: &str) -> Option<ColorImage> {
    let code = qrcode::QrCode::new(text.as_bytes()).ok()?;
    let modules = code.width();
    let side = (modules + QR_QUIET_MODULES * 2) * QR_MODULE_SCALE;
    let mut pixels = vec![Color32::WHITE; side * side];
    for y in 0..modules {
        for x in 0..modules {
            if code[(x, y)] != qrcode::Color::Dark {
                continue;
            }
            for dy in 0..QR_MODULE_SCALE {
                for dx in 0..QR_MODULE_SCALE {
                    let px = (x + QR_QUIET_MODULES) * QR_MODULE_SCALE + dx;
                    let py = (y + QR_QUIET_MODULES) * QR_MODULE_SCALE + dy;
                    pixels[py * side + px] = Color32::BLACK;
                }
            }
        }
    }
    Some(ColorImage {
        size: [side, side],
        pixels,
    })
}

#[cfg(test)]
mod tests {
    use super::render_qr_image;

    #[test]
    fn renders_a_payload_with_quiet_zone() {
        let image = render_qr_image("qpw-relay://192.168.1.20:48123?pin=123456").unwrap();
        let [width, height] = image.size;
        assert_eq!(width, height);
        assert!(width >= 25 * 6, "a version-2 code plus quiet zone");
        // Quiet zone corners stay white; the finder pattern puts dark
        // modules near the top-left inside the quiet zone.
        assert_eq!(image.pixels[0], eframe::egui::Color32::WHITE);
        let first_module = 4 * 6;
        assert_eq!(
            image.pixels[first_module * width + first_module],
            eframe::egui::Color32::BLACK
        );
    }

    #[test]
    fn rejects_unencodable_payloads() {
        // Longer than any QR version can carry.
        assert!(render_qr_image(&"x".repeat(30_000)).is_none());
    }
}
