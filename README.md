# qpwgraph-rs

Rust/egui patchbay for PipeWire, with optional ALSA Sequencer MIDI support.

## Workspace

The code is split into small crates:

- `pw-graph-core`: serializable nodes, ports, links, validation, and layout.
- `pw-graph-effects`: realtime-safe effect processor API, built-in noise gate,
  and the versioned WASM module ABI.
- `pw-graph-backend`: the driver abstraction, deterministic Demo backend,
  native PipeWire registry/link backend, and optional audio meters.
- `pw-graph-alsamidi`: native ALSA Sequencer enumeration and routing.
- `pw-graph-command`: undoable connect, grouped connect, disconnect,
  disconnect-all, and node-layout commands.
- `pw-graph-patchbay`: qpwgraph-compatible XML and JSON persistence/activation.
- `pw-graph-config`: TOML settings and XDG paths.
- `pw-graph-i18n`: English, Spanish, and French catalogs with English fallback.
- `pw-graph-ui`: the egui canvas plus reusable DOM-like controls and retained
  forms. Canvas rendering and interaction are split into
  `canvas/{mod,node,links,ports,names,geometry}.rs`; component usage is in
  [`docs/ui-components.md`](docs/ui-components.md).
- `pw-graph-app`: desktop shell, backend composition, panels, tray, and CLI.

## Interface

The application uses a fixed navigation rail for common graph/history actions
and a Preferences modal with Interface and Patchbay tabs. The graph itself is
rendered in the main area; there is no separate Graph/Patchbay screen or right
inspector panel.

The rail provides refresh, undo/redo, Easy/Advanced connect mode, layout
actions, filters, patchbay actions, and **Disconnect all**. Disconnect all is
one undoable command and removes the live connections from the saved patchbay
rules just like disconnecting an individual link.

The graph refreshes automatically when PipeWire registry events arrive. The
search field filters nodes and ports by name; right-clicking a node offers
node-local disconnect and arrange-selection actions. Dragging a group or a
node creates one undoable transaction.

Easy mode groups compatible audio channels. PipeWire ports use the backend's
`audio.channel` metadata when available; demo and ALSA ports use a conservative
name-suffix fallback. Advanced mode always renders one row per port.

Node names are displayed using read-only aliases. Native PipeWire rename is not
exposed because client-owned names cannot be changed safely by the graph UI.

### Panel components

Interactive controls in `crates/pw-graph-app/src/panels` use the retained
`pw-graph-ui::UiDocument` component layer. Each control has a stable DOM-like
ID, keeps its value available through `get_element_by_id`/`value`, and can be
grouped into a form or observed with change/input/click listeners. The app
starts one document frame at the beginning of each update and dispatches its
queued events after all panels render.

Panel code uses the shared adapters for text inputs, numbers, sliders,
selects, buttons, checkboxes, switches, tab labels, and modal dialogs.
Custom-painted icon buttons and effect cards register their clicks through the
same document, so they remain reusable without losing their existing
appearance. All dialogs paint a translucent backdrop while keeping the graph
rendered underneath, so modal windows do not replace the application with a
black background. See
[`docs/ui-components.md`](docs/ui-components.md) for the component and form
API.

## Run

```bash
cargo run -p pw-graph-app
cargo run -p pw-graph-app -- --demo
cargo run -p pw-graph-app -- --lang es
```

`--demo` starts the deterministic demo graph. Without it, a missing live
backend produces an empty graph and an explanatory status message.

## Releases

Releases are published by GitHub Actions from tags matching `vX.Y.Z`. The tag
must match the workspace version in `Cargo.toml`; prerelease suffixes such as
`-rc.1` are also accepted. The workflow runs the locked workspace tests,
builds the native Linux release binary, and publishes these assets:

- `qpwgraph-rs-<version>-x86_64-unknown-linux-gnu.tar.gz`, containing the
  binary, desktop integration files, documentation, and third-party license;
- the matching `.sha256` checksum file.

To publish a release after updating the workspace version:

```bash
git tag -a v0.1.0 -m "Release v0.1.0"
git push origin v0.1.0
```

The same workflow can be rerun from **Actions → Release** for an existing tag.
See [packaging/README.md](packaging/README.md) for download and installation
instructions.

Audio meters are opt-in. They can be off, on demand (the default), or always;
on-demand meters attach only while a meter is hovered or pinned. PipeWire
helper streams currently report one aggregate reading per node. The backend API
also accepts optional port-associated readings for backends that can expose
independent port buffers; the UI falls back to the node reading. **Reset audio
config** releases every helper stream.

Press F1 for shortcuts. The graph also supports drag-to-connect, rectangle and
multi-selection, node dragging, curved links, scroll-to-pan, zoom, media
filters, port sorting, thumbnail mode, and default node arrangement.

## Effects

The effects API lives in `pw-graph-effects`. It processes interleaved `f32`
audio buffers through a prepare/process/parameter/reset lifecycle. The first
built-in effect is `builtin.noise-gate`, with threshold, attack, hold, release,
and bypass parameters. The demo backend can insert it between a selected audio
link, display it as an effect node, persist its stable endpoint keys, and remove
it while restoring the original link.

Effect channels are processed independently. An enabled effect with an output
connection but no matching input emits a quiet diagnostic noise signal, making
an incomplete route audible instead of silently producing an undefined buffer.

User modules should target `wasm32-unknown-unknown` and implement the exports
documented by `pw_graph_effects::wasm::ABI_DOCUMENTATION`. The realtime ABI has
no WASI imports: module loading, validation, instantiation, and memory growth
belong on the control thread.

## Audio relay

The desktop app includes the relay UI when the default `relay` feature is
enabled. Open **Preferences → Relay** to configure the local device name,
host PIN and TCP port, client target/PIN, role, codec, frame duration, and
preferred local link. Host, client, discovery, and active-session controls are
available in that tab; relay settings are saved automatically in the app
configuration.

Starting a host or connecting to a peer creates two PipeWire virtual nodes:
`qpwgraph-rs.relay.source` exposes received peer audio as **Relay Microphone**,
and `qpwgraph-rs.relay.sink` sends audio routed into **Relay Speaker** to
receiving peers. Discovery uses mDNS (`_qpw-relay._udp`); manual `host:port`
entry remains available when multicast discovery is unavailable.

The relay requires the native PipeWire backend. Builds without relay support
remain usable for graph editing, but the Relay preferences tab reports that
relay is unavailable. The Android client and native bridge are documented in
[`android/README.md`](android/README.md):

```bash
cargo run -p pw-graph-app --features relay
cargo run -p pw-graph-app --no-default-features --features pipewire
```

## CLI

```text
-m, --minimized       start minimized
-d, --debug           enable debug logging
-n, --no-alsa-midi    disable the optional ALSA MIDI backend
    --lang <LANG>     set the UI language (`en`, `es`, or `fr`)
    --demo            use the deterministic demo backend
```

## Native backends

The PipeWire backend is implemented in Rust in
`crates/pw-graph-backend/src/pipewire.rs` using the official Rust bindings. It
subscribes to registry globals, rebuilds nodes/ports/links, creates links with
`link-factory`, reads `audio.channel` metadata, and provides optional capture
streams for meters. The project has no local C PipeWire shim.

The ALSA backend keeps a small native Sequencer interface and namespaces its
IDs so PipeWire and ALSA graphs can be displayed together. Native development
headers are required when the corresponding features are enabled.

Build without native backends when those libraries are unavailable:

```bash
cargo build --release -p pw-graph-app --no-default-features
cargo build --release -p pw-graph-app --no-default-features --features pipewire
```

## Patchbay files

Files ending in `.qpwgraph` or `.xml` use the qpwgraph XML shape. Other
extensions use JSON. The configured patchbay path is used for startup
activation. Save/load use native dialogs, recent files are retained in
configuration, and Preferences offers named patchbay profiles plus an editable
connection-rule list. Graph connection changes are also written to the active
patchbay path automatically, including effect-node links and undo/redo changes.
Effect-node links are restored with their saved effect instances even when
full patchbay activation is disabled.

## Checks

```bash
cargo fmt --all -- --check
cargo test --workspace
cargo check --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

See [TODO.md](TODO.md) for implementation notes and
[packaging/README.md](packaging/README.md) for desktop integration notes.
