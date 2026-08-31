mod args;
mod bridge;
mod canvas;
mod model;
mod names;
mod shortcuts;
mod source;
#[cfg(all(target_os = "linux", feature = "tray"))]
mod tray;

fn main() -> Result<(), slint::PlatformError> {
    bridge::UiBridge::new(args::parse_args())?.run()
}
