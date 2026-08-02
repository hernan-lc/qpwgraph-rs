# Packaging qpwgraph-rs

The release binary is built from the workspace root with:

```bash
cargo build --release -p pw-graph-app
install -Dm755 target/release/qpwgraph-rs ~/.local/bin/qpwgraph-rs
install -Dm644 packaging/qpwgraph-rs.desktop ~/.local/share/applications/qpwgraph-rs.desktop
install -Dm644 packaging/io.github.qpwgraph_rs.metainfo.xml ~/.local/share/metainfo/io.github.qpwgraph_rs.metainfo.xml
```

Native builds need Rust, PipeWire development headers and libraries, ALSA
Sequencer development headers and libraries, and the native file-dialog
dependencies used by `rfd`. Build without either native backend
when those libraries are unavailable:

```bash
cargo build --release -p pw-graph-app --no-default-features
cargo build --release -p pw-graph-app --no-default-features --features pipewire
```

The Flatpak manifest is in `io.github.qpwgraph_rs.yml`. It expects a Flatpak
SDK/runtime, grants PipeWire/session-bus access, and uses the workspace lockfile
for reproducible dependency resolution. Linux desktop environments without a
StatusNotifier host can still run the application; the tray integration simply
disables itself.
