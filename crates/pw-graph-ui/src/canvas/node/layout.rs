use crate::{GraphCanvas, NodeId, PortId};
use egui::{pos2, vec2, Pos2, Rect, Vec2};
use pw_graph_core::{Direction, Graph, Node, Port, PortType};

use super::super::ports::{grouped_rows, pair_ports};
use super::{
    AUDIO_CONTROLS_HEIGHT, COLLAPSED_NODE_HEIGHT, NODE_HEADER_HEIGHT, NODE_WIDTH, PORT_ROW_HEIGHT,
};

impl GraphCanvas {
    pub(crate) fn ordered_ports<'a>(&self, graph: &'a Graph, node: &Node) -> Vec<&'a Port> {
        let mut ports: Vec<&Port> = node
            .ports
            .iter()
            .filter_map(|id| graph.ports.get(id))
            .filter(|port| self.media_filter.matches_port_type(port.port_type))
            .filter(|port| self.search_matches_port(graph, port.id))
            .collect();
        if self.sort_ports_by_name {
            ports.sort_by_key(|port| port.name.to_ascii_lowercase());
        } else {
            ports.sort_by_key(|port| port.id);
        }
        if self.sort_ports_descending {
            ports.reverse();
        }
        ports
    }

    pub(crate) fn node_scene_size(&self, graph: &Graph, node: &Node) -> Vec2 {
        let height = if self.thumbnail_mode {
            62.0
        } else if self.node_appearance(node.id).collapsed {
            COLLAPSED_NODE_HEIGHT
        } else {
            let ports = self.ordered_ports(graph, node);
            let port_count = grouped_rows(self.connect_mode, &ports).len();
            let controls_height = if ports.iter().any(|port| port.port_type == PortType::Audio) {
                AUDIO_CONTROLS_HEIGHT
            } else {
                0.0
            };
            (NODE_HEADER_HEIGHT + controls_height + 14.0 + port_count as f32 * PORT_ROW_HEIGHT)
                .max(62.0)
        };
        vec2(NODE_WIDTH, height)
    }

    pub(crate) fn node_rect(&self, rect: Rect, graph: &Graph, node: &Node) -> Rect {
        let size = self.node_scene_size(graph, node) * self.zoom;
        let position = self
            .dragging_origin
            .get(&node.id)
            .copied()
            .map(|origin| {
                [
                    origin[0] + self.drag_delta.x / self.zoom,
                    origin[1] + self.drag_delta.y / self.zoom,
                ]
            })
            .unwrap_or(node.position);
        let top_left =
            rect.left_top() + self.pan + vec2(position[0] * self.zoom, position[1] * self.zoom);
        Rect::from_min_size(top_left, size)
    }

    pub(crate) fn port_anchor(&self, rect: Rect, graph: &Graph, port: &Port) -> Option<Pos2> {
        let node = graph.nodes.get(&port.node_id)?;
        let ordered = self.ordered_ports(graph, node);
        let rows = grouped_rows(self.connect_mode, &ordered);
        let index = rows
            .iter()
            .position(|row| row.iter().any(|item| item.id == port.id))?;
        let node_rect = self.node_rect(rect, graph, node);
        if self.node_appearance(node.id).collapsed {
            let x = if port.direction == Direction::Source {
                node_rect.right() - 12.0 * self.zoom
            } else {
                node_rect.left() + 12.0 * self.zoom
            };
            return Some(pos2(x, node_rect.center().y));
        }
        let x = if port.direction == Direction::Source {
            node_rect.right() - 12.0 * self.zoom
        } else {
            node_rect.left() + 12.0 * self.zoom
        };
        Some(pos2(
            x,
            node_rect.top()
                + (NODE_HEADER_HEIGHT
                    + if ordered.iter().any(|item| item.port_type == PortType::Audio) {
                        AUDIO_CONTROLS_HEIGHT
                    } else {
                        0.0
                    }
                    + 13.0
                    + index as f32 * PORT_ROW_HEIGHT)
                    * self.zoom,
        ))
    }

    /// The topmost visible node (other than `exclude`) whose rect contains
    /// `point`, used to find the drop target of an Easy-mode connect drag.
    pub(crate) fn node_at(
        &self,
        rect: Rect,
        graph: &Graph,
        point: Pos2,
        exclude: NodeId,
    ) -> Option<NodeId> {
        graph
            .nodes
            .values()
            .filter(|node| {
                node.id != exclude
                    && self.media_filter.matches_node(graph, node.id)
                    && self.search_matches_node(graph, node.id)
            })
            .find(|node| self.node_rect(rect, graph, node).contains(point))
            .map(|node| node.id)
    }

    /// Pairs `source`'s outputs with `target`'s inputs by channel metadata and
    /// compatible port type. Used for the whole-node Easy-mode connect drag,
    /// which operates on every raw port regardless of visual grouping.
    pub(crate) fn matching_port_pairs(
        &self,
        graph: &Graph,
        source: &Node,
        target: &Node,
    ) -> Vec<(PortId, PortId)> {
        let outputs: Vec<&Port> = self
            .ordered_ports(graph, source)
            .into_iter()
            .filter(|port| port.direction == Direction::Source)
            .collect();
        let inputs: Vec<&Port> = self
            .ordered_ports(graph, target)
            .into_iter()
            .filter(|port| port.direction == Direction::Sink)
            .collect();
        pair_ports(&outputs, &inputs)
    }
}
