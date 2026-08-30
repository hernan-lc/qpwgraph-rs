# Building

Native builds per platform, the feature flags that select a backend, Nix, and
the checks that have to pass before a change lands.

## Feature defaults

On Linux, the default build enables PipeWire, ALSA MIDI, relay, and tray
support. On Windows, PipeWire and ALSA are not built or required; the native
backends use Windows Core Audio/WASAPI through a dedicated COM worker thread
and WinMM for MIDI.

```bash
cargo build --release -p pw-graph-app
cargo build --release -p pw-graph-app --no-default-features
cargo build --release -p pw-graph-app --no-default-features --features pipewire
cargo build --release -p pw-graph-app --no-default-features --features alsa
```

The relay is enabled by the default `relay` feature and can be selected
explicitly:

```bash
cargo run -p pw-graph-app --features relay
cargo run -p pw-graph-app --no-default-features --features pipewire,relay
```

## Windows

On Windows, the standard MSVC commands are:

```powershell
cargo run -p pw-graph-app -- --demo
cargo build --release --locked -p pw-graph-app
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

## Development checks

```bash
cargo fmt --all -- --check
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo build --release --locked
```

## Related

- [Workspace architecture](architecture.md) — what each crate owns.
- [Packaging and releases](packaging.md) — producing distributable artifacts.
