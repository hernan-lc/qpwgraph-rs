//! egui canvas primitives. Backend mutations are returned as actions so the UI
//! never owns the driver or command stack.

use egui::{pos2, vec2, Color32, FontId, Pos2, Rect, Sense, Stroke, Ui, Vec2};
use pw_graph_core::{Direction, Graph, LinkId, Node, NodeId, Port, PortId, PortType};
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[derive(Clone, Debug, PartialEq)]
pub enum CanvasAction {
    Connect { output: PortId, input: PortId },
    Disconnect { link: LinkId },
    MoveNode { node: NodeId, position: [f32; 2] },
}

pub struct GraphCanvas {
    pub zoom: f32,
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

impl GraphCanvas {
    pub fn show(&mut self, ui: &mut Ui, graph: &Graph, connect_hint: &str) -> Vec<CanvasAction> {
        let rect = ui.available_rect_before_wrap();
        let canvas_response = ui.allocate_rect(rect, Sense::drag());
        let painter = ui.painter_at(rect);
        let mut actions = Vec::new();
        let mut anchors = HashMap::new();

        if self.repel_overlapping_nodes
            && self.dragging_node.is_none()
            && self.selection_start.is_none()
            && !self.thumbnail_mode
        {
            self.repel_overlaps(rect, graph, &mut actions);
        }

        painter.rect_filled(rect, 0.0, Color32::from_rgb(25, 28, 34));
        self.draw_grid(&painter, rect);

        if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.pending_output = None;
            self.selection_start = None;
            self.selection_current = None;
            self.selected_link = None;
        }
        if ui.input(|input| input.key_pressed(egui::Key::Delete)) {
            if let Some(link) = self.selected_link.take() {
                actions.push(CanvasAction::Disconnect { link });
            }
        }

        if canvas_response.drag_started() && self.dragging_node.is_none() {
            self.selection_start = ui.input(|input| input.pointer.interact_pos());
            self.selection_current = self.selection_start;
        }
        if self.selection_start.is_some() && canvas_response.dragged() {
            self.selection_current = ui.input(|input| input.pointer.interact_pos());
        }
        if canvas_response.drag_stopped() {
            if let (Some(start), Some(end)) = (self.selection_start, self.selection_current) {
                let selection = Rect::from_two_pos(start, end);
                self.selected_nodes = graph
                    .nodes
                    .values()
                    .filter(|node| self.node_rect(rect, graph, node).intersects(selection))
                    .map(|node| node.id)
                    .collect();
                self.selected_node = self.selected_nodes.iter().next().copied();
            }
            self.selection_start = None;
            self.selection_current = None;
        }
        if canvas_response.dragged()
            && self.dragging_node.is_none()
            && self.selection_start.is_none()
        {
            self.pan += canvas_response.drag_delta();
        }

        let scroll = ui.input(|input| input.raw_scroll_delta.y);
        if scroll.abs() > f32::EPSILON
            && rect.contains(ui.input(|input| input.pointer.hover_pos().unwrap_or(rect.center())))
        {
            self.zoom = (self.zoom * (1.0 + scroll * 0.001)).clamp(0.35, 2.5);
        }

        for (link_index, link) in graph.links.values().enumerate() {
            if self.thumbnail_mode {
                break;
            }
            if let (Some(source), Some(destination)) = (
                graph.ports.get(&link.output_port),
                graph.ports.get(&link.input_port),
            ) {
                if let (Some(source_pos), Some(destination_pos)) = (
                    self.port_anchor(rect, graph, source),
                    self.port_anchor(rect, graph, destination),
                ) {
                    let selected = self.selected_link == Some(link.id);
                    painter.line_segment(
                        [source_pos, destination_pos],
                        Stroke::new(
                            if selected { 3.0_f32 } else { 2.0_f32 },
                            if selected {
                                Color32::LIGHT_GREEN
                            } else {
                                Color32::from_rgb(115, 133, 154)
                            },
                        ),
                    );
                    let hit_rect = Rect::from_two_pos(source_pos, destination_pos).expand(8.0);
                    let link_widget_id = ui.id().with((
                        "graph-link",
                        link_index,
                        link.id,
                        link.output_port,
                        link.input_port,
                    ));
                    let response = ui.interact(hit_rect, link_widget_id, Sense::click());
                    if response.clicked() {
                        self.selected_link = Some(link.id);
                        self.selected_nodes.clear();
                        self.selected_node = None;
                    }
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

        if let (Some(start), Some(end)) = (self.selection_start, self.selection_current) {
            let selection = Rect::from_two_pos(start, end);
            painter.rect_filled(
                selection,
                0.0,
                Color32::from_rgba_unmultiplied(80, 130, 190, 40),
            );
            painter.rect_stroke(selection, 0.0, Stroke::new(1.0_f32, Color32::LIGHT_BLUE));
        }

        if let Some(output_id) = self.pending_output {
            if let Some(output) = graph.ports.get(&output_id) {
                if let Some(start) = anchors.get(&output_id).copied() {
                    let end = ui.input(|input| input.pointer.hover_pos()).unwrap_or(start);
                    painter.line_segment([start, end], Stroke::new(2.0_f32, Color32::LIGHT_GREEN));
                    painter.text(
                        start + vec2(8.0, -22.0),
                        egui::Align2::LEFT_TOP,
                        format!("{connect_hint} {} → …", output.name),
                        FontId::proportional(12.0),
                        Color32::LIGHT_GREEN,
                    );
                }
            }
        }

        actions
    }

    fn repel_overlaps(&self, rect: Rect, graph: &Graph, actions: &mut Vec<CanvasAction>) {
        let mut occupied = Vec::new();
        for node in graph.nodes.values() {
            let mut candidate = self.node_rect(rect, graph, node);
            let mut position = node.position;
            while occupied
                .iter()
                .any(|other: &Rect| other.intersects(candidate))
            {
                position[0] += 240.0;
                candidate = candidate.translate(vec2(240.0 * self.zoom, 0.0));
            }
            if position != node.position {
                actions.push(CanvasAction::MoveNode {
                    node: node.id,
                    position,
                });
            }
            occupied.push(candidate);
        }
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
        let ports = self.ordered_ports(graph, node);
        let node_rect = self.node_rect(rect, graph, node);
        let response = ui.interact(
            node_rect,
            ui.id().with(("graph-node", node.id)),
            Sense::click_and_drag(),
        );
        if response.clicked() {
            let shift = ui.input(|input| input.modifiers.shift);
            if !shift {
                self.selected_nodes.clear();
            }
            if !self.selected_nodes.insert(node.id) && shift {
                self.selected_nodes.remove(&node.id);
            }
            self.selected_node = self.selected_nodes.iter().next().copied();
            self.selected_link = None;

            if self.connect_through_nodes {
                if let Some(output_id) = self.pending_output {
                    let compatible_input = self
                        .ordered_ports(graph, node)
                        .into_iter()
                        .find(|port| {
                            port.direction == Direction::Sink
                                && graph.port(output_id).is_some_and(|output| {
                                    output.port_type == port.port_type
                                        || output.port_type == PortType::Unknown
                                        || port.port_type == PortType::Unknown
                                })
                        })
                        .map(|port| port.id);
                    if let Some(input) = compatible_input {
                        self.pending_output = None;
                        actions.push(CanvasAction::Connect {
                            output: output_id,
                            input,
                        });
                    }
                }
            }
        }
        if response.drag_started() {
            if !self.selected_nodes.contains(&node.id) {
                self.selected_nodes.clear();
                self.selected_nodes.insert(node.id);
                self.selected_node = Some(node.id);
            }
            self.dragging_node = Some(node.id);
            self.dragging_origin = self
                .selected_nodes
                .iter()
                .filter_map(|id| graph.node(*id).map(|item| (*id, item.position)))
                .collect();
        }
        if response.dragged() && self.dragging_node == Some(node.id) {
            self.drag_delta = response.drag_delta();
            let delta = response.drag_delta() / self.zoom;
            for (selected_id, origin) in &self.dragging_origin {
                actions.push(CanvasAction::MoveNode {
                    node: *selected_id,
                    position: [origin[0] + delta.x, origin[1] + delta.y],
                });
            }
        }
        if response.drag_stopped() && self.dragging_node == Some(node.id) {
            self.dragging_node = None;
            self.dragging_origin.clear();
            self.drag_delta = Vec2::ZERO;
        }

        let selected = self.selected_nodes.contains(&node.id);
        let fill = if selected {
            Color32::from_rgb(55, 72, 91)
        } else {
            Color32::from_rgb(42, 48, 58)
        };
        painter.rect(
            node_rect,
            6.0,
            fill,
            Stroke::new(
                if selected { 2.0_f32 } else { 1.0_f32 },
                if selected {
                    Color32::LIGHT_BLUE
                } else {
                    Color32::from_rgb(93, 108, 127)
                },
            ),
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

        if self.thumbnail_mode {
            return;
        }

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
                ui.id().with(("graph-port", node.id, index, port.id)),
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

    fn ordered_ports<'a>(&self, graph: &'a Graph, node: &Node) -> Vec<&'a Port> {
        let mut ports: Vec<&Port> = node
            .ports
            .iter()
            .filter_map(|id| graph.ports.get(id))
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

    fn node_rect(&self, rect: Rect, graph: &Graph, node: &Node) -> Rect {
        let port_count = if self.thumbnail_mode {
            0
        } else {
            self.ordered_ports(graph, node).len()
        };
        let width = 220.0 * self.zoom;
        let height = (32.0 + port_count as f32 * 26.0).max(58.0) * self.zoom;
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
        Rect::from_min_size(top_left, vec2(width, height))
    }

    fn port_anchor(&self, rect: Rect, graph: &Graph, port: &Port) -> Option<Pos2> {
        let node = graph.nodes.get(&port.node_id)?;
        let index = self
            .ordered_ports(graph, node)
            .iter()
            .position(|item| item.id == port.id)?;
        let node_rect = self.node_rect(rect, graph, node);
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
        assert!(canvas.selected_nodes.is_empty());
    }
}
