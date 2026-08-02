use crate::{GraphCanvas, PortId};
use egui::{pos2, vec2, Color32, FontId, Rect, Sense, Stroke, Ui};
use pw_graph_core::{Direction, Graph, Node, Port};
use pw_graph_i18n::I18n;
use std::collections::HashMap;

use super::super::names::compact_label;
use super::super::ports::{
    display_groups, link_exists, pair_ports, port_color, port_group_tooltip, port_role,
};
use super::PORT_ROW_HEIGHT;

impl GraphCanvas {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_node_ports(
        &mut self,
        ui: &mut Ui,
        painter: &egui::Painter,
        node: &Node,
        graph: &Graph,
        node_rect: Rect,
        ports: Vec<&Port>,
        has_audio: bool,
        accent: Color32,
        text_scale: f32,
        i18n: &I18n,
        anchors: &mut HashMap<PortId, egui::Pos2>,
        actions: &mut Vec<crate::CanvasAction>,
    ) {
        let groups = display_groups(self.connect_mode, ports, i18n);
        let mut group_label_totals: HashMap<(Direction, String), usize> = HashMap::new();
        for group in &groups {
            *group_label_totals
                .entry((group.direction, group.label.clone()))
                .or_insert(0) += 1;
        }
        let mut group_label_seen: HashMap<(Direction, String), usize> = HashMap::new();

        let controls_offset = self.node_controls_height(node, has_audio);
        for (index, group) in groups.into_iter().enumerate() {
            let y = node_rect.top()
                + (super::NODE_HEADER_HEIGHT
                    + controls_offset
                    + 13.0
                    + index as f32 * PORT_ROW_HEIGHT)
                    * self.zoom;
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
            let interaction_rect = if self.connect_mode == crate::ConnectMode::Easy {
                row_rect
            } else {
                hit_rect
            };
            let mut response = ui.interact(
                interaction_rect,
                ui.id()
                    .with(("graph-port", node.id, index, representative_id)),
                Sense::click_and_drag(),
            );
            let port_help = port_group_tooltip(node, &group, i18n);
            if group.port_type != pw_graph_core::PortType::Audio {
                response = response.on_hover_text(port_help);
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
            let dot_color = port_color(group.port_type, port_role(group.representative()));
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
            if group.direction == Direction::Sink && (response.clicked() || response.drag_stopped())
            {
                if let Some(output_ids) = self.pending_outputs.take() {
                    let output_ports: Vec<&Port> =
                        output_ids.iter().filter_map(|id| graph.port(*id)).collect();
                    let pairs: Vec<_> = pair_ports(&output_ports, &group.ports)
                        .into_iter()
                        .filter(|(output, input)| !link_exists(graph, *output, *input))
                        .collect();
                    if !pairs.is_empty() {
                        actions.push(crate::CanvasAction::ConnectMany { pairs });
                    }
                }
            }
        }
    }
}
