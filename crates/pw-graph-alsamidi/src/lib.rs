//! Optional ALSA Sequencer MIDI backend seam.
//!
//! The registry/enumeration calls are deliberately left behind the `alsa`
//! feature for the next milestone. This crate already implements the same
//! driver contract, so dispatch can be added without changing the app layers.

use pw_graph_backend::{BackendError, BackendResult, GraphDriver};
use pw_graph_core::{Graph, Link, LinkId, Node, NodeId, NodeType, PortId, PortType};

#[derive(Debug, Default)]
pub struct AlsaMidiDriver {
    graph: Graph,
}

impl AlsaMidiDriver {
    pub fn new() -> BackendResult<Self> {
        Ok(Self::default())
    }
}

impl GraphDriver for AlsaMidiDriver {
    fn refresh(&mut self) -> BackendResult<Vec<Node>> {
        Err(BackendError::Unsupported(
            "ALSA Sequencer enumeration is not wired yet".into(),
        ))
    }

    fn connect(&mut self, _src: PortId, _dst: PortId) -> BackendResult<Link> {
        Err(BackendError::Unsupported(
            "ALSA Sequencer connection is not wired yet".into(),
        ))
    }

    fn disconnect(&mut self, _link: LinkId) -> BackendResult<Link> {
        Err(BackendError::Unsupported(
            "ALSA Sequencer disconnection is not wired yet".into(),
        ))
    }

    fn rename_node(&mut self, _node: NodeId, _name: String) -> BackendResult<()> {
        Err(BackendError::Unsupported(
            "ALSA MIDI node rename is not an application operation".into(),
        ))
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
