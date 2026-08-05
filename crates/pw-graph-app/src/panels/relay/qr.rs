//! QR modal: renders the running host's connection payload (address, port,
//! and PIN) as a QR code so phones can scan it instead of typing.

use super::super::shared::{show_centered_dialog, show_close_button};
use crate::app::{QpwgraphApp, RelayUiState};
use eframe::egui::{self, Color32, ColorImage, RichText, TextureOptions, Ui};
use pw_graph_backend::relay_parse_qr_payload;
use pw_graph_ui::UiDocument;

const QR_DIALOG_WIDTH: f32 = 380.0;
const QR_QUIET_MODULES: usize = 4;
const QR_DISPLAY_SIZE: f32 = 260.0;
/// Smallest module edge in texture pixels; camera scanners need fat modules.
const QR_MIN_MODULE_SCALE: usize = 2;

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
                    // Show the texture at its native pixel size: it was
                    // rasterized at an integer module scale, so any resize
                    // here would re-introduce uneven modules.
                    let size = texture.size_vec2();
                    ui.add(egui::Image::new(egui::load::SizedTexture::new(
                        texture.id(),
                        size,
                    )));
                }
                ui.add_space(8.0);
                if let Some(payload) = payload {
                    // Human-readable fallback for phones that cannot scan:
                    // the endpoint and PIN as text.
                    if let Some(parsed) = relay_parse_qr_payload(payload) {
                        ui.label(RichText::new(parsed.target).monospace().strong());
                        if let Some(pin) = parsed.pin {
                            ui.label(self.tf("relay.qr_pin", &[("pin", pin)]));
                        }
                    }
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

/// Pixels per module so the rasterized code lands as close to the display
/// size as an integer scale allows. Non-integer scaling of QR textures is
/// what makes modules render uneven and breaks phone scanners.
fn qr_module_scale(total_modules: usize) -> usize {
    (QR_DISPLAY_SIZE as usize / total_modules).max(QR_MIN_MODULE_SCALE)
}

/// Rasterize a payload into a black-on-white image with a quiet zone.
fn render_qr_image(text: &str) -> Option<ColorImage> {
    let code = qrcode::QrCode::new(text.as_bytes()).ok()?;
    let modules = code.width();
    let scale = qr_module_scale(modules + QR_QUIET_MODULES * 2);
    let side = (modules + QR_QUIET_MODULES * 2) * scale;
    let mut pixels = vec![Color32::WHITE; side * side];
    for y in 0..modules {
        for x in 0..modules {
            if code[(x, y)] != qrcode::Color::Dark {
                continue;
            }
            for dy in 0..scale {
                for dx in 0..scale {
                    let px = (x + QR_QUIET_MODULES) * scale + dx;
                    let py = (y + QR_QUIET_MODULES) * scale + dy;
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
    use super::{qr_module_scale, render_qr_image, QR_DISPLAY_SIZE, QR_QUIET_MODULES};

    #[test]
    fn renders_a_payload_with_quiet_zone() {
        let payload = "qpw-relay://192.168.1.20:48123?pin=123456";
        let image = render_qr_image(payload).unwrap();
        let code = qrcode::QrCode::new(payload.as_bytes()).unwrap();
        let total_modules = code.width() + QR_QUIET_MODULES * 2;
        let scale = qr_module_scale(total_modules);
        let [width, height] = image.size;
        assert_eq!(width, height);
        assert_eq!(width, total_modules * scale, "integer module scale");
        assert!(
            width <= QR_DISPLAY_SIZE as usize,
            "the texture must not be upscaled at display time"
        );
        // Quiet zone corners stay white; the finder pattern puts dark
        // modules near the top-left inside the quiet zone.
        assert_eq!(image.pixels[0], eframe::egui::Color32::WHITE);
        let first_module = QR_QUIET_MODULES * scale;
        assert_eq!(
            image.pixels[first_module * width + first_module],
            eframe::egui::Color32::BLACK
        );
    }

    #[test]
    fn module_scale_keeps_a_floor_for_camera_scanners() {
        // Even an unrealistically dense code keeps at least 2px modules.
        assert_eq!(qr_module_scale(10_000), 2);
        // A typical version-3 payload code fills most of the display size.
        let scale = qr_module_scale(29 + QR_QUIET_MODULES * 2);
        assert!(scale >= 6, "expected fat modules, got {scale}px");
    }

    #[test]
    fn rejects_unencodable_payloads() {
        // Longer than any QR version can carry.
        assert!(render_qr_image(&"x".repeat(30_000)).is_none());
    }
}
