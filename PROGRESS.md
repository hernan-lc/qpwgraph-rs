# qpwgraph-rs progress report

The requested i18n and roadmap implementation is complete through M10 for the
current Rust/egui implementation. English and Spanish are bundled catalogs with
English fallback, runtime language switching, localized CLI help, status text,
panel controls, patchbay actions, tray labels, and configuration help
tooltips. The canvas widget-ID clash overlay was fixed by scoping every graph
interaction ID to its canvas and graph item. The command UI was then cleaned up
so actions render in one compact toolbar instead of duplicated menu and toolbar
rows; icon controls expose their labels and explanations on hover, while long
node and port names are truncated visually and preserved in tooltips.
All UI icons are now painted from vector geometry in the app itself, avoiding
emoji and font-symbol rendering differences across operating systems.
Node dragging now accumulates per-frame pointer deltas, keeps group movement
stable, and reserves node hit areas before canvas pan or rectangle selection.
The graph canvas now presents type-accented draggable node headers, compact
human-readable names, port-row hover states, media-colored curved links, and
full node, port, and link tooltips without hiding the raw PipeWire names.
The inspector was then redesigned with scrollable grouped sections, statistic
cards, clearer panel headers, a wider responsive side panel, and independent
text-size controls for application chrome, inspector panels, and graph nodes.
Audio ports now have a real PipeWire-backed hover meter with RMS/peak and dB
readouts, stale-data indication, and a pinned monitor card in the Graph panel;
private helper streams are excluded from the visible graph and non-metering
backends report unavailable data rather than rendering fake activity.

The desktop shell is modularized: `main.rs` is a small entry point, while
`app.rs`, `args.rs`, `backend.rs`, `panels.rs`, and `tray.rs` own application
state, CLI parsing, backend composition, screen panels, and tray integration.
The backend’s PipeWire implementation, patchbay XML support, and canvas
rendering are also separated into `pipewire.rs`, `xml.rs`, and `canvas.rs`.
The GUI uses a navigation rail with separate Graph, Patchbay, Interface, and
Diagnostics screens instead of placing every option in one inspector column.

## Roadmap status

| Milestone | Status | Notes |
|---|---|---|
| M0 – PipeWire FFI/data model | Implemented | Native PipeWire 0.3 registry shim enumerates nodes, ports, media types, and links; graph positions are retained across refreshes. |
| M1 – Connect/disconnect | Implemented | PipeWire `link-factory` creation and registry destruction are wired and integration-tested against the running daemon. |
| M2 – Minimal GUI | Implemented | egui desktop shell renders nodes, ports, links, zoom, pan, and color-coded media types. |
| M3 – Interactive connections | Implemented | Source-to-sink click/drag, link selection/deletion, rectangle and multi-selection, node movement, port sorting, overlap repulsion, connect-through-node mode, and thumbnail view are present. |
| M4 – Undo/redo | Implemented | Connect, disconnect, and rename commands support undo/redo; keyboard shortcuts and toolbar controls are wired. |
| M5 – Config persistence | Implemented | TOML persists language, window geometry, zoom, string-keyed node positions, sort state, toolbar/status visibility, patchbay flags/path, thumbnail, layout options, and independent UI/panel/node text scales. |
| M6 – Patchbay system | Implemented | qpwgraph-style XML and JSON are supported; name-based activation, startup activation, snapshot, pin/unpin, exclusive mode, auto-disconnect, idempotence, and activation reporting are wired. |
| M7 – ALSA MIDI | Implemented | Native ALSA Sequencer enumeration, existing-subscription discovery, namespaced IDs, connect, disconnect, refresh, and composite PipeWire+ALSA routing are implemented. |
| M8 – Extras | Implemented | `-m`, `-d`, `-n`, `--lang`, and `--demo` are available; thumbnail mode and Linux StatusNotifier tray Show/Hide/Quit actions are implemented. |
| M9 – Packaging | Implemented | Desktop entry, AppStream metadata, Flatpak manifest, reproducible lockfile, and packaging instructions are included. |
| M10 – Runtime meters | Implemented | PipeWire audio source streams provide normalized RMS/peak readings; audio-port hover popovers and a pinned Graph-panel monitor show live/stale state without platform-dependent symbols. |

## UI organization

- Graph: node counts, selection/rename, port sorting, and compact labels with full-name hover tooltips.
- Patchbay: persistent routing rules, activation behavior, live-link pinning,
  and disconnect actions.
- Interface: language selection, configuration save, one action-toolbar toggle,
  visibility toggles, graph presentation behavior, and independent typography
  controls for application, panel, and node text.
- Diagnostics: active backend, status, graph counts, and port color legend.

## Native/runtime notes

- Native PipeWire and ALSA features require their development headers at build
  time. A normal launch shows an empty graph when no live backend can be
  initialized; the deterministic demo graph is opt-in with `--demo`.
- The tray is enabled by default on Linux when a StatusNotifier host is
  available; it disables itself cleanly on headless sessions or unsupported
  desktops. Build with `--no-default-features` to omit all optional backends and
  tray support.
- PipeWire and ALSA client names are externally owned, so rename is supported by
  the command/UI layer where the backend permits it; native clients report a
  clear unsupported error instead of changing external metadata.
- Runtime audio meters are node-level readings associated with each audio port on
  that node. They are intentionally optional: the UI shows a clear unavailable
  message when a backend cannot provide live buffers.

## Verification

The following checks pass in the workspace:

```text
cargo fmt --all -- --check
cargo test --workspace
cargo check --all-features
LANG=es_ES.UTF-8 cargo run -p pw-graph-app -- --help
PW_GRAPH_TEST_LINKS=1 cargo test -p pw-graph-backend --all-features native_backend_can_create_and_destroy_a_link_when_enabled
PW_GRAPH_TEST_ALSA_LINKS=1 cargo test -p pw-graph-alsamidi --all-features native_backend_can_create_and_destroy_a_link_when_enabled
```

The two environment-gated native mutation tests are opt-in because they create
and immediately destroy a real live connection in the user’s PipeWire/ALSA
session.
