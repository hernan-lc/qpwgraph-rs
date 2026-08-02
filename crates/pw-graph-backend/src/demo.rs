//! Deterministic in-memory backend used by demo mode and tests.

use super::api::{
    BackendError, BackendResult, EffectDriver, EffectInsertRequest, EffectInstance,
    EffectNodeRequest, GraphDriver, NodeAudioControl,
};
use pw_graph_core::{
    Direction, Graph, GraphError, Link, LinkId, Node, NodeId, NodeType, Port, PortId, PortType,
};
use pw_graph_effects::{AudioSpec, EffectHost, EffectInstanceConfig, EffectProcessor};
use std::collections::BTreeMap;

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

    fn create_effect_node_internal(
        &mut self,
        request: EffectNodeRequest,
    ) -> BackendResult<EffectInstance> {
        if self.effects.contains_key(&request.instance_id) {
            return Err(BackendError::Native(format!(
                "effect instance {} already exists",
                request.instance_id
            )));
        }

        // Prepare and validate before touching the graph. This keeps failed
        // module/parameter requests from leaving an empty node behind.
        let mut processor = self
            .effect_host
            .create(&request.effect_id)
            .map_err(|error| BackendError::Native(format!("could not create effect: {error}")))?;
        processor
            .prepare(AudioSpec {
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

    /// Remove an effect node and all links touching it without restoring an
    /// original connection. Used for standalone-node removal and insertion
    /// rollback after a graph mutation fails.
    fn remove_effect_node_internal(&mut self, instance: &EffectInstance) {
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

        let mut instance = self.create_effect_node_internal(EffectNodeRequest {
            instance_id: request.instance_id,
            effect_id: request.effect_id,
            module_path: request.module_path,
            enabled: request.enabled,
            parameters: request.parameters,
            position: request.position,
        })?;

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
        instance.source = Some(request.source);
        instance.destination = Some(request.destination);
        self.effects
            .insert(instance.config.instance_id.clone(), instance.clone());
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
        let processor = self.effect_processors.get_mut(instance_id).ok_or_else(|| {
            BackendError::Native(format!("unknown effect instance {instance_id}"))
        })?;
        processor
            .set_parameter(parameter, value)
            .map_err(|error| BackendError::Native(error.to_string()))?;
        let instance = self.effects.get_mut(instance_id).ok_or_else(|| {
            BackendError::Native(format!("unknown effect instance {instance_id}"))
        })?;
        instance.config.parameters.insert(parameter.into(), value);
        Ok(())
    }

    fn remove_effect(&mut self, instance_id: &str) -> BackendResult<()> {
        let instance = self.effects.get(instance_id).cloned().ok_or_else(|| {
            BackendError::Native(format!("unknown effect instance {instance_id}"))
        })?;

        let restored_endpoints = match (&instance.source, &instance.destination) {
            (Some(source), Some(destination)) => Some((
                self.graph.resolve_port_key(source).ok_or_else(|| {
                    BackendError::Native("effect source disappeared while removing effect".into())
                })?,
                self.graph.resolve_port_key(destination).ok_or_else(|| {
                    BackendError::Native(
                        "effect destination disappeared while removing effect".into(),
                    )
                })?,
            )),
            (None, None) => None,
            _ => {
                return Err(BackendError::Native(
                    "effect routing is incomplete and cannot be restored".into(),
                ));
            }
        };
        let links: Vec<_> = self
            .graph
            .links
            .values()
            .filter(|link| {
                link.output_port == instance.output_port || link.input_port == instance.input_port
            })
            .map(|link| link.id)
            .collect();
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
        self.graph.ports.remove(&instance.input_port);
        self.graph.ports.remove(&instance.output_port);
        self.graph.nodes.remove(&instance.node_id);
        self.effects.remove(instance_id);
        self.effect_processors.remove(instance_id);
        Ok(())
    }
}
