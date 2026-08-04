//! Rendering and interaction for a single node: header dragging, whole-node
//! Easy-mode connect drags, and the port/group rows underneath.

use crate::{
    ButtonProps, CanvasAction, ConnectMode, GraphCanvas, MeterReading, PortId, UiDocument, Value,
};
use egui::{pos2, vec2, Color32, FontId, Pos2, Rect, Sense, Shape, Stroke, Ui, Vec2};
use pw_graph_core::{Direction, Graph, Node, Port, PortType};
use pw_graph_i18n::I18n;
use std::cell::Cell;
use std::collections::HashMap;

use super::geometry::bezier_points;
use super::names::{compact_label, display_node_name};
use super::ports::{
    display_groups, link_exists, pair_ports, port_color, port_group_tooltip, port_role,
};

mod controls;
mod helpers;
mod layout;
mod ports;
use helpers::{dominant_port, node_color, node_tooltip, node_type_label};

const NODE_WIDTH: f32 = 244.0;
const NODE_HEADER_HEIGHT: f32 = 42.0;
const COLLAPSED_NODE_HEIGHT: f32 = 50.0;
const PORT_ROW_HEIGHT: f32 = 25.0;
const AUDIO_CONTROLS_HEIGHT: f32 = 42.0;
/// The vertical space reserved around the effect control rows. The controls
/// share the canvas zoom so their panel, labels, and port rows all scale as a
/// single node.
pub(super) const EFFECT_CONTROLS_VERTICAL_PADDING: f32 = 10.0;
pub(super) const EFFECT_CONTROLS_MIN_HEIGHT: f32 = 42.0;
pub(super) const EFFECT_CONTROL_ROW_HEIGHT: f32 = 26.0;

pub(super) struct AudioInfo {
    pub(super) port_id: PortId,
    pub(super) port_help: String,
    pub(super) meter: Option<MeterReading>,
}

pub(super) struct NodeDrawContext<'a> {
    pub rect: Rect,
    pub graph: &'a Graph,
    pub i18n: &'a I18n,
    pub document: &'a mut UiDocument,
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
        let document = &mut *context.document;
        let anchors = &mut *context.anchors;
        let actions = &mut *context.actions;
        let ports = self.ordered_ports(graph, node);
        let has_audio = ports.iter().any(|port| port.port_type == PortType::Audio);
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
        let appearance = self.node_appearance(node.id);
        let visible_audio_controls = has_audio
            && node.node_type != pw_graph_core::NodeType::Effect
            && !appearance.collapsed
            && !self.thumbnail_mode;
        let visible_effect_controls = node.node_type == pw_graph_core::NodeType::Effect
            && self.effect_controls.contains_key(&node.id)
            && !appearance.collapsed
            && !self.thumbnail_mode;
        let monitor_port = ports
            .iter()
            .copied()
            .filter(|port| port.port_type == PortType::Audio && port.direction == Direction::Source)
            .max_by_key(|port| {
                (
                    matches!(port_role(port), super::ports::PortRole::Monitor),
                    port.id,
                )
            })
            .map(|port| port.id);
        let audio_info = monitor_port.and_then(|port_id| {
            let group = display_groups(self.connect_mode, ports.clone(), i18n)
                .into_iter()
                .find(|group| group.contains(port_id))?;
            let meter = self
                .port_meters
                .get(&port_id)
                .copied()
                .or_else(|| self.meters.get(&node.id).copied());
            Some(AudioInfo {
                port_id,
                port_help: port_group_tooltip(node, &group, i18n),
                meter,
            })
        });
        let audio_meter = audio_info
            .as_ref()
            .and_then(|audio_info| audio_info.meter)
            .or_else(|| self.meters.get(&node.id).copied());
        let controls_height = (if visible_audio_controls {
            AUDIO_CONTROLS_HEIGHT
        } else {
            0.0
        } + if visible_effect_controls {
            self.effect_controls_height(node)
        } else {
            0.0
        }) * self.zoom;
        let header_drag_rect = Rect::from_min_max(
            header.min,
            pos2(header.right() - 60.0 * self.zoom, header.bottom()),
        );
        let disconnect_node_label = i18n.text(if easy_connect {
            "canvas.disconnect_node_easy"
        } else {
            "canvas.disconnect_node_advanced"
        });
        let body_sense = if easy_connect {
            Sense::click_and_drag()
        } else {
            Sense::click()
        };
        // Keep the Easy-mode connect gesture out of the movable header. A
        // header drag is always a node move; the body below it is the
        // node-to-node connection target.
        let body_rect = Rect::from_min_max(
            pos2(node_rect.left(), header.bottom() + controls_height),
            node_rect.max,
        );
        let body_tooltip = if easy_connect {
            format!("{tooltip}\n\n{}", i18n.text("canvas.drag_body_connect"))
        } else {
            tooltip.clone()
        };
        let body_response = ui
            .interact(
                body_rect,
                ui.id().with(("graph-node-body", node.id)),
                body_sense,
            )
            .on_hover_text(body_tooltip);
        let disconnect_node_requested = Cell::new(false);
        let remove_effect_requested = Cell::new(false);
        let arrange_nodes_requested = Cell::new(false);
        body_response.context_menu(|ui| {
            if node_button(
                document,
                ui,
                &format!("node.{}.context.disconnect", node.id),
                disconnect_node_label.clone(),
            ) {
                disconnect_node_requested.set(true);
                ui.close_menu();
            }
            if node.node_type == pw_graph_core::NodeType::Effect
                && node_button(
                    document,
                    ui,
                    &format!("node.{}.context.remove-effect", node.id),
                    i18n.text("canvas.remove_effect"),
                )
            {
                remove_effect_requested.set(true);
                ui.close_menu();
            }
            if node_button(
                document,
                ui,
                &format!("node.{}.context.arrange", node.id),
                i18n.text("canvas.arrange_selection"),
            ) {
                arrange_nodes_requested.set(true);
                ui.close_menu();
            }
        });
        let header_response = ui
            .interact(
                header_drag_rect,
                ui.id().with(("graph-node-header", node.id)),
                Sense::click_and_drag(),
            )
            .on_hover_text(format!("{tooltip}\n\n{}", i18n.text("canvas.drag_header")));
        header_response.context_menu(|ui| {
            if node_button(
                document,
                ui,
                &format!("node.{}.header.disconnect", node.id),
                disconnect_node_label.clone(),
            ) {
                disconnect_node_requested.set(true);
                ui.close_menu();
            }
            if node.node_type == pw_graph_core::NodeType::Effect
                && node_button(
                    document,
                    ui,
                    &format!("node.{}.header.remove-effect", node.id),
                    i18n.text("canvas.remove_effect"),
                )
            {
                remove_effect_requested.set(true);
                ui.close_menu();
            }
            if node_button(
                document,
                ui,
                &format!("node.{}.header.arrange", node.id),
                i18n.text("canvas.arrange_selection"),
            ) {
                arrange_nodes_requested.set(true);
                ui.close_menu();
            }
        });
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
                        let pairs: Vec<_> = pairs
                            .into_iter()
                            .filter(|(output, input)| !link_exists(graph, *output, *input))
                            .collect();
                        if !pairs.is_empty() {
                            actions.push(CanvasAction::ConnectMany { pairs });
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
            if let Some(pointer) = ui.input(|input| {
                input
                    .pointer
                    .hover_pos()
                    .or_else(|| input.pointer.interact_pos())
            }) {
                if let Some(target) = self.node_at(rect, graph, pointer, node.id) {
                    if let Some(target_node) = graph.node(target) {
                        let pairs: Vec<_> = self
                            .matching_port_pairs(graph, node, target_node)
                            .into_iter()
                            .filter(|(output, input)| !link_exists(graph, *output, *input))
                            .collect();
                        if !pairs.is_empty() {
                            actions.push(CanvasAction::ConnectMany { pairs });
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
            let before: Vec<_> = self
                .dragging_origin
                .iter()
                .map(|(id, position)| (*id, *position))
                .collect();
            let delta = self.drag_delta / self.zoom;
            let after: Vec<_> = before
                .iter()
                .map(|(id, origin)| (*id, [origin[0] + delta.x, origin[1] + delta.y]))
                .collect();
            if before != after {
                actions.push(CanvasAction::CommitNodeMove { before, after });
            }
            self.dragging_node = None;
            self.dragging_origin.clear();
            self.drag_delta = Vec2::ZERO;
        }
        if header_response.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        } else if header_response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }

        if disconnect_node_requested.get() {
            actions.push(CanvasAction::DisconnectNode { node: node.id });
        }
        if remove_effect_requested.get() {
            actions.push(CanvasAction::RemoveEffect { node: node.id });
        }
        if arrange_nodes_requested.get() {
            let nodes = if self.selected_nodes.contains(&node.id) && self.selected_nodes.len() > 1 {
                self.selected_nodes.iter().copied().collect()
            } else {
                vec![node.id]
            };
            actions.push(CanvasAction::ArrangeNodes { nodes });
        }

        let selected = self.selected_nodes.contains(&node.id);
        let focused = body_response.has_focus() || header_response.has_focus();
        let text_scale = self.node_text_scale.clamp(0.80, 2.0);
        let accent = appearance
            .color
            .map(|color| Color32::from_rgba_unmultiplied(color[0], color[1], color[2], color[3]))
            .unwrap_or_else(|| {
                dominant_port(&ports)
                    .map(|port| port_color(port.port_type, port_role(port)))
                    .unwrap_or_else(|| node_color(node.node_type))
            });
        let fill = if selected {
            Color32::from_rgb(48, 60, 76)
        } else {
            Color32::from_rgb(38, 45, 56)
        };
        // A small, crisp shadow separates cards from the grid without the
        // heavy glow that would compete with connection lines.
        painter.rect_filled(
            node_rect.translate(vec2(0.0, 3.0 * self.zoom)),
            8.0,
            Color32::from_black_alpha(70),
        );
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
        painter.line_segment(
            [header.left_bottom(), header.right_bottom()],
            Stroke::new(1.0_f32, Color32::from_rgb(61, 73, 89)),
        );
        painter.rect_filled(
            Rect::from_min_max(
                header.min,
                pos2(header.left() + 4.0 * self.zoom, header.bottom()),
            ),
            8.0,
            accent,
        );
        painter.text(
            header.left_center() + vec2(12.0 * self.zoom, -7.0 * self.zoom),
            egui::Align2::LEFT_CENTER,
            compact_label(
                &display_node_name(
                    appearance.custom_name.as_deref().unwrap_or(&node.name),
                    i18n,
                ),
                22,
            ),
            FontId::proportional(13.5 * self.zoom * text_scale),
            Color32::WHITE,
        );
        painter.text(
            header.left_center() + vec2(12.0 * self.zoom, 9.0 * self.zoom),
            egui::Align2::LEFT_CENTER,
            node_type_label(node.node_type, i18n),
            FontId::proportional(9.5 * self.zoom * text_scale),
            Color32::from_rgb(170, 184, 201),
        );
        self.draw_node_header_controls(
            ui,
            node,
            header,
            &appearance,
            !ports.is_empty(),
            accent,
            actions,
            i18n,
            &tooltip,
            audio_info.as_ref(),
            document,
        );

        if visible_audio_controls {
            painter.line_segment(
                [
                    pos2(
                        node_rect.left() + 10.0 * self.zoom,
                        header.bottom() + AUDIO_CONTROLS_HEIGHT * self.zoom,
                    ),
                    pos2(
                        node_rect.right() - 10.0 * self.zoom,
                        header.bottom() + AUDIO_CONTROLS_HEIGHT * self.zoom,
                    ),
                ],
                Stroke::new(1.0_f32, Color32::from_rgb(52, 63, 78)),
            );
            self.draw_node_audio_controls(
                ui,
                node,
                node_rect,
                header,
                accent,
                audio_meter,
                actions,
                i18n,
                document,
            );
        }

        if visible_effect_controls {
            self.draw_node_effect_controls(ui, node, node_rect, header, actions, i18n, document);
        }

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

        if self.thumbnail_mode || appearance.collapsed {
            return;
        }

        self.draw_node_ports(
            ui, &painter, node, graph, node_rect, ports, has_audio, accent, text_scale, i18n,
            anchors, actions,
        );
    }
}

pub(super) fn node_button(
    document: &mut UiDocument,
    ui: &mut Ui,
    id: &str,
    label: impl Into<String>,
) -> bool {
    document.button(ui, ButtonProps::new(id, label)).clicked()
}

pub(super) fn sync_document_value(document: &mut UiDocument, id: &str, value: Value) {
    if document.value(id) != Some(&value) {
        document.set_value(id, value);
    }
}
