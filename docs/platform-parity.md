# Platform parity

What qpwgraph-rs can do on each backend, what it cannot, and which gaps are
worth closing. This is a status document, not a promise: entries change as the
backends do.

## How to read the categories

| Category | Meaning |
| --- | --- |
| **Equivalent** | Both platforms do the same thing, and the user cannot tell which backend is underneath. |
| **Partial** | Works, but with a reduced range, a coarser update, or a missing sub-feature. |
| **Missing** | Not implemented. Nothing structural prevents it; nobody has built it. |
| **Platform limitation** | The platform genuinely does not offer this. No amount of work in this repo changes it. |
| **Bug** | Implemented, but wrong. Needs fixing rather than building. |

The distinction between **Missing** and **Platform limitation** is the important
one: the first is a backlog item, the second is a fact about the operating
system that the UI has to present honestly instead of pretending around.

## Feature status

### Graph and topology

| Feature | Linux (PipeWire) | Windows (Core Audio) | Status |
| --- | --- | --- | --- |
| Read graph topology | Yes | Yes, as endpoints and application sessions | Equivalent |
| Node/port naming | Yes | Yes | Equivalent |
| Create a connection | Yes | MIDI only | Partial |
| Remove a connection | Yes | MIDI only | Partial |
| Select an existing connection | Yes | Yes | Equivalent |
| Drag an edge onto another port | Yes | MIDI only | Partial |
| Patchbay persistence | Yes | Not applicable | Platform limitation |

Windows Core Audio has no arbitrary patchbay. What the driver shows is the
routing Windows reports — which application session is playing to which
endpoint — and those relationships are observations, not links a user can
rewire. `WindowsAudioDriver` therefore reports `connect: false` and
`disconnect: false`, and `is_link_mutable` returns false for every link.

This is deliberate and must stay that way. Enabling `connect` to make the
connection UI light up would produce controls that cannot work. Selection and
inspection are unaffected: an observed link is still clickable, still
selectable, and still shown in the graph. Only mutation is refused.

### Audio state and controls

| Feature | Linux (PipeWire) | Windows (Core Audio) | Status |
| --- | --- | --- | --- |
| Set volume | Yes | Yes (endpoint and session) | Equivalent |
| Set mute | Yes | Yes (endpoint and session) | Equivalent |
| Read volume | Yes, from node Props | Yes | Equivalent |
| Read mute | Yes, from node Props | Yes | Equivalent |
| Follow external changes | At each rebuild | Yes, event driven | Partial (Linux) |
| Volume above unity | Yes, to 150% | No, clamped at 100% | Platform limitation, reported per node |
| Per-node capability reporting | Yes | Yes | Equivalent |

The backend owns audio state. `GraphDriver::node_audio_state` returns a
`NodeAudioState` whose `volume` and `muted` are `Option`, where `None` means
"this backend cannot tell you". The UI renders that as an unknown value — a
dimmed fader — and never substitutes a number of its own. Before this, every
card claimed 90% and unmuted regardless of the real system state, which was
visibly wrong on Windows.

Two gaps remain here:

PipeWire volume and mute are read back from each node's `Props` during a graph
rebuild, so a level set in pavucontrol or with a media key reaches the cards.
Windows goes further and follows changes by callback, without waiting for a
rebuild; doing the same on Linux would mean holding a param subscription per
node rather than reading on demand.
Windows volume and mute are event driven: `IAudioSessionEvents` and
`IAudioEndpointVolumeCallback` carry the new values in their payload, so the
cache follows a change made anywhere on the system without polling and without
marking the topology dirty. A fader move no longer forces a full endpoint and
session re-enumeration.

Maximum volume is a *node* capability, not audio state: `NodeCapabilities`
carries `volume_max`, PipeWire and demo report 1.5, and Windows reports unity.
The fader maps its whole travel into 0..=1 for a node that cannot boost, so the
top of a Windows fader is no longer dead travel that silently clamps.

### Metering

| Feature | Linux (PipeWire) | Windows (Core Audio) | Status |
| --- | --- | --- | --- |
| Meter a capture source | Yes | Yes | Equivalent |
| Meter a playback sink | Yes, through its monitor | Yes | Equivalent |
| Meter an application stream | Yes | Yes, peak only | Equivalent |
| Meter policies (off/on-demand/always) | Yes | Yes | Equivalent |

Playback sinks used to be excluded from metering on Linux: eligibility required
an audio *source* port, which a sink does not have, so speakers and other output
devices silently showed nothing even though the meter stream already knew how to
read a sink through its monitor. Fixed; `api::is_measurable_audio_node` now
holds the rule for both backends and is unit-tested.

On Windows, endpoints expose `IAudioMeterInformation` and application sessions
do not, so a session reports no meter capability rather than being given a meter
it can never fill. `IAudioMeterInformation` is an endpoint facility with a peak
reading and no RMS, which is why Windows endpoints report `meter_peak: true` and
`meter_rms: false`, and `audio_meters` reports `rms: 0.0`.

Per-session metering is *not* reachable by extending `IAudioSessionControl`.
The supported route is **process loopback capture**:
`ActivateAudioInterfaceAsync` with
`AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK`, which records what one process
tree renders, on build 20348 and newer. The driver already has the bridge it
needs -- `IAudioSessionControl2::GetProcessId` is read for every session -- so
one capture path would serve both per-application meters and relaying a single
application. It is *Missing*, not a platform limitation.

An implementation was attempted and reverted: the activation reproducibly
brought down the process with `STATUS_HEAP_CORRUPTION` on this machine, and a
memory-safety fault is not something to ship behind a feature flag. The likely
suspects are the `VT_BLOB` `PROPVARIANT` carrying
`AUDIOCLIENT_ACTIVATION_PARAMS` and the lifetime of that blob across the
asynchronous activation. Worth another attempt against Microsoft's
ApplicationLoopback sample rather than from the API reference alone.

Metering is intentionally conservative on PipeWire. Measuring a node means
attaching a real capture stream, which the session manager links like any other
client — it can resume suspended devices and make the daemon renegotiate the
graph rate. That is why the default policy is on-demand and why meter streams
are flagged passive, monitor-only, and non-reconnecting.

### Processing and networking

| Feature | Linux (PipeWire) | Windows (Core Audio) | Status |
| --- | --- | --- | --- |
| Effect nodes | Yes | No | Missing |
| Effect insertion into a link | Yes | No | Platform limitation (needs routing) |
| Relay: send this machine's audio | Yes | Yes | Equivalent |
| Relay: play a peer's audio here | Yes | Yes | Equivalent |
| Relay: peer audio as a microphone | Yes | No | Platform limitation |
| Relay: send one application only | Yes | No | Missing (build 20348+) |
| Relay: choose which endpoint | n/a | Yes | Equivalent |
| MIDI | ALSA | WinMM, with routing | Partial |

Effect *insertion* depends on rewiring an existing link, so it cannot exist on
Windows without routing. Free-standing effect nodes do not have that constraint
and are merely unbuilt.

### Relay

The relay engine — pairing, transport, Opus, discovery, QR — is platform
neutral and builds on both targets. Only the audio endpoints that drive
`RelayHandle::push_capture` and `RelayHandle::pull_playback` differ.

On Linux those endpoints are two virtual PipeWire nodes, so *any* application
can be routed into or out of the relay through the patchbay, in either
direction.

On Windows they are WASAPI streams on the default playback endpoint: a loopback
capture supplies what this machine is playing, and a render stream plays what
peers send. That is enough to use a phone as a speaker, or to play a phone's
audio here.

What Windows cannot do is present received audio as a **microphone** to other
applications. That needs a capture endpoint, and Windows has no user-mode API
for creating one — a selectable device requires a kernel-mode driver, which is
what tools like VB-Cable install. `relay_connect` therefore refuses an emit
role outright rather than accepting it and carrying no audio. For the same
reason, individual applications cannot be routed into the relay on Windows;
the loopback tap is whole-endpoint.

## Roadmap

Ordered by how much each one improves what a user actually sees. Everything
above the line has landed; what is left is blocked on something specific.

1. **Windows per-app output routing.** *Blocked on an undocumented ABI.* The
   edge the graph draws between an application session and an endpoint is the
   one relationship Windows lets a user change -- Settings calls it "App volume
   and device preferences". The object behind it,
   `Windows.Media.Internal.AudioPolicyConfig`, **does activate on this machine**
   (verified: `RoGetActivationFactory` returns S_OK on 10.0.19045). What is
   missing is a trustworthy vtable layout for `IAudioPolicyConfigFactory`: it is
   undocumented, differs between Windows 10 and 11, and calling the wrong slot
   is undefined behaviour rather than an error. Needs the layout confirmed
   against a known-good implementation before anything calls it.
2. **Windows process loopback.** *Blocked on the OS build here.* One capture
   path unlocks per-application meters and relaying a single application. It
   requires build 20348 or newer; this machine is 10.0.19045, so it cannot be
   exercised at all, and the first attempt from the API reference alone brought
   the process down with `STATUS_HEAP_CORRUPTION`. Needs a newer machine and
   Microsoft's ApplicationLoopback sample rather than the reference.
3. **`OnSessionCreated` handling.** Currently coarse; a new session should be
   folded in without a full re-enumeration. The periodic churn is gone, so this
   is now the only remaining source of unnecessary re-enumeration.
4. **Linux param subscriptions.** PipeWire controls are read at each rebuild;
   Windows follows them by callback. Holding a `Props` subscription per node
   would close that gap.
5. **Windows free-standing effect nodes.** Requires a processing host that does
   not depend on graph routing.
6. **Relay: send one application only.** Depends on process loopback above.

## Testing across platforms

Linux and Windows drivers compile on different machines, so no single run can
exercise both. Anything both backends depend on is expressed as platform-neutral
data and tested in `crates/pw-graph-backend/tests/parity_contract.rs`, which
runs everywhere: meter eligibility, meter policy resolution, audio state
semantics, per-node capability reporting, the SPA gain curve, and whether a
backend asks to be polled.

Behaviour that genuinely needs a live daemon stays in each driver's own tests
and is opt-in through environment variables (`PW_GRAPH_TEST_METERS`,
`PW_GRAPH_TEST_LINKS`, `PW_GRAPH_TEST_RELAY`, `PW_GRAPH_TEST_VOLUME`), so an
offline or containerised build does not fail for want of an audio server.

When adding a rule both backends rely on, put the rule in `api` and test it
there. A shared rule tested only inside one driver is untested on the other
platform.