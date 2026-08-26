# qpwgraph-rs — Platform Parity Phase 2

## Branch

Create this work from the latest `main`:

```bash
git switch main
git pull --ff-only
git switch -c feature/platform-parity-phase-2
````

Baseline merge commit at planning time:

```text
f829d391804c5396e0bd4baef54d580ab89283f3
```

Do not continue development on the already-merged
`feature/linux-windows-audio-parity-foundation` branch.

---

# Objective

Finish the current Linux/Windows platform-parity work to production quality.

This phase has four priorities:

1. fix correctness bugs already present in `main`;
2. finish integration of Windows MIDI/Core Audio with the composite driver and UI;
3. expose feasible Windows features that backend code already supports;
4. improve remaining platform-specific gaps without pretending unsupported
   Windows routing features exist.

The backend remains the source of truth.

Never invent UI state that the backend did not report.

Do not enable arbitrary Windows Core Audio `connect` / `disconnect` merely to
make the UI behave like PipeWire.

---

# Non-negotiable platform rule

Windows Core Audio observed session -> endpoint links are not ordinary mutable
patchbay links.

Keep:

```text
WindowsAudioDriver:
connect = false
disconnect = false
is_link_mutable = false
```

Windows MIDI is different:

```text
WindowsMidiDriver:
connect = true
disconnect = true
links are mutable
```

The composite/UI must preserve that distinction on a per-node/per-link basis.

---

# Phase 0 — Establish baseline

Before changing functionality:

```bash
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Record which commands run on the current OS.

Because Linux and Windows native drivers compile conditionally, final acceptance
requires both a Windows and Linux build.

Do not claim cross-platform validation from only one OS.

---

# Phase 1 — Fix CompositeDriver Windows MIDI delegation

This is the highest-priority correctness work.

Inspect:

```text
crates/pw-graph-app-core/src/lib.rs
crates/pw-graph-backend/src/windows_midi.rs
```

## 1.1 `is_link_mutable`

`WindowsMidiDriver` creates real native mutable links.

`CompositeDriver::is_link_mutable()` must delegate
`CompositeRoute::WindowsMidi` to `windows_midi`.

It must NOT fall into the current false/default branch.

Required behavior:

```rust
Some(CompositeRoute::WindowsMidi) => {
    #[cfg(target_os = "windows")]
    {
        return self
            .windows_midi
            .as_ref()
            .is_some_and(|driver| driver.is_link_mutable(link));
    }
}
```

Keep Windows Audio observed links immutable.

### Regression tests

Add tests proving:

* Windows Audio observed link -> immutable.
* Windows MIDI native link -> mutable.
* missing/unknown link -> immutable.
* composite result equals owning backend result.

---

## 1.2 Delegate Windows MIDI node positioning

`WindowsMidiDriver` implements `set_node_position`, but
`CompositeDriver::set_node_position()` currently rejects Windows MIDI nodes.

Fix this.

Expected behavior:

```text
Windows MIDI node drag
    -> CompositeDriver
    -> WindowsMidiDriver::set_node_position
    -> merged graph position updated
    -> success
```

This must also make `MoveNodesCommand` and Arrange work with Windows MIDI nodes.

### Tests

Add:

```text
windows_midi_node_position_is_forwarded
move_nodes_accepts_windows_midi_nodes
mixed_windows_audio_and_midi_arrange_does_not_fail_due_to_midi
```

Use deterministic/fake drivers where practical instead of requiring physical
MIDI hardware.

---

## 1.3 Delegate node/port ownership predicates

`CompositeDriver::is_node_type()` and `is_port_type()` must recognize the
Windows MIDI child.

Required:

```text
NodeType::WindowsMidi -> WindowsMidiDriver
PortType::MidiJack    -> WindowsMidiDriver
```

Add regression tests.

---

# Phase 2 — Correct composite refresh semantics

Review the contract:

```rust
GraphDriver::graph_dirty()
GraphDriver::reports_graph_changes()
```

The current composite logic must not claim that *every* live child reports graph
changes while silently excluding a Windows MIDI child from the calculation.

Windows Audio is event driven.

WinMM MIDI currently requires reconciliation/polling for device hotplug unless
a real device-change notification mechanism is added.

Do NOT solve this by returning to full Core Audio enumeration every 500 ms.

## Preferred architecture

Introduce per-child refresh responsibility instead of one global
"all children report changes" boolean if necessary.

Desired behavior:

```text
Windows Audio:
- refresh immediately when dirty
- otherwise rare safety reconciliation

Windows MIDI:
- refresh at a reasonable hotplug interval
- MIDI reconciliation must not force unnecessary Core Audio re-enumeration
```

A simple acceptable implementation is separate child refresh deadlines.

For example:

```text
Core Audio safety reconcile: ~5 s
WinMM device reconcile:      ~2-5 s
UI pump:                     remains inexpensive
```

Do not optimize blindly; keep the implementation simple and testable.

### Tests

Cover combinations:

```text
only event-driven child
only polling child
event-driven + polling child
dirty event-driven child
poll interval expiry
```

---

# Phase 3 — Make unknown audio state truly unknown

The central Phase-1 contract was:

```text
Some(value) = backend reported a value
None        = backend does not currently know
```

The UI must preserve that for BOTH volume and mute.

## 3.1 Unknown mute

Current projection must not do this conceptually:

```rust
muted.unwrap_or(false)
```

because that converts "unknown" into "unmuted".

Add explicit UI state:

```text
audio_mute_known: bool
audio_muted: bool
```

The value is only meaningful when `audio_mute_known == true`.

### UI behavior

If mute is unknown:

* do not display it as definitely unmuted;
* visually indicate unknown/disabled state;
* do not invent a boolean;
* if mute is writable but unreadable, allow the action only if the UI semantics
  remain understandable.

Prefer a small explicit "not read"/unknown visual state rather than silently
choosing false.

### Tests

```text
unknown_mute_is_not_projected_as_unmuted
known_false_mute_is_projected_as_unmuted
known_true_mute_is_projected_as_muted
```

---

# Phase 4 — Fix partial PipeWire Props readback

Inspect:

```text
crates/pw-graph-backend/src/pipewire/readback.rs
crates/pw-graph-backend/src/pipewire.rs
```

`volume` and `mute` are independent optional readings.

Do not create a default `NodeAudioControl` and then treat the mere presence of
that record as proof that both values were read.

Bad semantic result:

```text
PipeWire reports volume only
-> default mute false created
-> UI incorrectly claims mute=false
```

Likewise:

```text
PipeWire reports mute only
-> default volume unity created
-> UI incorrectly claims volume=100%
```

## Preferred fix

Store independent state, ideally using the existing `NodeAudioState` semantics
or an equivalent structure:

```rust
volume: Option<f32>
muted: Option<bool>
```

Maintain read/write capability separately from whether a value is currently
known.

### Tests

Add parsing/application tests for:

```text
Props with volume only
Props with mute only
Props with both
Props with channelVolumes only
Props with neither
```

Assert that missing properties stay `None`.

---

# Phase 5 — Promote valid Windows callback readings

Inspect Windows callback handling.

A valid Core Audio callback payload is itself authoritative state.

Do not discard it just because the initial synchronous read previously failed.

Change logic equivalent to:

```rust
if state.volume_readable {
    state.volume = Some(volume);
}
```

to semantics equivalent to:

```rust
state.volume = Some(volume);
state.volume_readable = true;

state.muted = Some(muted);
state.mute_readable = true;
```

where appropriate.

The callback is evidence that the value is readable.

### Tests

```text
callback_promotes_unknown_volume_to_known
callback_promotes_unknown_mute_to_known
callback_updates_existing_known_values
```

Keep topology dirty state unchanged for pure volume/mute updates.

Changing a fader must not trigger a full graph rebuild.

---

# Phase 6 — Make meter lifecycle capability-driven

Inspect:

```text
crates/pw-graph-slint/src/bridge/meters.rs
crates/pw-graph-slint/src/model.rs
crates/pw-graph-backend/src/api.rs
```

Meter requests must be based on meter capability, NOT audio-control visibility.

Do not use:

```rust
node.has_audio_controls
```

as the definition of meterability.

Use per-node backend capability:

```rust
node.audio.capabilities.has_any_meter()
```

or an equivalent authoritative field.

## Required behavior

A node may have:

```text
volume controls + no meter
meter + no writable controls
peak meter only
peak + RMS
nothing
```

All cases must render correctly.

### Windows specifically

Core Audio session/endpoint meters are peak-only where that is what the native
interface exposes.

Do not fabricate RMS by copying peak into RMS.

Do not render a permanently-zero RMS bar when `meter_rms == false`.

### Tests

Add:

```text
meter_only_node_is_requested
control_only_node_is_not_requested_for_metering
peak_only_node_does_not_render_rms_as_supported
unmeterable_node_does_not_stay_in_waiting_state
```

---

# Phase 7 — Per-node meter rendering

The UI should expose meter capabilities independently:

```text
meter_peak_supported
meter_rms_supported
```

Do not assume every meter has both values.

Rendering rules:

```text
peak=true, rms=false -> peak only
peak=true, rms=true  -> both
both false           -> no meter UI
```

Keep `Waiting`, `Unavailable`, `Disabled`, `Demo`, and `Live` state semantics
meaningful only for nodes that actually have meter capability.

---

# Phase 8 — Windows MIDI stability and native identity

Current WinMM identity must not depend only on transient device index if a
stronger stable identity is available.

Research and implement the documented WinMM device-interface query mechanism
where supported.

Prefer a stable native identity based on the Windows device interface rather
than:

```text
input index 0
output index 2
```

because enumeration indices can change after hotplug/reboot.

## Requirements

* stable node IDs when the same physical/logical device remains present;
* input/output namespaces never collide;
* existing qpwgraph links do not accidentally attach to a different device
  after enumeration order changes;
* gracefully fall back when a stable interface identity cannot be obtained.

Do not break backend namespace encoding.

### Tests

Where native APIs cannot be unit-tested, isolate stable-ID derivation into a
pure helper and test:

```text
same identity -> same graph ID
different identity -> different graph ID
input/output types cannot collide
enumeration order does not affect stable ID
```

---

# Phase 9 — Windows MIDI real routing integration

After delegation/identity fixes, exercise the full user path:

```text
graph enumeration
-> card rendering
-> node drag
-> pin drag
-> connect
-> link rendering
-> link selection
-> edge reroute
-> disconnect
-> undo
-> redo
```

Keep WinMM one-input routing restrictions explicit.

If WinMM cannot fan one input to multiple outputs without external MIDI-thru
support, reject the second connection clearly rather than silently replacing the
first.

Test mutation using fake/native-abstraction tests when CI has no physical MIDI
device.

---

# Phase 10 — Improve `OnSessionCreated`

Current Windows session callback only says "something changed" and causes a full
session re-enumeration.

Use the supplied `IAudioSessionControl` more precisely where safe.

Do not introduce unsafe cross-thread COM lifetime behavior merely to avoid a
small refresh.

## Preferred safe architecture

The callback identifies which endpoint/session manager changed.

Signal the existing Core Audio worker to refresh only that endpoint's sessions.

Avoid rebuilding every endpoint when one application starts playback.

Possible flow:

```text
OnSessionCreated
    -> enqueue EndpointSessionsDirty(endpoint_id)
    -> worker owns COM access
    -> enumerate sessions for that endpoint
    -> merge session delta
    -> update graph
```

Keep COM operations on the established worker/apartment model.

### Tests

Test the platform-neutral delta/merge logic independently.

---

# Phase 11 — Expose Windows relay endpoint selection

The backend already has Windows WASAPI relay support.

Expose capture/playback endpoint choice through config and Slint UI.

## Config

Add stable fields such as:

```rust
relay_capture_endpoint_id: Option<String>
relay_playback_endpoint_id: Option<String>
```

or equivalent backward-compatible representation.

Old config files must continue loading.

Unknown/removed endpoint IDs must gracefully fall back to system default.

## UI

Relay settings should expose:

```text
Capture source:
- System default
- <endpoint A>
- <endpoint B>

Playback destination:
- System default
- <endpoint A>
- <endpoint B>
```

Display friendly endpoint names while persisting stable IDs.

Changing the choice should restart/reconfigure only what is necessary.

Do not restart unrelated graph functionality.

### Tests

```text
relay_endpoint_config_round_trip
missing_saved_endpoint_falls_back_to_default
endpoint_selection_passes_stable_id_to_backend
```

---

# Phase 12 — Windows relay robustness

Exercise:

```text
host start
host stop
client connect
client disconnect
capture loop
render loop
endpoint changes
device removal
reconnect
```

Make runtime errors visible instead of silently terminating relay threads.

Do not claim microphone-device emulation on Windows.

Windows cannot create a general selectable capture endpoint from ordinary
user-mode Core Audio APIs.

Keep this documented as a platform limitation unless a separate driver-based
solution is intentionally introduced.

---

# Phase 13 — Linux control subscriptions

PipeWire currently reads Props during graph refresh.

Windows follows external volume/mute changes using callbacks.

Improve Linux parity by maintaining Props subscriptions for relevant nodes if it
can be implemented without destabilizing the PipeWire loop.

Desired behavior:

```text
pavucontrol/media-key change
-> backend cache updates
-> UI updates
-> no graph rebuild required
```

Reuse the backend-as-source-of-truth model.

Avoid one blocking roundtrip per UI frame.

If long-lived subscriptions prove too invasive, keep current rebuild readback
correct first and implement this as a separate commit/PR.

---

# Phase 14 — Windows effects: feasible subset only

Do NOT attempt to implement PipeWire-style effect insertion into arbitrary
Windows Core Audio session links.

That requires routing capabilities Core Audio does not provide through the
supported public graph model.

The feasible feature is free-standing DSP on streams qpwgraph-rs owns.

Possible first targets:

```text
relay capture processing
relay playback processing
application-owned loopback/render pipelines
```

Reuse `pw-graph-effects` DSP definitions where possible.

Architect effect processing independently of PipeWire graph rewiring.

Do not advertise effect insertion on observed Core Audio links.

---

# Phase 15 — Per-application Windows routing

Treat this separately and conservatively.

Do NOT ship guessed COM vtable layouts.

The previous investigation found Windows version-dependent undocumented
`AudioPolicyConfig` interfaces and unsafe guessed slots.

A wrong call can be undefined behavior and may modify persisted user audio
settings.

Only implement this if a trustworthy interface declaration/signature is found
from a reliable source and validated on supported Windows versions.

Requirements if attempted:

* exact IID detection;
* exact documented/reliably-derived method declarations;
* version gating;
* graceful `Unsupported` fallback;
* no guessed vtable slot probing in production code;
* test on multiple Windows builds.

Until then:

```text
Core Audio observed application->endpoint links remain immutable.
```

---

# Phase 16 — Single-application relay capture

This is different from per-session metering.

Metering can use a session peak meter.

Capturing the actual PCM stream for one process requires process-loopback
support.

Only implement against a Windows version that supports the required activation
API and use Microsoft's ApplicationLoopback implementation as the behavioral
reference.

Do not resurrect code that previously caused heap corruption without fully
understanding the `PROPVARIANT`/activation-parameter ownership and lifetime
requirements.

Feature-gate by Windows build/API availability and fail safely.

---

# Phase 17 — Connection/UI capability cleanup

Audit every connection action.

Backend-wide:

```rust
capabilities().connect
```

is only a broad "somewhere in this composite can connect" flag.

For pointer interactions use per-node/per-port ownership whenever possible.

Required on Windows mixed graph:

```text
Core Audio pin:
- selectable/inspectable
- not draggable for routing

Windows MIDI pin:
- draggable/connectable

Core Audio observed link:
- selectable
- not disconnectable
- not reroutable

Windows MIDI link:
- selectable
- disconnectable
- reroutable
```

Verify Easy and Advanced modes preserve these rules.

---

# Phase 18 — Patchbay persistence audit

Patchbay persistence must only include mutable/durable links.

Never save observed Windows Core Audio session relationships as user-owned
patchbay connections.

Windows MIDI links may be mutable, but decide explicitly whether they should be
persisted across application restarts.

If persistence is supported:

* use stable device identity, not transient WinMM indices;
* tolerate missing devices;
* restore when devices appear;
* do not connect to a different device that reused an index.

Add tests around `is_link_mutable` and snapshot filtering.

---

# Phase 19 — Documentation correctness

Update:

```text
docs/platform-parity.md
README.md
```

Audit every table against actual code.

Examples to fix:

* Windows per-session metering now exists where successfully supported.
* Windows meter is peak-only where RMS is not available.
* Windows MIDI routing exists.
* Windows Audio arbitrary patchbay routing does not.
* volume maximum is per-node.
* PipeWire native Props readback exists.
* refresh behavior reflects the final implementation.

Do not call a feature "Equivalent" if one platform only provides a reduced
measurement such as peak-only versus RMS+peak.

Use:

```text
Equivalent
Partial
Missing
Platform limitation
Bug
```

consistently.

---

# Phase 20 — CI is required

The project currently needs real cross-platform checks.

Add GitHub Actions for at least:

```text
Windows:
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings

Linux:
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Install native Linux development packages required by PipeWire/ALSA as part of
the workflow.

Do not require a live PipeWire daemon or physical MIDI device for ordinary CI.

Native hardware integration tests should remain opt-in.

Separate:

```text
unit/contract tests
compile tests
live audio integration tests
```

clearly.

CI must be running before claiming the branch is merge-ready.

---

# Required architecture principles

## Backend truth

Never keep an authoritative duplicate audio state inside the UI.

```text
native backend
   -> NodeAudioState / NodeCapabilities
   -> projection
   -> Slint
```

UI writes request a change.

The subsequent backend state is authoritative.

---

## Independent capabilities

Do not couple unrelated capabilities.

A node may be:

```text
volume readable but not writable
mute writable but not currently readable
meter peak-only
meter-only
connectable but no audio controls
non-connectable but fully controllable for volume
```

Model each explicitly.

---

## Platform limitations are not bugs

Do not implement fake parity.

Examples:

```text
PipeWire arbitrary routing:
supported

Windows Core Audio arbitrary graph routing:
not supported by the current public graph model

Windows MIDI routing:
supported through WinMM
```

The UI must adapt to the backend rather than lie.

---

# Suggested commit sequence

Keep commits reviewable.

```text
1. fix(windows): complete Windows MIDI composite delegation
2. fix(ui): preserve unknown mute state
3. fix(pipewire): preserve partial Props readback
4. fix(windows): promote Core Audio callback readings
5. fix(meters): drive requests and rendering from node capabilities
6. test: expand platform capability contract coverage
7. perf: separate native backend refresh responsibilities
8. fix(windows-midi): stabilize device identities across enumeration
9. feat(ui): expose Windows relay endpoint selection
10. fix(windows): refine audio session lifecycle updates
11. feat(pipewire): subscribe to control parameter changes
12. test(ci): add Linux and Windows validation workflows
13. docs: synchronize platform parity documentation
```

Large optional work should follow separately:

```text
feat(windows): application-owned effect processing
feat(windows): process loopback capture
feat(windows): per-app output routing
```

Do not combine unsafe/experimental Windows routing work into the correctness PR.

---

# Tests that must exist before merge

At minimum:

```text
windows_midi_link_is_mutable_through_composite
windows_audio_observed_link_stays_immutable
windows_midi_position_is_forwarded
windows_midi_node_type_is_recognized
windows_midi_port_type_is_recognized

unknown_mute_remains_unknown
known_unmuted_is_not_confused_with_unknown

pipewire_volume_only_readback_keeps_mute_unknown
pipewire_mute_only_readback_keeps_volume_unknown

windows_callback_promotes_unknown_audio_state

meter_only_node_is_requested
control_only_node_is_not_requested
peak_only_meter_does_not_claim_rms
unmeterable_node_does_not_show_waiting_forever

windows_audio_pin_is_not_connectable
windows_midi_pin_is_connectable
windows_audio_link_cannot_be_rerouted
windows_midi_link_can_be_rerouted

mixed_backend_refresh_does_not_reenumerate_core_audio_unnecessarily
```

Plus existing canvas and connection regression tests must stay green.

---

# Manual Windows acceptance checklist

Use a Windows machine with real Core Audio devices.

Verify:

```text
[ ] playback endpoints appear
[ ] capture endpoints appear
[ ] active application sessions appear
[ ] no duplicate pins/ports
[ ] existing Core Audio links are selectable
[ ] Core Audio links cannot be deleted/rerouted
[ ] endpoint volume reflects actual Windows value
[ ] application volume reflects actual session value
[ ] external volume changes update UI
[ ] mute changes update UI
[ ] Windows fader ends at 100%
[ ] application peak meter moves for the correct application
[ ] unrelated applications do not share the same meter
[ ] no fake RMS meter is shown
[ ] graph does not fully re-enumerate on every fader movement
[ ] Windows MIDI devices appear
[ ] Windows MIDI nodes can be moved
[ ] Windows MIDI pins can connect
[ ] MIDI links can disconnect
[ ] MIDI links can reroute
[ ] MIDI undo/redo works
[ ] Easy mode does not make Core Audio pins falsely routable
[ ] relay capture works
[ ] relay playback works
[ ] selected relay endpoints persist
```

---

# Manual Linux acceptance checklist

Verify:

```text
[ ] PipeWire sinks meter through monitor
[ ] PipeWire sources meter correctly
[ ] application streams meter correctly
[ ] volume readback matches pavucontrol
[ ] mute readback matches pavucontrol
[ ] missing Props fields stay unknown
[ ] 150% boost mapping still works
[ ] normal connection creation works
[ ] connection deletion works
[ ] connection rerouting works
[ ] Easy mode works
[ ] Advanced mode works
[ ] patchbay persistence works
[ ] ALSA MIDI behavior remains intact
[ ] relay behavior remains intact
```

---

# Definition of done

This phase is complete when:

1. all identified correctness issues above are fixed;
2. Windows MIDI is fully integrated through CompositeDriver and UI;
3. unknown backend state is never fabricated;
4. meter behavior is per-node capability-driven;
5. Windows Audio remains safely non-routable where unsupported;
6. relay endpoint selection is user-accessible;
7. refresh behavior avoids unnecessary Core Audio enumeration;
8. PipeWire regressions are prevented;
9. documentation matches actual behavior;
10. Linux and Windows CI both pass;
11. live Windows and Linux smoke tests are recorded separately from CI;
12. unsupported experimental Windows APIs are not called through guessed ABIs.
