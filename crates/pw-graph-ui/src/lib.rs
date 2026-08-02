//! egui canvas primitives. Backend mutations are returned as actions so the UI
//! never owns the driver or command stack.

use egui::{vec2, Pos2, Vec2};
use pw_graph_core::{Graph, LinkId, NodeId, PortId, PortType};
use std::collections::{BTreeMap, BTreeSet};

mod canvas;

#[derive(Clone, Debug, PartialEq)]
pub enum CanvasAction {
    Connect { output: PortId, input: PortId },
    Disconnect { link: LinkId },
    MoveNode { node: NodeId, position: [f32; 2] },
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MediaFilter {
    #[default]
    All,
    Audio,
    Video,
    Midi,
}

impl MediaFilter {
    pub const ALL: [Self; 4] = [Self::All, Self::Audio, Self::Video, Self::Midi];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Audio => "audio",
            Self::Video => "video",
            Self::Midi => "midi",
        }
    }

    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "audio" => Self::Audio,
            "video" => Self::Video,
            "midi" => Self::Midi,
            _ => Self::All,
        }
    }

    pub fn matches_port_type(self, port_type: PortType) -> bool {
        match self {
            Self::All => true,
            Self::Audio => port_type == PortType::Audio,
            Self::Video => port_type == PortType::Video,
            Self::Midi => matches!(port_type, PortType::MidiJack | PortType::MidiAlsa),
        }
    }

    pub fn matches_node(self, graph: &Graph, node_id: NodeId) -> bool {
        let Some(node) = graph.node(node_id) else {
            return false;
        };
        self == Self::All
            || node.ports.iter().any(|port_id| {
                graph
                    .port(*port_id)
                    .is_some_and(|port| self.matches_port_type(port.port_type))
            })
    }
}

/// Runtime audio data rendered by the canvas. The backend intentionally owns
/// collection and conversion; the UI only needs normalized levels and age.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeterReading {
    pub rms: f32,
    pub peak: f32,
    pub age_ms: u32,
    pub available: bool,
}

pub struct GraphCanvas {
    pub zoom: f32,
    pub node_text_scale: f32,
    pub pan: Vec2,
    pub sort_ports_by_name: bool,
    pub sort_ports_descending: bool,
    pub thumbnail_mode: bool,
    pub repel_overlapping_nodes: bool,
    pub connect_through_nodes: bool,
    pub media_filter: MediaFilter,
    pub meters: BTreeMap<NodeId, MeterReading>,
    pub pinned_meter: Option<PortId>,
    /// Node whose audio meter the pointer is revealing this frame. Recomputed
    /// by every [`GraphCanvas::show`] call.
    pub hovered_meter_node: Option<NodeId>,
    /// Set by the application when the backend may not measure levels at all,
    /// so the meter popover can say so instead of claiming no data exists.
    pub metering_disabled: bool,
    peak_hold: BTreeMap<NodeId, f32>,
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
            node_text_scale: 1.0,
            pan: vec2(24.0, 24.0),
            sort_ports_by_name: true,
            sort_ports_descending: false,
            thumbnail_mode: false,
            repel_overlapping_nodes: false,
            connect_through_nodes: false,
            media_filter: MediaFilter::All,
            meters: BTreeMap::new(),
            pinned_meter: None,
            hovered_meter_node: None,
            metering_disabled: false,
            peak_hold: BTreeMap::new(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use pw_graph_core::{Direction, Graph, Node, NodeType, Port, PortType};

    /// One audio node with a single source port, as the canvas would see it.
    fn metering_graph() -> Graph {
        let mut graph = Graph::default();
        graph
            .add_node(Node::new(NodeId(1), "Playback", NodeType::PipeWire))
            .unwrap();
        graph
            .add_port(Port::new(
                PortId(10),
                NodeId(1),
                "monitor_FL",
                Direction::Source,
                PortType::Audio,
            ))
            .unwrap();
        graph
    }

    #[test]
    fn canvas_has_expected_default_view() {
        let canvas = GraphCanvas::default();
        assert_eq!(canvas.zoom, 1.0);
        assert_eq!(canvas.selected_node, None);
        assert!(canvas.selected_nodes.is_empty());
    }

    #[test]
    fn idle_canvas_requests_no_meters() {
        let canvas = GraphCanvas::default();
        assert!(canvas.requested_meter_nodes(&metering_graph()).is_empty());
    }

    #[test]
    fn pinned_and_hovered_ports_request_their_nodes() {
        let graph = metering_graph();
        let canvas = GraphCanvas {
            pinned_meter: Some(PortId(10)),
            hovered_meter_node: Some(NodeId(4)),
            ..GraphCanvas::default()
        };
        assert_eq!(
            canvas.requested_meter_nodes(&graph),
            BTreeSet::from([NodeId(1), NodeId(4)])
        );
    }

    #[test]
    fn a_pinned_port_that_left_the_graph_requests_nothing() {
        let canvas = GraphCanvas {
            pinned_meter: Some(PortId(999)),
            ..GraphCanvas::default()
        };
        assert!(canvas.requested_meter_nodes(&metering_graph()).is_empty());
    }

    fn categorized_graph() -> Graph {
        let mut graph = Graph::default();
        for (id, name) in [(1, "Audio"), (2, "Video"), (3, "MIDI")] {
            graph
                .add_node(Node::new(NodeId(id), name, NodeType::PipeWire))
                .unwrap();
        }
        graph
            .add_port(Port::new(
                PortId(10),
                NodeId(1),
                "audio",
                Direction::Source,
                PortType::Audio,
            ))
            .unwrap();
        graph
            .add_port(Port::new(
                PortId(20),
                NodeId(2),
                "video",
                Direction::Source,
                PortType::Video,
            ))
            .unwrap();
        graph
            .add_port(Port::new(
                PortId(30),
                NodeId(3),
                "midi",
                Direction::Source,
                PortType::MidiJack,
            ))
            .unwrap();
        graph
    }

    #[test]
    fn media_filter_limits_nodes_and_ports() {
        let graph = categorized_graph();
        let mut canvas = GraphCanvas::default();
        assert_eq!(canvas.visible_counts(&graph), (3, 3, 0));

        canvas.media_filter = MediaFilter::Audio;
        assert_eq!(canvas.visible_node_ids(&graph), BTreeSet::from([NodeId(1)]));
        assert_eq!(canvas.visible_counts(&graph), (1, 1, 0));

        canvas.media_filter = MediaFilter::Midi;
        assert_eq!(canvas.visible_node_ids(&graph), BTreeSet::from([NodeId(3)]));
    }
}
