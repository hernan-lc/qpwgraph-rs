//! Public backend contracts shared by native and deterministic drivers.

use pw_graph_core::{
    Graph, GraphError, Link, LinkId, Node, NodeId, NodeType, PortId, PortKey, PortType,
};
use pw_graph_effects::{EffectDescriptor, EffectInstanceConfig};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

/// How freely a backend may open helper streams to measure audio levels.
///
/// Measuring a PipeWire node means connecting a real capture stream to it. The
/// session manager links that stream like any other client, which resumes
/// suspended devices and can make the daemon renegotiate the graph rate. Doing
/// that for every audio node continuously can visibly rewrite the user's audio
/// configuration, so metering defaults to [`MeterPolicy::OnDemand`], which is
/// limited to nodes represented by a currently visible application window.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MeterPolicy {
    Disabled,
    #[default]
    OnDemand,
    Always,
}

impl MeterPolicy {
    /// All variants, in declaration order.
    pub const ALL: [Self; 3] = [Self::Disabled, Self::OnDemand, Self::Always];

    /// Stable string representation of each variant.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "off",
            Self::OnDemand => "on-demand",
            Self::Always => "always",
        }
    }

    /// Parse with extended aliases per variant. Unknown values fall back to
    /// the default so a hand-edited or older config file still starts.
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" | "disabled" | "none" => Self::Disabled,
            "always" | "all" => Self::Always,
            "on-demand" => Self::OnDemand,
            _ => Self::default(),
        }
    }
}

#[derive(Debug, Error)]
pub enum BackendError {
    #[error(transparent)]
    Graph(#[from] GraphError),
    #[error("backend operation is not available: {0}")]
    Unsupported(String),
    #[error("native backend error: {0}")]
    Native(String),
}

impl BackendError {
    pub(crate) fn native(message: impl std::fmt::Display) -> Self {
        Self::Native(message.to_string())
    }

    pub(crate) fn unsupported(message: impl std::fmt::Display) -> Self {
        Self::Unsupported(message.to_string())
    }
}

macro_rules! backend_error_constructors {
    ($($name:ident => $message:expr),+ $(,)?) => {
        impl BackendError {
            $(
                pub(crate) fn $name() -> Self {
                    Self::native($message)
                }
            )+
        }
    };
    ($($name:ident($($arg:ident: $ty:ty),*) => $message:expr),+ $(,)?) => {
        impl BackendError {
            $(
                pub(crate) fn $name($($arg: $ty),*) -> Self {
                    Self::native($message)
                }
            )+
        }
    };
}

backend_error_constructors! {
    effect_source_unavailable => "effect source port is unavailable",
    effect_destination_unavailable => "effect destination port is unavailable",
    effect_not_linked => "effect source and destination are not linked",
    effect_source_disappeared => "effect source disappeared while removing effect",
    effect_destination_disappeared => "effect destination disappeared while removing effect",
    effect_routing_incomplete => "effect routing is incomplete and cannot be restored",
}

backend_error_constructors! {
    unknown_effect_instance(instance_id: &str) => format!("unknown effect instance {instance_id}"),
    effect_already_exists(instance_id: &str) => format!("effect instance {instance_id} already exists"),
    effect_create_failed(error: impl std::fmt::Display) => format!("could not create effect: {error}"),
}

pub type BackendResult<T> = Result<T, BackendError>;

/// Parameters used to create a free-standing effect node. The node has one
/// audio input and one audio output, so callers can patch it like any other
/// node in the graph.
#[derive(Clone, Debug)]
pub struct EffectNodeRequest {
    pub instance_id: String,
    pub effect_id: String,
    pub module_path: Option<String>,
    pub enabled: bool,
    pub parameters: BTreeMap<String, f32>,
    /// Initial canvas position in logical scene coordinates. Backends that do
    /// not persist layouts may still use it for their in-memory graph model.
    pub position: [f32; 2],
}

/// An effect insertion request. The endpoint keys are captured before the
/// graph is mutated so an effect can be restored after PipeWire global IDs
/// change.
#[derive(Clone, Debug)]
pub struct EffectInsertRequest {
    pub instance_id: String,
    pub effect_id: String,
    pub module_path: Option<String>,
    pub source: PortKey,
    pub destination: PortKey,
    pub enabled: bool,
    pub parameters: BTreeMap<String, f32>,
    /// Position for the newly inserted effect node.
    pub position: [f32; 2],
}

impl From<EffectInsertRequest> for EffectNodeRequest {
    fn from(request: EffectInsertRequest) -> Self {
        Self {
            instance_id: request.instance_id,
            effect_id: request.effect_id,
            module_path: request.module_path,
            enabled: request.enabled,
            parameters: request.parameters,
            position: request.position,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct EffectInstance {
    pub config: EffectInstanceConfig,
    pub node_id: NodeId,
    pub input_port: PortId,
    pub output_port: PortId,
    /// Original endpoints when this instance was inserted into an existing
    /// link. Free-standing nodes leave both endpoints unset and can be wired
    /// through the regular graph canvas.
    pub source: Option<PortKey>,
    pub destination: Option<PortKey>,
    pub error: Option<String>,
}

/// Effect operations are intentionally separate from topology operations. A
/// backend that cannot host realtime processing can still implement the graph
/// API and return a precise Unsupported error here.
pub trait EffectDriver {
    fn effect_descriptors(&self) -> Vec<EffectDescriptor> {
        Vec::new()
    }

    fn effect_instances(&self) -> Vec<EffectInstance> {
        Vec::new()
    }

    /// Whether the backend can create a processing node. Exposing an effect
    /// descriptor is not enough: the app uses this capability to avoid
    /// presenting an enabled Create action on a backend that cannot host DSP.
    fn supports_effect_nodes(&self) -> bool {
        false
    }

    /// Create an unconnected effect node which can be linked through normal
    /// graph operations.
    fn create_effect_node(&mut self, _request: EffectNodeRequest) -> BackendResult<EffectInstance> {
        Err(unsupported_effect())
    }

    fn insert_effect(&mut self, _request: EffectInsertRequest) -> BackendResult<EffectInstance> {
        Err(unsupported_effect())
    }

    fn set_effect_enabled(&mut self, _instance_id: &str, _enabled: bool) -> BackendResult<()> {
        Err(unsupported_effect())
    }

    fn set_effect_parameter(
        &mut self,
        _instance_id: &str,
        _parameter: &str,
        _value: f32,
    ) -> BackendResult<()> {
        Err(unsupported_effect())
    }

    fn remove_effect(&mut self, _instance_id: &str) -> BackendResult<()> {
        Err(unsupported_effect())
    }
}

fn unsupported_effect() -> BackendError {
    BackendError::unsupported("effect processing is not available for this backend")
}

fn unsupported_node_op(operation: &str) -> BackendError {
    BackendError::unsupported(format!("node {operation} is not supported by this backend"))
}

/// Operations a backend can perform on the resources it exposes.
///
/// Capabilities are advisory for presentation and command routing. Every
/// mutating method still validates the operation and returns
/// [`BackendError::Unsupported`] when a caller reaches an unavailable feature.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BackendCapabilities {
    pub topology: bool,
    pub connect: bool,
    pub disconnect: bool,
    pub volume: bool,
    pub mute: bool,
    pub meters: bool,
    pub effects: bool,
    pub relay: bool,
}

impl BackendCapabilities {
    /// Combine capabilities when a composite exposes multiple child drivers.
    pub const fn union(self, other: Self) -> Self {
        Self {
            topology: self.topology || other.topology,
            connect: self.connect || other.connect,
            disconnect: self.disconnect || other.disconnect,
            volume: self.volume || other.volume,
            mute: self.mute || other.mute,
            meters: self.meters || other.meters,
            effects: self.effects || other.effects,
            relay: self.relay || other.relay,
        }
    }
}

/// A normalized, node-level audio reading supplied by a backend.
///
/// PipeWire exposes graph topology separately from audio buffers, so meters
/// are deliberately kept as an optional side channel. An empty collection is
/// a valid result for backends that do not provide runtime audio data.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AudioMeter {
    pub node_id: NodeId,
    /// Port represented by this reading. A node-level fallback is used when
    /// the backend cannot expose a stable port association.
    pub port_id: Option<PortId>,
    /// Root-mean-square level normalized to 0..=1.
    pub rms: f32,
    /// Peak level normalized to 0..=1.
    pub peak: f32,
    /// Milliseconds since the backend received the last audio buffer.
    pub age_ms: u32,
    pub available: bool,
}

/// Whether a media class names a playback device whose output can be observed
/// through its monitor ports.
///
/// PipeWire device sinks (`Audio/Sink`) carry the audio a user actually hears.
/// Application streams (`Stream/Output/Audio`, `Stream/Input/Audio`) never
/// match, because a stream has no monitor of its own.
pub fn media_class_is_playback_sink(media_class: &str) -> bool {
    media_class.to_ascii_lowercase().contains("sink")
}

/// Whether a node can be metered at all.
///
/// A node is measurable when either
/// * it exposes an audio source port -- capture devices and the output side of
///   application streams, which a helper stream can read directly; or
/// * it is a playback sink with audio input ports, which is read through its
///   monitor. Sinks used to be excluded because the check only looked for
///   source ports, so speakers and other output devices never showed a meter
///   even though the meter stream already knew how to capture them.
///
/// Deciding this from plain data keeps the rule testable on every platform,
/// including the ones where the PipeWire driver is not compiled at all.
pub fn is_measurable_audio_node(
    media_class: &str,
    has_audio_source_port: bool,
    has_audio_sink_port: bool,
) -> bool {
    if has_audio_source_port {
        return true;
    }
    media_class_is_playback_sink(media_class) && has_audio_sink_port
}

/// Which nodes should own a meter right now, given a policy.
///
/// Shared by every backend so the three policies mean the same thing
/// everywhere: `Disabled` meters nothing, `Always` meters everything eligible,
/// and `OnDemand` meters only what the UI asked for and is eligible. A request
/// for a node that cannot be measured is ignored rather than honoured.
pub fn nodes_to_meter(
    policy: MeterPolicy,
    measurable: &BTreeSet<NodeId>,
    requested: &BTreeSet<NodeId>,
) -> BTreeSet<NodeId> {
    match policy {
        MeterPolicy::Disabled => BTreeSet::new(),
        MeterPolicy::Always => measurable.clone(),
        MeterPolicy::OnDemand => requested.intersection(measurable).copied().collect(),
    }
}

/// Audio controls exposed by a graph node when its backend supports them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NodeAudioControl {
    pub muted: bool,
    pub volume: f32,
}

impl Default for NodeAudioControl {
    fn default() -> Self {
        Self {
            muted: false,
            volume: 1.0,
        }
    }
}

/// What one node can actually do, as opposed to what its backend can do
/// somewhere in the graph.
///
/// [`BackendCapabilities`] answers "does this backend have volume support at
/// all", which is too coarse to drive a card: a Windows endpoint exposes
/// volume and mute while an observed application session may expose neither,
/// and a PipeWire effect node has no audio controls even though the PipeWire
/// backend does. The UI decides which controls to draw from this.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NodeCapabilities {
    pub volume_read: bool,
    pub volume_write: bool,
    pub mute_read: bool,
    pub mute_write: bool,
    pub meter_peak: bool,
    pub meter_rms: bool,
}

impl NodeCapabilities {
    /// Nothing is supported. The right answer for a node the backend does not
    /// recognise, and the safe answer while state is still unknown.
    pub const NONE: Self = Self {
        volume_read: false,
        volume_write: false,
        mute_read: false,
        mute_write: false,
        meter_peak: false,
        meter_rms: false,
    };

    /// Full read/write control plus both meter kinds.
    pub const FULL: Self = Self {
        volume_read: true,
        volume_write: true,
        mute_read: true,
        mute_write: true,
        meter_peak: true,
        meter_rms: true,
    };

    /// Whether any audio control at all should be drawn for this node.
    pub const fn has_any_control(self) -> bool {
        self.volume_read || self.volume_write || self.mute_read || self.mute_write
    }

    /// Whether any meter should be drawn for this node.
    pub const fn has_any_meter(self) -> bool {
        self.meter_peak || self.meter_rms
    }
}

/// The audio state of one node, as reported by the backend that owns it.
///
/// The backend is the source of truth. `None` means "this backend cannot tell
/// you", which is deliberately distinct from a real reading -- the UI must not
/// invent a value to fill the gap, because a fabricated 90%/unmuted card looks
/// exactly like a real one and misreports the system to the user.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NodeAudioState {
    /// Linear scalar where 1.0 is unity gain. Backends that allow boost report
    /// values above 1.0; backends that clamp at unity never do.
    pub volume: Option<f32>,
    pub muted: Option<bool>,
    pub volume_readable: bool,
    pub volume_writable: bool,
    pub mute_readable: bool,
    pub mute_writable: bool,
}

impl NodeAudioState {
    /// A node whose backend exposes no audio controls at all.
    pub const UNSUPPORTED: Self = Self {
        volume: None,
        muted: None,
        volume_readable: false,
        volume_writable: false,
        mute_readable: false,
        mute_writable: false,
    };

    /// A fully controllable node with both values already read.
    pub fn readable(volume: f32, muted: bool) -> Self {
        Self {
            volume: Some(volume),
            muted: Some(muted),
            volume_readable: true,
            volume_writable: true,
            mute_readable: true,
            mute_writable: true,
        }
    }

    /// Whether this node exposes any audio control.
    pub const fn is_supported(&self) -> bool {
        self.volume_readable || self.volume_writable || self.mute_readable || self.mute_writable
    }

    /// The control half of this node's capabilities. Meter capability is not
    /// derivable from audio state, so it is left off here and merged by the
    /// backend's `node_capabilities`.
    pub const fn control_capabilities(&self) -> NodeCapabilities {
        NodeCapabilities {
            volume_read: self.volume_readable,
            volume_write: self.volume_writable,
            mute_read: self.mute_readable,
            mute_write: self.mute_writable,
            meter_peak: false,
            meter_rms: false,
        }
    }
}

/// Common operations needed by commands, patchbay activation, and the UI.
pub trait GraphDriver: EffectDriver {
    /// Capabilities shared by the resources of this driver.
    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities::default()
    }

    fn refresh(&mut self) -> BackendResult<Vec<Node>>;
    fn connect(&mut self, src: PortId, dst: PortId) -> BackendResult<Link>;
    fn disconnect(&mut self, link: LinkId) -> BackendResult<Link>;

    /// Whether a link is a user-mutable native connection.
    ///
    /// Most graph backends expose only mutable links, so the default is true
    /// for links present in the driver's graph. Backends that project an
    /// observed relationship (for example a Windows application session's
    /// current endpoint) can override this and keep the relationship visible
    /// without allowing it into patchbay mutation or persistence flows.
    fn is_link_mutable(&self, link: LinkId) -> bool {
        self.graph().link(link).is_some()
    }

    /// Connect a stable pair, returning `None` when it is already present.
    /// The refresh is deliberately part of this helper: a UI action can be
    /// based on the previous frame while a PipeWire stream is being recreated.
    fn connect_by_key_if_missing(
        &mut self,
        output: &PortKey,
        input: &PortKey,
    ) -> BackendResult<Option<Link>> {
        self.refresh()?;
        self.allow_connection(output, input);
        if self.graph().find_link_by_keys(output, input).is_some() {
            return Ok(None);
        }
        let output_id = self.graph().resolve_port_key(output).ok_or_else(|| {
            BackendError::Native(format!(
                "source port {}:{} is no longer available",
                output.node_name, output.port_name
            ))
        })?;
        let input_id = self.graph().resolve_port_key(input).ok_or_else(|| {
            BackendError::Native(format!(
                "destination port {}:{} is no longer available",
                input.node_name, input.port_name
            ))
        })?;
        match self.connect(output_id, input_id) {
            Ok(link) => Ok(Some(link)),
            Err(BackendError::Graph(GraphError::DuplicateConnection(_, _))) => {
                self.refresh()?;
                if self.graph().find_link_by_keys(output, input).is_some() {
                    Ok(None)
                } else {
                    Err(BackendError::Graph(GraphError::DuplicateConnection(
                        output_id, input_id,
                    )))
                }
            }
            Err(error) => Err(error),
        }
    }

    /// Disconnect a stable pair, returning `None` when it already vanished.
    fn disconnect_by_key_if_present(
        &mut self,
        output: &PortKey,
        input: &PortKey,
    ) -> BackendResult<Option<Link>> {
        self.refresh()?;
        let link = self.graph().find_link_by_keys(output, input);
        let Some(link) = link else {
            self.suppress_connection(output, input);
            return Ok(None);
        };
        match self.disconnect(link.id) {
            Ok(link) => Ok(Some(link)),
            Err(BackendError::Graph(GraphError::MissingLink(_))) => {
                self.suppress_connection(output, input);
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    /// Native drivers may keep a short-lived suppression rule after a manual
    /// disconnect. An explicit connect/undo clears that rule.
    fn allow_connection(&mut self, _output: &PortKey, _input: &PortKey) {}

    /// Remember a manual deletion even if the link vanished during the
    /// refresh that preceded the operation.
    fn suppress_connection(&mut self, _output: &PortKey, _input: &PortKey) {}

    fn set_node_position(
        &mut self,
        node: pw_graph_core::NodeId,
        position: [f32; 2],
    ) -> BackendResult<()> {
        let _ = (node, position);
        Err(unsupported_node_op("layout"))
    }

    fn set_node_mute(&mut self, node: NodeId, muted: bool) -> BackendResult<()> {
        let _ = (node, muted);
        Err(unsupported_node_op("mute"))
    }

    fn set_node_volume(&mut self, node: NodeId, volume: f32) -> BackendResult<()> {
        let _ = (node, volume);
        Err(unsupported_node_op("volume"))
    }

    /// Current audio state of one node, read from the backend.
    ///
    /// The backend owns this: callers must render what is returned rather than
    /// remembering a value of their own. A backend that cannot read a node
    /// returns [`NodeAudioState::UNSUPPORTED`] rather than an error, so a graph
    /// containing one uncontrollable node still renders. An error is reserved
    /// for a node that does not exist or a backend that failed to answer.
    ///
    /// The default is `UNSUPPORTED`, which keeps every existing backend honest
    /// until it implements a real reader.
    fn node_audio_state(&self, node: NodeId) -> BackendResult<NodeAudioState> {
        let _ = node;
        Ok(NodeAudioState::UNSUPPORTED)
    }

    /// What this specific node supports, for deciding which controls to draw.
    ///
    /// The default derives the control half from [`Self::node_audio_state`] and
    /// the meter half from the backend-wide meter capability, which is right
    /// for backends that meter uniformly. Backends whose nodes differ (Windows
    /// endpoints meter, sessions do not) override this.
    fn node_capabilities(&self, node: NodeId) -> NodeCapabilities {
        let mut capabilities = self
            .node_audio_state(node)
            .map(|state| state.control_capabilities())
            .unwrap_or(NodeCapabilities::NONE);
        if self.capabilities().meters {
            capabilities.meter_peak = true;
            capabilities.meter_rms = true;
        }
        capabilities
    }

    fn graph(&self) -> &Graph;

    /// Returns whether registry state changed since the last `refresh`.
    /// Backends without event-driven registries may keep the default `false`.
    fn graph_dirty(&self) -> bool {
        false
    }

    fn is_node_type(&self, node_type: NodeType) -> bool {
        matches!(node_type, NodeType::PipeWire | NodeType::Effect)
    }

    fn is_port_type(&self, port_type: PortType) -> bool {
        matches!(
            port_type,
            PortType::Audio | PortType::Video | PortType::MidiJack
        )
    }

    /// Resolve the two endpoints of a link that an effect is about to replace,
    /// failing if either port vanished or no direct link exists between them.
    /// Both drivers share this preamble for `insert_effect`.
    fn effect_link_endpoints(
        &self,
        source: &PortKey,
        destination: &PortKey,
    ) -> BackendResult<(PortId, PortId, Link)> {
        let (output, input) = self.effect_resolve_and_validate_endpoints(
            source,
            destination,
            BackendError::effect_source_unavailable(),
            BackendError::effect_destination_unavailable(),
        )?;
        let link = self
            .graph()
            .links
            .values()
            .find(|link| link.output_port == output && link.input_port == input)
            .cloned()
            .ok_or_else(BackendError::effect_not_linked)?;
        Ok((output, input, link))
    }

    /// Resolve and validate the saved endpoints of an inserted effect. Both
    /// drivers call this while removing an effect so its original routing can
    /// still be restored after the effect node has been destroyed.
    fn effect_restore_endpoints(
        &self,
        source: &PortKey,
        destination: &PortKey,
    ) -> BackendResult<(PortId, PortId)> {
        self.effect_resolve_and_validate_endpoints(
            source,
            destination,
            BackendError::effect_source_disappeared(),
            BackendError::effect_destination_disappeared(),
        )
    }

    /// Shared endpoint resolution: resolve both ports by key, validate their
    /// directions, and return their numeric IDs.
    fn effect_resolve_and_validate_endpoints(
        &self,
        source: &PortKey,
        destination: &PortKey,
        source_error: BackendError,
        destination_error: BackendError,
    ) -> BackendResult<(PortId, PortId)> {
        let output = self.graph().resolve_port_key(source).ok_or(source_error)?;
        let input = self
            .graph()
            .resolve_port_key(destination)
            .ok_or(destination_error)?;
        let output_port = self
            .graph()
            .port(output)
            .ok_or(GraphError::MissingPort(output))?;
        let input_port = self
            .graph()
            .port(input)
            .ok_or(GraphError::MissingPort(input))?;
        if !output_port.direction.is_source() {
            return Err(GraphError::NotSource(output).into());
        }
        if !input_port.direction.is_sink() {
            return Err(GraphError::NotSink(input).into());
        }
        Ok((output, input))
    }

    fn audio_meters(&mut self) -> BackendResult<Vec<AudioMeter>> {
        Ok(Vec::new())
    }

    /// Choose how aggressively the backend may attach metering streams.
    fn set_meter_policy(&mut self, policy: MeterPolicy) -> BackendResult<()> {
        let _ = policy;
        Ok(())
    }

    /// Declare the nodes the UI currently wants a meter for.
    ///
    /// Under [`MeterPolicy::OnDemand`] this is the only thing that makes a
    /// backend open a helper stream. Callers are expected to repeat the
    /// request while the meter stays visible; backends may keep a stream alive
    /// briefly after the last request so minimizing/restoring a window does
    /// not thrash streams.
    fn request_meters(&mut self, nodes: &BTreeSet<NodeId>) -> BackendResult<()> {
        let _ = nodes;
        Ok(())
    }

    /// Release every helper stream this backend owns.
    ///
    /// This is the escape hatch for a session whose devices were resumed or
    /// renegotiated by metering: dropping the streams lets the session manager
    /// suspend and restore the nodes to their configured defaults.
    fn reset_audio_config(&mut self) -> BackendResult<()> {
        Ok(())
    }
}

/// Networked audio relay (phone-as-microphone / phone-as-speaker).
///
/// Like [`EffectDriver`], relay support is layered beside the graph API: a
/// backend without networking keeps the default `Unsupported` answers. The
/// PipeWire backend implements it with two client-owned virtual nodes —
/// `Relay Microphone` (a Source fed by peer audio) and `Relay Speaker` (a
/// Sink whose input is transmitted to peers) — plus the [`pw_graph_relay`]
/// engine for transport.
#[cfg(feature = "relay")]
pub use pw_graph_relay::{
    pairing::{
        build_qr_payload as relay_build_qr_payload, parse_qr_payload as relay_parse_qr_payload,
        QrPayload as RelayQrPayload,
    },
    qr as relay_qr, CodecKind as RelayCodecKind, DeviceKind as RelayDeviceKind,
    EngineStatus as RelayEngineStatus, LinkKind as RelayLinkKind, LocalLink as RelayLocalLink,
    PeerInfo as RelayPeerInfo, RelayEvent, Roles as RelayRoles, SessionId as RelaySessionId,
    SessionStatus as RelaySessionStatus, TransportPreference as RelayTransportPreference,
};

/// Parameters for starting the relay host.
#[cfg(feature = "relay")]
#[derive(Clone, Debug)]
pub struct RelayHostRequest {
    pub device_name: String,
    /// Pairing PIN clients must present.
    pub pin: String,
    /// TCP control port; 0 picks an ephemeral port.
    pub port: u16,
    pub codec: RelayCodecKind,
    pub frame_ms: u16,
    /// Preferred transport link for advertising and selection.
    pub transport: RelayTransportPreference,
}

#[cfg(feature = "relay")]
pub trait RelayDriver {
    /// Whether this backend can relay audio at all.
    fn relay_available(&self) -> bool {
        false
    }

    /// Snapshot of host/session state for the UI.
    fn relay_status(&self) -> RelayEngineStatus {
        RelayEngineStatus::default()
    }

    /// Whether the virtual relay microphone/speaker nodes currently exist.
    fn relay_devices_active(&self) -> bool {
        false
    }

    /// Create the virtual relay devices (if needed) and start listening for
    /// peers. Returns the bound TCP control port.
    fn relay_start_host(&mut self, _request: RelayHostRequest) -> BackendResult<u16> {
        Err(BackendError::Unsupported(
            "audio relay is not available for this backend".into(),
        ))
    }

    /// Stop listening; established sessions and virtual devices remain.
    fn relay_stop_host(&mut self) -> BackendResult<()> {
        Err(BackendError::Unsupported(
            "audio relay is not available for this backend".into(),
        ))
    }

    /// Create the virtual relay devices (if needed) and connect to a remote
    /// host as a client. Session outcome arrives via [`RelayEvent`]s.
    fn relay_connect(
        &mut self,
        _target: std::net::SocketAddr,
        _pin: &str,
        _roles: RelayRoles,
    ) -> BackendResult<()> {
        Err(BackendError::Unsupported(
            "audio relay is not available for this backend".into(),
        ))
    }

    fn relay_disconnect(&mut self, _session: RelaySessionId) -> BackendResult<()> {
        Err(BackendError::Unsupported(
            "audio relay is not available for this backend".into(),
        ))
    }

    /// Drain pending relay events. Call once per UI update.
    fn relay_events(&mut self) -> Vec<RelayEvent> {
        Vec::new()
    }

    fn relay_discovery_start(&mut self) -> BackendResult<()> {
        Err(BackendError::Unsupported(
            "audio relay discovery is not available for this backend".into(),
        ))
    }

    fn relay_discovery_stop(&mut self) {}

    fn relay_peers(&self) -> Vec<RelayPeerInfo> {
        Vec::new()
    }

    /// Local IPv4 links to display as relay endpoints, ranked best-first.
    /// Active links are preferred; a physical/default-interface fallback keeps
    /// the endpoint and QR visible when interface state flags are incomplete.
    fn relay_local_links(&self) -> Vec<RelayLocalLink> {
        Vec::new()
    }
}
