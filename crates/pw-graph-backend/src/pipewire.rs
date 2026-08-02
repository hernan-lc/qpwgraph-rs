//! Native PipeWire graph driver implementation.

use super::*;
use std::ffi::{c_char, c_void, CStr};

const MAX_NODES: usize = 4096;
const MAX_PORTS: usize = 16384;
const MAX_LINKS: usize = 16384;
const MAX_METERS: usize = 4096;

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

#[repr(C)]
struct RawMeter {
    node_id: u32,
    rms: f32,
    peak: f32,
    age_ms: u32,
    available: u32,
}

#[repr(C)]
struct RawMeterSnapshot {
    meter_count: u32,
    meters: [RawMeter; MAX_METERS],
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
    fn pw_graph_shim_meter_snapshot(shim: *mut c_void, snapshot: *mut RawMeterSnapshot) -> i32;
}

fn raw_text(value: &[c_char]) -> String {
    unsafe { CStr::from_ptr(value.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

fn native_error(operation: &str, code: i32) -> BackendError {
    BackendError::Native(format!("{operation} failed with code {code}"))
}

fn is_private_meter_node(name: &str) -> bool {
    name.starts_with("qpwgraph-rs audio meter:")
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
            let node_name = raw_text(&raw.name);
            if is_private_meter_node(&node_name) {
                continue;
            }
            let mut node = Node::new(id, node_name, NodeType::PipeWire);
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

    fn audio_meters(&mut self) -> BackendResult<Vec<AudioMeter>> {
        let mut snapshot = Box::<RawMeterSnapshot>::new_uninit();
        let result = unsafe { pw_graph_shim_meter_snapshot(self.native, snapshot.as_mut_ptr()) };
        if result < 0 {
            return Err(native_error("PipeWire audio meter snapshot", result));
        }
        let snapshot = unsafe { snapshot.assume_init() };
        Ok(
            snapshot.meters[..(snapshot.meter_count as usize).min(MAX_METERS)]
                .iter()
                .filter(|meter| meter.available != 0)
                .map(|meter| AudioMeter {
                    node_id: NodeId(meter.node_id as u64),
                    rms: meter.rms.clamp(0.0, 1.0),
                    peak: meter.peak.clamp(0.0, 1.0),
                    age_ms: meter.age_ms,
                    available: true,
                })
                .collect(),
        )
    }
}
