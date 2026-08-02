//! Core graph types shared by every backend and presentation layer.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
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

type LayoutNode = (NodeId, String, usize);
type LayoutGroups = BTreeMap<(u8, u8), Vec<LayoutNode>>;

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
            ports: Vec::new(),
            position: [0.0, 0.0],
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Port {
    pub id: PortId,
    pub node_id: NodeId,
    pub name: String,
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
            direction,
            port_type,
        }
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

    /// Suggest a readable first layout for a graph that does not have saved
    /// positions yet. Media categories form horizontal bands and the port
    /// direction forms columns: source-only nodes on the left, mixed nodes in
    /// the middle, and sink-only nodes on the right.
    pub fn default_node_positions(&self) -> BTreeMap<NodeId, [f32; 2]> {
        let mut groups: LayoutGroups = BTreeMap::new();
        for node in self.nodes.values() {
            let category = self.node_media_category(node);
            let role = self.node_layout_role(node);
            groups.entry((category, role)).or_default().push((
                node.id,
                node.name.to_ascii_lowercase(),
                node.ports.len(),
            ));
        }

        for nodes in groups.values_mut() {
            nodes.sort_by(|left, right| left.1.cmp(&right.1).then_with(|| left.0.cmp(&right.0)));
        }

        let categories: BTreeSet<u8> = groups.keys().map(|(category, _)| *category).collect();
        let mut positions = BTreeMap::new();
        let mut category_top = 40.0;
        for category in categories {
            let mut role_tops = [category_top; 3];
            for role in 0..3_u8 {
                let Some(nodes) = groups.get(&(category, role)) else {
                    continue;
                };
                for (node_id, _, port_count) in nodes {
                    positions.insert(
                        *node_id,
                        [40.0 + f32::from(role) * 320.0, role_tops[role as usize]],
                    );
                    let height = (34.0 + 14.0 + *port_count as f32 * 25.0).max(62.0);
                    role_tops[role as usize] += height + 70.0;
                }
            }
            category_top = role_tops
                .into_iter()
                .max_by(f32::total_cmp)
                .unwrap_or(category_top)
                + 100.0;
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
}
