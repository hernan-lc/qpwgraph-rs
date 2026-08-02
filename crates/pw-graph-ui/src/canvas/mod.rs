//! Graph canvas: top-level input handling and per-frame orchestration.
//!
//! Node rendering lives in [`node`], link rendering in [`links`], port
//! grouping/matching in [`ports`], and name formatting in [`names`], so each
//! concern can be read (and tested) on its own instead of one long file.

use crate::{CanvasAction, GraphCanvas, LinkId, NodeId};
use egui::{pos2, vec2, Color32, FontId, Pos2, Rect, Sense, Shape, Stroke, Ui};
use pw_graph_core::{Graph, NodeType, PortType};
use pw_graph_i18n::I18n;
use std::collections::{BTreeSet, HashMap};

mod geometry;
mod links;
mod names;
mod node;
mod ports;

use geometry::bezier_points;
use names::display_port_name;
use node::NodeDrawContext;

const MINIMAP_PANEL_SIZE: egui::Vec2 = vec2(238.0, 164.0);
const MINIMAP_MARGIN: f32 = 12.0;
const MINIMAP_INNER_MARGIN: f32 = 8.0;
const MINIMAP_TITLE_HEIGHT: f32 = 17.0;
const MINIMAP_NODE_SIZE: egui::Vec2 = vec2(244.0, 62.0);

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

fn minimap_link_color(port_type: PortType) -> Color32 {
    match port_type {
        PortType::Audio => Color32::from_rgb(106, 187, 147),
        PortType::Video => Color32::from_rgb(92, 157, 218),
        PortType::MidiJack => Color32::from_rgb(213, 111, 123),
        PortType::MidiAlsa => Color32::from_rgb(166, 126, 208),
        PortType::Unknown => Color32::from_rgb(138, 151, 169),
    }
}

fn node_accent(graph: &Graph, node_id: NodeId, node_type: NodeType) -> Color32 {
    graph
        .node(node_id)
        .and_then(|node| {
            node.ports
                .iter()
                .filter_map(|port_id| graph.port(*port_id))
                .map(|port| minimap_link_color(port.port_type))
                .next()
        })
        .unwrap_or(match node_type {
            NodeType::PipeWire => Color32::from_rgb(91, 172, 224),
            NodeType::AlsaMidi => Color32::from_rgb(180, 128, 220),
            NodeType::Unknown => Color32::from_rgb(153, 163, 175),
        })
}

impl GraphCanvas {
    pub fn show(&mut self, ui: &mut Ui, graph: &Graph, i18n: &I18n) -> Vec<CanvasAction> {
        self.show_with_keyboard_shortcuts(ui, graph, i18n, true)
    }

    pub fn show_with_keyboard_shortcuts(
        &mut self,
        ui: &mut Ui,
        graph: &Graph,
        i18n: &I18n,
        keyboard_shortcuts_enabled: bool,
    ) -> Vec<CanvasAction> {
        self.update_peak_holds();
        self.hovered_meter_node = None;
        let visible_node_ids = self.visible_node_ids(graph);
        self.prune_hidden_state(graph, &visible_node_ids);
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

        painter.rect_filled(rect, 0.0, Color32::from_rgb(25, 28, 34));
        self.draw_grid(&painter, rect);
        if canvas_response.has_focus() {
            painter.rect_stroke(rect, 0.0, Stroke::new(1.5_f32, Color32::LIGHT_BLUE));
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

        if !self.thumbnail_mode {
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
            );
        }

        for node in graph
            .nodes
            .values()
            .filter(|node| visible_node_ids.contains(&node.id))
        {
            let mut context = NodeDrawContext {
                rect,
                graph,
                i18n,
                anchors: &mut anchors,
                actions: &mut actions,
            };
            self.draw_node(ui, painter.clone(), node, &mut context);
        }

        // Handle this after the canvas has processed pointer input. This lets
        // a link be selected and deleted during the same egui frame, and keeps
        // the action in the same path as the context-menu disconnect.
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
            self.draw_minimap(&painter, rect, graph, i18n);
        }

        actions
    }

    /// Draw a compact overview of the complete graph, independently of the
    /// active media filter and search query. The viewport outline makes it
    /// clear which part of the full scene is currently visible.
    fn draw_minimap(&self, painter: &egui::Painter, canvas_rect: Rect, graph: &Graph, i18n: &I18n) {
        let panel_width = MINIMAP_PANEL_SIZE
            .x
            .min((canvas_rect.width() - 16.0).max(0.0));
        let panel_height = MINIMAP_PANEL_SIZE
            .y
            .min((canvas_rect.height() - 16.0).max(0.0));
        if panel_width <= 0.0 || panel_height <= 0.0 {
            return;
        }
        let panel_rect = Rect::from_min_size(
            pos2(
                canvas_rect.right() - panel_width - MINIMAP_MARGIN,
                canvas_rect.bottom() - panel_height - MINIMAP_MARGIN,
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
            panel_rect.left_top() + vec2(MINIMAP_INNER_MARGIN, 5.0),
            egui::Align2::LEFT_TOP,
            i18n.text("toolbar.minimap"),
            FontId::proportional(11.0),
            Color32::from_rgb(205, 216, 230),
        );

        let content_rect = Rect::from_min_max(
            pos2(
                panel_rect.left() + MINIMAP_INNER_MARGIN,
                panel_rect.top() + MINIMAP_TITLE_HEIGHT,
            ),
            pos2(
                panel_rect.right() - MINIMAP_INNER_MARGIN,
                panel_rect.bottom() - MINIMAP_INNER_MARGIN,
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
        for node in graph.nodes.values() {
            extend_rect(
                &mut scene_bounds,
                Rect::from_min_size(pos2(node.position[0], node.position[1]), MINIMAP_NODE_SIZE),
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
            let source_center =
                map_point(pos2(source.position[0], source.position[1]) + MINIMAP_NODE_SIZE * 0.5);
            let destination_center = map_point(
                pos2(destination.position[0], destination.position[1]) + MINIMAP_NODE_SIZE * 0.5,
            );
            minimap_painter.line_segment(
                [source_center, destination_center],
                Stroke::new(1.0_f32, minimap_link_color(output.port_type)),
            );
        }

        for node in graph.nodes.values() {
            let node_rect = map_rect_for_scene(Rect::from_min_size(
                pos2(node.position[0], node.position[1]),
                MINIMAP_NODE_SIZE,
            ));
            let fill = if self.selected_nodes.contains(&node.id) {
                Color32::from_rgb(78, 112, 145)
            } else {
                Color32::from_rgb(48, 58, 72)
            };
            let accent = node_accent(graph, node.id, node.node_type);
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
        painter.add(Shape::line(
            points.clone(),
            Stroke::new(4.0_f32, Color32::BLACK),
        ));
        painter.add(Shape::line(
            points,
            Stroke::new(2.0_f32, Color32::LIGHT_GREEN),
        ));
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

    /// Nodes the user is currently looking at a meter for: the pinned monitor
    /// and whatever audio port the pointer is revealing. On-demand metering
    /// attaches a helper stream only for these, so an idle window measures
    /// nothing and leaves the daemon's audio configuration alone.
    pub fn requested_meter_nodes(&self, graph: &Graph) -> BTreeSet<NodeId> {
        let pinned = self
            .pinned_meter
            .and_then(|port_id| graph.port(port_id))
            .map(|port| port.node_id);
        pinned.into_iter().chain(self.hovered_meter_node).collect()
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
