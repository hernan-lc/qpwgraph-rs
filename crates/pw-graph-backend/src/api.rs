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
    /// Never open helper streams. Meters report unavailable.
    Disabled,
    /// Measure filtered audio nodes while the application window is visible.
    #[default]
    OnDemand,
    /// Measure every audio node continuously.
    Always,
}

impl MeterPolicy {
    pub const ALL: [Self; 3] = [Self::Disabled, Self::OnDemand, Self::Always];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "off",
            Self::OnDemand => "on-demand",
            Self::Always => "always",
        }
    }

    /// Unknown values fall back to the safe default rather than failing a load,
    /// so a hand-edited or older config file still starts the application.
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" | "disabled" | "none" => Self::Disabled,
            "always" | "all" => Self::Always,
            _ => Self::OnDemand,
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
        Err(BackendError::Unsupported(
            "effect processing is not available for this backend".into(),
        ))
    }

    fn insert_effect(&mut self, _request: EffectInsertRequest) -> BackendResult<EffectInstance> {
        Err(BackendError::Unsupported(
            "effect processing is not available for this backend".into(),
        ))
    }

    fn set_effect_enabled(&mut self, _instance_id: &str, _enabled: bool) -> BackendResult<()> {
        Err(BackendError::Unsupported(
            "effect processing is not available for this backend".into(),
        ))
    }

    fn set_effect_parameter(
        &mut self,
        _instance_id: &str,
        _parameter: &str,
        _value: f32,
    ) -> BackendResult<()> {
        Err(BackendError::Unsupported(
            "effect processing is not available for this backend".into(),
        ))
    }

    fn remove_effect(&mut self, _instance_id: &str) -> BackendResult<()> {
        Err(BackendError::Unsupported(
            "effect processing is not available for this backend".into(),
        ))
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

/// Common operations needed by commands, patchbay activation, and the UI.
pub trait GraphDriver: EffectDriver {
    fn refresh(&mut self) -> BackendResult<Vec<Node>>;
    fn connect(&mut self, src: PortId, dst: PortId) -> BackendResult<Link>;
    fn disconnect(&mut self, link: LinkId) -> BackendResult<Link>;

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
        Err(BackendError::Unsupported(
            "node layout is not supported by this backend".into(),
        ))
    }

    fn set_node_mute(&mut self, node: NodeId, muted: bool) -> BackendResult<()> {
        let _ = (node, muted);
        Err(BackendError::Unsupported(
            "node mute is not supported by this backend".into(),
        ))
    }

    fn set_node_volume(&mut self, node: NodeId, volume: f32) -> BackendResult<()> {
        let _ = (node, volume);
        Err(BackendError::Unsupported(
            "node volume is not supported by this backend".into(),
        ))
    }

    fn graph(&self) -> &Graph;

    /// Returns whether registry state changed since the last `refresh`.
    /// Backends without event-driven registries may keep the default `false`.
    fn graph_dirty(&self) -> bool {
        false
    }

    fn is_node_type(&self, node_type: NodeType) -> bool;
    fn is_port_type(&self, port_type: PortType) -> bool;

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
