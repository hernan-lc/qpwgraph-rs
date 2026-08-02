//! Optional ALSA Sequencer MIDI backend.
//!
//! ALSA identifiers are namespaced in the high bit so this graph can be
//! merged with PipeWire without colliding with PipeWire global IDs.

use pw_graph_backend::{BackendError, BackendResult, GraphDriver};
#[cfg(feature = "alsa")]
use pw_graph_core::{Direction, GraphError, Port};
use pw_graph_core::{Graph, Link, LinkId, Node, NodeId, NodeType, PortId, PortType};
#[cfg(feature = "alsa")]
use std::ffi::{c_void, CStr};

#[cfg(feature = "alsa")]
const NAMESPACE: u64 = 1 << 63;
#[cfg(feature = "alsa")]
const MAX_NODES: usize = 256;
#[cfg(feature = "alsa")]
const MAX_PORTS: usize = 4096;

#[cfg(feature = "alsa")]
mod native {
    use super::*;

    #[repr(C)]
    pub struct RawNode {
        pub id: u32,
        pub name: [std::ffi::c_char; 256],
    }
    #[repr(C)]
    pub struct RawPort {
        pub id: u32,
        pub node_id: u32,
        pub direction: u32,
        pub name: [std::ffi::c_char; 256],
    }
    #[repr(C)]
    pub struct RawLink {
        pub output_port: u32,
        pub input_port: u32,
    }
    #[repr(C)]
    pub struct RawSnapshot {
        pub node_count: u32,
        pub port_count: u32,
        pub link_count: u32,
        pub nodes: [RawNode; MAX_NODES],
        pub ports: [RawPort; MAX_PORTS],
        pub links: [RawLink; MAX_PORTS],
    }

    unsafe extern "C" {
        pub fn alsa_shim_new() -> *mut c_void;
        pub fn alsa_shim_free(shim: *mut c_void);
        pub fn alsa_shim_snapshot(shim: *mut c_void, snapshot: *mut RawSnapshot) -> i32;
        pub fn alsa_shim_connect(shim: *mut c_void, output_port: u32, input_port: u32) -> i32;
        pub fn alsa_shim_disconnect(shim: *mut c_void, output_port: u32, input_port: u32) -> i32;
    }
}

#[cfg(feature = "alsa")]
fn native_error(operation: &str, code: i32) -> BackendError {
    BackendError::Native(format!("{operation} failed with code {code}"))
}

#[cfg(feature = "alsa")]
fn raw_text(value: &[std::ffi::c_char]) -> String {
    unsafe { CStr::from_ptr(value.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

#[cfg(feature = "alsa")]
#[derive(Debug)]
pub struct AlsaMidiDriver {
    native: *mut c_void,
    graph: Graph,
}

#[cfg(feature = "alsa")]
impl AlsaMidiDriver {
    pub fn new() -> BackendResult<Self> {
        let native = unsafe { native::alsa_shim_new() };
        if native.is_null() {
            return Err(BackendError::Native(
                "could not open the ALSA Sequencer".into(),
            ));
        }
        Ok(Self {
            native,
            graph: Graph::default(),
        })
    }

    fn snapshot(&mut self) -> BackendResult<()> {
        let mut snapshot = Box::<native::RawSnapshot>::new_uninit();
        let result = unsafe { native::alsa_shim_snapshot(self.native, snapshot.as_mut_ptr()) };
        if result < 0 {
            return Err(native_error("ALSA Sequencer snapshot", result));
        }
        let snapshot = unsafe { snapshot.assume_init() };
        let mut graph = Graph::default();
        for raw in snapshot.nodes[..(snapshot.node_count as usize).min(MAX_NODES)].iter() {
            let id = NodeId(NAMESPACE | raw.id as u64);
            let node = Node::new(id, raw_text(&raw.name), NodeType::AlsaMidi);
            graph.add_node(node)?;
        }
        for raw in snapshot.ports[..(snapshot.port_count as usize).min(MAX_PORTS)].iter() {
            let node_id = NodeId(NAMESPACE | raw.node_id as u64);
            if graph.node(node_id).is_none() {
                continue;
            }
            graph.add_port(Port::new(
                PortId(NAMESPACE | raw.id as u64),
                node_id,
                raw_text(&raw.name),
                if raw.direction == 1 {
                    Direction::Source
                } else {
                    Direction::Sink
                },
                PortType::MidiAlsa,
            ))?;
        }
        let default_positions = graph.default_node_positions();
        for (node_id, position) in default_positions {
            if let Some(node) = graph.nodes.get_mut(&node_id) {
                node.position = position;
            }
        }
        for raw in snapshot.links[..(snapshot.link_count as usize).min(MAX_PORTS)].iter() {
            let output = NAMESPACE | raw.output_port as u64;
            let input = NAMESPACE | raw.input_port as u64;
            let id = LinkId(NAMESPACE | ((raw.output_port as u64) << 32) | raw.input_port as u64);
            let _ = graph.insert_existing_link(Link {
                id,
                output_port: PortId(output),
                input_port: PortId(input),
            });
        }
        self.graph = graph;
        Ok(())
    }
}

#[cfg(feature = "alsa")]
impl Drop for AlsaMidiDriver {
    fn drop(&mut self) {
        if !self.native.is_null() {
            unsafe { native::alsa_shim_free(self.native) };
            self.native = std::ptr::null_mut();
        }
    }
}

#[cfg(feature = "alsa")]
impl GraphDriver for AlsaMidiDriver {
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
        let raw_src = (src.0 & !NAMESPACE) as u32;
        let raw_dst = (dst.0 & !NAMESPACE) as u32;
        let result = unsafe { native::alsa_shim_connect(self.native, raw_src, raw_dst) };
        if result < 0 {
            return Err(native_error("ALSA Sequencer connection", result));
        }
        let link = Link {
            id: LinkId(NAMESPACE | ((raw_src as u64) << 32) | raw_dst as u64),
            output_port: src,
            input_port: dst,
        };
        self.snapshot()?;
        Ok(link)
    }

    fn disconnect(&mut self, link: LinkId) -> BackendResult<Link> {
        let existing = self
            .graph
            .link(link)
            .cloned()
            .ok_or(GraphError::MissingLink(link))?;
        let raw_src = (existing.output_port.0 & !NAMESPACE) as u32;
        let raw_dst = (existing.input_port.0 & !NAMESPACE) as u32;
        let result = unsafe { native::alsa_shim_disconnect(self.native, raw_src, raw_dst) };
        if result < 0 {
            return Err(native_error("ALSA Sequencer disconnection", result));
        }
        self.snapshot()?;
        Ok(existing)
    }

    fn rename_node(&mut self, _node: NodeId, _name: String) -> BackendResult<()> {
        Err(BackendError::Unsupported(
            "ALSA client names are external metadata".into(),
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
        matches!(node_type, NodeType::AlsaMidi)
    }
    fn is_port_type(&self, port_type: PortType) -> bool {
        matches!(port_type, PortType::MidiAlsa)
    }
}

#[cfg(all(test, feature = "alsa"))]
mod tests {
    use super::*;

    #[test]
    fn native_backend_refreshes_alsa_registry() {
        let Ok(mut driver) = AlsaMidiDriver::new() else {
            // ALSA sequencer devices are not present in headless CI
            // containers. Run this live test when the host provides one.
            return;
        };
        let nodes = driver
            .refresh()
            .expect("ALSA registry snapshot should succeed");
        assert!(!nodes.is_empty());
        assert!(!driver.graph().ports.is_empty());
    }

    #[test]
    fn native_backend_can_create_and_destroy_a_link_when_enabled() {
        if std::env::var_os("PW_GRAPH_TEST_ALSA_LINKS").is_none() {
            return;
        }
        let mut driver = AlsaMidiDriver::new().expect("ALSA Sequencer should be available");
        driver
            .refresh()
            .expect("ALSA registry snapshot should succeed");
        let existing: std::collections::BTreeSet<_> = driver
            .graph()
            .links
            .values()
            .map(|link| (link.output_port, link.input_port))
            .collect();
        let pair = driver.graph().ports.values().find_map(|output| {
            if !output.direction.is_source() {
                return None;
            }
            driver.graph().ports.values().find_map(|input| {
                if !input.direction.is_sink() || existing.contains(&(output.id, input.id)) {
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
            .expect("ALSA link creation should succeed");
        assert!(driver.graph().link(link.id).is_some());
        driver
            .disconnect(link.id)
            .expect("ALSA link destruction should succeed");
    }
}

#[cfg(not(feature = "alsa"))]
#[derive(Debug, Default)]
pub struct AlsaMidiDriver {
    graph: Graph,
}

#[cfg(not(feature = "alsa"))]
impl AlsaMidiDriver {
    pub fn new() -> BackendResult<Self> {
        Err(BackendError::Unsupported(
            "compile with the alsa feature".into(),
        ))
    }
}

#[cfg(not(feature = "alsa"))]
impl GraphDriver for AlsaMidiDriver {
    fn refresh(&mut self) -> BackendResult<Vec<Node>> {
        Err(BackendError::Unsupported("ALSA feature is disabled".into()))
    }
    fn connect(&mut self, _src: PortId, _dst: PortId) -> BackendResult<Link> {
        Err(BackendError::Unsupported("ALSA feature is disabled".into()))
    }
    fn disconnect(&mut self, _link: LinkId) -> BackendResult<Link> {
        Err(BackendError::Unsupported("ALSA feature is disabled".into()))
    }
    fn rename_node(&mut self, _node: NodeId, _name: String) -> BackendResult<()> {
        Err(BackendError::Unsupported("ALSA feature is disabled".into()))
    }
    fn graph(&self) -> &Graph {
        &self.graph
    }
    fn is_node_type(&self, node_type: NodeType) -> bool {
        matches!(node_type, NodeType::AlsaMidi)
    }
    fn is_port_type(&self, port_type: PortType) -> bool {
        matches!(port_type, PortType::MidiAlsa)
    }
}
