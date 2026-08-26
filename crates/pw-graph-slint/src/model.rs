//! Framework-neutral state projected into Slint models.

use pw_graph_backend::{GraphDriver, NodeAudioState, NodeCapabilities};
use pw_graph_config::AppConfig;
use pw_graph_core::{
    Direction, Graph, LinkId, Node, NodeAppearance, NodeId, NodeType, Port, PortId, PortType,
};
use std::collections::{BTreeMap, BTreeSet, HashMap};

const NODE_WIDTH: f32 = 244.0;
const NODE_HEADER_HEIGHT: f32 = 42.0;
const COLLAPSED_NODE_HEIGHT: f32 = 50.0;
const PORT_ROW_HEIGHT: f32 = 25.0;
const AUDIO_CONTROLS_HEIGHT: f32 = 42.0;
const RELAY_SOURCE_NAME: &str = "qpwgraph-rs.relay.source";
const RELAY_SINK_NAME: &str = "qpwgraph-rs.relay.sink";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum MeterState {
    #[default]
    Unavailable,
    Disabled,
    Waiting,
    Live,
    Demo,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct MeterReading {
    pub(crate) rms: f32,
    pub(crate) peak: f32,
    pub(crate) state: MeterState,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum MediaFilter {
    #[default]
    All,
    Audio,
    Video,
    Midi,
}

impl MediaFilter {
    pub(crate) fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "audio" => Self::Audio,
            "video" => Self::Video,
            "midi" => Self::Midi,
            _ => Self::All,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Audio => "audio",
            Self::Video => "video",
            Self::Midi => "midi",
        }
    }

    pub(crate) fn matches_port_type(self, port_type: PortType) -> bool {
        match self {
            Self::All => true,
            Self::Audio => port_type == PortType::Audio,
            Self::Video => port_type == PortType::Video,
            Self::Midi => matches!(port_type, PortType::MidiJack | PortType::MidiAlsa),
        }
    }

    fn matches_node(self, graph: &Graph, node: &Node) -> bool {
        self == Self::All
            || node.ports.iter().any(|port_id| {
                graph
                    .port(*port_id)
                    .is_some_and(|port| self.matches_port_type(port.port_type))
            })
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum ConnectMode {
    #[default]
    Advanced,
    Easy,
}

impl ConnectMode {
    pub(crate) fn parse(value: &str) -> Self {
        if value.eq_ignore_ascii_case("easy") {
            Self::Easy
        } else {
            Self::Advanced
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Advanced => "advanced",
            Self::Easy => "easy",
        }
    }
}

/// Maps opaque backend IDs to nonzero Slint `int` values. It never casts the
/// original u64 values, so high-bit ALSA IDs and future PipeWire IDs remain
/// safe in the UI.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SlintIdMap {
    next: i32,
    nodes: BTreeMap<NodeId, i32>,
    ports: BTreeMap<PortId, i32>,
    links: BTreeMap<LinkId, i32>,
}

impl SlintIdMap {
    pub(crate) fn rebuild(&mut self, graph: &Graph) {
        self.nodes.retain(|id, _| graph.nodes.contains_key(id));
        self.ports.retain(|id, _| graph.ports.contains_key(id));
        self.links.retain(|id, _| graph.links.contains_key(id));
        for id in graph.nodes.keys() {
            self.allocate_node(*id);
        }
        for id in graph.ports.keys() {
            self.allocate_port(*id);
        }
        for id in graph.links.keys() {
            self.allocate_link(*id);
        }
    }

    pub(crate) fn node(&self, id: NodeId) -> Option<i32> {
        self.nodes.get(&id).copied()
    }

    pub(crate) fn port(&self, id: PortId) -> Option<i32> {
        self.ports.get(&id).copied()
    }

    pub(crate) fn port_id(&self, id: i32) -> Option<PortId> {
        self.ports
            .iter()
            .find_map(|(port_id, mapped)| (*mapped == id).then_some(*port_id))
    }

    pub(crate) fn link(&self, id: LinkId) -> Option<i32> {
        self.links.get(&id).copied()
    }

    pub(crate) fn node_id(&self, id: i32) -> Option<NodeId> {
        self.nodes
            .iter()
            .find_map(|(node_id, mapped)| (*mapped == id).then_some(*node_id))
    }

    pub(crate) fn link_id(&self, id: i32) -> Option<LinkId> {
        self.links
            .iter()
            .find_map(|(link_id, mapped)| (*mapped == id).then_some(*link_id))
    }

    fn allocate_node(&mut self, id: NodeId) {
        if !self.nodes.contains_key(&id) {
            let next = self.next_id();
            self.nodes.insert(id, next);
        }
    }

    fn allocate_port(&mut self, id: PortId) {
        if !self.ports.contains_key(&id) {
            let next = self.next_id();
            self.ports.insert(id, next);
        }
    }

    fn allocate_link(&mut self, id: LinkId) {
        if !self.links.contains_key(&id) {
            let next = self.next_id();
            self.links.insert(id, next);
        }
    }

    fn next_id(&mut self) -> i32 {
        self.next = self.next.max(1);
        let id = self.next;
        self.next = self.next.checked_add(1).unwrap_or(1);
        id
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PortGroupView {
    pub(crate) pin_id: i32,
    pub(crate) ports: Vec<PortId>,
    pub(crate) label: String,
    pub(crate) direction: Direction,
    pub(crate) port_type: PortType,
    pub(crate) color: [u8; 4],
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NodeView {
    pub(crate) id: i32,
    pub(crate) node_id: NodeId,
    pub(crate) title: String,
    pub(crate) node_type: NodeType,
    pub(crate) position: [f32; 2],
    pub(crate) width: f32,
    pub(crate) height: f32,
    pub(crate) selected: bool,
    pub(crate) collapsed: bool,
    pub(crate) thumbnail: bool,
    pub(crate) font_scale: f32,
    pub(crate) appearance: NodeAppearance,
    pub(crate) has_audio_controls: bool,
    /// Audio state and per-node capability, both read from the backend. The
    /// UI keeps no copy of its own: whatever is here is what the backend last
    /// reported, and an unknown value stays unknown.
    pub(crate) audio: NodeAudioProfile,
    pub(crate) meter: MeterReading,
    pub(crate) ports: Vec<PortGroupView>,
}

/// What one backend says about a node's audio controls.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct NodeAudioProfile {
    pub(crate) state: NodeAudioState,
    pub(crate) capabilities: NodeCapabilities,
}

/// Stand-in for a backend that supports everything, used where a test cares
/// about projection rather than about capability gating.
#[cfg(test)]
pub(crate) fn fully_capable_audio_profiles(graph: &Graph) -> BTreeMap<NodeId, NodeAudioProfile> {
    graph
        .nodes
        .keys()
        .map(|node_id| {
            (
                *node_id,
                NodeAudioProfile {
                    state: NodeAudioState::readable(1.0, false),
                    capabilities: NodeCapabilities::FULL,
                },
            )
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LinkView {
    pub(crate) id: i32,
    pub(crate) link_id: LinkId,
    pub(crate) start_pin_id: i32,
    pub(crate) end_pin_id: i32,
    pub(crate) color: [u8; 4],
    pub(crate) selected: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct GraphSnapshot {
    pub(crate) nodes: Vec<NodeView>,
    pub(crate) links: Vec<LinkView>,
}

pub(crate) struct UiGraphState {
    pub(crate) zoom: f32,
    pub(crate) pan: [f32; 2],
    pub(crate) node_text_scale: f32,
    pub(crate) sort_ports_by_name: bool,
    pub(crate) sort_ports_descending: bool,
    pub(crate) thumbnail_mode: bool,
    pub(crate) minimap_visible: bool,
    pub(crate) connect_mode: ConnectMode,
    pub(crate) media_filter: MediaFilter,
    pub(crate) search_query: String,
    pub(crate) relay_nodes_visible: bool,
    pub(crate) selected_nodes: BTreeSet<NodeId>,
    pub(crate) selected_links: BTreeSet<LinkId>,
    pub(crate) ids: SlintIdMap,
    local_positions: BTreeMap<NodeId, [f32; 2]>,
    local_appearances: BTreeMap<NodeId, NodeAppearance>,
}

impl UiGraphState {
    pub(crate) fn from_config(config: &AppConfig) -> Self {
        Self {
            zoom: config.zoom.clamp(0.35, 2.5),
            pan: [24.0, 24.0],
            node_text_scale: config.node_text_scale.clamp(0.8, 2.0),
            sort_ports_by_name: config.sort_type != "id",
            sort_ports_descending: config.sort_order == "descending",
            thumbnail_mode: config.thumbnail_view,
            minimap_visible: config.minimap_visible,
            connect_mode: ConnectMode::parse(&config.connect_mode),
            media_filter: MediaFilter::parse(&config.media_filter),
            search_query: config.graph_search.clone(),
            relay_nodes_visible: false,
            selected_nodes: BTreeSet::new(),
            selected_links: BTreeSet::new(),
            ids: SlintIdMap::default(),
            local_positions: BTreeMap::new(),
            local_appearances: BTreeMap::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn snapshot(&mut self, graph: &Graph, config: &AppConfig) -> GraphSnapshot {
        self.snapshot_with_meters(
            graph,
            config,
            &BTreeMap::new(),
            MeterState::Unavailable,
            &fully_capable_audio_profiles(graph),
        )
    }

    pub(crate) fn snapshot_with_meters(
        &mut self,
        graph: &Graph,
        config: &AppConfig,
        meters: &BTreeMap<NodeId, MeterReading>,
        meter_fallback: MeterState,
        audio_profiles: &BTreeMap<NodeId, NodeAudioProfile>,
    ) -> GraphSnapshot {
        self.ids.rebuild(graph);
        self.local_positions
            .retain(|id, _| graph.nodes.contains_key(id));
        self.local_appearances
            .retain(|id, _| graph.nodes.contains_key(id));

        let visible: BTreeSet<_> = graph
            .nodes
            .values()
            .filter(|node| self.relay_nodes_visible || !is_relay_node(node))
            .filter(|node| self.media_filter.matches_node(graph, node))
            .filter(|node| self.search_matches(graph, node))
            .map(|node| node.id)
            .collect();
        self.selected_nodes.retain(|id| visible.contains(id));
        self.selected_links
            .retain(|id| graph.links.contains_key(id));

        let appearances = configured_appearances(graph, config);
        let positions = self.effective_positions(graph, config, &appearances);
        let mut pin_groups = HashMap::<PortId, i32>::new();
        let mut nodes = Vec::new();

        for node in graph
            .nodes
            .values()
            .filter(|node| visible.contains(&node.id))
        {
            let appearance = self
                .local_appearances
                .get(&node.id)
                .cloned()
                .unwrap_or_else(|| appearances.get(&node.id).cloned().unwrap_or_default());
            let ports = self.project_ports(graph, node);
            for port in &ports {
                for id in &port.ports {
                    pin_groups.insert(*id, port.pin_id);
                }
            }
            // Which controls exist is the backend's call, not a guess from the
            // node type: a Windows application session and a Windows endpoint
            // are both "audio nodes" but expose different controls.
            let audio = audio_profiles.get(&node.id).copied().unwrap_or_default();
            let has_audio_controls = audio.capabilities.has_any_control()
                && node.ports.iter().any(|id| {
                    graph
                        .port(*id)
                        .is_some_and(|port| port.port_type == PortType::Audio)
                });
            let collapsed = appearance.collapsed;
            let thumbnail = self.thumbnail_mode;
            let height = node_height(thumbnail, collapsed, has_audio_controls, ports.len());
            nodes.push(NodeView {
                id: self.ids.node(node.id).unwrap_or_default(),
                node_id: node.id,
                title: appearance
                    .custom_name
                    .clone()
                    .unwrap_or_else(|| node.name.clone()),
                node_type: node.node_type,
                position: positions.get(&node.id).copied().unwrap_or(node.position),
                width: NODE_WIDTH,
                height,
                selected: self.selected_nodes.contains(&node.id),
                collapsed,
                thumbnail,
                font_scale: self.node_text_scale,
                appearance,
                has_audio_controls,
                audio,
                meter: meters.get(&node.id).copied().unwrap_or(MeterReading {
                    state: meter_fallback,
                    ..MeterReading::default()
                }),
                ports,
            });
        }

        let links = if !self.thumbnail_mode {
            graph
                .links
                .values()
                .filter_map(|link| {
                    let output = graph.port(link.output_port)?;
                    let input = graph.port(link.input_port)?;
                    (visible.contains(&output.node_id) && visible.contains(&input.node_id))
                        .then_some(LinkView {
                            id: self.ids.link(link.id)?,
                            link_id: link.id,
                            start_pin_id: *pin_groups.get(&link.output_port)?,
                            end_pin_id: *pin_groups.get(&link.input_port)?,
                            color: link_color(output.port_type, output.direction, &output.name),
                            selected: self.selected_links.contains(&link.id),
                        })
                })
                .collect()
        } else {
            Vec::new()
        };

        GraphSnapshot { nodes, links }
    }

    pub(crate) fn select_node(&mut self, node_id: i32, shift: bool) {
        let Some(node_id) = self.ids.node_id(node_id) else {
            return;
        };
        if !shift {
            self.selected_nodes.clear();
            self.selected_links.clear();
        }
        if shift && !self.selected_nodes.insert(node_id) {
            self.selected_nodes.remove(&node_id);
        } else {
            self.selected_nodes.insert(node_id);
        }
    }

    pub(crate) fn select_link(&mut self, link_id: i32, shift: bool) {
        let Some(link_id) = self.ids.link_id(link_id) else {
            return;
        };
        if !shift {
            self.selected_nodes.clear();
            self.selected_links.clear();
        }
        if shift && !self.selected_links.insert(link_id) {
            self.selected_links.remove(&link_id);
        } else {
            self.selected_links.insert(link_id);
        }
    }

    pub(crate) fn clear_selection(&mut self) {
        self.selected_nodes.clear();
        self.selected_links.clear();
    }

    pub(crate) fn select_box(
        &mut self,
        snapshot: &GraphSnapshot,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        shift: bool,
    ) {
        if !shift {
            self.clear_selection();
        }
        for node in &snapshot.nodes {
            if intersects(node.position, [node.width, node.height], x, y, w, h) {
                self.selected_nodes.insert(node.node_id);
            }
        }
        for link in &snapshot.links {
            let endpoint_in_box = |pin_id: i32| {
                snapshot
                    .nodes
                    .iter()
                    .find_map(|node| {
                        node.ports
                            .iter()
                            .enumerate()
                            .find(|(_, port)| port.pin_id == pin_id)
                            .map(|(index, port)| {
                                if node.collapsed {
                                    (
                                        if port.direction == Direction::Source {
                                            node.position[0] + node.width
                                        } else {
                                            node.position[0]
                                        },
                                        node.position[1] + NODE_HEADER_HEIGHT / 2.0,
                                    )
                                } else {
                                    let (offset_x, offset_y) = crate::canvas::pin_offset(
                                        node.width,
                                        index,
                                        node.has_audio_controls,
                                        port.direction != Direction::Sink,
                                    );
                                    (node.position[0] + offset_x, node.position[1] + offset_y)
                                }
                            })
                    })
                    .is_some_and(|point| point_in_box(point, x, y, w, h))
            };
            if endpoint_in_box(link.start_pin_id) || endpoint_in_box(link.end_pin_id) {
                self.selected_links.insert(link.link_id);
            }
        }
    }

    pub(crate) fn move_selected(
        &mut self,
        node_id: i32,
        delta_x: f32,
        delta_y: f32,
        snapshot: &GraphSnapshot,
    ) {
        let Some(dragged) = self.ids.node_id(node_id) else {
            return;
        };
        let selected: BTreeSet<_> = if self.selected_nodes.contains(&dragged) {
            self.selected_nodes.clone()
        } else {
            BTreeSet::from([dragged])
        };
        for node in snapshot
            .nodes
            .iter()
            .filter(|node| selected.contains(&node.node_id))
        {
            self.local_positions.insert(
                node.node_id,
                [node.position[0] + delta_x, node.position[1] + delta_y],
            );
        }
    }

    #[allow(dead_code)]
    pub(crate) fn set_local_position(&mut self, node_id: i32, x: f32, y: f32) {
        if let Some(node_id) = self.ids.node_id(node_id) {
            self.local_positions.insert(node_id, [x, y]);
        }
    }

    /// Adopt positions committed to the backend after an undoable command.
    /// The normal projection starts from persisted positions, but a successful
    /// move or arrange command makes the backend the new source of truth.
    pub(crate) fn adopt_backend_positions(&mut self, graph: &Graph) {
        self.local_positions = graph
            .nodes
            .values()
            .map(|node| (node.id, node.position))
            .collect();
    }

    pub(crate) fn toggle_local_collapse(&mut self, node_id: i32, snapshot: &GraphSnapshot) {
        let Some(node_id) = self.ids.node_id(node_id) else {
            return;
        };
        let Some(node) = snapshot.nodes.iter().find(|node| node.node_id == node_id) else {
            return;
        };
        let mut appearance = node.appearance.clone();
        appearance.collapsed = !appearance.collapsed;
        self.local_appearances.insert(node_id, appearance);
    }

    pub(crate) fn local_appearance(
        &self,
        node_id: NodeId,
        snapshot: &GraphSnapshot,
    ) -> Option<NodeAppearance> {
        snapshot
            .nodes
            .iter()
            .find(|node| node.node_id == node_id)
            .map(|node| {
                self.local_appearances
                    .get(&node_id)
                    .cloned()
                    .unwrap_or_else(|| node.appearance.clone())
            })
    }

    pub(crate) fn set_local_appearance(&mut self, node_id: NodeId, appearance: NodeAppearance) {
        self.local_appearances.insert(node_id, appearance);
    }

    /// Write the effective Slint layout and node appearance into the shared
    /// application configuration using the same stable keys as the desktop UI.
    pub(crate) fn write_to_config(&self, graph: &Graph, config: &mut AppConfig) {
        let configured_appearances = configured_appearances(graph, config);
        let effective_positions = self.effective_positions(graph, config, &configured_appearances);
        let mut key_counts = BTreeMap::<String, usize>::new();
        for node in graph.nodes.values() {
            *key_counts.entry(node_layout_key(node)).or_default() += 1;
        }

        config.node_positions = graph
            .nodes
            .values()
            .map(|node| {
                let position = effective_positions
                    .get(&node.id)
                    .copied()
                    .unwrap_or(node.position);
                (node.id.0.to_string(), position)
            })
            .collect();
        config.node_positions_by_name = graph
            .nodes
            .values()
            .filter_map(|node| {
                let key = node_layout_key(node);
                (key_counts.get(&key) == Some(&1)).then(|| {
                    let position = effective_positions
                        .get(&node.id)
                        .copied()
                        .unwrap_or(node.position);
                    (key, position)
                })
            })
            .collect();
        config.node_view_by_name = graph
            .nodes
            .values()
            .filter_map(|node| {
                let key = node_layout_key(node);
                if key_counts.get(&key) != Some(&1) {
                    return None;
                }
                let appearance = self
                    .local_appearances
                    .get(&node.id)
                    .cloned()
                    .or_else(|| configured_appearances.get(&node.id).cloned())
                    .unwrap_or_default();
                (appearance != NodeAppearance::default()).then_some((key, appearance))
            })
            .collect();
    }

    fn effective_positions(
        &self,
        graph: &Graph,
        config: &AppConfig,
        appearances: &BTreeMap<NodeId, NodeAppearance>,
    ) -> BTreeMap<NodeId, [f32; 2]> {
        let mut positions = configured_positions(graph, config);
        positions.extend(
            self.local_positions
                .iter()
                .map(|(id, position)| (*id, *position)),
        );
        if config.repel_overlapping_nodes {
            repel_positions(graph, positions, appearances, self.thumbnail_mode)
        } else {
            positions
        }
    }

    pub(crate) fn visible_counts(&self, snapshot: &GraphSnapshot) -> (usize, usize, usize) {
        (
            snapshot.nodes.len(),
            snapshot
                .nodes
                .iter()
                .flat_map(|node| &node.ports)
                .map(|port| port.ports.len())
                .sum(),
            snapshot.links.len(),
        )
    }

    /// Return the compatible output-to-input pairs for an Easy-mode drag.
    /// Try the visual drag direction first, then reverse it so effects and
    /// relay endpoints work without requiring users to know which side owns
    /// the source ports.
    pub(crate) fn matching_port_pairs(
        &self,
        graph: &Graph,
        source: NodeId,
        target: NodeId,
    ) -> Vec<(PortId, PortId)> {
        let Some(source) = graph.node(source) else {
            return Vec::new();
        };
        let Some(target) = graph.node(target) else {
            return Vec::new();
        };
        let source_ports = self.ordered_ports(graph, source);
        let target_ports = self.ordered_ports(graph, target);
        let source_outputs = source_ports
            .iter()
            .copied()
            .filter(|port| port.direction == Direction::Source)
            .collect::<Vec<_>>();
        let target_inputs = target_ports
            .iter()
            .copied()
            .filter(|port| port.direction == Direction::Sink)
            .collect::<Vec<_>>();
        let forward = pair_ports(&source_outputs, &target_inputs);
        if !forward.is_empty() {
            return forward;
        }
        let target_outputs = target_ports
            .iter()
            .copied()
            .filter(|port| port.direction == Direction::Source)
            .collect::<Vec<_>>();
        let source_inputs = source_ports
            .iter()
            .copied()
            .filter(|port| port.direction == Direction::Sink)
            .collect::<Vec<_>>();
        pair_ports(&target_outputs, &source_inputs)
    }

    /// The rendered pin a pin id belongs to. In Easy mode one pin stands for
    /// a whole channel group (`capture_FL` + `capture_FR`), so a gesture that
    /// starts on it means "connect the group", not "connect one port".
    pub(crate) fn port_group(&self, graph: &Graph, pin_id: i32) -> Option<PortGroupView> {
        let port = self.ids.port_id(pin_id)?;
        let node = graph.node(graph.port(port)?.node_id)?;
        self.project_ports(graph, node)
            .into_iter()
            .find(|group| group.pin_id == pin_id)
    }

    /// The output-to-input pairs for a drag between two rendered pins. The
    /// channels are matched inside the two groups, so left stays left and
    /// right stays right whichever way the drag was made.
    pub(crate) fn matching_pin_pairs(
        &self,
        graph: &Graph,
        source_pin: i32,
        target_pin: i32,
    ) -> Vec<(PortId, PortId)> {
        let Some(source) = self.port_group(graph, source_pin) else {
            return Vec::new();
        };
        let Some(target) = self.port_group(graph, target_pin) else {
            return Vec::new();
        };
        let (outputs, inputs) = match (source.direction, target.direction) {
            (Direction::Source, Direction::Sink) => (&source.ports, &target.ports),
            (Direction::Sink, Direction::Source) => (&target.ports, &source.ports),
            _ => return Vec::new(),
        };
        let resolve = |ports: &Vec<PortId>| {
            ports
                .iter()
                .filter_map(|port| graph.port(*port))
                .collect::<Vec<_>>()
        };
        pair_ports(&resolve(outputs), &resolve(inputs))
    }

    /// The pairs for a drag that started on a pin and was released over a
    /// card. Only the dragged group's channels are connected, matched against
    /// whichever ports of that card face the other way.
    pub(crate) fn matching_group_to_node_pairs(
        &self,
        graph: &Graph,
        source_pin: i32,
        target: NodeId,
    ) -> Vec<(PortId, PortId)> {
        let Some(group) = self.port_group(graph, source_pin) else {
            return Vec::new();
        };
        let Some(target) = graph.node(target) else {
            return Vec::new();
        };
        let group_ports = group
            .ports
            .iter()
            .filter_map(|port| graph.port(*port))
            .collect::<Vec<_>>();
        let facing = |direction: Direction| {
            self.ordered_ports(graph, target)
                .into_iter()
                .filter(|port| port.direction == direction)
                .collect::<Vec<_>>()
        };
        match group.direction {
            Direction::Source => pair_ports(&group_ports, &facing(Direction::Sink)),
            Direction::Sink => pair_ports(&facing(Direction::Source), &group_ports),
        }
    }

    pub(crate) fn node_at(
        &self,
        snapshot: &GraphSnapshot,
        x: f32,
        y: f32,
        exclude: NodeId,
    ) -> Option<NodeId> {
        self.node_at_with_margin(snapshot, x, y, exclude, 0.0)
    }

    pub(crate) fn node_at_with_margin(
        &self,
        snapshot: &GraphSnapshot,
        x: f32,
        y: f32,
        exclude: NodeId,
        margin: f32,
    ) -> Option<NodeId> {
        snapshot
            .nodes
            .iter()
            .find(|node| {
                node.node_id != exclude
                    && x >= node.position[0] - margin
                    && x <= node.position[0] + node.width + margin
                    && y >= node.position[1] - margin
                    && y <= node.position[1] + node.height + margin
            })
            .map(|node| node.node_id)
    }

    fn project_ports(&self, graph: &Graph, node: &Node) -> Vec<PortGroupView> {
        let ports = self.ordered_ports(graph, node);

        let mut groups: Vec<PortGroupView> = Vec::new();
        let mut group_index = HashMap::<(Direction, PortType, String), usize>::new();
        for port in ports {
            let key = (self.connect_mode == ConnectMode::Easy && port.port_type == PortType::Audio)
                .then(|| channel_base_name(port).map(|base| (port.direction, port.port_type, base)))
                .flatten();
            if let Some(index) = key.as_ref().and_then(|key| group_index.get(key).copied()) {
                groups[index].ports.push(port.id);
                if groups[index].ports.len() == 2 {
                    groups[index].label = key.as_ref().map(|key| key.2.clone()).unwrap_or_default();
                }
                continue;
            }
            let pin_id = self.ids.port(port.id).unwrap_or_default();
            let index = groups.len();
            groups.push(PortGroupView {
                pin_id,
                ports: vec![port.id],
                label: port.name.clone(),
                direction: port.direction,
                port_type: port.port_type,
                color: port_color(port.port_type, port.direction, &port.name),
            });
            if let Some(key) = key {
                group_index.insert(key, index);
            }
        }
        groups
    }

    fn ordered_ports<'a>(&self, graph: &'a Graph, node: &Node) -> Vec<&'a Port> {
        let mut ports: Vec<_> = node
            .ports
            .iter()
            .filter_map(|id| graph.port(*id))
            .filter(|port| self.media_filter.matches_port_type(port.port_type))
            .filter(|port| self.search_matches_port(node, port))
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

    fn search_matches(&self, graph: &Graph, node: &Node) -> bool {
        let query = self.search_query.trim().to_ascii_lowercase();
        query.is_empty()
            || node.name.to_ascii_lowercase().contains(&query)
            || node.ports.iter().any(|port_id| {
                graph
                    .port(*port_id)
                    .is_some_and(|port| port.name.to_ascii_lowercase().contains(&query))
            })
    }

    fn search_matches_port(&self, node: &Node, port: &Port) -> bool {
        let query = self.search_query.trim().to_ascii_lowercase();
        query.is_empty()
            || node.name.to_ascii_lowercase().contains(&query)
            || port.name.to_ascii_lowercase().contains(&query)
    }
}

pub(crate) fn node_type_color(node_type: NodeType) -> [u8; 4] {
    match node_type {
        NodeType::PipeWire => [63, 82, 101, 255],
        NodeType::Effect => [82, 117, 176, 255],
        NodeType::AlsaMidi => [138, 93, 159, 255],
        NodeType::WindowsAudioEndpoint => [49, 129, 143, 255],
        NodeType::WindowsAudioSession => [64, 157, 168, 255],
        NodeType::WindowsMidi => [155, 105, 174, 255],
        NodeType::Unknown => [112, 112, 112, 255],
    }
}

/// The canvas palette distinguishes input, output, and monitor ports within
/// each media family. Keeping the role calculation here means Slint and the
/// hit-tested graph model use the same colors for dots, accents, and links.
pub(crate) fn port_color(port_type: PortType, direction: Direction, name: &str) -> [u8; 4] {
    let monitor = name.to_ascii_lowercase().contains("monitor");
    match (port_type, monitor, direction) {
        (PortType::Audio, true, _) => [139, 231, 177, 255],
        (PortType::Audio, false, Direction::Sink) => [44, 151, 96, 255],
        (PortType::Audio, false, Direction::Source) => [82, 207, 133, 255],
        (PortType::Video, true, _) => [151, 213, 255, 255],
        (PortType::Video, false, Direction::Sink) => [43, 125, 202, 255],
        (PortType::Video, false, Direction::Source) => [91, 181, 244, 255],
        (PortType::MidiJack, true, _) => [255, 161, 177, 255],
        (PortType::MidiJack, false, Direction::Sink) => [186, 57, 87, 255],
        (PortType::MidiJack, false, Direction::Source) => [237, 108, 128, 255],
        (PortType::MidiAlsa, true, _) => [220, 177, 245, 255],
        (PortType::MidiAlsa, false, Direction::Sink) => [128, 78, 172, 255],
        (PortType::MidiAlsa, false, Direction::Source) => [190, 132, 225, 255],
        (PortType::Unknown, true, _) => [214, 222, 232, 255],
        (PortType::Unknown, false, Direction::Sink) => [116, 127, 141, 255],
        (PortType::Unknown, false, Direction::Source) => [177, 188, 202, 255],
    }
}

pub(crate) fn link_color(port_type: PortType, direction: Direction, name: &str) -> [u8; 4] {
    let monitor = name.to_ascii_lowercase().contains("monitor");
    match (port_type, monitor, direction) {
        (PortType::Audio, true, _) => [105, 194, 145, 255],
        (PortType::Audio, false, Direction::Sink) => [38, 126, 80, 255],
        (PortType::Audio, false, Direction::Source) => [62, 173, 109, 255],
        (PortType::Video, true, _) => [112, 177, 218, 255],
        (PortType::Video, false, Direction::Sink) => [37, 105, 170, 255],
        (PortType::Video, false, Direction::Source) => [69, 147, 204, 255],
        (PortType::MidiJack, true, _) => [220, 125, 145, 255],
        (PortType::MidiJack, false, Direction::Sink) => [151, 48, 72, 255],
        (PortType::MidiJack, false, Direction::Source) => [198, 83, 105, 255],
        (PortType::MidiAlsa, true, _) => [187, 145, 213, 255],
        (PortType::MidiAlsa, false, Direction::Sink) => [104, 62, 141, 255],
        (PortType::MidiAlsa, false, Direction::Source) => [157, 105, 191, 255],
        (PortType::Unknown, true, _) => [178, 188, 202, 255],
        (PortType::Unknown, false, Direction::Sink) => [93, 103, 116, 255],
        (PortType::Unknown, false, Direction::Source) => [143, 155, 171, 255],
    }
}

fn node_height(
    thumbnail: bool,
    collapsed: bool,
    has_audio_controls: bool,
    port_count: usize,
) -> f32 {
    if thumbnail {
        62.0
    } else if collapsed {
        COLLAPSED_NODE_HEIGHT
    } else {
        NODE_HEADER_HEIGHT
            + if has_audio_controls {
                AUDIO_CONTROLS_HEIGHT
            } else {
                0.0
            }
            + 14.0
            + port_count as f32 * PORT_ROW_HEIGHT
    }
}

fn intersects(position: [f32; 2], size: [f32; 2], x: f32, y: f32, w: f32, h: f32) -> bool {
    position[0] < x + w
        && position[0] + size[0] > x
        && position[1] < y + h
        && position[1] + size[1] > y
}

fn point_in_box(point: (f32, f32), x: f32, y: f32, width: f32, height: f32) -> bool {
    point.0 >= x && point.0 <= x + width && point.1 >= y && point.1 <= y + height
}

fn configured_positions(graph: &Graph, config: &AppConfig) -> BTreeMap<NodeId, [f32; 2]> {
    let defaults = graph.default_node_positions();
    let mut key_counts = BTreeMap::<String, usize>::new();
    for node in graph.nodes.values() {
        *key_counts.entry(node_layout_key(node)).or_default() += 1;
    }
    graph
        .nodes
        .values()
        .map(|node| {
            let key = node_layout_key(node);
            let by_id = config.node_positions.get(&node.id.0.to_string()).copied();
            let by_name = (key_counts.get(&key) == Some(&1))
                .then(|| config.node_positions_by_name.get(&key).copied())
                .flatten();
            (
                node.id,
                by_id
                    .or(by_name)
                    .or_else(|| defaults.get(&node.id).copied())
                    .unwrap_or(node.position),
            )
        })
        .collect()
}

fn configured_appearances(graph: &Graph, config: &AppConfig) -> BTreeMap<NodeId, NodeAppearance> {
    let mut key_counts = BTreeMap::<String, usize>::new();
    for node in graph.nodes.values() {
        *key_counts.entry(node_layout_key(node)).or_default() += 1;
    }
    graph
        .nodes
        .values()
        .filter_map(|node| {
            let key = node_layout_key(node);
            (key_counts.get(&key) == Some(&1)).then(|| {
                (
                    node.id,
                    config
                        .node_view_by_name
                        .get(&key)
                        .cloned()
                        .unwrap_or_default(),
                )
            })
        })
        .collect()
}

/// Produce a stable non-overlapping projection of the configured layout.
/// User movement remains an explicit `MoveNodesCommand`; this operation only
/// applies the preference to the rendered/persisted layout.
fn repel_positions(
    graph: &Graph,
    positions: BTreeMap<NodeId, [f32; 2]>,
    appearances: &BTreeMap<NodeId, NodeAppearance>,
    thumbnail: bool,
) -> BTreeMap<NodeId, [f32; 2]> {
    const GAP: f32 = 18.0;
    let mut result = BTreeMap::new();
    let mut placed = Vec::<([f32; 2], [f32; 2])>::new();

    for node in graph.nodes.values() {
        let mut position = positions.get(&node.id).copied().unwrap_or(node.position);
        let appearance = appearances.get(&node.id).cloned().unwrap_or_default();
        let height = node_height(
            thumbnail,
            appearance.collapsed,
            node.ports.iter().any(|port_id| {
                graph
                    .port(*port_id)
                    .is_some_and(|port| port.port_type == PortType::Audio)
            }),
            node.ports.len(),
        );
        let size = [NODE_WIDTH, height];

        let mut attempts = 0;
        while placed.iter().any(|(other, other_size)| {
            intersects(
                position,
                size,
                other[0] - GAP,
                other[1] - GAP,
                other_size[0] + GAP * 2.0,
                other_size[1] + GAP * 2.0,
            )
        }) && attempts < graph.nodes.len().saturating_mul(2).max(1)
        {
            let rightmost = placed
                .iter()
                .filter(|(other, other_size)| {
                    intersects(
                        position,
                        size,
                        other[0] - GAP,
                        other[1] - GAP,
                        other_size[0] + GAP * 2.0,
                        other_size[1] + GAP * 2.0,
                    )
                })
                .map(|(other, other_size)| other[0] + other_size[0] + GAP)
                .fold(position[0], f32::max);
            position[0] = rightmost;
            attempts += 1;
        }
        result.insert(node.id, position);
        placed.push((position, size));
    }
    result
}

pub(crate) fn node_layout_key(node: &Node) -> String {
    let kind = match node.node_type {
        NodeType::PipeWire => "PipeWire",
        NodeType::Effect => "Effect",
        NodeType::AlsaMidi => "AlsaMidi",
        NodeType::WindowsAudioEndpoint => "WindowsAudioEndpoint",
        NodeType::WindowsAudioSession => "WindowsAudioSession",
        NodeType::WindowsMidi => "WindowsMidi",
        NodeType::Unknown => "Unknown",
    };
    format!("{kind}:{}", node.name)
}

/// Apply the same stable layout lookup used by the rendered projection to the
/// backend, preserving startup position restoration semantics.
pub(crate) fn restore_node_positions(driver: &mut dyn GraphDriver, config: &AppConfig) {
    let positions = configured_positions(driver.graph(), config);
    for (node, position) in positions {
        let _ = driver.set_node_position(node, position);
    }
}

pub(crate) fn is_relay_node(node: &Node) -> bool {
    matches!(node.name.as_str(), RELAY_SOURCE_NAME | RELAY_SINK_NAME)
}

fn pair_ports(outputs: &[&Port], inputs: &[&Port]) -> Vec<(PortId, PortId)> {
    let channel_matched = pair_ports_by_channel(outputs, inputs);
    if !channel_matched.is_empty() {
        return channel_matched;
    }
    // Nothing lined up by channel — a mono endpoint meeting a stereo one, or
    // ports that carry no channel at all. Fall back to pairing them in order
    // so the drag still connects instead of reporting no compatible ports.
    pair_ports_in_order(outputs, inputs)
}

fn pair_ports_by_channel(outputs: &[&Port], inputs: &[&Port]) -> Vec<(PortId, PortId)> {
    let mut used = vec![false; inputs.len()];
    let mut pairs = Vec::new();
    for output in outputs {
        let candidate = inputs
            .iter()
            .enumerate()
            .filter(|(index, input)| {
                !used[*index]
                    && ports_compatible(output.port_type, input.port_type)
                    && channels_can_pair(output, input)
            })
            .max_by_key(|(index, input)| {
                (
                    channel_pair_score(output, input),
                    name_pair_score(output, input),
                    std::cmp::Reverse(*index),
                )
            })
            .map(|(index, _)| index);
        if let Some(index) = candidate {
            used[index] = true;
            pairs.push((output.id, inputs[index].id));
        }
    }
    pairs
}

fn pair_ports_in_order(outputs: &[&Port], inputs: &[&Port]) -> Vec<(PortId, PortId)> {
    let mut used = vec![false; inputs.len()];
    let mut pairs = Vec::new();
    for output in outputs {
        let candidate = inputs.iter().enumerate().find(|(index, input)| {
            !used[*index] && ports_compatible(output.port_type, input.port_type)
        });
        if let Some((index, input)) = candidate {
            used[index] = true;
            pairs.push((output.id, input.id));
        }
    }
    pairs
}

fn ports_compatible(output: PortType, input: PortType) -> bool {
    output == input || output == PortType::Unknown || input == PortType::Unknown
}

fn channels_can_pair(output: &Port, input: &Port) -> bool {
    match (channel_identity(output), channel_identity(input)) {
        (Some(output), Some(input)) => output.eq_ignore_ascii_case(&input),
        _ => true,
    }
}

fn channel_pair_score(output: &Port, input: &Port) -> u8 {
    match (channel_identity(output), channel_identity(input)) {
        (Some(output), Some(input)) if output.eq_ignore_ascii_case(&input) => 100,
        (Some(_), Some(_)) => 0,
        (Some(_), None) | (None, Some(_)) => 20,
        (None, None) => 10,
    }
}

fn name_pair_score(output: &Port, input: &Port) -> u8 {
    match (channel_base_name(output), channel_base_name(input)) {
        (Some(output), Some(input)) if output.eq_ignore_ascii_case(&input) => 10,
        _ => 0,
    }
}

fn channel_identity(port: &Port) -> Option<String> {
    port.channel
        .as_deref()
        .map(str::trim)
        .filter(|channel| !channel.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            let suffix = port
                .name
                .rsplit(['_', '-', ' ', ':', '.'])
                .next()
                .unwrap_or_default();
            is_channel_token(suffix).then(|| suffix.to_owned())
        })
}

fn channel_base_name(port: &Port) -> Option<String> {
    const DELIMITERS: [char; 5] = ['_', '-', ' ', ':', '.'];
    let name = port.name.as_str();
    let position = name.rfind(|character| DELIMITERS.contains(&character))?;
    let (base, suffix) = name.split_at(position);
    let suffix = suffix.trim_start_matches(DELIMITERS);
    (!base.is_empty() && is_channel_token(suffix)).then(|| base.to_owned())
}

fn is_channel_token(token: &str) -> bool {
    const TOKENS: [&str; 34] = [
        "FL", "FR", "RL", "RR", "SL", "SR", "FC", "RC", "LFE", "MONO", "LEFT", "RIGHT", "L", "R",
        "C", "FLC", "FRC", "TC", "TFL", "TFR", "TFC", "TRL", "TRR", "TRC", "BFL", "BFR", "BFC",
        "BL", "BR", "BC", "BLC", "BRC", "TBL", "TBR",
    ];
    TOKENS
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(token))
        || token.strip_prefix("AUX").is_some_and(|suffix| {
            !suffix.is_empty() && suffix.chars().all(|character| character.is_ascii_digit())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pw_graph_core::{LinkId, Node, Port};

    fn graph() -> Graph {
        let mut graph = Graph::default();
        let mut source = Node::new(NodeId(1), "Source", NodeType::PipeWire);
        source.position = [10.0, 20.0];
        graph.add_node(source).unwrap();
        graph
            .add_node(Node::new(NodeId(2), "Sink", NodeType::PipeWire))
            .unwrap();
        graph
            .add_port(Port::new(
                PortId(u64::MAX),
                NodeId(1),
                "output_FL",
                Direction::Source,
                PortType::Audio,
            ))
            .unwrap();
        graph
            .add_port(Port::new(
                PortId(2),
                NodeId(2),
                "input_FL",
                Direction::Sink,
                PortType::Audio,
            ))
            .unwrap();
        graph
            .add_link(LinkId(7), PortId(u64::MAX), PortId(2))
            .unwrap();
        graph
    }

    #[test]
    fn id_map_never_casts_large_backend_ids() {
        let graph = graph();
        let mut ids = SlintIdMap::default();
        ids.rebuild(&graph);
        assert_ne!(ids.port(PortId(u64::MAX)).unwrap() as u64, u64::MAX);
        assert_eq!(ids.node_id(ids.node(NodeId(1)).unwrap()), Some(NodeId(1)));
        assert_eq!(
            ids.port_id(ids.port(PortId(u64::MAX)).unwrap()),
            Some(PortId(u64::MAX))
        );
    }

    #[test]
    fn snapshot_filters_and_keeps_link_endpoints() {
        let graph = graph();
        let config = AppConfig::default();
        let mut state = UiGraphState::from_config(&config);
        let snapshot = state.snapshot(&graph, &config);
        assert_eq!(snapshot.nodes.len(), 2);
        assert_eq!(snapshot.links.len(), 1);
        state.media_filter = MediaFilter::Midi;
        assert!(state.snapshot(&graph, &config).nodes.is_empty());
    }

    #[test]
    fn box_selection_includes_links_by_endpoint() {
        let graph = graph();
        let config = AppConfig::default();
        let mut state = UiGraphState::from_config(&config);
        let snapshot = state.snapshot(&graph, &config);

        state.select_box(&snapshot, -1000.0, -1000.0, 5000.0, 5000.0, false);

        assert!(state.selected_nodes.contains(&NodeId(1)));
        assert!(state.selected_nodes.contains(&NodeId(2)));
        assert!(state.selected_links.contains(&LinkId(7)));
    }

    #[test]
    fn repel_preference_separates_configured_overlapping_cards() {
        let graph = graph();
        let mut config = AppConfig::default();
        config.node_positions.insert("1".into(), [0.0, 0.0]);
        config.node_positions.insert("2".into(), [0.0, 0.0]);
        config.repel_overlapping_nodes = true;
        let mut state = UiGraphState::from_config(&config);
        let snapshot = state.snapshot(&graph, &config);
        let first = snapshot
            .nodes
            .iter()
            .find(|node| node.node_id == NodeId(1))
            .unwrap();
        let second = snapshot
            .nodes
            .iter()
            .find(|node| node.node_id == NodeId(2))
            .unwrap();

        assert!(second.position[0] >= first.position[0] + first.width + 18.0);
    }

    #[test]
    fn local_positions_are_explicitly_written_to_config() {
        let graph = graph();
        let mut config = AppConfig::default();
        let mut state = UiGraphState::from_config(&config);
        let snapshot = state.snapshot(&graph, &config);
        state.set_local_position(snapshot.nodes[0].id, 99.0, 101.0);
        assert_eq!(
            state.snapshot(&graph, &config).nodes[0].position,
            [99.0, 101.0]
        );
        assert!(config.node_positions.is_empty());

        state.write_to_config(&graph, &mut config);

        assert_eq!(config.node_positions.get("1"), Some(&[99.0, 101.0]));
        assert_eq!(
            config.node_positions_by_name.get("PipeWire:Source"),
            Some(&[99.0, 101.0])
        );
    }

    #[test]
    fn thumbnail_projection_matches_the_compact_canvas_mode() {
        let graph = graph();
        let config = AppConfig::default();
        let mut state = UiGraphState::from_config(&config);
        state.thumbnail_mode = true;

        let snapshot = state.snapshot(&graph, &config);

        assert!(snapshot
            .nodes
            .iter()
            .all(|node| node.thumbnail && node.height == 62.0));
        assert!(snapshot.links.is_empty());
    }

    #[test]
    fn collapse_is_restored_through_config() {
        let graph = graph();
        let mut config = AppConfig::default();
        let mut state = UiGraphState::from_config(&config);
        let snapshot = state.snapshot(&graph, &config);

        state.toggle_local_collapse(snapshot.nodes[0].id, &snapshot);

        assert!(state.snapshot(&graph, &config).nodes[0].collapsed);
        assert!(config.node_view_by_name.is_empty());

        state.write_to_config(&graph, &mut config);
        let mut restored = UiGraphState::from_config(&config);
        assert!(restored.snapshot(&graph, &config).nodes[0].collapsed);
    }

    #[test]
    fn supplied_meters_are_projected_without_changing_the_graph() {
        let graph = graph();
        let config = AppConfig::default();
        let mut state = UiGraphState::from_config(&config);
        let mut meters = BTreeMap::new();
        meters.insert(
            NodeId(1),
            MeterReading {
                rms: 0.42,
                peak: 0.81,
                state: MeterState::Live,
            },
        );

        let snapshot = state.snapshot_with_meters(
            &graph,
            &config,
            &meters,
            MeterState::Waiting,
            &fully_capable_audio_profiles(&graph),
        );
        let source = snapshot
            .nodes
            .iter()
            .find(|node| node.node_id == NodeId(1))
            .unwrap();
        let sink = snapshot
            .nodes
            .iter()
            .find(|node| node.node_id == NodeId(2))
            .unwrap();

        assert_eq!(source.meter.rms, 0.42);
        assert_eq!(source.meter.peak, 0.81);
        assert_eq!(source.meter.state, MeterState::Live);
        assert_eq!(sink.meter.state, MeterState::Waiting);
        assert_eq!(graph.links.len(), 1);
    }

    #[test]
    fn relay_nodes_are_hidden_until_relay_activity_starts() {
        let mut graph = graph();
        graph
            .add_node(Node::new(NodeId(3), RELAY_SOURCE_NAME, NodeType::PipeWire))
            .unwrap();
        graph
            .add_node(Node::new(NodeId(4), RELAY_SINK_NAME, NodeType::PipeWire))
            .unwrap();
        let config = AppConfig::default();
        let mut state = UiGraphState::from_config(&config);

        let hidden = state.snapshot(&graph, &config);
        assert_eq!(hidden.nodes.len(), 2);
        assert!(hidden.nodes.iter().all(|node| node.node_id.0 < 3));

        state.relay_nodes_visible = true;
        let active = state.snapshot(&graph, &config);
        assert_eq!(active.nodes.len(), 4);
        assert!(active.nodes.iter().any(|node| node.node_id == NodeId(3)));
        assert!(active.nodes.iter().any(|node| node.node_id == NodeId(4)));
    }

    #[test]
    fn easy_mode_pairs_every_effect_node_in_either_drag_direction() {
        let mut graph = graph();
        graph
            .add_port(
                Port::new(
                    PortId(3),
                    NodeId(1),
                    "output_FR",
                    Direction::Source,
                    PortType::Audio,
                )
                .with_channel("FR"),
            )
            .unwrap();
        for (node_id, first_port) in [(NodeId(3), 30), (NodeId(4), 40)] {
            graph
                .add_node(Node::new(
                    node_id,
                    format!("Effect {}", node_id.0),
                    NodeType::Effect,
                ))
                .unwrap();
            for (offset, name, direction, channel) in [
                (0, "input_FL", Direction::Sink, "FL"),
                (1, "input_FR", Direction::Sink, "FR"),
                (2, "output_FL", Direction::Source, "FL"),
                (3, "output_FR", Direction::Source, "FR"),
            ] {
                graph
                    .add_port(
                        Port::new(
                            PortId(first_port + offset),
                            node_id,
                            name,
                            direction,
                            PortType::Audio,
                        )
                        .with_channel(channel),
                    )
                    .unwrap();
            }
        }
        let config = AppConfig {
            connect_mode: "easy".into(),
            ..AppConfig::default()
        };
        let state = UiGraphState::from_config(&config);

        for effect in [NodeId(3), NodeId(4)] {
            let into_effect = state.matching_port_pairs(&graph, NodeId(1), effect);
            assert_eq!(into_effect.len(), 2);
            let from_effect_to_sink = state.matching_port_pairs(&graph, NodeId(2), effect);
            assert_eq!(from_effect_to_sink.len(), 1);
            let output = graph.port(from_effect_to_sink[0].0).unwrap();
            let input = graph.port(from_effect_to_sink[0].1).unwrap();
            assert_eq!(output.node_id, effect);
            assert_eq!(input.node_id, NodeId(2));
        }
    }

    /// Two stereo cards, where the sink exposes two separate stereo groups.
    fn stereo_graph() -> Graph {
        let mut graph = Graph::default();
        graph
            .add_node(Node::new(NodeId(1), "Capture", NodeType::PipeWire))
            .unwrap();
        graph
            .add_node(Node::new(NodeId(2), "Playback", NodeType::PipeWire))
            .unwrap();
        let ports = [
            (1, NodeId(1), "capture_FL", Direction::Source, "FL"),
            (2, NodeId(1), "capture_FR", Direction::Source, "FR"),
            (3, NodeId(2), "main_FL", Direction::Sink, "FL"),
            (4, NodeId(2), "main_FR", Direction::Sink, "FR"),
            (5, NodeId(2), "aux_FL", Direction::Sink, "FL"),
            (6, NodeId(2), "aux_FR", Direction::Sink, "FR"),
        ];
        for (id, node, name, direction, channel) in ports {
            graph
                .add_port(
                    Port::new(PortId(id), node, name, direction, PortType::Audio)
                        .with_channel(channel),
                )
                .unwrap();
        }
        graph
    }

    fn easy_state() -> UiGraphState {
        let config = AppConfig {
            connect_mode: "easy".into(),
            ..AppConfig::default()
        };
        UiGraphState::from_config(&config)
    }

    #[test]
    fn easy_mode_groups_a_stereo_pair_behind_one_pin() {
        let graph = stereo_graph();
        let mut state = easy_state();
        state.ids.rebuild(&graph);
        let capture = state.ids.port(PortId(1)).unwrap();

        let group = state.port_group(&graph, capture).unwrap();

        assert_eq!(group.label, "capture");
        assert_eq!(group.ports, vec![PortId(1), PortId(2)]);
    }

    #[test]
    fn easy_pin_pairs_keep_left_on_left_in_both_drag_directions() {
        let graph = stereo_graph();
        let mut state = easy_state();
        state.ids.rebuild(&graph);
        let capture = state.ids.port(PortId(1)).unwrap();
        let main = state.ids.port(PortId(3)).unwrap();

        let forward = state.matching_pin_pairs(&graph, capture, main);
        let backward = state.matching_pin_pairs(&graph, main, capture);

        assert_eq!(
            forward,
            vec![(PortId(1), PortId(3)), (PortId(2), PortId(4))]
        );
        assert_eq!(backward, forward);
    }

    #[test]
    fn easy_pin_pairs_refuse_two_pins_that_face_the_same_way() {
        let graph = stereo_graph();
        let mut state = easy_state();
        state.ids.rebuild(&graph);
        let main = state.ids.port(PortId(3)).unwrap();
        let aux = state.ids.port(PortId(5)).unwrap();

        assert!(state.matching_pin_pairs(&graph, main, aux).is_empty());
    }

    #[test]
    fn a_group_dropped_on_a_card_only_fills_one_of_its_groups() {
        let graph = stereo_graph();
        let mut state = easy_state();
        state.ids.rebuild(&graph);
        let capture = state.ids.port(PortId(1)).unwrap();

        let pairs = state.matching_group_to_node_pairs(&graph, capture, NodeId(2));

        // Both channels land, both in the same destination group (the first
        // one the card renders), and left stays on left.
        assert_eq!(pairs.len(), 2);
        let base = |port: PortId| channel_base_name(graph.port(port).unwrap()).unwrap();
        let channel = |port: PortId| channel_identity(graph.port(port).unwrap()).unwrap();
        assert_eq!(base(pairs[0].1), base(pairs[1].1));
        assert!(pairs
            .iter()
            .all(|(output, input)| channel(*output) == channel(*input)));
        assert_ne!(pairs[0].1, pairs[1].1);
    }

    #[test]
    fn a_mono_endpoint_still_connects_to_a_stereo_one() {
        let mut graph = stereo_graph();
        graph
            .add_node(Node::new(NodeId(3), "Mono sink", NodeType::PipeWire))
            .unwrap();
        graph
            .add_port(
                Port::new(
                    PortId(7),
                    NodeId(3),
                    "input_MONO",
                    Direction::Sink,
                    PortType::Audio,
                )
                .with_channel("MONO"),
            )
            .unwrap();
        let mut state = easy_state();
        state.ids.rebuild(&graph);

        // Nothing lines up by channel, so the drag pairs in order instead of
        // reporting that there is nothing to connect.
        let pairs = state.matching_port_pairs(&graph, NodeId(1), NodeId(3));

        assert_eq!(pairs, vec![(PortId(1), PortId(7))]);
    }

    #[test]
    fn channel_matching_wins_over_port_order() {
        let mut graph = Graph::default();
        graph
            .add_node(Node::new(NodeId(1), "Capture", NodeType::PipeWire))
            .unwrap();
        graph
            .add_node(Node::new(NodeId(2), "Playback", NodeType::PipeWire))
            .unwrap();
        // The sink lists its right channel first.
        for (id, node, name, direction, channel) in [
            (1, NodeId(1), "capture_FL", Direction::Source, "FL"),
            (2, NodeId(1), "capture_FR", Direction::Source, "FR"),
            (3, NodeId(2), "playback_FR", Direction::Sink, "FR"),
            (4, NodeId(2), "playback_FL", Direction::Sink, "FL"),
        ] {
            graph
                .add_port(
                    Port::new(PortId(id), node, name, direction, PortType::Audio)
                        .with_channel(channel),
                )
                .unwrap();
        }
        let mut state = easy_state();
        state.ids.rebuild(&graph);

        let pairs = state.matching_port_pairs(&graph, NodeId(1), NodeId(2));

        assert_eq!(pairs, vec![(PortId(1), PortId(4)), (PortId(2), PortId(3))]);
    }
}
