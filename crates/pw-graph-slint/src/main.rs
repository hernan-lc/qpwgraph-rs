mod args;
mod bridge;
mod canvas;
mod model;
mod source;

fn main() -> Result<(), slint::PlatformError> {
    bridge::UiBridge::new(args::parse_args())?.run()
}
