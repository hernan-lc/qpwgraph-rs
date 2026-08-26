# qpwgraph-rs

Slint desktop audio graph and control UI for native PipeWire and Windows Core
Audio backends, with optional ALSA Sequencer MIDI, effects, metering, patchbay
persistence, and Linux audio relay support.

https://github.com/user-attachments/assets/d7a9b1d4-d6d3-4ef2-b0d1-4cfc2de64650

## Features

- PipeWire and ALSA MIDI graphs in one view on Linux, with invalid
  cross-backend links rejected.
- Windows playback/capture endpoints and active application audio sessions,
  with endpoint/session volume, mute, and endpoint peak metering.
- Native Windows Core Audio notifications for endpoint and session changes.
- Windows graph relationships are informational: arbitrary system-wide audio
  routing is not exposed as a mutable patchbay.
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
  Flatpak, AppImage, and portable Windows ZIP packaging.

The production UI is Slint and the canonical executable is always
`qpwgraph-rs`.

## Workspace

The code is split into focused crates:

- `pw-graph-core`: graph models, stable endpoint keys, validation, and layout.
- `pw-graph-effects`: realtime-safe effect processor API and built-in effects.
- `pw-graph-backend`: driver abstraction, demo backend, native PipeWire graph,
  Windows Core Audio endpoint/session graph, audio controls, and metering.
- `pw-graph-alsamidi`: ALSA Sequencer enumeration and routing.
- `pw-graph-command`: undoable graph commands and command history.
- `pw-graph-patchbay`: qpwgraph-compatible persistence and activation.
- `pw-graph-config`: TOML settings, compatibility preservation, and native
  platform configuration paths.
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

Without `--demo`, the application uses the available native backend. A missing
live backend is reported in the status bar and leaves the graph empty.

## CLI

```text
-m, --minimized       start minimized
-d, --debug           enable debug logging
-n, --no-midi          disable the optional MIDI backend (`--no-alsa-midi` remains an alias)
    --lang <LANG>     set the UI language (`en`, `es`, or `fr`)
    --demo            use the deterministic demo backend
```

Press F1 for the complete shortcut list. Graph shortcuts are ignored while a
text input owns keyboard focus.

## Native builds

On Linux, the default build enables PipeWire, ALSA MIDI, relay, and tray
support. On Windows, PipeWire and ALSA are not built or required; the native
backend uses Windows Core Audio/WASAPI through a dedicated COM worker thread.

```bash
cargo build --release -p pw-graph-app
cargo build --release -p pw-graph-app --no-default-features
cargo build --release -p pw-graph-app --no-default-features --features pipewire
cargo build --release -p pw-graph-app --no-default-features --features alsa
```

On Windows, the standard MSVC commands are:

```powershell
cargo run -p pw-graph-app -- --demo
cargo build --release --locked -p pw-graph-app
```

Graph IDs use explicit backend namespaces, so each native driver receives only
resources it owns. Linux PipeWire/ALSA routing remains mutable; Windows
endpoint/session relationships are observed but connection and disconnection
requests report unsupported.

## Configuration and patchbay files

The application reads the existing qpwgraph-rs TOML configuration and writes
it back without discarding unknown fields. Node positions and appearance use
stable numeric/name keys. Volume and mute are live controls and are not
silently restored at startup. Configuration is stored under
`~/.config/qpwgraph-rs` on Linux and `%APPDATA%\qpwgraph-rs` on Windows.

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
streams are requested only for visible PipeWire graph nodes and released when
the window is hidden or minimized. Windows uses Core Audio endpoint peak
readings where available; its legacy RMS field remains zero because Core Audio
does not provide an equivalent RMS value. **Reset audio config** releases all
meter streams.

## Audio relay

On Linux, the relay panel supports host start/stop, discovery, peer connection and
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

Tagged releases also publish a portable Windows artifact named
`qpwgraph-rs-X.Y.Z-x86_64-pc-windows-msvc.zip` containing the executable,
README, and license.

## Platform capabilities

| Feature | Linux | Windows |
| --- | --- | --- |
| Audio devices | PipeWire | Core Audio |
| Audio sessions | PipeWire nodes | Core Audio sessions |
| Arbitrary patch routing | Yes | No / future |
| Volume, mute, and metering | Yes | Yes, peak metering where available |
| Effects | Yes | Future |
| ALSA MIDI | Yes | N/A |
| Relay | Yes | Future |

## Development checks

```bash
cargo fmt --all -- --check
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo build --release --locked
```
