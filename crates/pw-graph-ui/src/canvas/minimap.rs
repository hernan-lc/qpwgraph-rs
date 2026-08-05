//! Overview renderer for the complete graph scene.

use crate::{GraphCanvas, NodeId};
use egui::{pos2, vec2, Color32, FontId, Pos2, Rect, Stroke};
use pw_graph_core::{Graph, NodeType};
use pw_graph_i18n::I18n;
use std::collections::BTreeSet;

use super::node::accent_color;
use super::ports::{link_color, port_color, port_role};

const PANEL_SIZE: egui::Vec2 = vec2(238.0, 164.0);
const PANEL_MARGIN: f32 = 12.0;
const INNER_MARGIN: f32 = 8.0;
const TITLE_HEIGHT: f32 = 17.0;

fn extend_rect(bounds: &mut Option<Rect>, candidate: Rect) {
    *bounds = Some(match *bounds {
        Some(current) => Rect::from_min_max(
            pos2(
                current.left().min(candidate.left()),
                current.top().min(candidate.top()),
            ),
            pos2(
                current.right().max(candidate.right()),
                current.bottom().max(candidate.bottom()),
            ),
        ),
        None => candidate,
    });
}

fn node_accent(
    canvas: &GraphCanvas,
    graph: &Graph,
    node_id: NodeId,
    node_type: NodeType,
) -> Color32 {
    let appearance = canvas.node_appearance(node_id);
    let port_accent = graph.node(node_id).and_then(|node| {
        node.ports
            .iter()
            .filter_map(|port_id| graph.port(*port_id))
            .map(|port| port_color(port.port_type, port_role(port)))
            .next()
    });
    accent_color(&appearance, port_accent, node_type)
}

impl GraphCanvas {
    /// Draw a compact overview of the currently visible graph. The viewport
    /// outline makes it clear which part of the filtered scene is visible.
    pub(super) fn draw_minimap(
        &self,
        painter: &egui::Painter,
        canvas_rect: Rect,
        graph: &Graph,
        i18n: &I18n,
        visible_node_ids: &BTreeSet<NodeId>,
    ) {
        let panel_width = PANEL_SIZE.x.min((canvas_rect.width() - 16.0).max(0.0));
        let panel_height = PANEL_SIZE.y.min((canvas_rect.height() - 16.0).max(0.0));
        if panel_width <= 0.0 || panel_height <= 0.0 {
            return;
        }
        let panel_rect = Rect::from_min_size(
            pos2(
                canvas_rect.right() - panel_width - PANEL_MARGIN,
                canvas_rect.bottom() - panel_height - PANEL_MARGIN,
            ),
            vec2(panel_width, panel_height),
        );
        painter.rect(
            panel_rect,
            8.0,
            Color32::from_rgba_unmultiplied(25, 29, 36, 238),
            Stroke::new(1.0_f32, Color32::from_rgb(86, 103, 125)),
        );
        painter.text(
            panel_rect.left_top() + vec2(INNER_MARGIN, 5.0),
            egui::Align2::LEFT_TOP,
            i18n.text("toolbar.minimap"),
            FontId::proportional(11.0),
            Color32::from_rgb(205, 216, 230),
        );

        let content_rect = Rect::from_min_max(
            pos2(
                panel_rect.left() + INNER_MARGIN,
                panel_rect.top() + TITLE_HEIGHT,
            ),
            pos2(
                panel_rect.right() - INNER_MARGIN,
                panel_rect.bottom() - INNER_MARGIN,
            ),
        );
        if content_rect.width() <= 0.0 || content_rect.height() <= 0.0 {
            return;
        }

        let viewport_scene = Rect::from_min_size(
            pos2(-self.pan.x / self.zoom, -self.pan.y / self.zoom),
            vec2(
                canvas_rect.width() / self.zoom,
                canvas_rect.height() / self.zoom,
            ),
        );
        let mut scene_bounds = None;
        for node in graph
            .nodes
            .values()
            .filter(|node| visible_node_ids.contains(&node.id))
        {
            extend_rect(
                &mut scene_bounds,
                Rect::from_min_size(
                    pos2(node.position[0], node.position[1]),
                    self.node_scene_size(graph, node),
                ),
            );
        }
        if scene_bounds.is_none() {
            scene_bounds = Some(viewport_scene);
        }
        let scene_bounds = scene_bounds
            .expect("the minimap always has either nodes or a viewport")
            .expand(32.0);
        let scale = (content_rect.width() / scene_bounds.width())
            .min(content_rect.height() / scene_bounds.height());
        if !scale.is_finite() || scale <= 0.0 {
            return;
        }
        let mapped_size = scene_bounds.size() * scale;
        let map_rect = Rect::from_center_size(content_rect.center(), mapped_size);
        let map_point = |point: Pos2| {
            pos2(
                map_rect.left() + (point.x - scene_bounds.left()) * scale,
                map_rect.top() + (point.y - scene_bounds.top()) * scale,
            )
        };
        let map_rect_for_scene =
            |scene: Rect| Rect::from_min_max(map_point(scene.min), map_point(scene.max));
        let minimap_painter = painter.with_clip_rect(content_rect);

        for link in graph.links.values() {
            let (Some(output), Some(input)) =
                (graph.port(link.output_port), graph.port(link.input_port))
            else {
                continue;
            };
            let (Some(source), Some(destination)) =
                (graph.node(output.node_id), graph.node(input.node_id))
            else {
                continue;
            };
            if !self.link_is_visible(graph, link, visible_node_ids) {
                continue;
            }
            let source_center = map_point(
                pos2(source.position[0], source.position[1])
                    + self.node_scene_size(graph, source) * 0.5,
            );
            let destination_center = map_point(
                pos2(destination.position[0], destination.position[1])
                    + self.node_scene_size(graph, destination) * 0.5,
            );
            minimap_painter.line_segment(
                [source_center, destination_center],
                Stroke::new(1.0_f32, link_color(output.port_type, port_role(output))),
            );
        }

        for node in graph
            .nodes
            .values()
            .filter(|node| visible_node_ids.contains(&node.id))
        {
            let node_rect = map_rect_for_scene(Rect::from_min_size(
                pos2(node.position[0], node.position[1]),
                self.node_scene_size(graph, node),
            ));
            let fill = if self.selected_nodes.contains(&node.id) {
                Color32::from_rgb(78, 112, 145)
            } else {
                Color32::from_rgb(48, 58, 72)
            };
            let accent = node_accent(self, graph, node.id, node.node_type);
            minimap_painter.rect(node_rect, 2.0, fill, Stroke::new(1.0_f32, accent));
            minimap_painter.rect_filled(
                Rect::from_min_max(
                    node_rect.min,
                    pos2(
                        (node_rect.left() + 2.5).min(node_rect.right()),
                        node_rect.bottom(),
                    ),
                ),
                2.0,
                accent,
            );
        }

        let viewport_rect = map_rect_for_scene(viewport_scene);
        if viewport_rect.width() > 0.0 && viewport_rect.height() > 0.0 {
            minimap_painter.rect_stroke(
                viewport_rect,
                2.0,
                Stroke::new(1.2_f32, Color32::from_rgb(130, 196, 245)),
            );
        }
    }
}
