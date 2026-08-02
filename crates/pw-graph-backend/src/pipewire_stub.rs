//! Compile-time fallback when the PipeWire feature is disabled.

use super::api::{BackendError, BackendResult, EffectDriver, GraphDriver};
use pw_graph_core::{Graph, GraphError, Link, LinkId, Node, NodeId, NodeType, PortId, PortType};

#[derive(Debug, Default)]
pub struct PipewireDriver {
    graph: Graph,
}

impl PipewireDriver {
    pub fn new() -> BackendResult<Self> {
        Err(BackendError::Unsupported(
            "compile pw-graph-backend with the pipewire feature".into(),
        ))
    }
}

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

    fn set_node_position(&mut self, node: NodeId, position: [f32; 2]) -> BackendResult<()> {
        self.graph
            .nodes
            .get_mut(&node)
            .ok_or(GraphError::MissingNode(node))?
            .position = position;
        Ok(())
    }

    fn set_node_mute(&mut self, _node: NodeId, _muted: bool) -> BackendResult<()> {
        Err(BackendError::Unsupported(
            "PipeWire feature is disabled".into(),
        ))
    }

    fn set_node_volume(&mut self, _node: NodeId, _volume: f32) -> BackendResult<()> {
        Err(BackendError::Unsupported(
            "PipeWire feature is disabled".into(),
        ))
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

impl EffectDriver for PipewireDriver {}
