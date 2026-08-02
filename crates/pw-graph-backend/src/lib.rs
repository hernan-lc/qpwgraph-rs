//! Backend abstraction. The demo driver makes the rest of the application
//! deterministic and testable while the native PipeWire driver talks to the
//! live registry.

use pw_graph_core::{
    Graph, GraphError, Link, LinkId, Node, NodeId, NodeType, Port, PortId, PortKey, PortType,
};
use pw_graph_effects::{EffectDescriptor, EffectHost, EffectInstanceConfig};
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
}

#[derive(Clone, Debug, PartialEq)]
pub struct EffectInstance {
    pub config: EffectInstanceConfig,
    pub node_id: NodeId,
    pub input_port: PortId,
    pub output_port: PortId,
    pub source: PortKey,
    pub destination: PortKey,
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
    /// backend open a helper stream. Callers are expected to repeat the request
    /// while the meter stays visible; backends may keep a stream alive briefly
    /// after the last request so minimizing/restoring a window does not thrash
    /// streams.
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

/// A deterministic demo backend that behaves like a PipeWire registry from
/// the perspective of the application. It is useful for `--demo`, examples,
/// and integration tests where a live PipeWire session is not available.
#[derive(Default)]
pub struct DemoDriver {
    graph: Graph,
    next_link_id: u64,
    audio_controls: BTreeMap<NodeId, NodeAudioControl>,
    effects: BTreeMap<String, EffectInstance>,
    effect_host: EffectHost,
    effect_processors: BTreeMap<String, Box<dyn pw_graph_effects::EffectProcessor>>,
    next_effect_id: u64,
}

impl DemoDriver {
    pub fn new(graph: Graph) -> Self {
        let next_link_id = graph.links.keys().map(|id| id.0).max().unwrap_or(0) + 1;
        Self {
            graph,
            next_link_id,
            audio_controls: BTreeMap::new(),
            effects: BTreeMap::new(),
            effect_host: EffectHost::new(),
            effect_processors: BTreeMap::new(),
            next_effect_id: 1000,
        }
    }

    pub fn demo() -> Self {
        let mut graph = Graph::default();
        let nodes = [
            (1, "Audio Capture", [80.0, 100.0]),
            (2, "Audio Playback", [520.0, 100.0]),
            (3, "MIDI Controller", [80.0, 360.0]),
            (4, "MIDI Monitor", [520.0, 360.0]),
        ];
        for (id, name, position) in nodes {
            let mut node = Node::new(NodeId(id), name, NodeType::PipeWire);
            node.position = position;
            graph.add_node(node).expect("demo node ids are unique");
        }
        add_demo_port(
            &mut graph,
            1,
            1,
            "capture_FL",
            Direction::Source,
            PortType::Audio,
        );
        add_demo_port(
            &mut graph,
            2,
            1,
            "capture_FR",
            Direction::Source,
            PortType::Audio,
        );
        add_demo_port(
            &mut graph,
            3,
            2,
            "playback_FL",
            Direction::Sink,
            PortType::Audio,
        );
        add_demo_port(
            &mut graph,
            4,
            2,
            "playback_FR",
            Direction::Sink,
            PortType::Audio,
        );
        add_demo_port(
            &mut graph,
            5,
            3,
            "midi_out",
            Direction::Source,
            PortType::MidiJack,
        );
        add_demo_port(
            &mut graph,
            6,
            4,
            "midi_in",
            Direction::Sink,
            PortType::MidiJack,
        );
        Self::new(graph)
    }

    pub fn into_graph(self) -> Graph {
        self.graph
    }

    fn allocate_link_id(&mut self) -> LinkId {
        let id = LinkId(self.next_link_id);
        self.next_link_id += 1;
        id
    }
}

/// Compatibility name retained for callers of the original backend API.
pub type InMemoryDriver = DemoDriver;

fn add_demo_port(
    graph: &mut Graph,
    id: u64,
    node_id: u64,
    name: &str,
    direction: Direction,
    port_type: PortType,
) {
    graph
        .add_port(Port::new(
            PortId(id),
            NodeId(node_id),
            name,
            direction,
            port_type,
        ))
        .expect("demo port ids are unique");
}

use pw_graph_core::Direction;

impl GraphDriver for DemoDriver {
    fn refresh(&mut self) -> BackendResult<Vec<Node>> {
        Ok(self.graph.nodes.values().cloned().collect())
    }

    fn connect(&mut self, src: PortId, dst: PortId) -> BackendResult<Link> {
        let link_id = self.allocate_link_id();
        let link = self.graph.add_link(link_id, src, dst)?;
        Ok(link)
    }

    fn disconnect(&mut self, link: LinkId) -> BackendResult<Link> {
        Ok(self.graph.remove_link(link)?)
    }

    fn set_node_position(&mut self, node: NodeId, position: [f32; 2]) -> BackendResult<()> {
        self.graph
            .nodes
            .get_mut(&node)
            .ok_or(GraphError::MissingNode(node))?
            .position = position;
        Ok(())
    }

    fn set_node_mute(&mut self, node: NodeId, muted: bool) -> BackendResult<()> {
        if !self.graph.nodes.contains_key(&node) {
            return Err(GraphError::MissingNode(node).into());
        }
        self.audio_controls.entry(node).or_default().muted = muted;
        Ok(())
    }

    fn set_node_volume(&mut self, node: NodeId, volume: f32) -> BackendResult<()> {
        if !self.graph.nodes.contains_key(&node) {
            return Err(GraphError::MissingNode(node).into());
        }
        self.audio_controls.entry(node).or_default().volume = volume.clamp(0.0, 1.5);
        Ok(())
    }

    fn graph(&self) -> &Graph {
        &self.graph
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
}

impl EffectDriver for DemoDriver {
    fn effect_descriptors(&self) -> Vec<EffectDescriptor> {
        self.effect_host.descriptors()
    }

    fn effect_instances(&self) -> Vec<EffectInstance> {
        self.effects.values().cloned().collect()
    }

    fn insert_effect(&mut self, request: EffectInsertRequest) -> BackendResult<EffectInstance> {
        if self.effects.contains_key(&request.instance_id) {
            return Err(BackendError::Native(format!(
                "effect instance {} already exists",
                request.instance_id
            )));
        }
        let output = self
            .graph
            .resolve_port_key(&request.source)
            .ok_or_else(|| BackendError::Native("effect source port is unavailable".into()))?;
        let input = self
            .graph
            .resolve_port_key(&request.destination)
            .ok_or_else(|| BackendError::Native("effect destination port is unavailable".into()))?;
        let original = self
            .graph
            .links
            .values()
            .find(|link| link.output_port == output && link.input_port == input)
            .cloned()
            .ok_or_else(|| {
                BackendError::Native("effect source and destination are not linked".into())
            })?;
        let mut processor = self
            .effect_host
            .create(&request.effect_id)
            .map_err(|error| BackendError::Native(format!("could not create effect: {error}")))?;
        processor
            .prepare(pw_graph_effects::AudioSpec {
                sample_rate: 48_000,
                channels: 2,
                max_frames: 1024,
            })
            .map_err(|error| BackendError::Native(error.to_string()))?;
        pw_graph_effects::apply_parameters(&mut *processor, &request.parameters)
            .map_err(|error| BackendError::Native(error.to_string()))?;

        let node_id = NodeId(self.next_effect_id);
        self.next_effect_id += 1;
        let input_port = PortId(self.next_effect_id);
        self.next_effect_id += 1;
        let output_port = PortId(self.next_effect_id);
        self.next_effect_id += 1;
        let mut node = Node::new(node_id, request.effect_id.clone(), NodeType::Effect)
            .with_effect_instance(request.instance_id.clone());
        node.position = [250.0, 180.0];
        self.graph.add_node(node)?;
        self.graph.add_port(Port::new(
            input_port,
            node_id,
            "input",
            Direction::Sink,
            PortType::Audio,
        ))?;
        self.graph.add_port(Port::new(
            output_port,
            node_id,
            "output",
            Direction::Source,
            PortType::Audio,
        ))?;

        // Commit the graph only after all validation and object creation have
        // succeeded. DemoDriver uses the same transaction shape as the live
        // PipeWire implementation will use.
        self.graph.remove_link(original.id)?;
        let first = self.allocate_link_id();
        let second = self.allocate_link_id();
        if let Err(error) = self.graph.add_link(first, output, input_port) {
            self.graph.remove_link(first).ok();
            self.graph.ports.remove(&input_port);
            self.graph.ports.remove(&output_port);
            self.graph.nodes.remove(&node_id);
            self.graph.add_link(original.id, output, input).ok();
            return Err(error.into());
        }
        if let Err(error) = self.graph.add_link(second, output_port, input) {
            self.graph.remove_link(first).ok();
            self.graph.ports.remove(&input_port);
            self.graph.ports.remove(&output_port);
            self.graph.nodes.remove(&node_id);
            self.graph.add_link(original.id, output, input).ok();
            return Err(error.into());
        }
        let config = EffectInstanceConfig {
            instance_id: request.instance_id.clone(),
            effect_id: request.effect_id,
            module_path: request.module_path,
            enabled: request.enabled,
            parameters: request.parameters,
        };
        let instance = EffectInstance {
            config,
            node_id,
            input_port,
            output_port,
            source: request.source,
            destination: request.destination,
            error: None,
        };
        self.effects
            .insert(instance.config.instance_id.clone(), instance.clone());
        self.effect_processors
            .insert(instance.config.instance_id.clone(), processor);
        Ok(instance)
    }

    fn set_effect_enabled(&mut self, instance_id: &str, enabled: bool) -> BackendResult<()> {
        let instance = self.effects.get_mut(instance_id).ok_or_else(|| {
            BackendError::Native(format!("unknown effect instance {instance_id}"))
        })?;
        instance.config.enabled = enabled;
        Ok(())
    }

    fn set_effect_parameter(
        &mut self,
        instance_id: &str,
        parameter: &str,
        value: f32,
    ) -> BackendResult<()> {
        let instance = self.effects.get_mut(instance_id).ok_or_else(|| {
            BackendError::Native(format!("unknown effect instance {instance_id}"))
        })?;
        instance.config.parameters.insert(parameter.into(), value);
        Ok(())
    }

    fn remove_effect(&mut self, instance_id: &str) -> BackendResult<()> {
        let instance = self.effects.remove(instance_id).ok_or_else(|| {
            BackendError::Native(format!("unknown effect instance {instance_id}"))
        })?;
        self.effect_processors.remove(instance_id);
        let links: Vec<_> = self
            .graph
            .links
            .values()
            .filter(|link| {
                link.output_port == instance.output_port || link.input_port == instance.input_port
            })
            .map(|link| link.id)
            .collect();
        for link in links {
            self.graph.remove_link(link)?;
        }
        let source = self
            .graph
            .resolve_port_key(&instance.source)
            .ok_or_else(|| {
                BackendError::Native("effect source disappeared while removing effect".into())
            })?;
        let destination = self
            .graph
            .resolve_port_key(&instance.destination)
            .ok_or_else(|| {
                BackendError::Native("effect destination disappeared while removing effect".into())
            })?;
        let link_id = self.allocate_link_id();
        self.graph.add_link(link_id, source, destination)?;
        self.graph.ports.remove(&instance.input_port);
        self.graph.ports.remove(&instance.output_port);
        self.graph.nodes.remove(&instance.node_id);
        Ok(())
    }
}

#[cfg(feature = "pipewire")]
mod pipewire;

#[cfg(feature = "pipewire")]
pub use pipewire::PipewireDriver;

#[cfg(not(feature = "pipewire"))]
#[derive(Debug, Default)]
pub struct PipewireDriver {
    graph: Graph,
}

#[cfg(not(feature = "pipewire"))]
impl PipewireDriver {
    pub fn new() -> BackendResult<Self> {
        Err(BackendError::Unsupported(
            "compile pw-graph-backend with the pipewire feature".into(),
        ))
    }
}

#[cfg(not(feature = "pipewire"))]
impl GraphDriver for PipewireDriver {
    fn refresh(&mut self) -> BackendResult<Vec<Node>> {
        Err(BackendError::Unsupported(
            "PipeWire feature is disabled".into(),
        ))
    }

    fn connect(&mut self, _src: PortId, _dst: PortId) -> BackendResult<Link> {
        Err(BackendError::Unsupported(
            "PipeWire feature is disabled".into(),
        ))
    }

    fn disconnect(&mut self, _link: LinkId) -> BackendResult<Link> {
        Err(BackendError::Unsupported(
            "PipeWire feature is disabled".into(),
        ))
    }

    fn set_node_position(&mut self, node: NodeId, position: [f32; 2]) -> BackendResult<()> {
        self.graph
            .nodes
            .get_mut(&node)
            .ok_or(GraphError::MissingNode(node))?
            .position = position;
        Ok(())
    }

    fn set_node_mute(&mut self, _node: NodeId, _muted: bool) -> BackendResult<()> {
        Err(BackendError::Unsupported(
            "PipeWire feature is disabled".into(),
        ))
    }

    fn set_node_volume(&mut self, _node: NodeId, _volume: f32) -> BackendResult<()> {
        Err(BackendError::Unsupported(
            "PipeWire feature is disabled".into(),
        ))
    }

    fn graph(&self) -> &Graph {
        &self.graph
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
}

impl EffectDriver for PipewireDriver {}

/// Used by patchbay activation to avoid reconnecting identical links.
pub fn existing_connections(driver: &dyn GraphDriver) -> BTreeSet<(PortId, PortId)> {
    driver
        .graph()
        .links
        .values()
        .map(|link| (link.output_port, link.input_port))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meter_policy_round_trips_and_defaults_safely() {
        for policy in MeterPolicy::ALL {
            assert_eq!(MeterPolicy::parse(policy.as_str()), policy);
        }
        assert_eq!(MeterPolicy::parse("OFF"), MeterPolicy::Disabled);
        assert_eq!(MeterPolicy::parse("all"), MeterPolicy::Always);
        // An unreadable or older config must not silently start metering
        // everything, so anything unrecognized lands on the default.
        assert_eq!(MeterPolicy::parse("nonsense"), MeterPolicy::default());
        assert_eq!(MeterPolicy::default(), MeterPolicy::OnDemand);
    }

    #[test]
    fn demo_backend_connects_and_disconnects() {
        let mut driver = DemoDriver::demo();
        let link = driver.connect(PortId(1), PortId(3)).unwrap();
        assert_eq!(driver.graph().links.len(), 1);
        driver.disconnect(link.id).unwrap();
        assert!(driver.graph().links.is_empty());
    }

    #[test]
    fn demo_backend_has_a_stable_graph_for_demo_runs() {
        let driver = DemoDriver::demo();
        assert_eq!(driver.graph().nodes.len(), 4);
        assert_eq!(driver.graph().ports.len(), 6);
        assert!(driver
            .graph()
            .nodes
            .values()
            .all(|node| node.node_type == NodeType::PipeWire));
    }

    #[test]
    fn demo_backend_inserts_and_removes_an_effect_transactionally() {
        let mut driver = DemoDriver::demo();
        driver.connect(PortId(1), PortId(3)).unwrap();
        let source = driver.graph().port_key(PortId(1)).unwrap();
        let destination = driver.graph().port_key(PortId(3)).unwrap();
        let instance = driver
            .insert_effect(EffectInsertRequest {
                instance_id: "test-effect".into(),
                effect_id: pw_graph_effects::NOISE_GATE_ID.into(),
                module_path: None,
                source,
                destination,
                enabled: true,
                parameters: BTreeMap::new(),
            })
            .unwrap();
        assert_eq!(driver.effects.len(), 1);
        assert_eq!(
            driver.graph().nodes[&instance.node_id].node_type,
            NodeType::Effect
        );
        assert_eq!(driver.graph().links.len(), 2);
        driver.remove_effect("test-effect").unwrap();
        assert!(driver.effects.is_empty());
        assert_eq!(driver.graph().links.len(), 1);
        assert_eq!(driver.graph().nodes.len(), 4);
    }

    #[cfg(feature = "pipewire")]
    #[test]
    fn native_backend_refreshes_running_pipewire_registry() {
        let Ok(mut driver) = PipewireDriver::new() else {
            // CI and development containers may not have a user PipeWire
            // daemon. The live test is exercised automatically when one is
            // available, but should not make offline builds fail.
            return;
        };
        let nodes = driver
            .refresh()
            .expect("PipeWire registry snapshot should succeed");
        assert!(!nodes.is_empty());
        assert!(!driver.graph().ports.is_empty());
    }

    /// Regression guard for the startup behaviour users actually noticed: the
    /// driver used to open a capture stream against every audio node as soon as
    /// the graph was first read, which resumed suspended devices and made the
    /// daemon renegotiate their format.
    #[cfg(feature = "pipewire")]
    #[test]
    fn native_backend_meters_nothing_until_it_is_asked_to() {
        let Ok(mut driver) = PipewireDriver::new() else {
            return;
        };
        driver.refresh().expect("registry snapshot should succeed");
        assert_eq!(driver.active_meter_count(), 0);
        assert!(driver.audio_meters().unwrap().is_empty());
    }

    /// Opt-in: this one attaches a real (passive, monitor-flagged) stream to a
    /// node in the user's live session, so it is not part of a default run.
    #[cfg(feature = "pipewire")]
    #[test]
    fn native_backend_attaches_and_releases_a_requested_meter() {
        if std::env::var_os("PW_GRAPH_TEST_METERS").is_none() {
            return;
        }
        let mut driver = PipewireDriver::new().expect("PipeWire daemon should be available");
        driver.refresh().expect("registry snapshot should succeed");
        let target = driver.graph().nodes.values().find(|node| {
            node.ports.iter().any(|port_id| {
                driver.graph().port(*port_id).is_some_and(|port| {
                    port.direction.is_source() && port.port_type == PortType::Audio
                })
            })
        });
        let Some(target) = target.map(|node| node.id) else {
            return;
        };

        driver
            .request_meters(&BTreeSet::from([target]))
            .expect("requesting a meter should succeed");
        assert_eq!(driver.active_meter_count(), 1);

        // Regression guard: `process` runs on PipeWire's realtime data thread,
        // which the thread-loop lock does not exclude. Reading meters from this
        // thread while that thread publishes used to hit `RefCell already
        // borrowed` inside a callback that cannot unwind, aborting the process.
        // Polling hard for a second reliably reproduced it.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        let mut polls = 0_u32;
        while std::time::Instant::now() < deadline {
            for meter in driver
                .audio_meters()
                .expect("reading meters should succeed")
            {
                assert!(meter.rms.is_finite() && (0.0..=1.0).contains(&meter.rms));
                assert!(meter.peak.is_finite() && (0.0..=1.0).contains(&meter.peak));
            }
            polls += 1;
        }
        assert!(polls > 0);

        driver
            .reset_audio_config()
            .expect("releasing meters should succeed");
        assert_eq!(driver.active_meter_count(), 0);
        assert!(driver.audio_meters().unwrap().is_empty());
    }

    #[cfg(feature = "pipewire")]
    #[test]
    fn native_backend_can_create_and_destroy_a_link_when_enabled() {
        if std::env::var_os("PW_GRAPH_TEST_LINKS").is_none() {
            return;
        }
        let mut driver = PipewireDriver::new().expect("PipeWire daemon should be available");
        driver
            .refresh()
            .expect("PipeWire registry snapshot should succeed");
        let existing = existing_connections(&driver);
        let pair = driver.graph().ports.values().find_map(|output| {
            if !output.direction.is_source() {
                return None;
            }
            driver.graph().ports.values().find_map(|input| {
                if !input.direction.is_sink()
                    || (output.port_type != input.port_type
                        && output.port_type != PortType::Unknown
                        && input.port_type != PortType::Unknown)
                    || existing.contains(&(output.id, input.id))
                {
                    return None;
                }
                Some((output.id, input.id))
            })
        });
        let Some((output, input)) = pair else {
            return;
        };
        let link = driver
            .connect(output, input)
            .expect("PipeWire link creation should succeed");
        assert!(driver.graph().link(link.id).is_some());
        driver
            .disconnect(link.id)
            .expect("PipeWire link destruction should succeed");
    }
}
