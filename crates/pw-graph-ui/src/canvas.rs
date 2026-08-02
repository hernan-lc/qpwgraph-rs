//! Graph rendering and interaction implementation.

use super::*;
use egui::{
    pos2, vec2, Color32, FontId, Pos2, ProgressBar, Rect, RichText, Sense, Shape, Stroke, Ui, Vec2,
};
use pw_graph_core::{Direction, Graph, Node, NodeType, Port, PortType};
use pw_graph_i18n::I18n;
use std::cell::Cell;
use std::collections::HashMap;

const NODE_WIDTH: f32 = 244.0;
const NODE_HEADER_HEIGHT: f32 = 34.0;
const PORT_ROW_HEIGHT: f32 = 25.0;
const EDGE_HIT_DISTANCE: f32 = 9.0;

impl GraphCanvas {
    pub fn show(&mut self, ui: &mut Ui, graph: &Graph, i18n: &I18n) -> Vec<CanvasAction> {
        self.update_peak_holds();
        let rect = ui.available_rect_before_wrap();
        let canvas_response = ui.allocate_rect(rect, Sense::drag());
        let painter = ui.painter_at(rect);
        let mut actions = Vec::new();
        let mut anchors = HashMap::new();
        let pointer_pos = ui.input(|input| input.pointer.interact_pos());
        let pointer_over_node = pointer_pos.is_some_and(|pointer| {
            graph
                .nodes
                .values()
                .any(|node| self.node_rect(rect, graph, node).contains(pointer))
        });

        if self.repel_overlapping_nodes
            && self.dragging_node.is_none()
            && self.selection_start.is_none()
            && !pointer_over_node
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

        if canvas_response.drag_started() && self.dragging_node.is_none() && !pointer_over_node {
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
            && !pointer_over_node
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
                    let points = bezier_points(
                        source_pos,
                        destination_pos,
                        (link_index as i32 % 5 - 2) as f32 * 5.0,
                    );
                    let hovered = pointer_pos.is_some_and(|pointer| {
                        point_near_polyline(pointer, &points, EDGE_HIT_DISTANCE)
                    });
                    let color = if selected {
                        Color32::LIGHT_GREEN
                    } else if hovered {
                        Color32::WHITE
                    } else {
                        edge_color(source.port_type)
                    };
                    painter.add(Shape::line(
                        points.clone(),
                        Stroke::new(if selected { 5.0_f32 } else { 3.5_f32 }, Color32::BLACK),
                    ));
                    painter.add(Shape::line(
                        points.clone(),
                        Stroke::new(if selected { 2.8_f32 } else { 1.6_f32 }, color),
                    ));
                    let hit_rect = points_bounds(&points).expand(EDGE_HIT_DISTANCE);
                    let link_widget_id = ui.id().with((
                        "graph-link",
                        link_index,
                        link.id,
                        link.output_port,
                        link.input_port,
                    ));
                    let mut response = ui.interact(hit_rect, link_widget_id, Sense::hover());
                    if hovered {
                        response =
                            response.on_hover_text(link_tooltip(graph, source, destination, i18n));
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    let clicked = hovered
                        && !pointer_over_node
                        && ui.input(|input| input.pointer.primary_clicked());
                    if clicked {
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
                i18n,
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
                    let points = bezier_points(start, end, 0.0);
                    painter.add(Shape::line(
                        points.clone(),
                        Stroke::new(4.0_f32, Color32::BLACK),
                    ));
                    painter.add(Shape::line(
                        points,
                        Stroke::new(2.0_f32, Color32::LIGHT_GREEN),
                    ));
                    painter.text(
                        start + vec2(8.0, -22.0),
                        egui::Align2::LEFT_TOP,
                        i18n.format(
                            "canvas.pending_connection",
                            &[
                                ("action", i18n.text("canvas.connect_hint")),
                                ("port", display_port_name(&output.name, i18n)),
                            ],
                        ),
                        FontId::proportional(
                            12.0 * self.zoom * self.node_text_scale.clamp(0.80, 2.0),
                        ),
                        Color32::LIGHT_GREEN,
                    );
                }
            }
        }

        actions
    }

    pub fn meter_peak_hold(&self, node_id: NodeId, fallback: f32) -> f32 {
        self.peak_hold.get(&node_id).copied().unwrap_or(fallback)
    }

    fn update_peak_holds(&mut self) {
        self.peak_hold
            .retain(|node_id, _| self.meters.contains_key(node_id));
        for (node_id, reading) in &self.meters {
            let hold = self.peak_hold.entry(*node_id).or_insert(0.0);
            *hold = (*hold - 0.012).max(reading.peak).clamp(0.0, 1.0);
        }
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
        i18n: &I18n,
        anchors: &mut HashMap<PortId, Pos2>,
        actions: &mut Vec<CanvasAction>,
    ) {
        let ports = self.ordered_ports(graph, node);
        let node_rect = self.node_rect(rect, graph, node);
        let header = Rect::from_min_max(
            node_rect.min,
            pos2(
                node_rect.max.x,
                node_rect.min.y + NODE_HEADER_HEIGHT * self.zoom,
            ),
        );
        let tooltip = node_tooltip(node, &ports, i18n);
        let body_response = ui
            .interact(
                node_rect,
                ui.id().with(("graph-node-body", node.id)),
                Sense::click(),
            )
            .on_hover_text(tooltip.clone());
        let header_response = ui
            .interact(
                header,
                ui.id().with(("graph-node-header", node.id)),
                Sense::click_and_drag(),
            )
            .on_hover_text(format!("{tooltip}\n\n{}", i18n.text("canvas.drag_header")));
        if body_response.clicked() || header_response.clicked() {
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
        if header_response.drag_started() {
            if !self.selected_nodes.contains(&node.id) {
                self.selected_nodes.clear();
                self.selected_nodes.insert(node.id);
                self.selected_node = Some(node.id);
            }
            self.dragging_node = Some(node.id);
            self.drag_delta = Vec2::ZERO;
            self.dragging_origin = self
                .selected_nodes
                .iter()
                .filter_map(|id| graph.node(*id).map(|item| (*id, item.position)))
                .collect();
        }
        if header_response.dragged() && self.dragging_node == Some(node.id) {
            // egui reports the movement since the previous frame. Accumulate it
            // so the node follows the pointer for the whole drag gesture.
            self.drag_delta += header_response.drag_delta();
            let delta = self.drag_delta / self.zoom;
            for (selected_id, origin) in &self.dragging_origin {
                actions.push(CanvasAction::MoveNode {
                    node: *selected_id,
                    position: [origin[0] + delta.x, origin[1] + delta.y],
                });
            }
        }
        if header_response.drag_stopped() && self.dragging_node == Some(node.id) {
            self.dragging_node = None;
            self.dragging_origin.clear();
            self.drag_delta = Vec2::ZERO;
        }
        if header_response.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        } else if header_response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }

        let selected = self.selected_nodes.contains(&node.id);
        let text_scale = self.node_text_scale.clamp(0.80, 2.0);
        let accent = dominant_port_type(&ports)
            .map(port_color)
            .unwrap_or_else(|| node_color(node.node_type));
        let fill = if selected {
            Color32::from_rgb(48, 60, 76)
        } else {
            Color32::from_rgb(38, 45, 56)
        };
        painter.rect(
            node_rect,
            8.0,
            fill,
            Stroke::new(
                if selected { 2.0_f32 } else { 1.0_f32 },
                if selected {
                    accent
                } else {
                    Color32::from_rgb(86, 103, 125)
                },
            ),
        );
        painter.rect_filled(header, 8.0, Color32::from_rgb(48, 58, 72));
        painter.rect_filled(
            Rect::from_min_max(
                header.min,
                pos2(header.left() + 4.0 * self.zoom, header.bottom()),
            ),
            8.0,
            accent,
        );
        let inputs = ports
            .iter()
            .filter(|port| port.direction == Direction::Sink)
            .count();
        let outputs = ports
            .iter()
            .filter(|port| port.direction == Direction::Source)
            .count();
        painter.text(
            header.left_center() + vec2(12.0 * self.zoom, 0.0),
            egui::Align2::LEFT_CENTER,
            compact_label(&display_node_name(&node.name, i18n), 22),
            FontId::proportional(13.0 * self.zoom * text_scale),
            Color32::WHITE,
        );
        if !ports.is_empty() {
            painter.text(
                pos2(header.right() - 28.0 * self.zoom, header.center().y),
                egui::Align2::RIGHT_CENTER,
                format!("{inputs}/{outputs}"),
                FontId::proportional(10.0 * self.zoom * text_scale),
                Color32::from_rgb(178, 193, 210),
            );
        }
        paint_drag_grip(
            &painter,
            pos2(header.right() - 10.0 * self.zoom, header.center().y),
            self.zoom,
            Color32::from_rgb(174, 189, 204),
        );

        if self.thumbnail_mode {
            return;
        }

        let mut port_name_totals: HashMap<(Direction, String), usize> = HashMap::new();
        for port in &ports {
            let name = display_port_name(&port.name, i18n);
            *port_name_totals
                .entry((port.direction, name))
                .or_insert(0) += 1;
        }
        let mut port_name_seen: HashMap<(Direction, String), usize> = HashMap::new();

        for (index, port) in ports.into_iter().enumerate() {
            let y = node_rect.top()
                + (NODE_HEADER_HEIGHT + 13.0 + index as f32 * PORT_ROW_HEIGHT) * self.zoom;
            let x = if port.direction == Direction::Source {
                node_rect.right() - 12.0 * self.zoom
            } else {
                node_rect.left() + 12.0 * self.zoom
            };
            let anchor = pos2(x, y);
            anchors.insert(port.id, anchor);
            let row_rect = Rect::from_min_max(
                pos2(node_rect.left() + 5.0 * self.zoom, y - 10.0 * self.zoom),
                pos2(node_rect.right() - 5.0 * self.zoom, y + 10.0 * self.zoom),
            );
            let hit_rect = Rect::from_center_size(anchor, vec2(22.0, 22.0) * self.zoom.max(0.7));
            let mut response = ui.interact(
                hit_rect,
                ui.id().with(("graph-port", node.id, index, port.id)),
                Sense::click_and_drag(),
            );
            let pin_requested = Cell::new(false);
            let port_help = port_tooltip(node, port, i18n);
            if port.port_type == PortType::Audio {
                let meter = self.meters.get(&node.id).copied();
                response = response.on_hover_ui(|ui| {
                    ui.label(RichText::new(port_help).strong());
                    ui.separator();
                    ui.label(RichText::new(i18n.text("canvas.audio_meter_title")).strong());
                    match meter {
                        Some(reading) if reading.available => {
                            let stale = reading.age_ms > 750;
                            let state = if stale {
                                i18n.text("canvas.audio_meter_stale")
                            } else {
                                i18n.text("canvas.audio_meter_live")
                            };
                            ui.label(RichText::new(state).weak());
                            ui.add(
                                ProgressBar::new(reading.rms.clamp(0.0, 1.0))
                                    .desired_width(190.0)
                                    .text(format!(
                                        "{}  {:.1} dB",
                                        i18n.text("canvas.audio_meter_rms"),
                                        level_db(reading.rms)
                                    )),
                            );
                            ui.add(
                                ProgressBar::new(reading.peak.clamp(0.0, 1.0))
                                    .desired_width(190.0)
                                    .text(format!(
                                        "{}  {:.1} dB",
                                        i18n.text("canvas.audio_meter_peak_hold"),
                                        level_db(self.meter_peak_hold(node.id, reading.peak))
                                    )),
                            );
                            ui.label(
                                RichText::new(i18n.format(
                                    "canvas.audio_meter_age",
                                    &[("age", reading.age_ms.to_string())],
                                ))
                                .small()
                                .weak(),
                            );
                        }
                        _ => {
                            ui.label(
                                RichText::new(i18n.text("canvas.audio_meter_unavailable")).weak(),
                            );
                        }
                    }
                    if ui
                        .button(if self.pinned_meter == Some(port.id) {
                            i18n.text("canvas.audio_meter_pinned")
                        } else {
                            i18n.text("canvas.audio_meter_pin")
                        })
                        .clicked()
                    {
                        pin_requested.set(true);
                    }
                });
            } else {
                response = response.on_hover_text(port_help);
            }
            if pin_requested.get() {
                self.pinned_meter = Some(port.id);
            }
            let pending = self.pending_output == Some(port.id);
            if response.hovered() || pending {
                painter.rect_filled(
                    row_rect,
                    4.0,
                    Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 42),
                );
            }
            let radius = 6.0 * self.zoom.max(0.7);
            let dot_color = port_color(port.port_type);
            painter.circle_stroke(
                anchor,
                radius + 0.6,
                Stroke::new(1.2_f32, Color32::from_black_alpha(170)),
            );
            painter.circle_filled(anchor, radius, dot_color);
            if response.hovered() || pending {
                painter.circle_stroke(anchor, radius + 3.0, Stroke::new(1.5_f32, Color32::WHITE));
            }
            let text_pos = if port.direction == Direction::Source {
                anchor - vec2(10.0, 0.0)
            } else {
                anchor + vec2(10.0, 0.0)
            };
            let base_name = display_port_name(&port.name, i18n);
            let name_key = (port.direction, base_name.clone());
            let label = if port_name_totals.get(&name_key).copied().unwrap_or(1) > 1 {
                let seen = port_name_seen.entry(name_key).or_insert(0);
                *seen += 1;
                format!("{base_name} #{seen}")
            } else {
                base_name
            };
            painter.text(
                text_pos,
                if port.direction == Direction::Source {
                    egui::Align2::RIGHT_CENTER
                } else {
                    egui::Align2::LEFT_CENTER
                },
                compact_label(&label, 25),
                FontId::proportional(11.5 * self.zoom * text_scale),
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
        let width = NODE_WIDTH * self.zoom;
        let height = if self.thumbnail_mode {
            62.0
        } else {
            (NODE_HEADER_HEIGHT + 14.0 + port_count as f32 * PORT_ROW_HEIGHT).max(62.0)
        } * self.zoom;
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
            node_rect.top()
                + (NODE_HEADER_HEIGHT + 13.0 + index as f32 * PORT_ROW_HEIGHT) * self.zoom,
        ))
    }
}

fn level_db(value: f32) -> f32 {
    (20.0 * value.max(0.000001).log10()).clamp(-120.0, 0.0)
}

fn dominant_port_type(ports: &[&Port]) -> Option<PortType> {
    let mut counts: HashMap<PortType, usize> = HashMap::new();
    for port in ports {
        if port.port_type != PortType::Unknown {
            *counts.entry(port.port_type).or_insert(0) += 1;
        }
    }
    counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(port_type, _)| port_type)
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

fn edge_color(port_type: PortType) -> Color32 {
    match port_type {
        PortType::Audio => Color32::from_rgb(106, 187, 147),
        PortType::Video => Color32::from_rgb(92, 157, 218),
        PortType::MidiJack => Color32::from_rgb(213, 111, 123),
        PortType::MidiAlsa => Color32::from_rgb(166, 126, 208),
        PortType::Unknown => Color32::from_rgb(138, 151, 169),
    }
}

fn node_color(node_type: NodeType) -> Color32 {
    match node_type {
        NodeType::PipeWire => Color32::from_rgb(91, 172, 224),
        NodeType::AlsaMidi => Color32::from_rgb(180, 128, 220),
        NodeType::Unknown => Color32::from_rgb(153, 163, 175),
    }
}

fn node_tooltip(node: &Node, ports: &[&Port], i18n: &I18n) -> String {
    let inputs = ports
        .iter()
        .filter(|port| port.direction == Direction::Sink)
        .count();
    let outputs = ports
        .iter()
        .filter(|port| port.direction == Direction::Source)
        .count();
    i18n.format(
        "canvas.node_tooltip",
        &[
            ("type", node_type_label(node.node_type, i18n)),
            ("name", node.name.clone()),
            ("inputs", inputs.to_string()),
            ("outputs", outputs.to_string()),
        ],
    )
}

fn link_tooltip(graph: &Graph, source: &Port, destination: &Port, i18n: &I18n) -> String {
    let source_node = graph
        .node(source.node_id)
        .map(|node| node.name.clone())
        .unwrap_or_else(|| i18n.text("canvas.unknown_node"));
    let destination_node = graph
        .node(destination.node_id)
        .map(|node| node.name.clone())
        .unwrap_or_else(|| i18n.text("canvas.unknown_node"));
    i18n.format(
        "canvas.link_tooltip",
        &[
            ("type", port_type_label(source.port_type, i18n)),
            ("source_node", source_node),
            ("source_port", source.name.clone()),
            ("destination_node", destination_node),
            ("destination_port", destination.name.clone()),
        ],
    )
}

fn port_tooltip(node: &Node, port: &Port, i18n: &I18n) -> String {
    let direction = if port.direction == Direction::Source {
        i18n.text("canvas.output")
    } else {
        i18n.text("canvas.input")
    };
    i18n.format(
        "canvas.port_tooltip",
        &[
            ("direction", direction),
            ("type", port_type_label(port.port_type, i18n)),
            ("port", port.name.clone()),
            ("node", node.name.clone()),
            (
                "help",
                if port.direction == Direction::Source {
                    i18n.text("canvas.output_help")
                } else {
                    i18n.text("canvas.input_help")
                },
            ),
        ],
    )
}

fn node_type_label(node_type: NodeType, i18n: &I18n) -> String {
    match node_type {
        NodeType::PipeWire => i18n.text("canvas.node_type_pipewire"),
        NodeType::AlsaMidi => i18n.text("canvas.node_type_alsa_midi"),
        NodeType::Unknown => i18n.text("canvas.node_type_unknown"),
    }
}

fn port_type_label(port_type: PortType, i18n: &I18n) -> String {
    match port_type {
        PortType::Audio => i18n.text("port.audio"),
        PortType::Video => i18n.text("port.video"),
        PortType::MidiJack => i18n.text("port.pw_midi"),
        PortType::MidiAlsa => i18n.text("port.alsa_midi"),
        PortType::Unknown => i18n.text("canvas.unknown"),
    }
}

fn display_node_name(name: &str, i18n: &I18n) -> String {
    let (kind_key, detail) = if let Some(detail) = name.strip_prefix("alsa_input.") {
        ("canvas.node_name_alsa_input", short_device_name(detail))
    } else if let Some(detail) = name.strip_prefix("alsa_output.") {
        ("canvas.node_name_alsa_output", short_device_name(detail))
    } else if let Some(detail) = name.strip_prefix("bluez_input.") {
        ("canvas.node_name_bluetooth_input", bluetooth_suffix(detail))
    } else if let Some(detail) = name.strip_prefix("bluez_output.") {
        (
            "canvas.node_name_bluetooth_output",
            bluetooth_suffix(detail),
        )
    } else if let Some(detail) = name.strip_prefix("bluez_capture_internal.") {
        (
            "canvas.node_name_bluetooth_capture",
            bluetooth_suffix(detail),
        )
    } else if name.starts_with("bluez_midi.") {
        ("canvas.node_name_bluetooth_midi", None)
    } else if name.starts_with("v4l2_input.") {
        ("canvas.node_name_camera_input", None)
    } else if name.starts_with("Midi Through:") {
        ("canvas.node_name_midi_through", None)
    } else {
        return name.replace(['_', '-'], " ");
    };
    let kind = i18n.text(kind_key);
    detail
        .filter(|detail| !detail.is_empty())
        .map(|detail| format!("{kind} - {detail}"))
        .unwrap_or(kind)
}

fn short_device_name(detail: &str) -> Option<String> {
    let device = detail.split("usb-").nth(1).unwrap_or(detail);
    let words: Vec<_> = device
        .split(|character: char| !character.is_ascii_alphabetic())
        .filter(|word| !word.is_empty())
        .take(3)
        .collect();
    (!words.is_empty()).then(|| words.join(" "))
}

fn bluetooth_suffix(detail: &str) -> Option<String> {
    if !detail
        .chars()
        .all(|character| character.is_ascii_hexdigit() || matches!(character, ':' | '_' | '-'))
    {
        return None;
    }
    let digits: String = detail
        .chars()
        .filter(|character| character.is_ascii_hexdigit())
        .collect();
    (digits.len() >= 4).then(|| {
        let suffix = &digits[digits.len() - 4..];
        format!("{}:{}", &suffix[..2], &suffix[2..])
    })
}

fn display_port_name(name: &str, i18n: &I18n) -> String {
    let name = name.rsplit(": ").next().unwrap_or(name);
    let name = name
        .replace("(capture)", &i18n.text("canvas.capture"))
        .replace("(playback)", &i18n.text("canvas.playback"))
        .replace(['_', '-'], " ");
    let mut characters = name.chars();
    let Some(first) = characters.next() else {
        return String::new();
    };
    format!("{}{}", first.to_uppercase(), characters.as_str())
}

fn paint_drag_grip(painter: &egui::Painter, center: Pos2, zoom: f32, color: Color32) {
    let spacing = 3.0 * zoom;
    for column in [-0.5_f32, 0.5] {
        for row in [-1.0_f32, 0.0, 1.0] {
            painter.circle_filled(
                center + vec2(column * spacing, row * spacing),
                0.9 * zoom,
                color,
            );
        }
    }
}

fn bezier_points(start: Pos2, end: Pos2, lane_offset: f32) -> Vec<Pos2> {
    let handle = (end.x - start.x).abs().clamp(72.0, 220.0) * 0.45;
    let control_one = start + vec2(handle, lane_offset);
    let control_two = end - vec2(handle, lane_offset);
    (0..=28)
        .map(|index| {
            let t = index as f32 / 28.0;
            let inverse = 1.0 - t;
            let point = start.to_vec2() * inverse.powi(3)
                + control_one.to_vec2() * (3.0 * inverse.powi(2) * t)
                + control_two.to_vec2() * (3.0 * inverse * t.powi(2))
                + end.to_vec2() * t.powi(3);
            pos2(point.x, point.y)
        })
        .collect()
}

fn points_bounds(points: &[Pos2]) -> Rect {
    let mut min = points[0];
    let mut max = points[0];
    for point in points.iter().skip(1) {
        min.x = min.x.min(point.x);
        min.y = min.y.min(point.y);
        max.x = max.x.max(point.x);
        max.y = max.y.max(point.y);
    }
    Rect::from_min_max(min, max)
}

fn point_near_polyline(point: Pos2, points: &[Pos2], threshold: f32) -> bool {
    let threshold_squared = threshold * threshold;
    points.windows(2).any(|segment| {
        point_to_segment_distance_squared(point, segment[0], segment[1]) <= threshold_squared
    })
}

fn point_to_segment_distance_squared(point: Pos2, start: Pos2, end: Pos2) -> f32 {
    let segment = end - start;
    let length_squared = segment.length_sq();
    if length_squared <= f32::EPSILON {
        return point.distance_sq(start);
    }
    let factor = ((point - start).dot(segment) / length_squared).clamp(0.0, 1.0);
    point.distance_sq(start + segment * factor)
}

fn compact_label(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let compact: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{compact}...")
    } else {
        compact
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_names_hide_transport_prefixes_without_losing_device_identity() {
        let i18n = I18n::default();
        assert_eq!(display_node_name("Dummy-Driver", &i18n), "Dummy Driver");
        assert_eq!(
            display_node_name("bluez_output.B0:F0:0C:5E:99:5A", &i18n),
            "Bluetooth Output - 99:5A"
        );
        assert_eq!(
            display_port_name("Midi Through: Port-0 (capture)", &i18n),
            "Port 0 Capture"
        );
    }

    #[test]
    fn display_names_use_the_selected_locale() {
        let i18n = I18n::from_language("es");
        assert_eq!(
            display_node_name("bluez_input.B0:F0:0C:5E:99:5A", &i18n),
            "Entrada Bluetooth - 99:5A"
        );
        assert_eq!(
            display_port_name("Midi Through: Port-0 (capture)", &i18n),
            "Port 0 Captura"
        );
    }

    #[test]
    fn bezier_curve_starts_and_ends_at_ports() {
        let start = pos2(20.0, 40.0);
        let end = pos2(220.0, 120.0);
        let points = bezier_points(start, end, 0.0);
        assert_eq!(points.first().copied(), Some(start));
        assert_eq!(points.last().copied(), Some(end));
        assert!(point_near_polyline(points[14], &points, 1.0));
    }
}
