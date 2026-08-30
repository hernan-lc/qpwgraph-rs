# Audit follow-ups — resolved

The remaining audit findings have been implemented in code and covered by
regression tests. This file is a short map to the implementation; it is not a
list of deferred fixes.

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

The protocol and API contracts are documented in
[`relay-protocol.md`](relay-protocol.md), [`audio-relay.md`](audio-relay.md),
and the public relay SDK documentation. CI configuration is intentionally not
part of this change.
