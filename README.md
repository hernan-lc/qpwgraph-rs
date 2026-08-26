# qpwgraph-rs

Slint desktop patchbay for PipeWire, with optional ALSA Sequencer MIDI,
effects, metering, patchbay persistence, and audio relay support.

https://github.com/user-attachments/assets/d7a9b1d4-d6d3-4ef2-b0d1-4cfc2de64650

## Features

- PipeWire and ALSA MIDI graphs in one view, with invalid cross-backend links
  rejected.
- Easy grouped-channel and Advanced individual-port connection modes.
- Multi-selection, box selection, node movement, arrange, minimap, thumbnails,
  search, media filters, overlap avoidance, and connect-through behavior.
- Undo, redo, command history, and atomic grouped graph operations.
- qpwgraph-compatible `.qpwgraph`, `.xml`, and JSON patchbay files, including
  autosave, profiles, recent files, rules, activation, and stable endpoint
  restoration.
- Node names, colors, collapsed state, positions, preferences, and effect
  instances persisted in the shared application configuration.
- Built-in effect gallery with routed insertion, standalone nodes, every
  parameter, bypass, restoration, and cleanup.
- Disabled, on-demand, and always-on audio metering.
- Optional relay host, discovery, client sessions, QR pairing, and virtual
  relay nodes.
- English, Spanish, and French localization.
- Linux tray integration, start-minimized mode, native file dialogs, Nix,
  Flatpak, and AppImage packaging.

The production UI is Slint and the canonical executable is always
`qpwgraph-rs`.

## Workspace

The code is split into focused crates:

- `pw-graph-core`: graph models, stable endpoint keys, validation, and layout.
- `pw-graph-effects`: realtime-safe effect processor API and built-in effects.
- `pw-graph-backend`: driver abstraction, demo backend, native PipeWire graph,
  audio controls, and metering.
- `pw-graph-alsamidi`: ALSA Sequencer enumeration and routing.
- `pw-graph-command`: undoable graph commands and command history.
- `pw-graph-patchbay`: qpwgraph-compatible persistence and activation.
- `pw-graph-config`: TOML settings, compatibility preservation, and XDG paths.
- `pw-graph-i18n`: localized message catalogs.
- `pw-graph-app-core`: framework-neutral composite application driver.
- `pw-graph-app`: canonical Slint application shell and UI bridge.

Shared SVG assets live in [`assets/icons`](assets/icons). The
`pw-graph-app-core` crate owns the framework-neutral composite backend boundary;
the canonical `pw-graph-app` bridge owns application commands, patchbay
synchronization, effects, relay, configuration, metering policy, and
persistence. The Slint shell displays that state and sends intents through the
bridge.

## Run

```bash
cargo run -p pw-graph-app
cargo run -p pw-graph-app -- --demo
cargo run -p pw-graph-app -- --lang es
```

Without `--demo`, the application uses the available native backends. A missing
live backend is reported in the status bar and leaves the graph empty.

## CLI

```text
-m, --minimized       start minimized
-d, --debug           enable debug logging
-n, --no-alsa-midi    disable the optional ALSA MIDI backend
    --lang <LANG>     set the UI language (`en`, `es`, or `fr`)
    --demo            use the deterministic demo backend
```

Press F1 for the complete shortcut list. Graph shortcuts are ignored while a
text input owns keyboard focus.

## Native builds

The default build enables PipeWire, ALSA MIDI, relay, and Linux tray support.
Native development headers are required for the corresponding features.

```bash
cargo build --release -p pw-graph-app
cargo build --release -p pw-graph-app --no-default-features
cargo build --release -p pw-graph-app --no-default-features --features pipewire
cargo build --release -p pw-graph-app --no-default-features --features alsa
```

The backend IDs are kept separate internally, so PipeWire-to-PipeWire and
ALSA-to-ALSA routing use their correct driver while cross-backend requests are
rejected.

## Configuration and patchbay files

The application reads the existing qpwgraph-rs TOML configuration and writes
it back without discarding unknown fields. Node positions and appearance use
stable numeric/name keys. Volume and mute are live controls and are not
silently restored at startup.

Patchbay files retain the qpwgraph XML shape for `.qpwgraph` and `.xml` files;
other extensions use JSON. Save/load use native dialogs. The active path,
recent files, named profiles, editable rules, auto-pin, exclusive activation,
auto-disconnect, and startup activation are persisted. Live graph changes,
undo, and redo keep the saved patchbay state synchronized.

## Effects and metering

Open **Effects** to create a standalone processing node or insert an effect
into a selected audio link. Effect parameters, bypass state, stable routing,
positions, and restoration are persisted. Startup restores standalone effects,
activates the patchbay when configured, and then restores routed effects.

Audio meters can be **Disabled**, **OnDemand**, or **Always**. On-demand helper
streams are requested only for visible graph nodes and released when the window
is hidden or minimized. **Reset audio config** releases all meter streams.

## Audio relay

The relay panel supports host start/stop, discovery, peer connection and
disconnection, configurable role/codec/frame/transport, QR payload generation
and parsing, local endpoint discovery, level updates, and virtual relay graph
nodes. It is enabled by the default `relay` feature:

```bash
cargo run -p pw-graph-app --features relay
cargo run -p pw-graph-app --no-default-features --features pipewire,relay
```

## Nix

```bash
nix develop
nix build
nix run
nix flake check
```

`nix run` launches the Slint application and the default package installs the
`qpwgraph-rs` executable.

## Packaging and releases

The release workflow builds the canonical binary for the native tarball,
Flatpak, and AppImage. Local packaging instructions are in
[`packaging/README.md`](packaging/README.md).

For an AppImage, build the release binary and run:

```bash
bash packaging/appimage/build-appimage.sh 0.1.0 ./linuxdeploy-x86_64.AppImage
```

For Flatpak:

```bash
flatpak-builder --force-clean --repo=repo builddir \
  packaging/io.github.nglmercer.qpwgraph-rs.yml
flatpak build-bundle repo qpwgraph-rs-0.1.0-x86_64.flatpak \
  io.github.nglmercer.qpwgraph-rs stable \
  --runtime-repo=https://releases.freedesktop-sdk.io/freedesktop-sdk.flatpakrepo
```

## Development checks

```bash
cargo fmt --all -- --check
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo build --release --locked
```
