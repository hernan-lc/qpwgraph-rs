# Audio relay

Carrying audio between this machine and a peer — typically a phone — over the
local network. This page covers the feature as the desktop application exposes
it; the wire format is documented separately in
[Relay wire protocol, version 3](relay-protocol.md).

## The panel

The relay panel supports host start/stop, discovery, peer connection and
disconnection, configurable role/codec/frame/transport, QR payload generation
and parsing, local endpoint discovery, level updates, and virtual relay graph
nodes on Linux.

A host generates a fresh random pairing PIN for each hosting session. It is
shown in the panel and encoded in the QR payload, is never written to disk, and
is retired when the host stops — so a PIN that has been displayed or
photographed does not keep working into the next session.

## USB tethering, Wi-Fi, and ADB

The desktop application's discoverable host default is TCP port `48123`. The
relay SDK still accepts `port(0)` for callers that explicitly want an ephemeral
port, but a USB direct scan cannot discover an ephemeral listener. If
`48123` is occupied, the application reports the bind error; choose another
explicit port and use its address manually.

`Auto` selects the best active local network link in this order: USB/RNDIS/NCM
tether, Wi-Fi, Bluetooth PAN, then LAN. It does not silently connect to an
untrusted peer. While discovery is active, mDNS and the bounded direct USB
probe run independently; either one may find a host, and stopping discovery
terminates both workers and clears transient peers.

A plain USB debugging/ADB cable is not a relay network link. The current relay
uses TCP control plus authenticated/encrypted UDP audio, so ADB detection or
`adb reverse` alone does not provide working audio transport. Enable USB
tethering, or use Wi-Fi/LAN/Bluetooth PAN. Pairing still requires the current
PIN and an explicit user action; trusted-device key auto-reconnect is not
enabled by default.

On Android, the platform audio endpoints currently run mono PCM16. Android
rejects stereo relay geometry until stereo `AudioRecord`/`AudioTrack` I/O is
implemented, and it requests microphone permission only for Emit, Both, and
Host modes. Receive-only playback uses no microphone permission.

## Windows

On Windows, the relay uses WASAPI loopback and render streams; the panel
exposes independent capture and playback endpoint choices by stable Core Audio
ID, with system-default fallback when a saved device disappears. Windows cannot
create a microphone endpoint for peer audio, and Windows relay capture is
whole-endpoint rather than per-application.

## Enabling it

The relay is enabled by the default `relay` feature:

```bash
cargo run -p pw-graph-app --features relay
cargo run -p pw-graph-app --no-default-features --features pipewire,relay
```

## Embedding the relay

`pw-graph-relay-sdk` is the stable API for third-party applications, with
`RelayHostBuilder` and `RelayClientBuilder` as the entry points.
`pw-graph-relay-android` wraps that SDK in JNI bindings for Android.

`.audio(sample_rate, channels, frame_ms)` sets both the negotiated wire
geometry and the local geometry of the PCM you push and pull; use
`.wire_audio()` / `.local_audio()` only when the two genuinely differ.
Geometries outside the negotiable set (16/24/48 kHz, mono or stereo,
5/10/20/40/60 ms) are rejected at `build()`.

## Related

- [Relay wire protocol, version 3](relay-protocol.md) — frames, pairing,
  resume authentication, and crypto.
- [Audit follow-ups](audit-follow-ups.md) — resolved audit items and regression
  coverage.
