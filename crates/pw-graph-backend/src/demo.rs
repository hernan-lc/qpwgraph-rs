//! Deterministic in-memory backend used by demo mode and tests.

#[cfg(feature = "relay")]
use super::api::RelayDriver;
use super::api::{
    BackendCapabilities, BackendError, BackendResult, EffectDriver, EffectInsertRequest,
    EffectInstance, EffectNodeRequest, GraphDriver, NodeAudioControl, NodeAudioState,
    NodeCapabilities,
};
use pw_graph_core::{
    Direction, Graph, GraphError, Link, LinkId, Node, NodeId, NodeType, Port, PortId, PortType,
};
use pw_graph_effects::{AudioSpec, EffectHost, EffectInstanceConfig, EffectProcessor};
use std::collections::{BTreeMap, BTreeSet};

/// Boost headroom, matching PipeWire so demo mode behaves like the real thing.
const DEMO_MAX_VOLUME: f32 = 1.5;

/// A deterministic demo backend that behaves like a PipeWire registry from
/// the perspective of the application. It is useful for `--demo`, examples,
/// and integration tests where a live PipeWire session is not available.
#[derive(Default)]
pub struct DemoDriver {
    graph: Graph,
    next_link_id: u64,
    audio_controls: BTreeMap<NodeId, NodeAudioControl>,
    observed_links: BTreeSet<LinkId>,
    effects: BTreeMap<String, EffectInstance>,
    effect_host: EffectHost,
    effect_processors: BTreeMap<String, Box<dyn EffectProcessor>>,
    next_effect_id: u64,
}

impl DemoDriver {
    pub fn new(graph: Graph) -> Self {
        let next_link_id = graph.links.keys().map(|id| id.0).max().unwrap_or(0) + 1;
        Self {
            graph,
            next_link_id,
            audio_controls: BTreeMap::new(),
            observed_links: BTreeSet::new(),
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

    /// Mark a graph relationship as observed rather than user-mutable.
    ///
    /// Demo mode normally models a mutable PipeWire graph. This hook also
    /// lets deterministic tests model backends that expose informational
    /// links, such as Windows audio-session relationships.
    pub fn mark_link_observed(&mut self, link: LinkId) {
        self.observed_links.insert(link);
    }

    fn allocate_link_id(&mut self) -> LinkId {
        let id = LinkId(self.next_link_id);
        self.next_link_id += 1;
        id
    }

    fn create_effect_node_internal(
        &mut self,
        request: EffectNodeRequest,
    ) -> BackendResult<EffectInstance> {
        if self.effects.contains_key(&request.instance_id) {
            return Err(BackendError::effect_already_exists(&request.instance_id));
        }

        // Prepare and validate before touching the graph. This keeps failed
        // module/parameter requests from leaving an empty node behind.
        let mut processor = self
            .effect_host
            .create(&request.effect_id)
            .map_err(BackendError::effect_create_failed)?;
        processor
            .prepare(AudioSpec {
                sample_rate: 48_000,
                channels: 2,
                max_frames: 1024,
            })
            .map_err(BackendError::native)?;
        pw_graph_effects::apply_parameters(&mut *processor, &request.parameters)
            .map_err(BackendError::native)?;

        // `PortKey` identifies a saved/manual connection by node name and
        // port name. Use the stable instance id in the visible node name so
        // several copies of the same effect never collapse into one routing
        // target when a patchbay or undo command is restored.
        let node_name = format!("{} ({})", processor.descriptor().name, request.instance_id);

        let node_id = NodeId(self.next_effect_id);
        self.next_effect_id += 1;
        let input_port = PortId(self.next_effect_id);
        self.next_effect_id += 1;
        let output_port = PortId(self.next_effect_id);
        self.next_effect_id += 1;
        let mut node = Node::new(node_id, node_name, NodeType::Effect)
            .with_effect_instance(request.instance_id.clone());
        node.position = request.position;
        self.graph.add_node(node)?;
        if let Err(error) = self.graph.add_port(Port::new(
            input_port,
            node_id,
            "input",
            Direction::Sink,
            PortType::Audio,
        )) {
            self.graph.nodes.remove(&node_id);
            return Err(error.into());
        }
        if let Err(error) = self.graph.add_port(Port::new(
            output_port,
            node_id,
            "output",
            Direction::Source,
            PortType::Audio,
        )) {
            self.graph.ports.remove(&input_port);
            self.graph.nodes.remove(&node_id);
            return Err(error.into());
        }

        let instance = EffectInstance {
            config: EffectInstanceConfig {
                instance_id: request.instance_id.clone(),
                effect_id: request.effect_id,
                module_path: request.module_path,
                enabled: request.enabled,
                parameters: request.parameters,
            },
            node_id,
            input_port,
            output_port,
            source: None,
            destination: None,
            error: None,
        };
        self.effects
            .insert(instance.config.instance_id.clone(), instance.clone());
        self.effect_processors
            .insert(instance.config.instance_id.clone(), processor);
        Ok(instance)
    }

    /// Links attached to either of an effect node's DSP ports. Both removal
    /// paths (standalone node and inserted-effect rollback) collect these.
    fn links_touching(&self, instance: &EffectInstance) -> Vec<LinkId> {
        self.graph
            .links
            .values()
            .filter(|link| {
                link.output_port == instance.output_port || link.input_port == instance.input_port
            })
            .map(|link| link.id)
            .collect()
    }

    /// Remove an effect node and all links touching it without restoring an
    /// original connection. Used for standalone-node removal and insertion
    /// rollback after a graph mutation fails.
    fn remove_effect_node_internal(&mut self, instance: &EffectInstance) {
        for link in self.links_touching(instance) {
            let _ = self.graph.remove_link(link);
        }
        self.graph.ports.remove(&instance.input_port);
        self.graph.ports.remove(&instance.output_port);
        self.graph.nodes.remove(&instance.node_id);
        self.effects.remove(&instance.config.instance_id);
        self.effect_processors.remove(&instance.config.instance_id);
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

impl GraphDriver for DemoDriver {
    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            topology: true,
            connect: true,
            disconnect: true,
            volume: true,
            mute: true,
            meters: true,
            effects: true,
            relay: false,
        }
    }

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

    fn is_link_mutable(&self, link: LinkId) -> bool {
        self.graph.link(link).is_some() && !self.observed_links.contains(&link)
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
        self.audio_controls.entry(node).or_default().volume = volume.clamp(0.0, DEMO_MAX_VOLUME);
        Ok(())
    }

    /// The demo driver owns its controls outright, so this is a real read of
    /// backend state rather than a placeholder. Effect nodes carry no audio
    /// controls, exactly as a DSP node would not on a native backend.
    fn node_audio_state(&self, node: NodeId) -> BackendResult<NodeAudioState> {
        let record = self
            .graph
            .nodes
            .get(&node)
            .ok_or(GraphError::MissingNode(node))?;
        if record.node_type == NodeType::Effect {
            return Ok(NodeAudioState::UNSUPPORTED);
        }
        let control = self.audio_controls.get(&node).copied().unwrap_or_default();
        Ok(NodeAudioState::readable(control.volume, control.muted))
    }

    fn node_capabilities(&self, node: NodeId) -> NodeCapabilities {
        let Ok(state) = self.node_audio_state(node) else {
            return NodeCapabilities::NONE;
        };
        let mut capabilities = state.control_capabilities();
        // Effect nodes are pass-through DSP: nothing to meter and nothing to
        // control, so they must not be given a meter either.
        if state.is_supported() {
            capabilities.volume_max = DEMO_MAX_VOLUME;
            capabilities.meter_peak = true;
            capabilities.meter_rms = true;
        }
        capabilities
    }

    fn graph(&self) -> &Graph {
        &self.graph
    }
}

#[cfg(feature = "relay")]
impl RelayDriver for DemoDriver {}

impl EffectDriver for DemoDriver {
    fn effect_descriptors(&self) -> Vec<pw_graph_effects::EffectDescriptor> {
        self.effect_host.descriptors()
    }

    fn effect_instances(&self) -> Vec<EffectInstance> {
        self.effects.values().cloned().collect()
    }

    fn supports_effect_nodes(&self) -> bool {
        true
    }

    fn create_effect_node(&mut self, request: EffectNodeRequest) -> BackendResult<EffectInstance> {
        self.create_effect_node_internal(request)
    }

    fn insert_effect(&mut self, request: EffectInsertRequest) -> BackendResult<EffectInstance> {
        let source = request.source.clone();
        let destination = request.destination.clone();
        let (output, input, original) = self.effect_link_endpoints(&source, &destination)?;

        let mut instance = self.create_effect_node_internal(request.into())?;

        // Commit the link rewrite only after the free node has been fully
        // created. Every failure below removes that node and restores the
        // original direct connection.
        self.graph.remove_link(original.id)?;
        let first = self.allocate_link_id();
        let second = self.allocate_link_id();
        if let Err(error) = self.graph.add_link(first, output, instance.input_port) {
            self.remove_effect_node_internal(&instance);
            self.graph.add_link(original.id, output, input).ok();
            return Err(error.into());
        }
        if let Err(error) = self.graph.add_link(second, instance.output_port, input) {
            self.graph.remove_link(first).ok();
            self.remove_effect_node_internal(&instance);
            self.graph.add_link(original.id, output, input).ok();
            return Err(error.into());
        }
        instance.source = Some(source);
        instance.destination = Some(destination);
        self.effects
            .insert(instance.config.instance_id.clone(), instance.clone());
        Ok(instance)
    }

    fn set_effect_enabled(&mut self, instance_id: &str, enabled: bool) -> BackendResult<()> {
        let instance = self
            .effects
            .get_mut(instance_id)
            .ok_or_else(|| BackendError::unknown_effect_instance(instance_id))?;
        instance.config.enabled = enabled;
        Ok(())
    }

    fn set_effect_parameter(
        &mut self,
        instance_id: &str,
        parameter: &str,
        value: f32,
    ) -> BackendResult<()> {
        let processor = self
            .effect_processors
            .get_mut(instance_id)
            .ok_or_else(|| BackendError::unknown_effect_instance(instance_id))?;
        processor
            .set_parameter(parameter, value)
            .map_err(BackendError::native)?;
        let instance = self
            .effects
            .get_mut(instance_id)
            .ok_or_else(|| BackendError::unknown_effect_instance(instance_id))?;
        instance.config.parameters.insert(parameter.into(), value);
        Ok(())
    }

    fn remove_effect(&mut self, instance_id: &str) -> BackendResult<()> {
        let instance = self
            .effects
            .get(instance_id)
            .cloned()
            .ok_or_else(|| BackendError::unknown_effect_instance(instance_id))?;

        let restored_endpoints = match (&instance.source, &instance.destination) {
            (Some(source), Some(destination)) => {
                Some(self.effect_restore_endpoints(source, destination)?)
            }
            (None, None) => None,
            _ => return Err(BackendError::effect_routing_incomplete()),
        };
        let links = self.links_touching(&instance);
        let removed_links: Vec<_> = links
            .iter()
            .filter_map(|link| self.graph.link(*link).cloned())
            .collect();
        for link in links {
            self.graph.remove_link(link)?;
        }
        if let Some((source, destination)) = restored_endpoints {
            let link_id = self.allocate_link_id();
            if let Err(error) = self.graph.add_link(link_id, source, destination) {
                for link in removed_links {
                    self.graph.insert_existing_link(link).ok();
                }
                return Err(error.into());
            }
        }
        self.remove_effect_node_internal(&instance);
        Ok(())
    }
}
