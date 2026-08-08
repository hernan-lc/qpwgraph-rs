# Packaging qpwgraph-rs

## GitHub releases

Pushing a tag in the form `vX.Y.Z` starts
`.github/workflows/release.yml`. The workflow requires the tag version to
match the workspace version, runs the locked workspace tests, and publishes
the native tarball, a standalone Flatpak bundle, and an AppImage to GitHub
Releases. A `SHA256SUMS` file is generated for all release assets. Tags with a
prerelease suffix such as `v0.1.0-rc.1` are published as prereleases.

Download the release assets with GitHub CLI and verify the archive before
installing it:

```bash
gh release download v0.1.0 \
  --pattern 'qpwgraph-rs-*.tar.gz' \
  --pattern 'qpwgraph-rs-*.flatpak' \
  --pattern 'qpwgraph-rs-*.AppImage' \
  --pattern 'SHA256SUMS'
sha256sum --ignore-missing --check SHA256SUMS
flatpak --user install ./qpwgraph-rs-0.1.0-x86_64.flatpak
chmod +x ./qpwgraph-rs-0.1.0-x86_64.AppImage
./qpwgraph-rs-0.1.0-x86_64.AppImage
tar -xzf qpwgraph-rs-0.1.0-x86_64-unknown-linux-gnu.tar.gz
```

Then install the files from the extracted release directory. Replace the
directory name below when installing another version:

```bash
release_dir=qpwgraph-rs-0.1.0-x86_64-unknown-linux-gnu
sudo install -Dm755 "$release_dir/bin/qpwgraph-rs" /usr/local/bin/qpwgraph-rs
install -Dm644 "$release_dir/share/applications/io.github.nglmercer.qpwgraph-rs.desktop" \
  ~/.local/share/applications/io.github.nglmercer.qpwgraph-rs.desktop
install -Dm644 "$release_dir/share/metainfo/io.github.nglmercer.qpwgraph-rs.metainfo.xml" \
  ~/.local/share/metainfo/io.github.nglmercer.qpwgraph-rs.metainfo.xml
```

The published binary targets `x86_64-unknown-linux-gnu` on the GitHub-hosted
Ubuntu runner. The Flatpak bundle uses the App ID
`io.github.nglmercer.qpwgraph-rs` and can be installed without adding a
third-party application catalog:

```bash
flatpak --user install ./qpwgraph-rs-0.1.0-x86_64.flatpak
flatpak run io.github.nglmercer.qpwgraph-rs
```

The AppImage is portable and does not require an installation step. It still
connects to the host PipeWire service and ALSA devices, just like the native
build.

## Local installation from source

The release binary is built from the workspace root with:

```bash
cargo build --release -p pw-graph-app
install -Dm755 target/release/qpwgraph-rs ~/.local/bin/qpwgraph-rs
install -Dm644 packaging/io.github.nglmercer.qpwgraph-rs.desktop \
  ~/.local/share/applications/io.github.nglmercer.qpwgraph-rs.desktop
install -Dm644 packaging/io.github.nglmercer.qpwgraph-rs.metainfo.xml \
  ~/.local/share/metainfo/io.github.nglmercer.qpwgraph-rs.metainfo.xml
```

Native builds need Rust, CMake, Clang/libclang, PipeWire development headers
and libraries, ALSA Sequencer development headers and libraries, Opus, and the
native file-dialog dependencies used by `rfd`. Build without either native backend
when those libraries are unavailable:

```bash
cargo build --release -p pw-graph-app --no-default-features
cargo build --release -p pw-graph-app --no-default-features --features pipewire
```

The Flatpak manifest is in
`packaging/io.github.nglmercer.qpwgraph-rs.yml`. It expects the Freedesktop
SDK/runtime and uses the workspace lockfile plus vendored Cargo sources for a
reproducible dependency resolution. Build a local bundle with:

```bash
flatpak-builder --force-clean --repo=repo builddir \
  packaging/io.github.nglmercer.qpwgraph-rs.yml
flatpak build-bundle repo qpwgraph-rs-0.1.0-x86_64.flatpak \
  io.github.nglmercer.qpwgraph-rs stable \
  --runtime-repo=https://releases.freedesktop-sdk.io/freedesktop-sdk.flatpakrepo
```

`flatpakref` files are references to a hosted Flatpak repository; they are not
self-contained installers. For GitHub Releases the standalone `.flatpak`
bundle is the correct artifact. A `.flatpakref` can be added later if a
separate static Flatpak repository is hosted.

Build an AppImage locally after building the release binary by downloading the
official `linuxdeploy` AppImage and running:

```bash
bash packaging/appimage/build-appimage.sh 0.1.0 ./linuxdeploy-x86_64.AppImage
```
