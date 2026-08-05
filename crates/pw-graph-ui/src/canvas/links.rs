//! Drawing and click/hover handling for existing links between ports.

use crate::{CanvasAction, GraphCanvas, NodeId};
use egui::{Color32, Pos2, Rect, Sense, Shape, Stroke, Ui};
use pw_graph_core::{Graph, Port};
use pw_graph_i18n::I18n;
use std::cell::Cell;
use std::collections::BTreeSet;

use super::geometry::{bezier_points, point_near_polyline, points_bounds};
use super::ports::{link_color, port_role};

const EDGE_HIT_DISTANCE: f32 = 9.0;

impl GraphCanvas {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_links(
        &mut self,
        ui: &Ui,
        painter: &egui::Painter,
        rect: Rect,
        graph: &Graph,
        i18n: &I18n,
        visible_node_ids: &BTreeSet<NodeId>,
        pointer_pos: Option<Pos2>,
        pointer_over_node: bool,
        actions: &mut Vec<CanvasAction>,
    ) -> bool {
        let disconnect_label = if self.connect_mode == crate::ConnectMode::Easy {
            i18n.text("canvas.disconnect_group")
        } else {
            i18n.text("toolbar.disconnect")
        };
        let mut pointer_over_link = false;
        for (link_index, link) in graph.links.values().enumerate() {
            let (Some(source), Some(destination)) = (
                graph.ports.get(&link.output_port),
                graph.ports.get(&link.input_port),
            ) else {
                continue;
            };
            if !self.link_is_visible(graph, link, visible_node_ids) {
                continue;
            }
            let (Some(source_pos), Some(destination_pos)) = (
                self.port_anchor(rect, graph, source),
                self.port_anchor(rect, graph, destination),
            ) else {
                continue;
            };
            let selected = self.selected_link.is_some_and(|selected| {
                self.selected_links_for(graph, selected).contains(&link.id)
            });
            let points = bezier_points(
                source_pos,
                destination_pos,
                (link_index as i32 % 5 - 2) as f32 * 5.0,
            );
            let hovered = pointer_pos
                .is_some_and(|pointer| point_near_polyline(pointer, &points, EDGE_HIT_DISTANCE));
            pointer_over_link |= hovered;
            let hit_rect = points_bounds(&points).expand(EDGE_HIT_DISTANCE);
            let link_widget_id = ui.id().with((
                "graph-link",
                link_index,
                link.id,
                link.output_port,
                link.input_port,
            ));
            let mut response = ui.interact(hit_rect, link_widget_id, Sense::click());
            let disconnect_requested = Cell::new(false);
            if hovered {
                response = response.on_hover_text(link_tooltip(graph, source, destination, i18n));
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                response.context_menu(|ui| {
                    if ui.button(&disconnect_label).clicked() {
                        disconnect_requested.set(true);
                        ui.close_menu();
                    }
                });
            }
            let focused = response.has_focus();
            let color = if selected {
                Color32::LIGHT_GREEN
            } else if focused {
                Color32::LIGHT_BLUE
            } else if hovered {
                Color32::WHITE
            } else {
                link_color(source.port_type, port_role(source))
            };
            painter.add(Shape::line(
                points.clone(),
                Stroke::new(
                    if selected {
                        5.0_f32
                    } else if focused {
                        4.5_f32
                    } else {
                        3.5_f32
                    },
                    Color32::BLACK,
                ),
            ));
            painter.add(Shape::line(
                points.clone(),
                Stroke::new(
                    if selected {
                        2.8_f32
                    } else if focused {
                        2.4_f32
                    } else {
                        1.6_f32
                    },
                    color,
                ),
            ));
            let keyboard_clicked = response.clicked()
                && ui.input(|input| {
                    input.key_pressed(egui::Key::Enter) || input.key_pressed(egui::Key::Space)
                });
            let pointer_clicked = hovered && !pointer_over_node && response.clicked();
            let clicked = keyboard_clicked || pointer_clicked;
            if clicked {
                self.selected_link = Some(link.id);
                self.selected_nodes.clear();
                self.selected_node = None;
            }
            if disconnect_requested.get() {
                self.selected_link = Some(link.id);
                self.selected_nodes.clear();
                self.selected_node = None;
                let links = self.selected_links_for(graph, link.id);
                if links.len() > 1 {
                    actions.push(CanvasAction::DisconnectMany { links });
                } else {
                    actions.push(CanvasAction::Disconnect { link: link.id });
                }
            }
        }
        pointer_over_link
    }
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
            (
                "type",
                super::ports::port_type_label(source.port_type, i18n),
            ),
            ("source_node", source_node),
            ("source_port", source.name.clone()),
            ("destination_node", destination_node),
            ("destination_port", destination.name.clone()),
        ],
    )
}
