//! Core graph types shared by every backend and presentation layer.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

macro_rules! id_type {
    ($name:ident) => {
        #[derive(
            Clone,
            Copy,
            Debug,
            Default,
            Deserialize,
            Eq,
            Hash,
            Ord,
            PartialEq,
            Serialize,
            PartialOrd,
        )]
        #[serde(transparent)]
        pub struct $name(pub u64);

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }
    };
}

id_type!(NodeId);
id_type!(PortId);
id_type!(LinkId);

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum Direction {
    #[default]
    Source,
    Sink,
}

impl Direction {
    pub fn is_source(self) -> bool {
        matches!(self, Self::Source)
    }

    pub fn is_sink(self) -> bool {
        matches!(self, Self::Sink)
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum NodeType {
    #[default]
    PipeWire,
    Effect,
    AlsaMidi,
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum PortType {
    #[default]
    Audio,
    Video,
    MidiJack,
    MidiAlsa,
    Unknown,
}

impl PortType {
    pub fn color_hex(self) -> &'static str {
        match self {
            Self::Audio => "#57c785",
            Self::Video => "#4e9de6",
            Self::MidiJack => "#e35d6a",
            Self::MidiAlsa => "#a979d1",
            Self::Unknown => "#a5a5a5",
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Node {
    pub id: NodeId,
    pub name: String,
    pub node_type: NodeType,
    /// Backend-provided identity that survives global-ID churn when possible.
    /// PipeWire exposes this as `object.serial`; demo and ALSA nodes leave it
    /// unset and are resolved by their names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial: Option<u64>,
    /// Stable identity assigned by the effect host. PipeWire global IDs are
    /// intentionally not used for effect persistence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect_instance_id: Option<String>,
    pub ports: Vec<PortId>,
    /// Canvas position in logical scene coordinates.
    pub position: [f32; 2],
}

impl Node {
    pub fn new(id: NodeId, name: impl Into<String>, node_type: NodeType) -> Self {
        Self {
            id,
            name: name.into(),
            node_type,
            serial: None,
            effect_instance_id: None,
            ports: Vec::new(),
            position: [0.0, 0.0],
        }
    }

    pub fn with_serial(mut self, serial: u64) -> Self {
        self.serial = Some(serial);
        self
    }

    pub fn with_effect_instance(mut self, instance_id: impl Into<String>) -> Self {
        self.effect_instance_id = Some(instance_id.into());
        self
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Port {
    pub id: PortId,
    pub node_id: NodeId,
    pub name: String,
    /// Optional backend-provided channel position (for example `FL` or
    /// `FR`). Backends that do not expose channel metadata leave this unset,
    /// allowing presentation code to use a conservative name-based fallback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    pub direction: Direction,
    pub port_type: PortType,
}

/// Stable description of a port used when PipeWire recreates a stream and
/// assigns it a new global ID. The numeric [`PortId`] remains useful for the
/// current graph, while this key is used by commands and patchbay operations
/// that can outlive one registry snapshot.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PortKey {
    pub node_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_serial: Option<u64>,
    pub node_type: NodeType,
    pub port_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    pub direction: Direction,
    pub port_type: PortType,
}

impl Port {
    pub fn new(
        id: PortId,
        node_id: NodeId,
        name: impl Into<String>,
        direction: Direction,
        port_type: PortType,
    ) -> Self {
        Self {
            id,
            node_id,
            name: name.into(),
            channel: None,
            direction,
            port_type,
        }
    }

    /// Attach an optional backend-provided channel position to this port.
    pub fn with_channel(mut self, channel: impl Into<String>) -> Self {
        self.channel = Some(channel.into());
        self
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Link {
    pub id: LinkId,
    pub output_port: PortId,
    pub input_port: PortId,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct Graph {
    pub nodes: BTreeMap<NodeId, Node>,
    pub ports: BTreeMap<PortId, Port>,
    pub links: BTreeMap<LinkId, Link>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum GraphError {
    #[error("node {0} already exists")]
    DuplicateNode(NodeId),
    #[error("port {0} already exists")]
    DuplicatePort(PortId),
    #[error("link {0} already exists")]
    DuplicateLink(LinkId),
    #[error("node {0} does not exist")]
    MissingNode(NodeId),
    #[error("port {0} does not exist")]
    MissingPort(PortId),
    #[error("port {0} belongs to node {1}, not node {2}")]
    PortNodeMismatch(PortId, NodeId, NodeId),
    #[error("source port {0} must be a source")]
    NotSource(PortId),
    #[error("destination port {0} must be a sink")]
    NotSink(PortId),
    #[error("ports {0} and {1} are not compatible")]
    IncompatiblePorts(PortId, PortId),
    #[error("ports {0} and {1} are already linked")]
    DuplicateConnection(PortId, PortId),
    #[error("link {0} does not exist")]
    MissingLink(LinkId),
}

impl Graph {
    pub fn add_node(&mut self, node: Node) -> Result<(), GraphError> {
        if self.nodes.contains_key(&node.id) {
            return Err(GraphError::DuplicateNode(node.id));
        }
        self.nodes.insert(node.id, node);
        Ok(())
    }

    pub fn add_port(&mut self, port: Port) -> Result<(), GraphError> {
        let node = self
            .nodes
            .get_mut(&port.node_id)
            .ok_or(GraphError::MissingNode(port.node_id))?;
        if self.ports.contains_key(&port.id) {
            return Err(GraphError::DuplicatePort(port.id));
        }
        node.ports.push(port.id);
        self.ports.insert(port.id, port);
        Ok(())
    }

    pub fn remove_link(&mut self, link_id: LinkId) -> Result<Link, GraphError> {
        self.links
            .remove(&link_id)
            .ok_or(GraphError::MissingLink(link_id))
    }

    pub fn link(&self, link_id: LinkId) -> Option<&Link> {
        self.links.get(&link_id)
    }

    pub fn port(&self, port_id: PortId) -> Option<&Port> {
        self.ports.get(&port_id)
    }

    pub fn port_key(&self, port_id: PortId) -> Option<PortKey> {
        let port = self.port(port_id)?;
        let node = self.node(port.node_id)?;
        Some(PortKey {
            node_name: node.name.clone(),
            node_serial: node.serial,
            node_type: node.node_type,
            port_name: port.name.clone(),
            channel: port.channel.clone(),
            direction: port.direction,
            port_type: port.port_type,
        })
    }

    /// Resolve a stable port key against the current registry snapshot.
    /// Serial is preferred, but a name fallback is intentional: a playback
    /// stream often receives a new serial when it is resumed.
    pub fn resolve_port_key(&self, key: &PortKey) -> Option<PortId> {
        self.ports
            .values()
            .filter(|port| port.name == key.port_name)
            .filter(|port| port.direction == key.direction)
            .filter(|port| {
                port.port_type == key.port_type
                    || port.port_type == PortType::Unknown
                    || key.port_type == PortType::Unknown
            })
            .filter_map(|port| {
                let node = self.node(port.node_id)?;
                if node.name != key.node_name || node.node_type != key.node_type {
                    return None;
                }
                if let (Some(expected), Some(actual)) =
                    (key.channel.as_ref(), port.channel.as_ref())
                {
                    if expected != actual {
                        return None;
                    }
                }
                let serial_score = match (key.node_serial, node.serial) {
                    (Some(expected), Some(actual)) if expected == actual => 100,
                    (Some(_), Some(_)) => 0,
                    (None, None) => 20,
                    (None, Some(_)) => 10,
                    (Some(_), None) => 5,
                };
                Some((serial_score, port.id))
            })
            .max_by_key(|(score, id)| (*score, *id))
            .map(|(_, id)| id)
    }

    pub fn find_link_by_keys(&self, output: &PortKey, input: &PortKey) -> Option<Link> {
        let output_id = self.resolve_port_key(output)?;
        let input_id = self.resolve_port_key(input)?;
        self.links
            .values()
            .find(|link| link.output_port == output_id && link.input_port == input_id)
            .cloned()
    }

    pub fn node(&self, node_id: NodeId) -> Option<&Node> {
        self.nodes.get(&node_id)
    }

    pub fn add_link(
        &mut self,
        link_id: LinkId,
        output_port: PortId,
        input_port: PortId,
    ) -> Result<Link, GraphError> {
        if self.links.contains_key(&link_id) {
            return Err(GraphError::DuplicateLink(link_id));
        }
        let output = self
            .ports
            .get(&output_port)
            .ok_or(GraphError::MissingPort(output_port))?;
        let input = self
            .ports
            .get(&input_port)
            .ok_or(GraphError::MissingPort(input_port))?;
        if !output.direction.is_source() {
            return Err(GraphError::NotSource(output_port));
        }
        if !input.direction.is_sink() {
            return Err(GraphError::NotSink(input_port));
        }
        if output.port_type != input.port_type {
            return Err(GraphError::IncompatiblePorts(output_port, input_port));
        }
        if self
            .links
            .values()
            .any(|link| link.output_port == output_port && link.input_port == input_port)
        {
            return Err(GraphError::DuplicateConnection(output_port, input_port));
        }
        let link = Link {
            id: link_id,
            output_port,
            input_port,
        };
        self.links.insert(link_id, link.clone());
        Ok(link)
    }

    /// Insert a link reported by a backend snapshot. Backends may know about
    /// legacy or partially-described links that cannot be revalidated locally.
    pub fn insert_existing_link(&mut self, link: Link) -> Result<(), GraphError> {
        if self.links.contains_key(&link.id) {
            return Err(GraphError::DuplicateLink(link.id));
        }
        if !self.ports.contains_key(&link.output_port) {
            return Err(GraphError::MissingPort(link.output_port));
        }
        if !self.ports.contains_key(&link.input_port) {
            return Err(GraphError::MissingPort(link.input_port));
        }
        self.links.insert(link.id, link);
        Ok(())
    }

    pub fn links_for_port(&self, port_id: PortId) -> impl Iterator<Item = &Link> {
        self.links
            .values()
            .filter(move |link| link.output_port == port_id || link.input_port == port_id)
    }

    /// Suggest a readable, deterministic layout for every node in the graph.
    ///
    /// Connected nodes are assigned to layers following the direction of
    /// their links, so sources appear before their sinks and multi-hop graphs
    /// spread across several columns. Unconnected nodes use their port role as
    /// a fallback layer. Ordering inside a layer is stable by media category,
    /// direction, display name, and numeric ID, which keeps repeated refreshes
    /// from shuffling the graph.
    pub fn default_node_positions(&self) -> BTreeMap<NodeId, [f32; 2]> {
        let mut incoming: BTreeMap<NodeId, usize> =
            self.nodes.keys().copied().map(|node| (node, 0)).collect();
        let mut outgoing: BTreeMap<NodeId, Vec<NodeId>> = self
            .nodes
            .keys()
            .copied()
            .map(|node| (node, Vec::new()))
            .collect();
        for link in self.links.values() {
            let (Some(output), Some(input)) =
                (self.port(link.output_port), self.port(link.input_port))
            else {
                continue;
            };
            if output.node_id == input.node_id || !self.nodes.contains_key(&output.node_id) {
                continue;
            }
            outgoing
                .entry(output.node_id)
                .or_default()
                .push(input.node_id);
            if let Some(count) = incoming.get_mut(&input.node_id) {
                *count += 1;
            }
        }
        for targets in outgoing.values_mut() {
            targets.sort_unstable();
            targets.dedup();
        }

        let node_limit = self.nodes.len().saturating_sub(1);
        let mut graph_layers: BTreeMap<NodeId, usize> =
            self.nodes.keys().copied().map(|node| (node, 0)).collect();
        let mut queue: std::collections::VecDeque<NodeId> = incoming
            .iter()
            .filter_map(|(node, count)| (*count == 0).then_some(*node))
            .collect();
        if queue.is_empty() {
            queue.extend(self.nodes.keys().copied().take(1));
        }
        while let Some(node_id) = queue.pop_front() {
            let current_layer = graph_layers.get(&node_id).copied().unwrap_or_default();
            for target in outgoing.get(&node_id).into_iter().flatten() {
                let candidate = (current_layer + 1).min(node_limit);
                let target_layer = graph_layers.entry(*target).or_default();
                if candidate > *target_layer {
                    *target_layer = candidate;
                    queue.push_back(*target);
                }
            }
        }

        // A disconnected cycle has no zero-indegree root. Seed each remaining
        // component deterministically so it still receives a useful layer.
        for node_id in self.nodes.keys().copied() {
            if incoming.get(&node_id).copied().unwrap_or_default() > 0
                && graph_layers.get(&node_id).copied().unwrap_or_default() == 0
            {
                queue.push_back(node_id);
                while let Some(current) = queue.pop_front() {
                    let current_layer = graph_layers.get(&current).copied().unwrap_or_default();
                    for target in outgoing.get(&current).into_iter().flatten() {
                        let candidate = (current_layer + 1).min(node_limit);
                        let target_layer = graph_layers.entry(*target).or_default();
                        if candidate > *target_layer {
                            *target_layer = candidate;
                            queue.push_back(*target);
                        }
                    }
                }
            }
        }

        let mut layers: BTreeMap<usize, Vec<NodeId>> = BTreeMap::new();
        for node in self.nodes.values() {
            let graph_layer = graph_layers.get(&node.id).copied().unwrap_or_default();
            let role_layer = match self.node_layout_role(node) {
                0 => 0,
                1 => 1,
                2 => 2,
                _ => 0,
            };
            let layer = if graph_layer == 0 {
                role_layer
            } else {
                graph_layer
            };
            layers.entry(layer).or_default().push(node.id);
        }

        for nodes in layers.values_mut() {
            nodes.sort_by(|left, right| {
                let left_node = self.nodes.get(left).expect("layout node exists");
                let right_node = self.nodes.get(right).expect("layout node exists");
                self.node_media_category(left_node)
                    .cmp(&self.node_media_category(right_node))
                    .then_with(|| {
                        self.node_layout_role(left_node)
                            .cmp(&self.node_layout_role(right_node))
                    })
                    .then_with(|| {
                        left_node
                            .name
                            .to_ascii_lowercase()
                            .cmp(&right_node.name.to_ascii_lowercase())
                    })
                    .then_with(|| left.cmp(right))
            });
        }

        let mut positions = BTreeMap::new();
        for (layer, nodes) in layers {
            let mut top = 40.0;
            for node_id in nodes {
                let node = self.nodes.get(&node_id).expect("layout node exists");
                positions.insert(node_id, [40.0 + layer as f32 * 360.0, top]);
                let height = (34.0 + 14.0 + node.ports.len() as f32 * 25.0).max(62.0);
                top += height + 70.0;
            }
        }
        positions
    }

    fn node_media_category(&self, node: &Node) -> u8 {
        let mut has_audio = false;
        let mut has_video = false;
        let mut has_midi = false;
        for port_id in &node.ports {
            match self.port(*port_id).map(|port| port.port_type) {
                Some(PortType::Audio) => has_audio = true,
                Some(PortType::Video) => has_video = true,
                Some(PortType::MidiJack | PortType::MidiAlsa) => has_midi = true,
                _ => {}
            }
        }
        if has_audio {
            0
        } else if has_video {
            1
        } else if has_midi {
            2
        } else {
            3
        }
    }

    fn node_layout_role(&self, node: &Node) -> u8 {
        let mut has_source = false;
        let mut has_sink = false;
        for port_id in &node.ports {
            match self.port(*port_id).map(|port| port.direction) {
                Some(Direction::Source) => has_source = true,
                Some(Direction::Sink) => has_sink = true,
                None => {}
            }
        }
        match (has_source, has_sink) {
            (true, false) => 0,
            (false, true) => 2,
            _ => 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph() -> Graph {
        let mut graph = Graph::default();
        graph
            .add_node(Node::new(NodeId(1), "Source", NodeType::PipeWire))
            .unwrap();
        graph
            .add_node(Node::new(NodeId(2), "Sink", NodeType::PipeWire))
            .unwrap();
        graph
            .add_port(Port::new(
                PortId(1),
                NodeId(1),
                "out",
                Direction::Source,
                PortType::Audio,
            ))
            .unwrap();
        graph
            .add_port(Port::new(
                PortId(2),
                NodeId(2),
                "in",
                Direction::Sink,
                PortType::Audio,
            ))
            .unwrap();
        graph
    }

    #[test]
    fn validates_and_removes_links() {
        let mut graph = graph();
        graph.add_link(LinkId(1), PortId(1), PortId(2)).unwrap();
        assert_eq!(graph.links.len(), 1);
        graph.remove_link(LinkId(1)).unwrap();
        assert!(graph.links.is_empty());
    }

    #[test]
    fn rejects_wrong_direction() {
        let mut graph = graph();
        let error = graph.add_link(LinkId(1), PortId(2), PortId(1)).unwrap_err();
        assert_eq!(error, GraphError::NotSource(PortId(2)));
    }

    #[test]
    fn default_layout_groups_media_and_direction() {
        let mut graph = Graph::default();
        for (id, name) in [(1, "Audio source"), (2, "Audio sink"), (3, "MIDI source")] {
            graph
                .add_node(Node::new(NodeId(id), name, NodeType::PipeWire))
                .unwrap();
        }
        graph
            .add_port(Port::new(
                PortId(10),
                NodeId(1),
                "out",
                Direction::Source,
                PortType::Audio,
            ))
            .unwrap();
        graph
            .add_port(Port::new(
                PortId(11),
                NodeId(2),
                "in",
                Direction::Sink,
                PortType::Audio,
            ))
            .unwrap();
        graph
            .add_port(Port::new(
                PortId(12),
                NodeId(3),
                "out",
                Direction::Source,
                PortType::MidiJack,
            ))
            .unwrap();

        let positions = graph.default_node_positions();
        assert!(positions[&NodeId(1)][0] < positions[&NodeId(2)][0]);
        assert!(positions[&NodeId(1)][1] < positions[&NodeId(3)][1]);
    }

    #[test]
    fn default_layout_places_connected_hops_in_ordered_layers() {
        let mut graph = Graph::default();
        for (id, name) in [(1, "Source"), (2, "Mixer"), (3, "Sink")] {
            graph
                .add_node(Node::new(NodeId(id), name, NodeType::PipeWire))
                .unwrap();
        }
        for (id, node, name, direction) in [
            (10, 1, "out", Direction::Source),
            (11, 2, "in", Direction::Sink),
            (12, 2, "out", Direction::Source),
            (13, 3, "in", Direction::Sink),
        ] {
            graph
                .add_port(Port::new(
                    PortId(id),
                    NodeId(node),
                    name,
                    direction,
                    PortType::Audio,
                ))
                .unwrap();
        }
        graph.add_link(LinkId(20), PortId(10), PortId(11)).unwrap();
        graph.add_link(LinkId(21), PortId(12), PortId(13)).unwrap();

        let positions = graph.default_node_positions();
        assert!(positions[&NodeId(1)][0] < positions[&NodeId(2)][0]);
        assert!(positions[&NodeId(2)][0] < positions[&NodeId(3)][0]);
        assert_eq!(positions, graph.default_node_positions());
    }
}
