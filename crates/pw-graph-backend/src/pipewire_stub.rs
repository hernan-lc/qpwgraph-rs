//! Compile-time fallback when the PipeWire feature is disabled.

use super::api::{BackendError, BackendResult, EffectDriver, GraphDriver};
use pw_graph_core::{Graph, GraphError, Link, LinkId, Node, NodeId, PortId};

#[derive(Debug, Default)]
pub struct PipewireDriver {
    graph: Graph,
}

impl PipewireDriver {
    pub fn new() -> BackendResult<Self> {
        Err(BackendError::unsupported(
            "compile pw-graph-backend with the pipewire feature",
        ))
    }
}

impl GraphDriver for PipewireDriver {
    fn refresh(&mut self) -> BackendResult<Vec<Node>> {
        Err(BackendError::unsupported("PipeWire feature is disabled"))
    }

    fn connect(&mut self, _src: PortId, _dst: PortId) -> BackendResult<Link> {
        Err(BackendError::unsupported("PipeWire feature is disabled"))
    }

    fn disconnect(&mut self, _link: LinkId) -> BackendResult<Link> {
        Err(BackendError::unsupported("PipeWire feature is disabled"))
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
        Err(BackendError::unsupported("PipeWire feature is disabled"))
    }

    fn set_node_volume(&mut self, _node: NodeId, _volume: f32) -> BackendResult<()> {
        Err(BackendError::unsupported("PipeWire feature is disabled"))
    }

    fn graph(&self) -> &Graph {
        &self.graph
    }
}

impl EffectDriver for PipewireDriver {}
