# Configuration and patchbay files

Where qpwgraph-rs keeps its state, what it restores at startup, and what it
deliberately does not.

## Application configuration

The application reads the existing qpwgraph-rs TOML configuration and writes
it back without discarding unknown fields. Node positions and appearance use
stable numeric/name keys. Volume and mute are live controls and are not
silently restored at startup. Configuration is stored under
`~/.config/qpwgraph-rs` on Linux and `%APPDATA%\qpwgraph-rs` on Windows.

Preserving unknown fields is what lets an older and a newer build share a
configuration file without either one stripping the other's settings.

Pairing PINs are the exception to persistence: the host and client relay PINs
are held in memory only and never written to disk.

## Patchbay files

Patchbay files retain the qpwgraph XML shape for `.qpwgraph` and `.xml` files;
other extensions use JSON. Save/load use native dialogs. The active path,
recent files, named profiles, editable rules, auto-pin, exclusive activation,
auto-disconnect, and startup activation are persisted. Live graph changes,
undo, and redo keep the saved patchbay state synchronized.

## Related

- [Effects and metering](effects-and-metering.md) — startup restoration order.
- [Audio relay](audio-relay.md) — relay endpoint selection and its persistence.
