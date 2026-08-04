# qpwgraph Relay Android client

This directory contains the Android client for the qpwgraph audio relay. It
reuses the Rust relay engine through `crates/pw-graph-relay-sdk` and a JNI
bridge in `crates/pw-graph-relay-android`.

## Build the native library

Install Android Studio, the Android SDK/NDK, Rust Android targets, and
`cargo-ndk`. The first supported ABI is `arm64-v8a`:

```bash
rustup target add aarch64-linux-android
cargo install cargo-ndk
cargo ndk -t arm64-v8a -o android/app/src/main/jniLibs build \
  -p pw-graph-relay-android --release
```

Build additional ABIs only after the corresponding Rust targets and Opus
native build are available. Do not commit generated `.so` files.

## Build the app

Open `android/` in Android Studio, or run the Gradle task from a machine with
the Android SDK configured:

```bash
./gradlew :app:assembleDebug
./gradlew :app:installDebug
```

The app requests microphone permission because Emit and Both capture audio.
Android 13+ notification permission is requested for the foreground audio
service.

## Use

1. Start the desktop qpwgraph-rs application with the default `relay` feature.
2. Open **Preferences → Relay**, set a six-digit PIN, and start the host.
3. Enter the desktop `host:port` and PIN in the Android app.
4. Choose Emit, Receive, or Both and press **Connect**.

Manual address entry is the primary path. The relay protocol uses TCP control
and UDP audio, so both devices must be able to reach each other on the local
network. VPNs, guest Wi-Fi isolation, and firewalls can block the connection.

## Troubleshooting

- **No microphone audio:** grant `RECORD_AUDIO`; check Android's active input
  route and verify the app is not muted by system privacy controls.
- **No connection:** use the host's actual TCP port, verify the PIN, and test
  LAN reachability without guest-network isolation.
- **Connected but silent:** ensure the selected role matches the direction,
  keep the app's foreground notification active, and check the desktop graph's
  Relay Microphone/Relay Speaker virtual nodes.
- **Discovery:** mDNS is optional; manual `host:port` remains supported when
  multicast is unavailable.
