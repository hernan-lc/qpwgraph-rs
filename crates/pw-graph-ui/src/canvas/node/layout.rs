use crate::{GraphCanvas, NodeId, PortId};
use egui::{pos2, vec2, Pos2, Rect, Vec2};
use pw_graph_core::{Direction, Graph, Node, Port, PortType};

use super::super::ports::{grouped_rows, pair_ports};
use super::{
    AUDIO_CONTROLS_HEIGHT, COLLAPSED_NODE_HEIGHT, EFFECT_CONTROLS_MIN_HEIGHT,
    EFFECT_CONTROLS_VERTICAL_PADDING, EFFECT_CONTROL_MIN_SCREEN_ROW_HEIGHT,
    EFFECT_CONTROL_ROW_HEIGHT, NODE_HEADER_HEIGHT, NODE_WIDTH, PORT_ROW_HEIGHT,
};

impl GraphCanvas {
    /// Height, in scene units, reserved for the inline controls of an effect
    /// node. Every parameter gets its own row so the following port rows can
    /// never be painted on top of an effect control.
    ///
    /// At low canvas zoom, keep each row at least one usable widget-height on
    /// screen. The rest of the node continues to scale normally, but the
    /// sliders and checkboxes remain clickable instead of being clipped by
    /// the port area.
    pub(super) fn effect_controls_height(&self, node: &Node) -> f32 {
        if node.node_type != pw_graph_core::NodeType::Effect {
            return 0.0;
        }
        let Some(control) = self.effect_controls.get(&node.id) else {
            return 0.0;
        };

        let row_height = EFFECT_CONTROL_ROW_HEIGHT
            .max(EFFECT_CONTROL_MIN_SCREEN_ROW_HEIGHT / self.zoom.max(f32::EPSILON));
        let row_count = control.parameters.len().saturating_add(1) as f32;
        (EFFECT_CONTROLS_VERTICAL_PADDING + row_count * row_height).max(EFFECT_CONTROLS_MIN_HEIGHT)
    }

    /// Total vertical space occupied by the inline controls for a visible,
    /// expanded node. Keeping this in one place ensures the card size, link
    /// anchors, and painted port rows agree about where ports start.
    pub(super) fn node_controls_height(&self, node: &Node, has_audio: bool) -> f32 {
        let audio_controls_height =
            if has_audio && node.node_type != pw_graph_core::NodeType::Effect {
                AUDIO_CONTROLS_HEIGHT
            } else {
                0.0
            };
        audio_controls_height + self.effect_controls_height(node)
    }

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
            let controls_height = self.node_controls_height(
                node,
                ports.iter().any(|port| port.port_type == PortType::Audio),
            );
            (NODE_HEADER_HEIGHT + controls_height + 14.0 + port_count as f32 * PORT_ROW_HEIGHT)
                .max(COLLAPSED_NODE_HEIGHT)
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
        let controls_height = self.node_controls_height(
            node,
            ordered.iter().any(|item| item.port_type == PortType::Audio),
        );
        Some(pos2(
            x,
            node_rect.top()
                + (NODE_HEADER_HEIGHT + controls_height + 13.0 + index as f32 * PORT_ROW_HEIGHT)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EffectNodeControl, EffectNodeParameter};
    use egui::{pos2, vec2, Rect, Vec2};
    use pw_graph_core::{Direction, NodeType};

    fn effect_graph() -> (Graph, NodeId, PortId) {
        let node_id = NodeId(1);
        let port_id = PortId(10);
        let mut graph = Graph::default();
        graph
            .add_node(Node::new(node_id, "Effect", NodeType::Effect))
            .unwrap();
        graph
            .add_port(Port::new(
                port_id,
                node_id,
                "output",
                Direction::Source,
                PortType::Audio,
            ))
            .unwrap();
        (graph, node_id, port_id)
    }

    fn effect_control(parameter_count: usize) -> EffectNodeControl {
        EffectNodeControl {
            enabled: true,
            parameters: (0..parameter_count)
                .map(|index| EffectNodeParameter {
                    id: format!("parameter-{index}"),
                    name: format!("Parameter {index}"),
                    minimum: 0.0,
                    maximum: 1.0,
                    value: 0.5,
                    unit: String::new(),
                    boolean: false,
                })
                .collect(),
        }
    }

    #[test]
    fn effect_parameters_expand_the_node_and_push_ports_below_controls() {
        let (graph, node_id, port_id) = effect_graph();
        let node = graph.node(node_id).unwrap();
        let mut canvas = GraphCanvas {
            pan: Vec2::ZERO,
            ..GraphCanvas::default()
        };

        canvas.effect_controls.insert(node_id, effect_control(1));
        let one_parameter_height = canvas.effect_controls_height(node);
        canvas.effect_controls.insert(node_id, effect_control(7));
        let controls_height = canvas.effect_controls_height(node);

        assert_eq!(
            controls_height - one_parameter_height,
            6.0 * EFFECT_CONTROL_ROW_HEIGHT
        );
        // Effect nodes have audio ports but do not render the generic audio
        // controls, so only the effect panel contributes to their offset.
        assert_eq!(canvas.node_controls_height(node, true), controls_height);

        let scene_size = canvas.node_scene_size(&graph, node);
        assert_eq!(
            scene_size.y,
            NODE_HEADER_HEIGHT + controls_height + 14.0 + PORT_ROW_HEIGHT
        );

        let canvas_rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(1200.0, 800.0));
        let anchor = canvas
            .port_anchor(canvas_rect, &graph, graph.port(port_id).unwrap())
            .unwrap();
        let panel_bottom = NODE_HEADER_HEIGHT + controls_height;

        // The first port row begins three scene pixels after the effect panel
        // (its visual top is anchor - 10), keeping controls and ports apart.
        assert_eq!(anchor.y, panel_bottom + 13.0);
        assert!(anchor.y - 10.0 > panel_bottom);
    }
}
