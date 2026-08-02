//! egui canvas primitives. Backend mutations are returned as actions so the UI
//! never owns the driver or command stack.

use egui::{vec2, Pos2, Vec2};
use pw_graph_core::{LinkId, NodeId, PortId};
use std::collections::{BTreeMap, BTreeSet};

mod canvas;

#[derive(Clone, Debug, PartialEq)]
pub enum CanvasAction {
    Connect { output: PortId, input: PortId },
    Disconnect { link: LinkId },
    MoveNode { node: NodeId, position: [f32; 2] },
}

pub struct GraphCanvas {
    pub zoom: f32,
    pub node_text_scale: f32,
    pub pan: Vec2,
    pub sort_ports_by_name: bool,
    pub sort_ports_descending: bool,
    pub thumbnail_mode: bool,
    pub repel_overlapping_nodes: bool,
    pub connect_through_nodes: bool,
    pending_output: Option<PortId>,
    pub selected_node: Option<NodeId>,
    pub selected_nodes: BTreeSet<NodeId>,
    selected_link: Option<LinkId>,
    selection_start: Option<Pos2>,
    selection_current: Option<Pos2>,
    dragging_node: Option<NodeId>,
    dragging_origin: BTreeMap<NodeId, [f32; 2]>,
    drag_delta: Vec2,
}

impl Default for GraphCanvas {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            node_text_scale: 1.0,
            pan: vec2(24.0, 24.0),
            sort_ports_by_name: true,
            sort_ports_descending: false,
            thumbnail_mode: false,
            repel_overlapping_nodes: false,
            connect_through_nodes: false,
            pending_output: None,
            selected_node: None,
            selected_nodes: BTreeSet::new(),
            selected_link: None,
            selection_start: None,
            selection_current: None,
            dragging_node: None,
            dragging_origin: BTreeMap::new(),
            drag_delta: Vec2::ZERO,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canvas_has_expected_default_view() {
        let canvas = GraphCanvas::default();
        assert_eq!(canvas.zoom, 1.0);
        assert_eq!(canvas.selected_node, None);
        assert!(canvas.selected_nodes.is_empty());
    }
}
