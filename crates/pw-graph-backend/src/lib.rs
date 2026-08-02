//! Backend abstraction. The in-memory driver makes the rest of the application
//! deterministic and testable while a PipeWire driver is added incrementally.

use pw_graph_core::{
    Graph, GraphError, Link, LinkId, Node, NodeId, NodeType, Port, PortId, PortType,
};
use std::collections::BTreeSet;
use thiserror::Error;

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

/// A normalized, node-level audio reading supplied by a backend.
///
/// PipeWire exposes graph topology separately from audio buffers, so meters
/// are deliberately kept as an optional side channel. An empty collection is
/// a valid result for backends that do not provide runtime audio data.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AudioMeter {
    pub node_id: NodeId,
    /// Root-mean-square level normalized to 0..=1.
    pub rms: f32,
    /// Peak level normalized to 0..=1.
    pub peak: f32,
    /// Milliseconds since the backend received the last audio buffer.
    pub age_ms: u32,
    pub available: bool,
}

/// Common operations needed by commands, patchbay activation, and the UI.
pub trait GraphDriver {
    fn refresh(&mut self) -> BackendResult<Vec<Node>>;
    fn connect(&mut self, src: PortId, dst: PortId) -> BackendResult<Link>;
    fn disconnect(&mut self, link: LinkId) -> BackendResult<Link>;
    fn rename_node(&mut self, node: pw_graph_core::NodeId, name: String) -> BackendResult<()>;
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
    fn graph(&self) -> &Graph;
    fn is_node_type(&self, node_type: NodeType) -> bool;
    fn is_port_type(&self, port_type: PortType) -> bool;
    fn audio_meters(&mut self) -> BackendResult<Vec<AudioMeter>> {
        Ok(Vec::new())
    }
}

/// A small backend that behaves like a PipeWire registry from the perspective
/// of the application. It is also useful for examples and integration tests.
#[derive(Clone, Debug, Default)]
pub struct InMemoryDriver {
    graph: Graph,
    next_link_id: u64,
}

impl InMemoryDriver {
    pub fn new(graph: Graph) -> Self {
        let next_link_id = graph.links.keys().map(|id| id.0).max().unwrap_or(0) + 1;
        Self {
            graph,
            next_link_id,
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

impl GraphDriver for InMemoryDriver {
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

    fn rename_node(&mut self, node: NodeId, name: String) -> BackendResult<()> {
        self.graph
            .nodes
            .get_mut(&node)
            .ok_or(GraphError::MissingNode(node))?
            .name = name;
        Ok(())
    }

    fn set_node_position(&mut self, node: NodeId, position: [f32; 2]) -> BackendResult<()> {
        self.graph
            .nodes
            .get_mut(&node)
            .ok_or(GraphError::MissingNode(node))?
            .position = position;
        Ok(())
    }

    fn graph(&self) -> &Graph {
        &self.graph
    }

    fn is_node_type(&self, node_type: NodeType) -> bool {
        matches!(node_type, NodeType::PipeWire)
    }

    fn is_port_type(&self, port_type: PortType) -> bool {
        matches!(
            port_type,
            PortType::Audio | PortType::Video | PortType::MidiJack
        )
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

    fn rename_node(&mut self, _node: NodeId, _name: String) -> BackendResult<()> {
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

    fn graph(&self) -> &Graph {
        &self.graph
    }

    fn is_node_type(&self, node_type: NodeType) -> bool {
        matches!(node_type, NodeType::PipeWire)
    }

    fn is_port_type(&self, port_type: PortType) -> bool {
        matches!(
            port_type,
            PortType::Audio | PortType::Video | PortType::MidiJack
        )
    }
}

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
    fn demo_backend_connects_and_disconnects() {
        let mut driver = InMemoryDriver::demo();
        let link = driver.connect(PortId(1), PortId(3)).unwrap();
        assert_eq!(driver.graph().links.len(), 1);
        driver.disconnect(link.id).unwrap();
        assert!(driver.graph().links.is_empty());
    }

    #[cfg(feature = "pipewire")]
    #[test]
    fn native_backend_refreshes_running_pipewire_registry() {
        let mut driver = PipewireDriver::new().expect("PipeWire daemon should be available");
        let nodes = driver
            .refresh()
            .expect("PipeWire registry snapshot should succeed");
        assert!(!nodes.is_empty());
        assert!(!driver.graph().ports.is_empty());
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
