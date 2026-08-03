# Packaging qpwgraph-rs

## GitHub releases

Pushing a tag in the form `vX.Y.Z` starts
`.github/workflows/release.yml`. The workflow requires the tag version to
match the workspace version, runs the locked workspace tests, builds the
native Linux application, and publishes a tarball plus SHA-256 checksum to
GitHub Releases. Tags with a prerelease suffix such as `v0.1.0-rc.1` are
published as prereleases.

Download the release assets with GitHub CLI and verify the archive before
installing it:

```bash
gh release download v0.1.0 \
  --pattern 'qpwgraph-rs-*.tar.gz' \
  --pattern 'qpwgraph-rs-*.tar.gz.sha256'
sha256sum --check qpwgraph-rs-0.1.0-x86_64-unknown-linux-gnu.tar.gz.sha256
tar -xzf qpwgraph-rs-0.1.0-x86_64-unknown-linux-gnu.tar.gz
```

Then install the files from the extracted release directory. Replace the
directory name below when installing another version:

```bash
release_dir=qpwgraph-rs-0.1.0-x86_64-unknown-linux-gnu
sudo install -Dm755 "$release_dir/bin/qpwgraph-rs" /usr/local/bin/qpwgraph-rs
install -Dm644 "$release_dir/share/applications/qpwgraph-rs.desktop" \
  ~/.local/share/applications/qpwgraph-rs.desktop
install -Dm644 "$release_dir/share/metainfo/io.github.qpwgraph_rs.metainfo.xml" \
  ~/.local/share/metainfo/io.github.qpwgraph_rs.metainfo.xml
```

The published binary targets `x86_64-unknown-linux-gnu` on the GitHub-hosted
Ubuntu runner. The Flatpak manifest remains available for sandboxed installs
and is not currently attached automatically to the GitHub release.

## Local installation from source

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
