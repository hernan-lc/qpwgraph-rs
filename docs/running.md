# Running qpwgraph-rs

Launching the application, choosing a backend, and the command line.

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

## Keyboard

Press F1 for the complete shortcut list. Graph shortcuts are ignored while a
text input owns keyboard focus or a dialog is open, `Ctrl` is `Cmd` on macOS,
and the Windows/Super key is never an application modifier. The full contract —
focus routing, auto-repeat, Escape precedence — is in
[the keyboard contract](keyboard.md).

## Related

- [Building](building.md) — feature flags and release builds.
- [Configuration and patchbay files](configuration.md) — where settings live.
