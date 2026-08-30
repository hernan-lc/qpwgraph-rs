# Audit follow-ups

Items from the audit against `6b61464` that are **not** yet fixed. Each is
either a protocol change or a larger refactor than the pass that closed the
rest, so they are recorded here rather than half-done.

The items already closed are not listed; look for their regression tests.

## 1. Session resume is not bound to the original session secret

`Resume` currently authenticates with a known `session_id` plus a fresh PAKE
against the *global host PIN*. Nothing in that proves possession of material
belonging to the session being resumed, so a second device that knows the host
PIN and learns a session id can take over that session's control channel — not
its UDP audio keys, but enough authority to disrupt it (`Bye`, for instance).

The fix is a session-specific resume secret derived from the original pairing's
shared secret, e.g.

```text
resume_secret = HKDF(session shared secret, "qpw-relay resume authentication")
proof         = HMAC(resume_secret, session_id || client_nonce || context)
```

stored in `SessionRecord`, never transmitted raw, compared in constant time,
with fresh nonces against replay and a control-key rederivation after a
successful resume. The host PIN PAKE may stay as an extra factor, but knowing
only the PIN must not authorise taking over an existing session.

Needs: a wire-format change in `protocol.rs`, key derivation in `crypto.rs`,
and storage in `SessionRecord`. Tests to write: valid owner resumes; wrong
resume secret fails; correct PIN with wrong session secret fails; a replayed
resume proof fails; one session's secret cannot resume another; a fresh resume
produces fresh control keys.

## 2. Patchbay activation is not transactional

`pw-graph-patchbay` tracks links it *removed* during an activation (`undone`)
but not links it *created*. A failure partway can therefore restore the
original links while leaving newly created ones in place — a graph that is
neither the original state nor the requested patchbay, and a direct violation
of the requested policy when `exclusive = true`.

The fix is to track `removed_by_activation` and `created_by_activation`
separately and, on fatal failure, undo both in reverse order, reporting how
many of each could not be undone (extending `PatchbayError::ActivationNotRolledBack`
or replacing its `stranded: usize` with precise counts).

It also needs a single stated policy for non-fatal connection failures: either
atomic activation (any failure rolls the whole activation back) or best-effort
with **iteration-scoped** rollback state. The present code mixes a global
`undone` vector with per-route intent, which is what allows the hybrid state.

Tests to write: exclusive activation where a removal and one creation succeed
and a later creation fails leaves no hybrid state; auto-disconnect failures;
rollback disconnect failure; rollback reconnect failure; immutable links stay
untouched.

## 3. `TransportPreference::Auto` still binds every interface

`Auto` resolves to no specific address, so the host listener ends up on
`0.0.0.0` — LAN, VPN, virtual and public-facing adapters included. The traffic
is authenticated and encrypted now, so this is exposure rather than a hole, but
it is still broader than necessary.

Intended order: explicit `bind_addr` → explicit transport preference → best
active relay-capable interface under the existing USB / Wi-Fi / Bluetooth PAN /
LAN ranking → `0.0.0.0` only as a deliberate, documented fallback. mDNS must
advertise addresses consistent with whatever is actually being listened on, and
loopback integration tests must keep working.

Tests to write, driven by synthetic `LocalLink`s: Auto with USB + Wi-Fi picks
USB; Auto with only Wi-Fi picks Wi-Fi; explicit Wi-Fi picks Wi-Fi; explicit USB
with no USB link follows the documented fallback; no usable links reaches the
intended fallback.

## 4. Android JNI audio calls hold the global registry lock

`Java_io_qpwgraph_relay_NativeBridge_{pushCapture,pullPlayback}` and their
`host*` counterparts allocate a `Vec<f32>` per call and hold the process-wide
client/host registry mutex across the engine audio call. If these are meant to
be driven from `AudioRecord`/`AudioTrack` loops, a control operation on any
*other* handle can block the audio thread.

The shape of the fix is: clone the `RelayHandle` out of the registry under the
lock, drop the lock, then do the audio work through
`try_push_capture`/`try_pull_playback`, with a reusable per-thread buffer
instead of a fresh `Vec`. (`RelayHandle` is now re-exported from
`pw-graph-relay-sdk`, which is what such a change needs to name.)

Deferred deliberately: it changes the return-value semantics of the JNI
functions — a contended `try_push` returns 0 where the blocking version always
reported the full length — so the Kotlin side under `android/` has to be read
and updated in the same change. Decide and document the contract first: are
these realtime APIs or not?

## 5. Whole-path realtime audit

Item 13 of the audit asks for a focused review of *every* function reachable
from the PipeWire process callback for allocation, blocking locks, I/O, sleeps,
thread creation, string formatting and unbounded loops. The mixing and
conversion paths were done as part of this pass (see
`MAX_REALTIME_QUANTUM_SAMPLES` and `Converter::prepare`), but `queue.rs` and
`pw-graph-backend/src/pipewire/relay.rs` have not been walked end to end.
