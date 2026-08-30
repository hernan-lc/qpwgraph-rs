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
are held in memory only and never written to disk. The relay installation ID
is persisted so a peer remains recognizable across Wi-Fi/USB address changes.
After explicit PIN pairing, per-peer trusted credentials are persisted as
owner-only hex values. They are used only when discovery presents the same
stable peer ID; arbitrary discovered peers are never auto-connected. Set
`relay_auto_connect_trusted = false` to keep trusted peers manual.
Android stores the equivalent installation ID and credentials in private
preferences and excludes that preference file from cloud backup and
device-to-device transfer, so restoring an app backup cannot clone a trusted
installation identity.

## Patchbay files

Patchbay files retain the qpwgraph XML shape for `.qpwgraph` and `.xml` files;
other extensions use JSON. Save/load use native dialogs. The active path,
recent files, named profiles, editable rules, auto-pin, exclusive activation,
auto-disconnect, and startup activation are persisted. Live graph changes,
undo, and redo keep the saved patchbay state synchronized.

## Related

- [Effects and metering](effects-and-metering.md) — startup restoration order.
- [Audio relay](audio-relay.md) — relay endpoint selection and its persistence.
