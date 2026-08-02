# qpwgraph-rs progress report

## Newly implemented

- Catalog-based i18n crate with English fallback.
- English and Spanish locale catalogs.
- Runtime language selector in the inspector.
- Persisted language preference in `config.toml`.
- `--lang en`, `--lang es`, and `--lang=<LANG>` CLI support.
- Localized CLI help, toolbar, inspector, status messages, debug line, and canvas connection hint.

## Roadmap status

| Milestone | Status | Notes |
|---|---|---|
| M0 – PipeWire FFI/data model | Partial | Graph model and `GraphDriver` seam are implemented; real PipeWire registry listeners are still pending. |
| M1 – Connect/disconnect | Partial | Fully tested against the in-memory backend; native `pw_link_new`/destroy integration is pending. |
| M2 – Minimal GUI | Implemented | egui desktop shell, graph nodes/ports/links, pan, zoom, and color-coded port types. |
| M3 – Interactive connections | Partial | Click/drag source-to-sink connection and link deletion are present; rectangle selection and full qpwgraph shortcut parity remain. |
| M4 – Undo/redo | Implemented | Generic command stack with connect, disconnect, and rename commands; keyboard Ctrl/Cmd+Z and Shift+Z are wired. |
| M5 – Config persistence | Partial | TOML config persists window size, zoom, sort/patchbay flags, paths, and language; actual toolbar/menu visibility controls are not yet exposed. |
| M6 – Patchbay | Implemented | JSON save/load, activation, idempotence, exclusive mode, auto-disconnect, pin state, and activation reporting. Exact qpwgraph XML compatibility is pending. |
| M7 – ALSA MIDI | Scaffolded | Separate driver crate and type dispatch seam exist; ALSA Sequencer enumeration and connection calls are pending. |
| M8 – Extras | Partial | CLI flags and minimized startup are present; system tray, thumbnail view, and full option parity are pending. |
| M9 – Packaging | Not started | No distro packages, installers, desktop file, or release automation yet. |

## Current limitation

The runnable app intentionally uses `InMemoryDriver::demo()` so it works without
PipeWire development headers or a live daemon. The next major implementation step
is replacing that wiring with a registry-backed PipeWire driver while preserving
the existing core, command, patchbay, i18n, and UI APIs.

## Verification

The workspace currently passes:

```text
cargo fmt --all -- --check
cargo test --workspace
cargo check --all-features
LANG=es_ES.UTF-8 cargo run -p pw-graph-app -- --help
```

