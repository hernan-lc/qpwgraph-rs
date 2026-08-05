//! Graph canvas: top-level input handling and per-frame orchestration.
//!
//! Node rendering lives in [`node`], link rendering in [`links`], port
//! grouping/matching in [`ports`], and name formatting in [`names`], so each
//! concern can be read (and tested) on its own instead of one long file.

use crate::{CanvasAction, GraphCanvas, LinkId, NodeId, UiDocument};
use egui::{pos2, vec2, Color32, FontId, Rect, Sense, Stroke, Ui};
use pw_graph_core::Graph;
use pw_graph_i18n::I18n;
use std::collections::{BTreeSet, HashMap};

mod geometry;
mod icons;
mod links;
mod minimap;
mod names;
mod node;
mod ports;

use geometry::{bezier_points, paint_bezier};
use names::display_port_name;
use node::NodeDrawContext;

impl GraphCanvas {
    pub fn show(
        &mut self,
        ui: &mut Ui,
        graph: &Graph,
        i18n: &I18n,
        document: &mut UiDocument,
    ) -> Vec<CanvasAction> {
        self.show_with_keyboard_shortcuts(ui, graph, i18n, document, true)
    }

    pub fn show_with_keyboard_shortcuts(
        &mut self,
        ui: &mut Ui,
        graph: &Graph,
        i18n: &I18n,
        document: &mut UiDocument,
        keyboard_shortcuts_enabled: bool,
    ) -> Vec<CanvasAction> {
        self.update_peak_holds();
        let visible_node_ids = self.visible_node_ids(graph);
        self.prune_hidden_state(graph, &visible_node_ids);
        let rect = ui.available_rect_before_wrap();
        let canvas_response = ui.allocate_rect(rect, Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        let mut actions = Vec::new();
        let mut anchors = HashMap::new();
        let pointer_pos = ui.input(|input| input.pointer.interact_pos());
        let pointer_over_node = pointer_pos.is_some_and(|pointer| {
            graph
                .nodes
                .values()
                .filter(|node| visible_node_ids.contains(&node.id))
                .any(|node| self.node_rect(rect, graph, node).contains(pointer))
        });

        if self.repel_overlapping_nodes
            && self.dragging_node.is_none()
            && self.selection_start.is_none()
            && !pointer_over_node
            && !self.thumbnail_mode
        {
            self.repel_overlaps(rect, graph, &visible_node_ids, &mut actions);
        }

        if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.pending_outputs = None;
            self.pending_node_connect = None;
            self.selection_start = None;
            self.selection_current = None;
            self.selected_link = None;
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
                    .filter(|node| visible_node_ids.contains(&node.id))
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

        // Ctrl/Cmd+scroll zooms; Shift+scroll pans horizontally; a plain
        // scroll pans (vertically, or horizontally too if the device already
        // reports a horizontal component, e.g. a trackpad swipe).
        let pointer_over_canvas = ui
            .input(|input| input.pointer.hover_pos())
            .is_some_and(|pointer| rect.contains(pointer));
        if pointer_over_canvas {
            let scroll = ui.input(|input| input.raw_scroll_delta);
            let modifiers = ui.input(|input| input.modifiers);
            if scroll.x.abs() > f32::EPSILON || scroll.y.abs() > f32::EPSILON {
                if modifiers.command {
                    self.zoom = (self.zoom * (1.0 + scroll.y * 0.001)).clamp(0.35, 2.5);
                } else if modifiers.shift {
                    // Some backends already turn Shift+wheel into a
                    // horizontal delta; fall back to the vertical one so the
                    // shortcut works either way.
                    let delta = if scroll.x.abs() > scroll.y.abs() {
                        scroll.x
                    } else {
                        scroll.y
                    };
                    self.pan.x += delta;
                } else {
                    self.pan += scroll;
                }
            }
        }

        self.clamp_pan(rect, graph, &visible_node_ids);
        painter.rect_filled(rect, 0.0, Color32::from_rgb(25, 28, 34));
        self.draw_grid(&painter, rect);
        if canvas_response.has_focus() {
            painter.rect_stroke(rect, 0.0, Stroke::new(1.5_f32, Color32::LIGHT_BLUE));
        }

        let pointer_over_link = if !self.thumbnail_mode {
            self.draw_links(
                ui,
                &painter,
                rect,
                graph,
                i18n,
                &visible_node_ids,
                pointer_pos,
                pointer_over_node,
                &mut actions,
            )
        } else {
            false
        };

        for node in graph
            .nodes
            .values()
            .filter(|node| visible_node_ids.contains(&node.id))
        {
            let mut context = NodeDrawContext {
                rect,
                graph,
                i18n,
                document,
                anchors: &mut anchors,
                actions: &mut actions,
            };
            self.draw_node(ui, painter.clone(), node, &mut context);
        }

        // A pending connection is a transient gesture. Clicking the canvas
        // itself cancels it, while clicks on nodes, ports, or links retain
        // their normal connection/selection behavior.
        if canvas_response.clicked() && !pointer_over_node && !pointer_over_link {
            self.pending_outputs = None;
            self.pending_node_connect = None;
        }

        // Handle this after the canvas has processed pointer input. This lets
        // a link be selected and deleted during the same egui frame, and keeps
        // the action in the same path as the context-menu disconnect. With no
        // link selected, Delete removes selected effect nodes; normal PipeWire
        // nodes remain protected from accidental removal.
        if keyboard_shortcuts_enabled
            && ui.input(|input| {
                input.key_pressed(egui::Key::Delete) || input.key_pressed(egui::Key::Backspace)
            })
        {
            if let Some(link) = self.selected_link.take() {
                let links = self.selected_links_for(graph, link);
                if links.len() > 1 {
                    actions.push(CanvasAction::DisconnectMany { links });
                } else {
                    actions.push(CanvasAction::Disconnect { link });
                }
            } else {
                let effects: Vec<_> = self
                    .selected_nodes
                    .iter()
                    .copied()
                    .filter(|node_id| {
                        graph
                            .node(*node_id)
                            .is_some_and(|node| node.node_type == pw_graph_core::NodeType::Effect)
                    })
                    .collect();
                for node in effects {
                    actions.push(CanvasAction::RemoveEffect { node });
                    self.selected_nodes.remove(&node);
                }
                self.selected_node = self.selected_nodes.iter().next().copied();
            }
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

        self.draw_pending_connection(ui, &painter, graph, i18n, &anchors);

        if self.minimap_visible {
            self.draw_minimap(&painter, rect, graph, i18n, &visible_node_ids);
        }

        actions
    }

    /// The bezier preview drawn from a pending Advanced-mode single-port (or
    /// Easy-mode grouped-row) connection to wherever the pointer is.
    fn draw_pending_connection(
        &self,
        ui: &Ui,
        painter: &egui::Painter,
        graph: &Graph,
        i18n: &I18n,
        anchors: &HashMap<crate::PortId, egui::Pos2>,
    ) {
        let Some(output_ids) = &self.pending_outputs else {
            return;
        };
        let Some(&first_id) = output_ids.first() else {
            return;
        };
        let Some(output) = graph.ports.get(&first_id) else {
            return;
        };
        let Some(start) = anchors.get(&first_id).copied() else {
            return;
        };
        let end = ui.input(|input| input.pointer.hover_pos()).unwrap_or(start);
        let points = bezier_points(start, end, 0.0);
        paint_bezier(
            painter,
            points,
            4.0,
            Color32::BLACK,
            2.0,
            Color32::LIGHT_GREEN,
        );
        let label = if output_ids.len() > 1 {
            i18n.format(
                "canvas.pending_connection_group",
                &[
                    ("action", i18n.text("canvas.connect_hint")),
                    ("count", output_ids.len().to_string()),
                ],
            )
        } else {
            i18n.format(
                "canvas.pending_connection",
                &[
                    ("action", i18n.text("canvas.connect_hint")),
                    ("port", display_port_name(&output.name, i18n)),
                ],
            )
        };
        painter.text(
            start + vec2(8.0, -22.0),
            egui::Align2::LEFT_TOP,
            label,
            FontId::proportional(12.0 * self.zoom * self.node_text_scale.clamp(0.80, 2.0)),
            Color32::LIGHT_GREEN,
        );
    }

    pub fn visible_node_ids(&self, graph: &Graph) -> BTreeSet<NodeId> {
        graph
            .nodes
            .values()
            .filter(|node| self.media_filter.matches_node(graph, node.id))
            .filter(|node| self.search_matches_node(graph, node.id))
            .map(|node| node.id)
            .collect()
    }

    fn clamp_pan(&mut self, rect: Rect, graph: &Graph, visible_node_ids: &BTreeSet<NodeId>) {
        let Some(scene_bounds) = self.visible_scene_bounds(graph, visible_node_ids) else {
            return;
        };
        let viewport_size = rect.size() / self.zoom;
        let view_left = clamp_scene_view(
            -self.pan.x / self.zoom,
            viewport_size.x,
            scene_bounds.left(),
            scene_bounds.right(),
        );
        let view_top = clamp_scene_view(
            -self.pan.y / self.zoom,
            viewport_size.y,
            scene_bounds.top(),
            scene_bounds.bottom(),
        );
        self.pan = vec2(-view_left * self.zoom, -view_top * self.zoom);
    }

    fn visible_scene_bounds(
        &self,
        graph: &Graph,
        visible_node_ids: &BTreeSet<NodeId>,
    ) -> Option<Rect> {
        let mut bounds: Option<Rect> = None;
        for node in graph
            .nodes
            .values()
            .filter(|node| visible_node_ids.contains(&node.id))
        {
            let candidate = Rect::from_min_size(
                egui::pos2(node.position[0], node.position[1]),
                self.node_scene_size(graph, node),
            );
            bounds = Some(match bounds {
                Some(current) => current.union(candidate),
                None => candidate,
            });
        }
        bounds.map(|bounds| bounds.expand(SCENE_MARGIN))
    }

    fn search_matches_node(&self, graph: &Graph, node_id: NodeId) -> bool {
        let query = self.search_query.trim().to_ascii_lowercase();
        if query.is_empty() {
            return true;
        }
        let Some(node) = graph.node(node_id) else {
            return false;
        };
        node.name.to_ascii_lowercase().contains(&query)
            || node
                .ports
                .iter()
                .any(|port_id| self.search_matches_port(graph, *port_id))
    }

    /// A node-name match keeps all of that node's ports visible. A port-name
    /// match keeps only the matching port rows visible, which makes the search
    /// useful for finding a single channel in a large graph.
    pub(super) fn search_matches_port(
        &self,
        graph: &Graph,
        port_id: pw_graph_core::PortId,
    ) -> bool {
        let query = self.search_query.trim().to_ascii_lowercase();
        if query.is_empty() {
            return true;
        }
        let Some(port) = graph.port(port_id) else {
            return false;
        };
        graph
            .node(port.node_id)
            .is_some_and(|node| node.name.to_ascii_lowercase().contains(&query))
            || port.name.to_ascii_lowercase().contains(&query)
    }

    pub fn selected_link(&self) -> Option<LinkId> {
        self.selected_link
    }

    /// Returns the selected link and, in Easy mode, its matching node-to-node
    /// links. Advanced mode intentionally keeps selection port-specific.
    pub fn selected_links(&self, graph: &Graph) -> Vec<LinkId> {
        self.selected_link
            .map(|link| self.selected_links_for(graph, link))
            .unwrap_or_default()
    }

    pub(super) fn selected_links_for(&self, graph: &Graph, selected: LinkId) -> Vec<LinkId> {
        let Some(selected_link) = graph.link(selected) else {
            return Vec::new();
        };
        if self.connect_mode != crate::ConnectMode::Easy {
            return vec![selected];
        }
        let (Some(selected_source), Some(selected_destination)) = (
            graph.port(selected_link.output_port),
            graph.port(selected_link.input_port),
        ) else {
            return vec![selected];
        };
        graph
            .links
            .values()
            .filter(|link| {
                let (Some(source), Some(destination)) =
                    (graph.port(link.output_port), graph.port(link.input_port))
                else {
                    return false;
                };
                source.node_id == selected_source.node_id
                    && destination.node_id == selected_destination.node_id
                    && source.port_type == selected_source.port_type
                    && destination.port_type == selected_destination.port_type
            })
            .map(|link| link.id)
            .collect()
    }

    pub fn clear_selected_link(&mut self) {
        self.selected_link = None;
    }

    pub fn visible_counts(&self, graph: &Graph) -> (usize, usize, usize) {
        let visible_nodes = self.visible_node_ids(graph);
        let ports = graph
            .ports
            .values()
            .filter(|port| {
                visible_nodes.contains(&port.node_id)
                    && self.media_filter.matches_port_type(port.port_type)
                    && self.search_matches_port(graph, port.id)
            })
            .count();
        let links = graph
            .links
            .values()
            .filter(|link| {
                let Some(source) = graph.port(link.output_port) else {
                    return false;
                };
                let Some(destination) = graph.port(link.input_port) else {
                    return false;
                };
                visible_nodes.contains(&source.node_id)
                    && visible_nodes.contains(&destination.node_id)
                    && self.media_filter.matches_port_type(source.port_type)
                    && self.media_filter.matches_port_type(destination.port_type)
                    && self.search_matches_port(graph, source.id)
                    && self.search_matches_port(graph, destination.id)
            })
            .count();
        (visible_nodes.len(), ports, links)
    }

    fn prune_hidden_state(&mut self, graph: &Graph, visible_node_ids: &BTreeSet<NodeId>) {
        self.node_appearances
            .retain(|node_id, _| graph.nodes.contains_key(node_id));
        self.node_audio
            .retain(|node_id, _| graph.nodes.contains_key(node_id));
        self.node_name_drafts
            .retain(|node_id, _| graph.nodes.contains_key(node_id));
        self.selected_nodes
            .retain(|node_id| visible_node_ids.contains(node_id));
        if self
            .selected_node
            .is_some_and(|node_id| !visible_node_ids.contains(&node_id))
        {
            self.selected_node = self.selected_nodes.iter().next().copied();
        }
        if self.pending_outputs.as_ref().is_some_and(|port_ids| {
            port_ids.iter().any(|port_id| {
                graph.port(*port_id).is_none_or(|port| {
                    !visible_node_ids.contains(&port.node_id)
                        || !self.media_filter.matches_port_type(port.port_type)
                        || !self.search_matches_port(graph, port.id)
                })
            })
        }) {
            self.pending_outputs = None;
        }
        if self
            .pending_node_connect
            .is_some_and(|node_id| !visible_node_ids.contains(&node_id))
        {
            self.pending_node_connect = None;
        }
        if self.selected_link.is_some_and(|link_id| {
            graph.link(link_id).is_none_or(|link| {
                let Some(source) = graph.port(link.output_port) else {
                    return true;
                };
                let Some(destination) = graph.port(link.input_port) else {
                    return true;
                };
                !visible_node_ids.contains(&source.node_id)
                    || !visible_node_ids.contains(&destination.node_id)
                    || !self.media_filter.matches_port_type(source.port_type)
                    || !self.media_filter.matches_port_type(destination.port_type)
                    || !self.search_matches_port(graph, source.id)
                    || !self.search_matches_port(graph, destination.id)
            })
        }) {
            self.selected_link = None;
        }
    }

    pub fn meter_peak_hold(&self, node_id: NodeId, fallback: f32) -> f32 {
        self.peak_hold.get(&node_id).copied().unwrap_or(fallback)
    }

    /// Audio nodes represented by the current filtered canvas, plus a pinned
    /// monitor that may sit outside the active filter. The application only
    /// submits this set while its native window is visible.
    pub fn requested_meter_nodes(&self, graph: &Graph) -> BTreeSet<NodeId> {
        let pinned = self
            .pinned_meter
            .and_then(|port_id| graph.port(port_id))
            .map(|port| port.node_id);
        self.visible_node_ids(graph)
            .into_iter()
            .filter(|node_id| {
                graph.node(*node_id).is_some_and(|node| {
                    node.ports.iter().any(|port_id| {
                        graph
                            .port(*port_id)
                            .is_some_and(|port| port.port_type == pw_graph_core::PortType::Audio)
                    })
                })
            })
            .chain(pinned)
            .collect()
    }

    fn update_peak_holds(&mut self) {
        self.peak_hold
            .retain(|node_id, _| self.meters.contains_key(node_id));
        for (node_id, reading) in &self.meters {
            let hold = self.peak_hold.entry(*node_id).or_insert(0.0);
            *hold = (*hold - 0.012).max(reading.peak).clamp(0.0, 1.0);
        }
    }

    fn repel_overlaps(
        &self,
        rect: Rect,
        graph: &Graph,
        visible_node_ids: &BTreeSet<NodeId>,
        actions: &mut Vec<CanvasAction>,
    ) {
        let mut occupied = Vec::new();
        for node in graph
            .nodes
            .values()
            .filter(|node| visible_node_ids.contains(&node.id))
        {
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
}

const SCENE_MARGIN: f32 = 160.0;

fn clamp_scene_view(view_start: f32, viewport_size: f32, scene_min: f32, scene_max: f32) -> f32 {
    let scene_size = scene_max - scene_min;
    if viewport_size >= scene_size {
        (scene_min + scene_max - viewport_size) * 0.5
    } else {
        view_start.clamp(scene_min, scene_max - viewport_size)
    }
}

#[cfg(test)]
mod bounds_tests {
    use super::clamp_scene_view;

    #[test]
    fn scene_view_is_centered_when_content_is_smaller_than_viewport() {
        assert_eq!(clamp_scene_view(-100.0, 200.0, -20.0, 60.0), -80.0);
    }

    #[test]
    fn scene_view_is_clamped_to_dynamic_content_bounds() {
        assert_eq!(clamp_scene_view(-100.0, 40.0, -20.0, 60.0), -20.0);
        assert_eq!(clamp_scene_view(100.0, 40.0, -20.0, 60.0), 20.0);
    }
}
