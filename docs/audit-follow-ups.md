# Audit follow-ups — resolved and extended relay UX

The original audit findings and the broader trusted-cable follow-ups are
implemented in code and covered by regression tests where they are testable
without hardware. This file is a short map to the implementation; physical
Android RNDIS and ADB sessions still need device-level validation.

| Area | Resolution |
| --- | --- |
| Session resume | Protocol 3 uses a session-bound HKDF resume key, a fresh client/server challenge, constant-time HMAC verification, eligibility state, generation rotation, and fresh control keys. A host-wide PIN and session id alone cannot resume a session. |
| Graph transactions | Patchbay activation, `ConnectMany` execute/undo, reroute, and node movement track both mutation directions and report partial rollback precisely. |
| Interface exposure | `Auto` selects the best active relay link (USB, Wi-Fi, Bluetooth PAN, LAN); the selected bind address is reused for host audio, mDNS, QR, endpoint display, and status. Wildcard binding is only the documented no-link or explicit-wildcard fallback. |
| Android concurrency/audio | JNI clones stable relay handles before engine work, does not hold registry locks across connect or audio operations, reuses thread-local PCM scratch, uses realtime `try_*` APIs, and reports accepted/produced sample counts. |
| Realtime bounds | Realtime capture rejects oversized quanta; converter, mixer, and queue capacities are prepared and regression-tested, and bounded queue pushes do not grow their ring buffer. |
| Worker startup | Outgoing, RX/TX, and accepted-peer worker spawn failures release admission state, surface errors, and cannot leave an apparently established zombie session. |
| Android validation/lifecycle | JNI integer conversions are checked before narrowing, creation returns JSON errors, and a host stop returns its handle to a reusable prepared state. |
| Persistence | Atomic writes check file and parent-directory sync, use unique sibling temporary names, clean up on error, and preserve private-file permissions. |
| Secrets and compatibility | Desktop PINs remain ephemeral, SDK PINs remain caller-owned, Android PINs are not persisted, authenticated audio metadata ordering and replay protection remain intact, and audio geometry/bounded-resource guarantees are preserved. |
| Trusted cable auto-connect | An explicit PIN pairing creates a random credential bound to stable installation IDs. Desktop persists it with owner-only atomic TOML writes; Android encrypts it with a non-exportable Keystore AES-GCM key. Discovery can auto-connect only an exact trusted peer, never an arbitrary USB result, and users can revoke it live. |
| ADB-only cable transport | ADB mode carries control and sealed audio over separate authenticated TCP streams, using `adb reverse` or `adb forward`; it does not pretend ordinary ADB provides UDP. The audio stream has an independent bounded reconnect supervisor. |
| Live link migration | A running host rebinds its listener on the same port when the selected interface changes, and authenticated resume tries newly discovered addresses for the authenticated peer identity. Normal UDP binds to the selected interface; Android feeds link-classified discovery into the active client engine. Healthy Wi-Fi sessions intentionally move only during authenticated resume/failover. |
| Android service death | Service teardown reports mode and handle after native cleanup; the ViewModel invalidates the matching client/host handle before another operation can reuse it. |
| UDP wildcard → interface migration | Migrating a host's audio socket from `0.0.0.0:PORT` to a specific address on the same port now removes the socket from its slot, waits for outstanding worker leases to drain within a bounded window, and closes it *before* the new bind. Previously only the `Arc` was unlinked, so a lease kept the wildcard socket open and the specific bind failed with `EADDRINUSE`. The reverse direction is handled identically. A failed bind reinstalls a wildcard socket on the original port rather than leaving the slot empty, and a drain that times out restores the still-live socket and reports an explicit error. Same-interface resume still installs nothing. |
| Legacy relay patchbay files | The relay filter ports were renamed from bare `FL`/`FR` to `capture_FL`/`capture_FR` and `playback_FL`/`playback_FR` so the canvas groups them as a stereo pair. Patchbays saved before that rename now resolve through one narrow read-only rewrite, scoped to the two relay node names, the two channel names, and the direction each node actually has ports in. Unrelated devices with real `FL`/`FR` ports are untouched, and new saves keep the role-prefixed names. |

## CI and release-workflow follow-ups

| Area | Resolution |
| --- | --- |
| Legacy-UI guard | The guard invoked `rg`, which is absent from the Ubuntu runner image. `rg` exited 127, bash read the missing command as a false `if` condition, and the step passed while checking nothing. It now uses POSIX `grep` with `--include` filtering and asserts the tool exists first, so an unavailable utility can never again be reported as a clean tree. |
| Android NDK selection | CI exported `ANDROID_NDK_HOME` and `ANDROID_NDK_ROOT` from different runner-provided NDKs, which `cargo-ndk` warned about. Both now point at `ANDROID_NDK_LATEST_HOME`. |
| Android application coverage | A new `Android app checks` job builds the JNI libraries into the app's gitignored `jniLibs` and runs `:app:testDebugUnitTest`, `:app:lintDebug`, and `:app:assembleDebug`, so the Kotlin/Compose sources, manifest, adaptive and monochrome launcher icons, notification drawable, foreground service, and credential-store tests are all compiled and validated. A Gradle 8.9 wrapper is committed with its distribution checksum pinned. The Rust ABI builds and Android Clippy coverage are unchanged. |
| Release supply chain | The release workflow defaults to `contents: read` and grants `contents: write` only to the publishing job; build checkouts use `persist-credentials: false`. `linuxdeploy` is verified against its upstream SHA-256 before it is made executable, the Flatpak container image is pinned by immutable digest beside its human-readable tag, `cargo-ndk` and `cargo-deny` are installed at pinned versions, and the Actions whose pinned revisions still targeted the deprecated Node 20 runtime were moved to maintained Node 24 revisions, still pinned by full commit SHA. |
| Feature-matrix build | Several trusted-candidate helpers and one `Application` field were missing their `#[cfg(feature = "relay")]` gate, so `cargo check -p pw-graph-app --no-default-features` did not build. CI never reached that step because formatting failed first. |

The protocol and API contracts are documented in
[`relay-protocol.md`](relay-protocol.md), [`audio-relay.md`](audio-relay.md),
and the public relay SDK documentation.

Physical Android validation is still outstanding and is **not** claimed here:
USB tether/RNDIS, live Wi-Fi to USB failover, `adb reverse`/`forward`
reconnection, foreground-service lifecycle and notification-tap behaviour on a
real device, launcher/adaptive/monochrome icon appearance, and on-device
microphone and playback routing all need a physical handset.
