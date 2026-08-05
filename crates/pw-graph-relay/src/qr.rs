//! QR rasterization for the relay pairing payload.
//!
//! Encoding comes from the `qrcode` crate; this module turns a payload into
//! a dependency-free [`QrBitmap`] that any UI layer can blit into a texture,
//! and that tests and the `qr-preview` example can verify without a GUI.

use qrcode::{Color, QrCode};

/// Quiet zone width in modules. Four is the QR specification minimum.
pub const DEFAULT_QUIET_MODULES: usize = 4;

/// Camera scanners need fat modules; never rasterize below this scale.
pub const MIN_MODULE_SCALE: usize = 2;

/// Dark/light bitmap of a rendered QR code, quiet zone included.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QrBitmap {
    pub width: usize,
    pub height: usize,
    /// Row-major; `true` marks a dark module pixel.
    pub dark: Vec<bool>,
}

impl QrBitmap {
    pub fn get(&self, x: usize, y: usize) -> bool {
        self.dark[y * self.width + x]
    }

    /// Render the bitmap as Unicode half-block text for terminal inspection.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        let mut y = 0;
        while y < self.height {
            for x in 0..self.width {
                let top = self.get(x, y);
                let bottom = y + 1 < self.height && self.get(x, y + 1);
                out.push(match (top, bottom) {
                    (true, true) => '█',
                    (true, false) => '▀',
                    (false, true) => '▄',
                    (false, false) => ' ',
                });
            }
            out.push('\n');
            y += 2;
        }
        out
    }

    /// Encode the bitmap as an uncompressed 24-bit BMP so verification does
    /// not need an image library.
    pub fn to_bmp(&self) -> Vec<u8> {
        let row_bytes = self.width * 3;
        let padded = row_bytes.div_ceil(4) * 4;
        let pixel_data = padded * self.height;
        let file_size = 54 + pixel_data;
        let mut bytes = Vec::with_capacity(file_size);
        bytes.extend_from_slice(b"BM");
        bytes.extend_from_slice(&(file_size as u32).to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&54u32.to_le_bytes());
        bytes.extend_from_slice(&40u32.to_le_bytes());
        bytes.extend_from_slice(&(self.width as u32).to_le_bytes());
        bytes.extend_from_slice(&(self.height as u32).to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&24u16.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&(pixel_data as u32).to_le_bytes());
        bytes.extend_from_slice(&2835u32.to_le_bytes());
        bytes.extend_from_slice(&2835u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        for y in (0..self.height).rev() {
            for x in 0..self.width {
                let value = if self.get(x, y) { 0 } else { 255 };
                bytes.extend_from_slice(&[value, value, value]);
            }
            bytes.extend(std::iter::repeat_n(0u8, padded - row_bytes));
        }
        bytes
    }
}

/// Rasterize `text` into a bitmap with `module_scale` pixels per module and
/// a `quiet_modules`-wide quiet zone. Returns `None` when the payload is too
/// long for any QR version.
pub fn render(text: &str, module_scale: usize, quiet_modules: usize) -> Option<QrBitmap> {
    let scale = module_scale.max(1);
    let code = QrCode::new(text.as_bytes()).ok()?;
    let modules = code.width();
    let side = (modules + quiet_modules * 2) * scale;
    let mut dark = vec![false; side * side];
    for y in 0..modules {
        for x in 0..modules {
            if code[(x, y)] != Color::Dark {
                continue;
            }
            for dy in 0..scale {
                for dx in 0..scale {
                    let px = (x + quiet_modules) * scale + dx;
                    let py = (y + quiet_modules) * scale + dy;
                    dark[py * side + px] = true;
                }
            }
        }
    }
    Some(QrBitmap {
        width: side,
        height: side,
        dark,
    })
}

/// The fattest integer module scale that keeps the rendered code at or below
/// `max_size` pixels, floored at [`MIN_MODULE_SCALE`].
pub fn module_scale_for(text: &str, max_size: usize) -> Option<usize> {
    let code = QrCode::new(text.as_bytes()).ok()?;
    let total_modules = code.width() + DEFAULT_QUIET_MODULES * 2;
    Some((max_size / total_modules).max(MIN_MODULE_SCALE))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAYLOAD: &str = "qpw-relay://192.168.1.20:48123?pin=123456";

    #[test]
    fn renders_with_quiet_zone_and_integer_scale() {
        let bitmap = render(PAYLOAD, 6, DEFAULT_QUIET_MODULES).unwrap();
        let code = QrCode::new(PAYLOAD.as_bytes()).unwrap();
        let side = (code.width() + DEFAULT_QUIET_MODULES * 2) * 6;
        assert_eq!(bitmap.width, side);
        assert_eq!(bitmap.height, side);
        assert_eq!(bitmap.dark.len(), side * side);
        // Quiet-zone corner stays light…
        assert!(!bitmap.get(0, 0));
        // …and the top-left finder pattern module is dark.
        let first = DEFAULT_QUIET_MODULES * 6;
        assert!(bitmap.get(first, first));
    }

    #[test]
    fn module_scale_stays_inside_the_display_budget() {
        let scale = module_scale_for(PAYLOAD, 260).unwrap();
        let bitmap = render(PAYLOAD, scale, DEFAULT_QUIET_MODULES).unwrap();
        assert!(bitmap.width <= 260, "texture would need upscaling");
        assert!(bitmap.width > 260 / 2, "code unexpectedly tiny");
    }

    #[test]
    fn module_scale_keeps_a_camera_friendly_floor() {
        // Even an unrealistically dense payload keeps at least 2px modules.
        let huge = "x".repeat(2000);
        assert_eq!(module_scale_for(&huge, 260).unwrap(), MIN_MODULE_SCALE);
    }

    #[test]
    fn rejects_unencodable_payloads() {
        assert!(render(&"x".repeat(30_000), 4, DEFAULT_QUIET_MODULES).is_none());
        assert!(module_scale_for(&"x".repeat(30_000), 260).is_none());
    }

    #[test]
    fn text_preview_draws_dark_modules() {
        let bitmap = render(PAYLOAD, 1, 0).unwrap();
        let text = bitmap.to_text();
        assert!(text.contains('█') || text.contains('▀') || text.contains('▄'));
    }

    #[test]
    fn bmp_encoding_carries_the_right_geometry() {
        let bitmap = render(PAYLOAD, 2, DEFAULT_QUIET_MODULES).unwrap();
        let bytes = bitmap.to_bmp();
        assert_eq!(&bytes[0..2], b"BM");
        let width = u32::from_le_bytes(bytes[18..22].try_into().unwrap()) as usize;
        let height = u32::from_le_bytes(bytes[22..26].try_into().unwrap()) as usize;
        assert_eq!((width, height), (bitmap.width, bitmap.height));
        assert_eq!(
            bytes.len(),
            u32::from_le_bytes(bytes[2..6].try_into().unwrap()) as usize
        );
    }
}
