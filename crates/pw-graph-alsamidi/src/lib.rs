//! Optional ALSA Sequencer MIDI backend.
//!
//! ALSA identifiers use the shared backend namespace so this graph can be
//! merged with other native backends without colliding with their IDs.

#[cfg(all(target_os = "linux", feature = "alsa"))]
use pw_graph_backend::BackendCapabilities;
use pw_graph_backend::{BackendError, BackendResult, GraphDriver};
#[cfg(all(target_os = "linux", feature = "alsa"))]
use pw_graph_core::{decode_backend_local_id, encode_backend_id, BackendNamespace, NodeId};
#[cfg(all(target_os = "linux", feature = "alsa"))]
use pw_graph_core::{Direction, GraphError, Port};
use pw_graph_core::{Graph, Link, LinkId, Node, NodeType, PortId, PortType};
#[cfg(all(target_os = "linux", feature = "alsa"))]
use std::collections::BTreeMap;
#[cfg(all(target_os = "linux", feature = "alsa"))]
use std::ffi::{c_void, CStr};

#[cfg(all(target_os = "linux", feature = "alsa"))]
const NAMESPACE: BackendNamespace = BackendNamespace::AlsaMidi;
#[cfg(all(target_os = "linux", feature = "alsa"))]
const MAX_NODES: usize = 256;
#[cfg(all(target_os = "linux", feature = "alsa"))]
const MAX_PORTS: usize = 4096;

#[cfg(all(target_os = "linux", feature = "alsa"))]
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

#[cfg(all(target_os = "linux", feature = "alsa"))]
fn native_error(operation: &str, code: i32) -> BackendError {
    BackendError::Native(format!("{operation} failed with code {code}"))
}

#[cfg(all(target_os = "linux", feature = "alsa"))]
fn raw_text(value: &[std::ffi::c_char]) -> String {
    unsafe { CStr::from_ptr(value.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

#[cfg(all(target_os = "linux", feature = "alsa"))]
fn graph_id(native_id: u64) -> u64 {
    encode_backend_id(NAMESPACE, native_id)
}

#[cfg(all(target_os = "linux", feature = "alsa"))]
fn link_local_id(output_port: u32, input_port: u32) -> u64 {
    // The ALSA shim exposes the two endpoint IDs but not a native link ID.
    // Hash the pair into the 56-bit local-ID space instead of packing two
    // u32s into the full u64, which would overwrite the namespace byte.
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in output_port
        .to_le_bytes()
        .into_iter()
        .chain(input_port.to_le_bytes())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash & pw_graph_core::LOCAL_ID_MASK
}

#[cfg(all(target_os = "linux", feature = "alsa"))]
fn graph_link_id(output_port: u32, input_port: u32) -> LinkId {
    LinkId(graph_id(link_local_id(output_port, input_port)))
}

#[cfg(all(target_os = "linux", feature = "alsa"))]
#[derive(Debug)]
pub struct AlsaMidiDriver {
    native: *mut c_void,
    graph: Graph,
    positions: BTreeMap<NodeId, [f32; 2]>,
}

#[cfg(all(target_os = "linux", feature = "alsa"))]
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
            positions: BTreeMap::new(),
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
            let id = NodeId(graph_id(raw.id as u64));
            let node = Node::new(id, raw_text(&raw.name), NodeType::AlsaMidi);
            graph.add_node(node)?;
        }
        for raw in snapshot.ports[..(snapshot.port_count as usize).min(MAX_PORTS)].iter() {
            let node_id = NodeId(graph_id(raw.node_id as u64));
            if graph.node(node_id).is_none() {
                continue;
            }
            graph.add_port(Port::new(
                PortId(graph_id(raw.id as u64)),
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
                node.position = self.positions.get(&node_id).copied().unwrap_or(position);
            }
        }
        for raw in snapshot.links[..(snapshot.link_count as usize).min(MAX_PORTS)].iter() {
            let output = graph_id(raw.output_port as u64);
            let input = graph_id(raw.input_port as u64);
            let id = graph_link_id(raw.output_port, raw.input_port);
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

#[cfg(all(target_os = "linux", feature = "alsa"))]
impl Drop for AlsaMidiDriver {
    fn drop(&mut self) {
        if !self.native.is_null() {
            unsafe { native::alsa_shim_free(self.native) };
            self.native = std::ptr::null_mut();
        }
    }
}

#[cfg(all(target_os = "linux", feature = "alsa"))]
impl GraphDriver for AlsaMidiDriver {
    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            topology: true,
            connect: true,
            disconnect: true,
            ..BackendCapabilities::default()
        }
    }

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
        let raw_src = decode_backend_local_id(src.0) as u32;
        let raw_dst = decode_backend_local_id(dst.0) as u32;
        let result = unsafe { native::alsa_shim_connect(self.native, raw_src, raw_dst) };
        if result < 0 {
            return Err(native_error("ALSA Sequencer connection", result));
        }
        let link = Link {
            id: graph_link_id(raw_src, raw_dst),
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
        let raw_src = decode_backend_local_id(existing.output_port.0) as u32;
        let raw_dst = decode_backend_local_id(existing.input_port.0) as u32;
        let result = unsafe { native::alsa_shim_disconnect(self.native, raw_src, raw_dst) };
        if result < 0 {
            return Err(native_error("ALSA Sequencer disconnection", result));
        }
        self.snapshot()?;
        Ok(existing)
    }

    fn set_node_position(&mut self, node: NodeId, position: [f32; 2]) -> BackendResult<()> {
        self.graph
            .nodes
            .get_mut(&node)
            .ok_or(GraphError::MissingNode(node))?
            .position = position;
        self.positions.insert(node, position);
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

impl pw_graph_backend::EffectDriver for AlsaMidiDriver {}

#[cfg(all(test, target_os = "linux", feature = "alsa"))]
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

#[cfg(not(all(target_os = "linux", feature = "alsa")))]
#[derive(Debug, Default)]
pub struct AlsaMidiDriver {
    graph: Graph,
}

#[cfg(not(all(target_os = "linux", feature = "alsa")))]
impl AlsaMidiDriver {
    pub fn new() -> BackendResult<Self> {
        Err(BackendError::Unsupported(
            "compile with the alsa feature".into(),
        ))
    }
}

#[cfg(not(all(target_os = "linux", feature = "alsa")))]
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
