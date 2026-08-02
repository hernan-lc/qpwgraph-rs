# qpwgraph-rs

An incremental Rust reimplementation scaffold for a PipeWire graph patchbay.

The workspace currently provides:

- `pw-graph-core`: serializable nodes, ports, links, validation, and graph operations.
- `pw-graph-backend`: a driver trait and deterministic in-memory backend with a demo graph.
- `pw-graph-alsamidi`: optional ALSA MIDI driver seam, ready for Sequencer enumeration.
- `pw-graph-command`: connect/disconnect commands with undo/redo.
- `pw-graph-patchbay`: JSON patchbay persistence and activation.
- `pw-graph-config`: TOML application settings and XDG config path helpers.
- `pw-graph-i18n`: catalog-based English/Spanish localization with fallback.
- `pw-graph-ui`: egui graph canvas and drag-to-connect interaction.
- `pw-graph-app`: a runnable egui desktop shell and CLI flags.

## Run

```bash
cargo run -p pw-graph-app
```

Use `--lang es` to start in Spanish. The language can also be changed from the
inspector and is persisted in the application config.

The default build uses the in-memory backend, so it can be built and tested without
PipeWire development headers or a running PipeWire daemon. This is intentional: it
makes the graph model, command stack, persistence, and UI testable independently.

## CLI

```text
-m, --minimized       start with the window minimized (currently recorded for the UI shell)
-d, --debug           enable debug logging
-n, --no-alsa-midi    reserve/disable the optional ALSA MIDI backend
```

## Architecture

The `GraphDriver` trait is the integration seam for a real PipeWire driver. The next
implementation step is to add PipeWire registry listeners and `pw_link_new`/
`pw_link_destroy` calls behind the optional `pipewire` feature in
`pw-graph-backend`, without changing the UI or command layers.

## Checks

```bash
cargo fmt --all -- --check
cargo test --workspace
cargo check --workspace
```

See [PROGRESS.md](PROGRESS.md) for the M0–M9 implementation status and remaining
native backend work.
