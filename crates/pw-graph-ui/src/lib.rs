//! egui canvas primitives. Backend mutations are returned as actions so the UI
//! never owns the driver or command stack.

use egui::{pos2, vec2, Color32, FontId, Id, Pos2, Rect, Sense, Stroke, Ui, Vec2};
use pw_graph_core::{Direction, Graph, Node, NodeId, Port, PortId, PortType};
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CanvasAction {
    Connect { output: PortId, input: PortId },
    Disconnect { link: pw_graph_core::LinkId },
}

pub struct GraphCanvas {
    pub zoom: f32,
    pub pan: Vec2,
    pending_output: Option<PortId>,
    pub selected_node: Option<NodeId>,
}

impl Default for GraphCanvas {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            pan: vec2(24.0, 24.0),
            pending_output: None,
            selected_node: None,
        }
    }
}

impl GraphCanvas {
    pub fn show(&mut self, ui: &mut Ui, graph: &Graph) -> Vec<CanvasAction> {
        let rect = ui.available_rect_before_wrap();
        let canvas_response = ui.allocate_rect(rect, Sense::drag());
        let painter = ui.painter_at(rect);
        let mut actions = Vec::new();
        let mut anchors = HashMap::new();

        painter.rect_filled(rect, 0.0, Color32::from_rgb(25, 28, 34));
        self.draw_grid(&painter, rect);

        if canvas_response.dragged() {
            self.pan += canvas_response.drag_delta();
        }
        let scroll = ui.input(|input| input.raw_scroll_delta.y);
        if scroll.abs() > f32::EPSILON
            && rect.contains(ui.input(|input| input.pointer.hover_pos().unwrap_or(rect.center())))
        {
            self.zoom = (self.zoom * (1.0 + scroll * 0.001)).clamp(0.35, 2.5);
        }

        for link in graph.links.values() {
            if let (Some(source), Some(destination)) = (
                graph.ports.get(&link.output_port),
                graph.ports.get(&link.input_port),
            ) {
                if let (Some(source_pos), Some(destination_pos)) = (
                    self.port_anchor(rect, graph, source),
                    self.port_anchor(rect, graph, destination),
                ) {
                    painter.line_segment(
                        [source_pos, destination_pos],
                        Stroke::new(2.0_f32, Color32::from_rgb(115, 133, 154)),
                    );
                }
            }
        }

        for node in graph.nodes.values() {
            self.draw_node(
                ui,
                painter.clone(),
                rect,
                graph,
                node,
                &mut anchors,
                &mut actions,
            );
        }

        if let Some(output_id) = self.pending_output {
            if let Some(output) = graph.ports.get(&output_id) {
                if let Some(start) = anchors.get(&output_id).copied() {
                    let end = ui.input(|input| input.pointer.hover_pos()).unwrap_or(start);
                    painter.line_segment([start, end], Stroke::new(2.0_f32, Color32::LIGHT_GREEN));
                    painter.text(
                        start + vec2(8.0, -22.0),
                        egui::Align2::LEFT_TOP,
                        format!("Connect {} → …", output.name),
                        FontId::proportional(12.0),
                        Color32::LIGHT_GREEN,
                    );
                }
            }
        }

        actions
    }

    fn draw_grid(&self, painter: &egui::Painter, rect: Rect) {
        let spacing = 32.0 * self.zoom;
        let origin = rect.left_top() + self.pan;
        let color = Color32::from_rgb(38, 43, 51);
        let mut x = origin.x.rem_euclid(spacing) + rect.left();
        while x < rect.right() {
            painter.line_segment(
                [pos2(x, rect.top()), pos2(x, rect.bottom())],
                Stroke::new(1.0_f32, color),
            );
            x += spacing;
        }
        let mut y = origin.y.rem_euclid(spacing) + rect.top();
        while y < rect.bottom() {
            painter.line_segment(
                [pos2(rect.left(), y), pos2(rect.right(), y)],
                Stroke::new(1.0_f32, color),
            );
            y += spacing;
        }
    }

    fn draw_node(
        &mut self,
        ui: &mut Ui,
        painter: egui::Painter,
        rect: Rect,
        graph: &Graph,
        node: &Node,
        anchors: &mut HashMap<PortId, Pos2>,
        actions: &mut Vec<CanvasAction>,
    ) {
        let ports: Vec<&Port> = node
            .ports
            .iter()
            .filter_map(|id| graph.ports.get(id))
            .collect();
        let width = 220.0 * self.zoom;
        let height = (32.0 + ports.len() as f32 * 26.0).max(58.0) * self.zoom;
        let top_left = rect.left_top()
            + self.pan
            + vec2(node.position[0] * self.zoom, node.position[1] * self.zoom);
        let node_rect = Rect::from_min_size(top_left, vec2(width, height));
        let response = ui.interact(node_rect, Id::new(("node", node.id)), Sense::click());
        if response.clicked() {
            self.selected_node = Some(node.id);
        }
        let fill = if self.selected_node == Some(node.id) {
            Color32::from_rgb(55, 72, 91)
        } else {
            Color32::from_rgb(42, 48, 58)
        };
        painter.rect(
            node_rect,
            6.0,
            fill,
            Stroke::new(1.0_f32, Color32::from_rgb(93, 108, 127)),
        );
        let header = Rect::from_min_max(
            node_rect.min,
            pos2(node_rect.max.x, node_rect.min.y + 30.0 * self.zoom),
        );
        painter.rect_filled(header, 6.0, Color32::from_rgb(51, 59, 70));
        painter.text(
            header.left_center() + vec2(10.0, 0.0),
            egui::Align2::LEFT_CENTER,
            &node.name,
            FontId::proportional(14.0 * self.zoom),
            Color32::WHITE,
        );

        for (index, port) in ports.into_iter().enumerate() {
            let y = node_rect.top() + (43.0 + index as f32 * 26.0) * self.zoom;
            let x = if port.direction == Direction::Source {
                node_rect.right() - 12.0 * self.zoom
            } else {
                node_rect.left() + 12.0 * self.zoom
            };
            let anchor = pos2(x, y);
            anchors.insert(port.id, anchor);
            let hit_rect = Rect::from_center_size(anchor, vec2(22.0, 22.0) * self.zoom.max(0.7));
            let response = ui.interact(
                hit_rect,
                Id::new(("port", port.id)),
                Sense::click_and_drag(),
            );
            painter.circle_filled(anchor, 6.0 * self.zoom.max(0.7), port_color(port.port_type));
            let text_pos = if port.direction == Direction::Source {
                anchor - vec2(10.0, 0.0)
            } else {
                anchor + vec2(10.0, 0.0)
            };
            painter.text(
                text_pos,
                if port.direction == Direction::Source {
                    egui::Align2::RIGHT_CENTER
                } else {
                    egui::Align2::LEFT_CENTER
                },
                &port.name,
                FontId::proportional(12.0 * self.zoom),
                Color32::from_rgb(215, 220, 227),
            );

            if port.direction == Direction::Source
                && (response.clicked() || response.drag_started())
            {
                self.pending_output = Some(port.id);
            }
            if port.direction == Direction::Sink && (response.clicked() || response.drag_stopped())
            {
                if let Some(output) = self.pending_output.take() {
                    actions.push(CanvasAction::Connect {
                        output,
                        input: port.id,
                    });
                }
            }
        }
    }

    fn port_anchor(&self, rect: Rect, graph: &Graph, port: &Port) -> Option<Pos2> {
        let node = graph.nodes.get(&port.node_id)?;
        let index = node.ports.iter().position(|id| *id == port.id)?;
        let width = 220.0 * self.zoom;
        let top_left = rect.left_top()
            + self.pan
            + vec2(node.position[0] * self.zoom, node.position[1] * self.zoom);
        let node_rect = Rect::from_min_size(
            top_left,
            vec2(
                width,
                (32.0 + node.ports.len() as f32 * 26.0).max(58.0) * self.zoom,
            ),
        );
        let x = if port.direction == Direction::Source {
            node_rect.right() - 12.0 * self.zoom
        } else {
            node_rect.left() + 12.0 * self.zoom
        };
        Some(pos2(
            x,
            node_rect.top() + (43.0 + index as f32 * 26.0) * self.zoom,
        ))
    }
}

fn port_color(port_type: PortType) -> Color32 {
    match port_type {
        PortType::Audio => Color32::from_rgb(87, 199, 133),
        PortType::Video => Color32::from_rgb(78, 157, 230),
        PortType::MidiJack => Color32::from_rgb(227, 93, 106),
        PortType::MidiAlsa => Color32::from_rgb(169, 121, 209),
        PortType::Unknown => Color32::from_rgb(165, 165, 165),
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
    }
}
