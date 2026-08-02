//! Rendering and interaction for a single node: header dragging, whole-node
//! Easy-mode connect drags, and the port/group rows underneath.

use crate::{CanvasAction, ConnectMode, GraphCanvas, NodeId, PortId};
use egui::{pos2, vec2, Color32, FontId, Pos2, ProgressBar, Rect, RichText, Sense, Shape, Stroke, Ui, Vec2};
use pw_graph_core::{Direction, Graph, Node, NodeType, Port, PortType};
use pw_graph_i18n::I18n;
use std::cell::Cell;
use std::collections::HashMap;

use super::geometry::bezier_points;
use super::names::{compact_label, display_node_name};
use super::ports::{display_groups, link_exists, pair_ports, port_color, port_group_tooltip};

const NODE_WIDTH: f32 = 244.0;
const NODE_HEADER_HEIGHT: f32 = 34.0;
const PORT_ROW_HEIGHT: f32 = 25.0;

pub(super) struct NodeDrawContext<'a> {
    pub rect: Rect,
    pub graph: &'a Graph,
    pub i18n: &'a I18n,
    pub anchors: &'a mut HashMap<PortId, Pos2>,
    pub actions: &'a mut Vec<CanvasAction>,
}

impl GraphCanvas {
    pub(super) fn draw_node(
        &mut self,
        ui: &mut Ui,
        painter: egui::Painter,
        node: &Node,
        context: &mut NodeDrawContext<'_>,
    ) {
        let rect = context.rect;
        let graph = context.graph;
        let i18n = context.i18n;
        let anchors = &mut *context.anchors;
        let actions = &mut *context.actions;
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
        let easy_connect = self.connect_mode == ConnectMode::Easy;
        let body_sense = if easy_connect {
            Sense::click_and_drag()
        } else {
            Sense::click()
        };
        let body_tooltip = if easy_connect {
            format!("{tooltip}\n\n{}", i18n.text("canvas.drag_body_connect"))
        } else {
            tooltip.clone()
        };
        let body_response = ui
            .interact(
                node_rect,
                ui.id().with(("graph-node-body", node.id)),
                body_sense,
            )
            .on_hover_text(body_tooltip);
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
                if let Some(output_ids) = self.pending_outputs.clone() {
                    let output_ports: Vec<&Port> =
                        output_ids.iter().filter_map(|id| graph.port(*id)).collect();
                    let inputs: Vec<&Port> = self
                        .ordered_ports(graph, node)
                        .into_iter()
                        .filter(|port| port.direction == Direction::Sink)
                        .collect();
                    let pairs = pair_ports(&output_ports, &inputs);
                    if !pairs.is_empty() {
                        self.pending_outputs = None;
                        for (output, input) in pairs {
                            if !link_exists(graph, output, input) {
                                actions.push(CanvasAction::Connect { output, input });
                            }
                        }
                    }
                }
            }
        }
        if easy_connect && body_response.drag_started() {
            self.pending_outputs = None;
            self.pending_node_connect = Some(node.id);
        }
        if self.pending_node_connect == Some(node.id) && body_response.drag_stopped() {
            self.pending_node_connect = None;
            if let Some(pointer) = ui.input(|input| input.pointer.interact_pos()) {
                if let Some(target) = self.node_at(rect, graph, pointer, node.id) {
                    if let Some(target_node) = graph.node(target) {
                        for (output, input) in self.matching_port_pairs(graph, node, target_node)
                        {
                            if !link_exists(graph, output, input) {
                                actions.push(CanvasAction::Connect { output, input });
                            }
                        }
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
        let focused = body_response.has_focus() || header_response.has_focus();
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
                if selected || focused {
                    2.0_f32
                } else {
                    1.0_f32
                },
                if selected {
                    accent
                } else if focused {
                    Color32::LIGHT_BLUE
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

        if let Some(source_id) = self.pending_node_connect {
            if source_id == node.id {
                if let Some(pointer) = ui.input(|input| input.pointer.interact_pos()) {
                    let start = node_rect.right_center();
                    let points = bezier_points(start, pointer, 0.0);
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
                            "canvas.pending_node_connection",
                            &[
                                ("action", i18n.text("canvas.connect_hint")),
                                ("node", display_node_name(&node.name, i18n)),
                            ],
                        ),
                        FontId::proportional(12.0 * self.zoom * text_scale),
                        Color32::LIGHT_GREEN,
                    );
                }
            } else if let Some(pointer) = ui.input(|input| input.pointer.interact_pos()) {
                if node_rect.contains(pointer) {
                    painter.rect_stroke(node_rect, 8.0, Stroke::new(2.5_f32, Color32::LIGHT_GREEN));
                }
            }
        }

        if self.thumbnail_mode {
            return;
        }

        let groups = display_groups(self.connect_mode, ports, i18n);
        let mut group_label_totals: HashMap<(Direction, String), usize> = HashMap::new();
        for group in &groups {
            *group_label_totals
                .entry((group.direction, group.label.clone()))
                .or_insert(0) += 1;
        }
        let mut group_label_seen: HashMap<(Direction, String), usize> = HashMap::new();

        for (index, group) in groups.into_iter().enumerate() {
            let y = node_rect.top()
                + (NODE_HEADER_HEIGHT + 13.0 + index as f32 * PORT_ROW_HEIGHT) * self.zoom;
            let x = if group.direction == Direction::Source {
                node_rect.right() - 12.0 * self.zoom
            } else {
                node_rect.left() + 12.0 * self.zoom
            };
            let anchor = pos2(x, y);
            for port in &group.ports {
                anchors.insert(port.id, anchor);
            }
            let row_rect = Rect::from_min_max(
                pos2(node_rect.left() + 5.0 * self.zoom, y - 10.0 * self.zoom),
                pos2(node_rect.right() - 5.0 * self.zoom, y + 10.0 * self.zoom),
            );
            let hit_rect = Rect::from_center_size(anchor, vec2(22.0, 22.0) * self.zoom.max(0.7));
            let representative_id = group.representative().id;
            let mut response = ui.interact(
                hit_rect,
                ui.id().with(("graph-port", node.id, index, representative_id)),
                Sense::click_and_drag(),
            );
            let pin_requested = Cell::new(false);
            let port_help = port_group_tooltip(node, &group, i18n);
            if group.port_type == PortType::Audio {
                let meter = self.meters.get(&node.id).copied();
                response = response.on_hover_ui(|ui| {
                    ui.label(RichText::new(port_help.clone()).strong());
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
                        Some(_) => {
                            ui.label(
                                RichText::new(i18n.text("canvas.audio_meter_unavailable")).weak(),
                            );
                        }
                        None if self.metering_disabled => {
                            ui.label(
                                RichText::new(i18n.text("canvas.audio_meter_disabled")).weak(),
                            );
                        }
                        // Having no reading yet is the normal case for on-demand
                        // metering: hovering is what asks the backend to attach a
                        // stream, and negotiating it takes a moment.
                        None => {
                            ui.label(
                                RichText::new(i18n.text("canvas.audio_meter_starting")).weak(),
                            );
                        }
                    }
                    if ui
                        .button(if self.pinned_meter == Some(representative_id) {
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
                self.pinned_meter = Some(representative_id);
            }
            if group.port_type == PortType::Audio && response.hovered() {
                self.hovered_meter_node = Some(node.id);
            }
            let pending = self
                .pending_outputs
                .as_ref()
                .is_some_and(|pending| pending.iter().any(|id| group.contains(*id)));
            if response.hovered() || pending || response.has_focus() {
                painter.rect_filled(
                    row_rect,
                    4.0,
                    Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 42),
                );
            }
            let radius = 6.0 * self.zoom.max(0.7);
            let dot_color = port_color(group.port_type);
            painter.circle_stroke(
                anchor,
                radius + 0.6,
                Stroke::new(1.2_f32, Color32::from_black_alpha(170)),
            );
            painter.circle_filled(anchor, radius, dot_color);
            if response.hovered() || pending || response.has_focus() {
                painter.circle_stroke(
                    anchor,
                    radius + 3.0,
                    Stroke::new(
                        1.5_f32,
                        if response.has_focus() && !response.hovered() && !pending {
                            Color32::LIGHT_BLUE
                        } else {
                            Color32::WHITE
                        },
                    ),
                );
            }
            let text_pos = if group.direction == Direction::Source {
                anchor - vec2(10.0, 0.0)
            } else {
                anchor + vec2(10.0, 0.0)
            };
            let name_key = (group.direction, group.label.clone());
            let base_label = if group_label_totals.get(&name_key).copied().unwrap_or(1) > 1 {
                let seen = group_label_seen.entry(name_key).or_insert(0);
                *seen += 1;
                format!("{} #{}", group.label, seen)
            } else {
                group.label.clone()
            };
            let label = if group.ports.len() > 1 {
                format!("{base_label} \u{d7}{}", group.ports.len())
            } else {
                base_label
            };
            painter.text(
                text_pos,
                if group.direction == Direction::Source {
                    egui::Align2::RIGHT_CENTER
                } else {
                    egui::Align2::LEFT_CENTER
                },
                compact_label(&label, 25),
                FontId::proportional(11.5 * self.zoom * text_scale),
                Color32::from_rgb(215, 220, 227),
            );

            if group.direction == Direction::Source
                && (response.clicked() || response.drag_started())
            {
                self.pending_outputs = Some(group.ports.iter().map(|port| port.id).collect());
            }
            if group.direction == Direction::Sink
                && (response.clicked() || response.drag_stopped())
            {
                if let Some(output_ids) = self.pending_outputs.take() {
                    let output_ports: Vec<&Port> =
                        output_ids.iter().filter_map(|id| graph.port(*id)).collect();
                    for (output, input) in pair_ports(&output_ports, &group.ports) {
                        if !link_exists(graph, output, input) {
                            actions.push(CanvasAction::Connect { output, input });
                        }
                    }
                }
            }
        }
    }

    pub(super) fn ordered_ports<'a>(&self, graph: &'a Graph, node: &Node) -> Vec<&'a Port> {
        let mut ports: Vec<&Port> = node
            .ports
            .iter()
            .filter_map(|id| graph.ports.get(id))
            .filter(|port| self.media_filter.matches_port_type(port.port_type))
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

    pub(super) fn node_rect(&self, rect: Rect, graph: &Graph, node: &Node) -> Rect {
        let port_count = if self.thumbnail_mode {
            0
        } else {
            let ports = self.ordered_ports(graph, node);
            super::ports::grouped_rows(self.connect_mode, &ports).len()
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

    pub(super) fn port_anchor(&self, rect: Rect, graph: &Graph, port: &Port) -> Option<Pos2> {
        let node = graph.nodes.get(&port.node_id)?;
        let ordered = self.ordered_ports(graph, node);
        let rows = super::ports::grouped_rows(self.connect_mode, &ordered);
        let index = rows
            .iter()
            .position(|row| row.iter().any(|item| item.id == port.id))?;
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

    /// The topmost visible node (other than `exclude`) whose rect contains
    /// `point`, used to find the drop target of an Easy-mode connect drag.
    pub(super) fn node_at(
        &self,
        rect: Rect,
        graph: &Graph,
        point: Pos2,
        exclude: NodeId,
    ) -> Option<NodeId> {
        graph
            .nodes
            .values()
            .filter(|node| node.id != exclude && self.media_filter.matches_node(graph, node.id))
            .find(|node| self.node_rect(rect, graph, node).contains(point))
            .map(|node| node.id)
    }

    /// Pairs `source`'s outputs with `target`'s inputs in port order, matching
    /// compatible port types (e.g. stereo L/R landing on L/R in order). Used
    /// for the whole-node Easy-mode connect drag, which always operates on
    /// every raw port regardless of how they're visually grouped.
    pub(super) fn matching_port_pairs(
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

fn node_type_label(node_type: NodeType, i18n: &I18n) -> String {
    match node_type {
        NodeType::PipeWire => i18n.text("canvas.node_type_pipewire"),
        NodeType::AlsaMidi => i18n.text("canvas.node_type_alsa_midi"),
        NodeType::Unknown => i18n.text("canvas.node_type_unknown"),
    }
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
