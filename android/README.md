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
service. Pairing PINs are entered for the current client or host lifetime and
are not persisted; there is no insecure app-wide default.

The app mirrors the desktop relay panel with three tabs:

- **Receiver** — connect to a relay host (phone as microphone/speaker).
- **Emitter** — run a relay host so the desktop can connect to the phone.
  The default control port is `48123`, which the desktop probes for when it
  scans USB tether subnets, so keep it unless you have a conflict.
- **Discover** — browse for relay hosts. While discovery runs the app probes
  USB tether subnets directly in addition to mDNS, because mDNS often does
  not cross a USB tether.

USB is not a link option: the app (like the desktop) auto-detects an active
USB tether, shows it under the tab bar, and `Auto` prefers it.

## Pair by QR code

The desktop Host tab renders the host's addresses and port and offers a
**Show QR** button while the host runs. The QR carries a
`qpw-relay://host:port?pin=123456` payload. In the Android Receiver tab, tap
**Scan QR** (camera permission required) to fill in the address and PIN
automatically, then press **Connect**. Plain `host:port` QR codes work too.

## Test over USB tethering

ADB USB debugging alone is only for installing and inspecting the app. For
relay audio over USB, enable **USB tethering** on the Android device and use
the USB network address assigned to the phone/Linux host:

1. Enable Android **USB tethering** and keep the phone unlocked.
2. On Linux, identify the USB/RNDIS interface, usually `usb0`, `rndis0`, or
   an `enx...` address; the desktop relay panel shows the detected link and
   its address automatically.
3. Start the desktop relay host with a PIN such as `123456`; use a fixed TCP port such
   as `48123` so the address is easy to enter.
4. Keep the preferred link on **Auto**: the relay panel auto-detects the USB
   tether and shows its address (for example `usb0 · 192.168.42.129`), and
   prefers the USB link automatically.
5. In Android, open the **Discover** tab and start discovery — the desktop
   host is probed over the USB tether directly. Tap **Connect**, or enter the
   Linux USB/RNDIS address as `host:port` manually in the Receiver tab. Enter
   the same PIN used by the desktop host and choose Emit/Receive/Both.
6. Confirm the desktop shows a relay session and that the Relay Microphone or
   Relay Speaker node carries audio.

Do not use the ADB device serial or `127.0.0.1` as the relay target. If Linux
cannot ping the phone-side USB address, USB tethering is not active; ADB
connectivity does not prove network reachability.

## Use

1. Start the desktop qpwgraph-rs application with the default `relay` feature.
2. Open the relay panel from the navigation rail, set a six-digit PIN, and
   start the host.
3. Enter the desktop `host:port` and PIN in the Android app, scan the host's
   QR code, or find the host in the **Discover** tab.
4. Choose Emit, Receive, or Both and press **Connect**.

Manual address entry remains supported as a fallback. The relay protocol uses
TCP control and UDP audio, so both devices must be able to reach each other
on the local network. VPNs, guest Wi-Fi isolation, and firewalls can block
the connection.

## Troubleshooting

- **No microphone audio:** grant `RECORD_AUDIO`; check Android's active input
  route and verify the app is not muted by system privacy controls.
- **No connection:** use the host's actual TCP port, verify the PIN, and test
  LAN reachability without guest-network isolation.
- **Connected but silent:** ensure the selected role matches the direction,
  keep the app's foreground notification active, and check the desktop graph's
  Relay Microphone/Relay Speaker virtual nodes.
- **Discovery:** mDNS is optional; while discovery runs, the desktop also
  probes USB tether subnets directly (mDNS often does not cross a USB
  tether). Manual `host:port` remains supported when neither works.
