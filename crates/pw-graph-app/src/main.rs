mod app;
mod args;
mod backend;
mod icons;
mod panels;
#[cfg(all(target_os = "linux", feature = "tray"))]
mod tray;

fn main() -> eframe::Result<()> {
    app::run(args::parse_args())
}
