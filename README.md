# qpwgraph-rs

An incremental Rust reimplementation of a PipeWire graph patchbay with optional
ALSA Sequencer MIDI support.

The workspace provides:

- `pw-graph-core`: serializable nodes, ports, links, validation, and graph operations.
- `pw-graph-backend`: a driver trait, deterministic demo backend, and native PipeWire registry/link backend.
- `pw-graph-alsamidi`: native ALSA Sequencer enumeration and connection backend.
- `pw-graph-command`: connect, disconnect, and rename commands with undo/redo.
- `pw-graph-patchbay`: qpwgraph-compatible XML plus JSON persistence and activation.
- `pw-graph-config`: TOML application settings and XDG config path helpers.
- `pw-graph-i18n`: catalog-based English/Spanish localization with fallback.
- `pw-graph-ui`: egui graph canvas, drag-to-connect, selection, moving, sorting, and thumbnail view.
- `pw-graph-app`: a runnable egui desktop shell, backend selection, patchbay controls, localization, and CLI flags.

## Run

```bash
cargo run -p pw-graph-app
```

Use `--lang es` to start in Spanish. The language can also be changed from the
configuration section in the inspector and is persisted in the application
config. The same section exposes patchbay, interface, graph-behavior, layout,
and thumbnail settings; hover any icon for an explanation and use Save
configuration to write immediately. Use `--demo` to force the deterministic
in-memory graph.

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

The PipeWire backend uses a small C ABI shim over the installed PipeWire 0.3
registry API to keep the Rust graph layer independent of the local PipeWire crate
version. The ALSA backend uses the ALSA Sequencer API and namespaces its IDs so
both graphs can be displayed together. The app automatically falls back to the
demo graph when no native backend is available.

## Patchbay files

Files ending in `.qpwgraph` or `.xml` use the qpwgraph XML shape and resolve rules
by node and port names; other extensions use JSON. The inspector and toolbar can
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
