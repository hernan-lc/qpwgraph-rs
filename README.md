# qpwgraph-rs

An incremental Rust reimplementation of a PipeWire graph patchbay with optional
ALSA Sequencer MIDI support.

The workspace provides:

- `pw-graph-core`: serializable nodes, ports, links, validation, and graph operations.
- `pw-graph-backend`: a driver trait, deterministic demo backend, native PipeWire registry/link backend, and optional normalized audio-meter stream channel. The native driver is isolated in `pipewire.rs` and uses the official Rust bindings.
- `pw-graph-alsamidi`: native ALSA Sequencer enumeration and connection backend.
- `pw-graph-command`: connect, disconnect, and rename commands with undo/redo.
- `pw-graph-patchbay`: qpwgraph-compatible XML plus JSON persistence and activation. XML serialization/parsing is isolated in `xml.rs`.
- `pw-graph-config`: TOML application settings and XDG config path helpers.
- `pw-graph-i18n`: catalog-based English/Spanish localization with fallback.
- `pw-graph-ui`: egui graph canvas with type-accented nodes, readable port aliases, curved media-colored links, drag-to-connect, reliable node/group moving, selection, media filtering, sorting, default media/direction layout, and thumbnail view. Canvas interaction/rendering is isolated in `canvas.rs`.
- `pw-graph-app`: a runnable egui desktop shell, backend selection, patchbay controls, localization, and CLI flags. Its application state, argument parsing, composite backend, tray integration, and GUI panels live in separate modules.

## Run

```bash
cargo run -p pw-graph-app
```

Use `--lang es` to start in Spanish. The language can also be changed from the
Interface screen and is persisted in the application config. The right-side
panel is split into Graph, Patchbay, Interface, and Diagnostics screens so
layout, routing, presentation, and status options remain separate. Each screen
uses scrollable grouped sections and compact statistic cards; the Interface
screen exposes independent application, panel, and node text-size controls.
Hover any icon for its label and explanation, and use Save configuration to write immediately. The
left navigation rail contains shared graph/history controls, optional patchbay
actions, and the media filter, so common actions remain available while
preferences are open. Toolbar, navigation, and settings icons are drawn as
platform-independent vector geometry rather than font glyphs. Use
`--demo` to force the deterministic in-memory graph.

Audio ports expose a hover monitor with RMS, peak, dB, freshness, and a pin action.
Pinned monitors remain visible in the Graph inspector. Live meters are provided by
PipeWire capture streams and their private helper nodes are filtered from the graph;
backends without runtime audio data show an explicit unavailable state instead of
simulated levels.

Measuring a node means attaching a real capture stream to it, so metering is
opt-in per node. The Graph panel exposes the policy as **Measure levels**:

| Policy | Behavior |
|---|---|
| Off | No metering stream is ever created. |
| On demand (default) | A stream is attached only while a meter is hovered or pinned, and released a few seconds after the last request. |
| Always | Every audio node is measured continuously, which keeps devices awake. |

The same panel has **Reset audio config**, which releases every metering stream
so PipeWire can suspend those nodes again and restore their configured settings.
The policy is persisted as `audio_meters` in `config.toml`.

Application options and filters are autosaved shortly after they change,
including language, layout, zoom, toolbar visibility, typography, and node
positions. Patchbay connection files remain separate and continue to use their
explicit Save Patchbay action.

The Graph panel's **Show media** filter can display all nodes or only Audio,
Video, or MIDI nodes and ports; MIDI includes both PipeWire/JACK and ALSA MIDI.
Fresh nodes are initially organized into media-category bands with source-only
nodes on the left and sink-only nodes on the right. **Arrange nodes** reapplies
that layout when existing saved positions need to be cleaned up.

Press **F1** for the keyboard-shortcuts window. It covers undo/redo, saving and
loading patchbay/configuration state, refresh, arrangement, thumbnail mode,
media filters (`0`–`3`), and graph zoom (`+`/`-`).

The default application build enables native PipeWire, ALSA MIDI, and Linux tray
support. PipeWire and ALSA are feature-gated so the application can still be
built without native development headers:

```bash
cargo build --release -p pw-graph-app --no-default-features
cargo build --release -p pw-graph-app --no-default-features --features pipewire
```

## CLI

```text
-m, --minimized       start with the window minimized
-d, --debug           enable debug logging
-n, --no-alsa-midi    disable the optional ALSA MIDI backend
    --lang <LANG>     set the UI language (`en` or `es`)
    --demo            use the deterministic in-memory graph
```

## Native backends

The PipeWire backend is implemented entirely in Rust in `pipewire.rs`. It owns a
PipeWire `ThreadLoop`, subscribes to registry globals, rebuilds the graph from
nodes/ports/links, creates and destroys links through `link-factory`, and uses
PipeWire capture streams for normalized audio meters. Metering streams are marked
`stream.monitor` and `node.passive`, never request a rate or channel count, target
their node by `object.serial`, and set `stream.capture.sink` when the target is a
sink, so attaching one neither resumes a suspended device, forces a graph-rate
renegotiation, nor reroutes the capture to the default source. The project has no local
C PipeWire shim or C build script anymore. The official Rust bindings still link
to the system PipeWire and SPA libraries at runtime, so the native development
packages remain a platform dependency when the `pipewire` feature is enabled.

The ALSA backend continues to use the small native ALSA Sequencer interface and
namespaces its IDs so both graphs can be displayed together. A normal launch
does not invent a mock graph when no native backend is available; it shows an
empty canvas with a clear status message. Use `--demo` when a deterministic
sample graph is wanted.

## Patchbay files

Files ending in `.qpwgraph` or `.xml` use the qpwgraph XML shape and resolve rules
by node and port names; other extensions use JSON. The inspector and sidebar can
snapshot live links, activate saved links, enable exclusive activation, and
auto-disconnect sink conflicts.

## Checks

```bash
cargo fmt --all -- --check
cargo test --workspace
cargo check --workspace
```

See [PROGRESS.md](PROGRESS.md) for milestone status and [packaging/README.md](packaging/README.md)
for release and desktop-integration instructions.
