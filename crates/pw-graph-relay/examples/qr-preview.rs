//! Standalone QR verification for the relay pairing payload.
//!
//! Renders the exact payload the desktop QR modal shows — without any GUI —
//! so encoding and rasterization can be inspected first:
//!
//! ```bash
//! cargo run -p pw-graph-relay --example qr-preview
//! cargo run -p pw-graph-relay --example qr-preview -- "qpw-relay://192.168.1.20:48123?pin=123456"
//! ```
//!
//! Prints a Unicode preview to the terminal and writes `qr-preview.bmp`
//! next to where it runs, which any image viewer (or phone camera, after
//! zooming) can open.

use pw_graph_relay::qr::{module_scale_for, render, DEFAULT_QUIET_MODULES};

const DISPLAY_SIZE: usize = 260;

fn main() {
    let payload = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "qpw-relay://192.168.1.20:48123?pin=123456".to_owned());

    let Some(scale) = module_scale_for(&payload, DISPLAY_SIZE) else {
        eprintln!("payload too long for any QR version");
        std::process::exit(1);
    };
    let Some(bitmap) = render(&payload, scale, DEFAULT_QUIET_MODULES) else {
        eprintln!("failed to encode payload");
        std::process::exit(1);
    };

    println!("payload : {payload}");
    println!(
        "modules : {}x{} code, {scale}px/module, {}px quiet zone",
        bitmap.width / scale - DEFAULT_QUIET_MODULES * 2,
        bitmap.height / scale - DEFAULT_QUIET_MODULES * 2,
        DEFAULT_QUIET_MODULES,
    );
    println!(
        "texture : {}x{} px (display budget {DISPLAY_SIZE}px)",
        bitmap.width, bitmap.height
    );
    println!();
    println!("{}", bitmap.to_text());

    let path = std::path::Path::new("qr-preview.bmp");
    std::fs::write(path, bitmap.to_bmp()).expect("writing qr-preview.bmp");
    println!("saved {}", path.display());
}
