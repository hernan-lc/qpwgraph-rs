//! Backend abstraction. The in-memory driver makes the rest of the application
//! deterministic and testable while a PipeWire driver is added incrementally.

use pw_graph_core::{Graph, GraphError, Link, LinkId, Node, NodeType, Port, PortId, PortType};
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

use pw_graph_core::{Direction, NodeId};

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
mod pipewire_native {
    use super::*;
    use std::ffi::{c_char, c_void, CStr};

    const MAX_NODES: usize = 4096;
    const MAX_PORTS: usize = 16384;
    const MAX_LINKS: usize = 16384;

    #[repr(C)]
    struct RawNode {
        id: u32,
        name: [c_char; 256],
    }

    #[repr(C)]
    struct RawPort {
        id: u32,
        node_id: u32,
        direction: u32,
        name: [c_char; 256],
        media_type: [c_char; 64],
    }

    #[repr(C)]
    struct RawLink {
        id: u32,
        output_port: u32,
        input_port: u32,
    }

    #[repr(C)]
    struct RawSnapshot {
        node_count: u32,
        port_count: u32,
        link_count: u32,
        nodes: [RawNode; MAX_NODES],
        ports: [RawPort; MAX_PORTS],
        links: [RawLink; MAX_LINKS],
    }

    unsafe extern "C" {
        fn pw_graph_shim_new() -> *mut c_void;
        fn pw_graph_shim_free(shim: *mut c_void);
        fn pw_graph_shim_snapshot(shim: *mut c_void, snapshot: *mut RawSnapshot) -> i32;
        fn pw_graph_shim_create_link(
            shim: *mut c_void,
            output_node: u32,
            output_port: u32,
            input_node: u32,
            input_port: u32,
            link_id: *mut u32,
        ) -> i32;
        fn pw_graph_shim_destroy_link(shim: *mut c_void, link_id: u32) -> i32;
    }

    fn raw_text(value: &[c_char]) -> String {
        unsafe { CStr::from_ptr(value.as_ptr()) }
            .to_string_lossy()
            .into_owned()
    }

    fn native_error(operation: &str, code: i32) -> BackendError {
        BackendError::Native(format!("{operation} failed with code {code}"))
    }

    #[derive(Debug)]
    pub struct PipewireDriver {
        native: *mut c_void,
        graph: Graph,
        positions: std::collections::BTreeMap<NodeId, [f32; 2]>,
    }

    impl PipewireDriver {
        pub fn new() -> BackendResult<Self> {
            let native = unsafe { pw_graph_shim_new() };
            if native.is_null() {
                return Err(BackendError::Native(
                    "could not connect to the PipeWire daemon".into(),
                ));
            }
            Ok(Self {
                native,
                graph: Graph::default(),
                positions: std::collections::BTreeMap::new(),
            })
        }

        fn snapshot(&mut self) -> BackendResult<()> {
            let mut snapshot = Box::<RawSnapshot>::new_uninit();
            let result = unsafe { pw_graph_shim_snapshot(self.native, snapshot.as_mut_ptr()) };
            if result < 0 {
                return Err(native_error("PipeWire registry snapshot", result));
            }
            let snapshot = unsafe { snapshot.assume_init() };

            let mut graph = Graph::default();
            for (index, raw) in snapshot.nodes[..(snapshot.node_count as usize).min(MAX_NODES)]
                .iter()
                .enumerate()
            {
                let id = NodeId(raw.id as u64);
                let mut node = Node::new(id, raw_text(&raw.name), NodeType::PipeWire);
                node.position = self.positions.get(&id).copied().unwrap_or_else(|| {
                    let column = (index % 4) as f32;
                    let row = (index / 4) as f32;
                    [40.0 + column * 280.0, 40.0 + row * 180.0]
                });
                self.positions.insert(id, node.position);
                graph.add_node(node)?;
            }
            for raw in snapshot.ports[..(snapshot.port_count as usize).min(MAX_PORTS)].iter() {
                let node_id = NodeId(raw.node_id as u64);
                if graph.node(node_id).is_none() {
                    continue;
                }
                graph.add_port(Port::new(
                    PortId(raw.id as u64),
                    node_id,
                    raw_text(&raw.name),
                    if raw.direction == 1 {
                        Direction::Source
                    } else {
                        Direction::Sink
                    },
                    match raw_text(&raw.media_type).to_ascii_lowercase().as_str() {
                        "audio" => PortType::Audio,
                        "video" => PortType::Video,
                        "midi" => PortType::MidiJack,
                        _ => PortType::Unknown,
                    },
                ))?;
            }
            for raw in snapshot.links[..(snapshot.link_count as usize).min(MAX_LINKS)].iter() {
                let _ = graph.insert_existing_link(Link {
                    id: LinkId(raw.id as u64),
                    output_port: PortId(raw.output_port as u64),
                    input_port: PortId(raw.input_port as u64),
                });
            }
            self.graph = graph;
            Ok(())
        }
    }

    impl Drop for PipewireDriver {
        fn drop(&mut self) {
            if !self.native.is_null() {
                unsafe { pw_graph_shim_free(self.native) };
                self.native = std::ptr::null_mut();
            }
        }
    }

    impl GraphDriver for PipewireDriver {
        fn refresh(&mut self) -> BackendResult<Vec<Node>> {
            self.snapshot()?;
            Ok(self.graph.nodes.values().cloned().collect())
        }

        fn connect(&mut self, src: PortId, dst: PortId) -> BackendResult<Link> {
            let output = self.graph.port(src).ok_or(GraphError::MissingPort(src))?;
            let input = self.graph.port(dst).ok_or(GraphError::MissingPort(dst))?;
            if !output.direction.is_source() {
                return Err(GraphError::NotSource(src).into());
            }
            if !input.direction.is_sink() {
                return Err(GraphError::NotSink(dst).into());
            }
            if output.port_type != input.port_type
                && output.port_type != PortType::Unknown
                && input.port_type != PortType::Unknown
            {
                return Err(GraphError::IncompatiblePorts(src, dst).into());
            }
            let mut link_id = 0;
            let result = unsafe {
                pw_graph_shim_create_link(
                    self.native,
                    output.node_id.0 as u32,
                    src.0 as u32,
                    input.node_id.0 as u32,
                    dst.0 as u32,
                    &mut link_id,
                )
            };
            if result < 0 {
                return Err(native_error("PipeWire link creation", result));
            }
            self.snapshot()?;
            Ok(self
                .graph
                .link(LinkId(link_id as u64))
                .cloned()
                .unwrap_or(Link {
                    id: LinkId(link_id as u64),
                    output_port: src,
                    input_port: dst,
                }))
        }

        fn disconnect(&mut self, link: LinkId) -> BackendResult<Link> {
            let existing = self
                .graph
                .link(link)
                .cloned()
                .ok_or(GraphError::MissingLink(link))?;
            let result = unsafe { pw_graph_shim_destroy_link(self.native, link.0 as u32) };
            if result < 0 {
                return Err(native_error("PipeWire link destruction", result));
            }
            self.snapshot()?;
            Ok(existing)
        }

        fn rename_node(&mut self, _node: NodeId, _name: String) -> BackendResult<()> {
            Err(BackendError::Unsupported(
                "PipeWire node names are owned by the producing client".into(),
            ))
        }

        fn set_node_position(&mut self, node: NodeId, position: [f32; 2]) -> BackendResult<()> {
            self.positions.insert(node, position);
            if let Some(node_data) = self.graph.nodes.get_mut(&node) {
                node_data.position = position;
                Ok(())
            } else {
                Err(GraphError::MissingNode(node).into())
            }
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
                PortType::Audio | PortType::Video | PortType::MidiJack | PortType::Unknown
            )
        }
    }
}

#[cfg(feature = "pipewire")]
pub use pipewire_native::PipewireDriver;

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
