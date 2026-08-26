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
| Create a connection | Yes | No | Platform limitation |
| Remove a connection | Yes | No | Platform limitation |
| Select an existing connection | Yes | Yes | Equivalent |
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
| Read volume | Only after this app writes it | Yes | Partial (Linux) |
| Read mute | Only after this app writes it | Yes | Partial (Linux) |
| Follow external changes | No | Only at refresh | Partial (both) |
| Volume above unity | Yes, to 150% | No, clamped at 100% | Platform limitation |
| Per-node capability reporting | Yes | Yes | Equivalent |

The backend owns audio state. `GraphDriver::node_audio_state` returns a
`NodeAudioState` whose `volume` and `muted` are `Option`, where `None` means
"this backend cannot tell you". The UI renders that as an unknown value — a
dimmed fader — and never substitutes a number of its own. Before this, every
card claimed 90% and unmuted regardless of the real system state, which was
visibly wrong on Windows.

Two gaps remain here:

- **Linux native readback** — *Missing*. Reading PipeWire volume and mute needs
  a `Props` param listener bound to each node proxy, which the driver does not
  set up yet. Until it does, `node_audio_state` reports a value only for nodes
  this process has written, and `volume_readable`/`mute_readable` stay false
  otherwise. The write path is unaffected.
- **Windows volume range** — *Bug*. The UI track runs to 150% while Core Audio
  clamps a scalar at 100%. The driver now records the clamped value, so the
  card no longer shows a boost the system did not apply, but the track itself
  should be limited per backend rather than showing a range that cannot be
  reached. `NodeAudioState` has no maximum-volume field yet; adding one is the
  natural fix.

### Metering

| Feature | Linux (PipeWire) | Windows (Core Audio) | Status |
| --- | --- | --- | --- |
| Meter a capture source | Yes | Yes | Equivalent |
| Meter a playback sink | Yes, through its monitor | Yes | Equivalent |
| Meter an application stream | Yes | No | Missing (Windows) |
| Meter policies (off/on-demand/always) | Yes | Yes | Equivalent |

Playback sinks used to be excluded from metering on Linux: eligibility required
an audio *source* port, which a sink does not have, so speakers and other output
devices silently showed nothing even though the meter stream already knew how to
read a sink through its monitor. Fixed; `api::is_measurable_audio_node` now
holds the rule for both backends and is unit-tested.

On Windows, endpoints expose `IAudioMeterInformation` and application sessions
do not, so a session reports no meter capability rather than being given a meter
it can never fill. `IAudioSessionControl` can be extended to per-session
metering; that is *Missing*, not a platform limitation.

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
| Audio relay | Yes | No | Missing |
| ALSA MIDI | Yes | No | Missing |

Effect *insertion* depends on rewiring an existing link, so it cannot exist on
Windows without routing. Free-standing effect nodes do not have that constraint
and are merely unbuilt.

## Roadmap

Ordered by how much each one improves what a user actually sees.

1. **PipeWire native volume/mute readback.** Closes the last place where a card
   can show an unknown value on Linux. Needs a `Props` param listener per node
   proxy and a cache invalidated by param-changed events.
2. **Backend-aware volume range.** Add a maximum to `NodeAudioState` and drive
   the fader's top of scale from it, so the Windows track stops offering a
   boost that cannot be applied.
3. **Windows session metering.** Extend `IAudioSessionControl` handling so
   per-application meters work, and flip session `meter_peak`/`meter_rms` on.
4. **Refresh churn.** The app refreshes roughly every 500 ms even when the
   Windows topology is not dirty. `graph_dirty` already exists; the refresh loop
   should consult it.
5. **`OnSessionCreated` handling.** Currently coarse; a new session should be
   folded in without a full re-enumeration.
6. **Windows free-standing effect nodes.** Requires a processing host that does
   not depend on graph routing.

## Testing across platforms

Linux and Windows drivers compile on different machines, so no single run can
exercise both. Anything both backends depend on is expressed as platform-neutral
data and tested in `crates/pw-graph-backend/tests/parity_contract.rs`, which
runs everywhere: meter eligibility, meter policy resolution, audio state
semantics, and per-node capability reporting.

Behaviour that genuinely needs a live daemon stays in each driver's own tests
and is opt-in through environment variables (`PW_GRAPH_TEST_METERS`,
`PW_GRAPH_TEST_LINKS`, `PW_GRAPH_TEST_RELAY`), so an offline or containerised
build does not fail for want of an audio server.

When adding a rule both backends rely on, put the rule in `api` and test it
there. A shared rule tested only inside one driver is untested on the other
platform.
