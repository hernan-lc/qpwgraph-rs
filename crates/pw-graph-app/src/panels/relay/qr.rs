//! QR modal: renders the running host's connection payload (address, port,
//! and PIN) as a QR code so phones can scan it instead of typing.

use super::super::shared::{show_centered_dialog, show_close_button};
use crate::app::{QpwgraphApp, RelayUiState};
use eframe::egui::{self, Color32, ColorImage, RichText, TextureOptions, Ui};
use pw_graph_backend::{relay_parse_qr_payload, relay_qr};
use pw_graph_ui::UiDocument;

const QR_DIALOG_WIDTH: f32 = 380.0;
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
                        ui.label(
                            RichText::new(parsed.target)
                                .monospace()
                                .strong()
                                .color(Color32::from_rgb(240, 244, 250)),
                        );
                        if let Some(pin) = parsed.pin {
                            ui.label(
                                RichText::new(self.tf("relay.qr_pin", &[("pin", pin)]))
                                    .color(Color32::from_rgb(215, 225, 238)),
                            );
                        }
                    }
                }
            } else if self.driver.relay_status().host_active {
                // The host is already listening; only the link is missing, so
                // do not tell the user to start the host again.
                ui.label(
                    RichText::new(self.i18n.text("relay.qr_no_link"))
                        .color(Color32::from_rgb(180, 195, 215)),
                );
            } else {
                ui.label(
                    RichText::new(self.i18n.text("relay.qr_unavailable"))
                        .color(Color32::from_rgb(180, 195, 215)),
                );
            }
            ui.add_space(4.0);
            ui.label(
                RichText::new(self.i18n.text("relay.qr_hint"))
                    .small()
                    .color(Color32::from_rgb(180, 195, 215)),
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

/// Rasterize a payload into a black-on-white egui image. The QR encoding and
/// module math live in `pw_graph_relay::qr` (verified by its own tests and
/// the `qr-preview` example); this is only the texture-format glue.
fn render_qr_image(text: &str) -> Option<ColorImage> {
    let scale = relay_qr::module_scale_for(text, QR_DISPLAY_SIZE as usize)?;
    let bitmap = relay_qr::render(text, scale, relay_qr::DEFAULT_QUIET_MODULES)?;
    let pixels = bitmap
        .dark
        .iter()
        .map(|dark| {
            if *dark {
                Color32::BLACK
            } else {
                Color32::WHITE
            }
        })
        .collect();
    Some(ColorImage {
        size: [bitmap.width, bitmap.height],
        pixels,
    })
}

#[cfg(test)]
mod tests {
    use super::{render_qr_image, QR_DISPLAY_SIZE};

    #[test]
    fn renders_a_payload_that_fits_the_display_budget() {
        let image = render_qr_image("qpw-relay://192.168.1.20:48123?pin=123456").unwrap();
        let [width, height] = image.size;
        assert_eq!(width, height);
        assert!(
            width <= QR_DISPLAY_SIZE as usize,
            "the texture must not be upscaled at display time"
        );
        // Quiet zone corners stay white; the finder pattern puts dark
        // modules near the top-left inside the quiet zone.
        assert_eq!(image.pixels[0], eframe::egui::Color32::WHITE);
        let has_dark = image.pixels.contains(&eframe::egui::Color32::BLACK);
        assert!(has_dark, "an all-white texture would scan as nothing");
    }

    #[test]
    fn rejects_unencodable_payloads() {
        // Longer than any QR version can carry.
        assert!(render_qr_image(&"x".repeat(30_000)).is_none());
    }
}
