mod args;
mod bridge;
mod model;
mod source;

fn main() -> Result<(), slint::PlatformError> {
    bridge::UiBridge::new(args::parse_args())?.run()
}
