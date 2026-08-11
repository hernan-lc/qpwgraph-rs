//! Framework-neutral state projected into Slint models.

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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum MeterState {
    #[default]
    Unavailable,
    Disabled,
    Waiting,
    Live,
    Demo,
}

impl MeterState {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Unavailable => "N/A",
            Self::Disabled => "OFF",
            Self::Waiting => "WAIT",
            Self::Live => "LIVE",
            Self::Demo => "DEMO",
        }
    }
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
    pub(crate) meter: MeterReading,
    pub(crate) ports: Vec<PortGroupView>,
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
            selected_nodes: BTreeSet::new(),
            selected_links: BTreeSet::new(),
            ids: SlintIdMap::default(),
            local_positions: BTreeMap::new(),
            local_appearances: BTreeMap::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn snapshot(&mut self, graph: &Graph, config: &AppConfig) -> GraphSnapshot {
        self.snapshot_with_meters(graph, config, &BTreeMap::new(), MeterState::Unavailable)
    }

    pub(crate) fn snapshot_with_meters(
        &mut self,
        graph: &Graph,
        config: &AppConfig,
        meters: &BTreeMap<NodeId, MeterReading>,
        meter_fallback: MeterState,
    ) -> GraphSnapshot {
        self.ids.rebuild(graph);
        self.local_positions
            .retain(|id, _| graph.nodes.contains_key(id));
        self.local_appearances
            .retain(|id, _| graph.nodes.contains_key(id));

        let visible: BTreeSet<_> = graph
            .nodes
            .values()
            .filter(|node| self.media_filter.matches_node(graph, node))
            .filter(|node| self.search_matches(graph, node))
            .map(|node| node.id)
            .collect();
        self.selected_nodes.retain(|id| visible.contains(id));
        self.selected_links
            .retain(|id| graph.links.contains_key(id));

        let positions = configured_positions(graph, config);
        let appearances = configured_appearances(graph, config);
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
            let has_audio_controls = node.node_type != NodeType::Effect
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
                position: self
                    .local_positions
                    .get(&node.id)
                    .copied()
                    .or_else(|| positions.get(&node.id).copied())
                    .unwrap_or(node.position),
                width: NODE_WIDTH,
                height,
                selected: self.selected_nodes.contains(&node.id),
                collapsed,
                thumbnail,
                font_scale: self.node_text_scale,
                appearance,
                has_audio_controls,
                meter: meters.get(&node.id).copied().unwrap_or(MeterReading {
                    state: meter_fallback,
                    ..MeterReading::default()
                }),
                ports,
            });
        }

        let links = (!self.thumbnail_mode)
            .then(|| {
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
                                color: port_type_color(output.port_type),
                                selected: self.selected_links.contains(&link.id),
                            })
                    })
                    .collect()
            })
            .unwrap_or_default();

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
    }

    pub(crate) fn set_local_position(&mut self, node_id: i32, x: f32, y: f32) {
        if let Some(node_id) = self.ids.node_id(node_id) {
            self.local_positions.insert(node_id, [x, y]);
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

    /// Write the effective Slint layout and node appearance into the shared
    /// application configuration using the same stable keys as the Egui UI.
    pub(crate) fn write_to_config(&self, graph: &Graph, config: &mut AppConfig) {
        let configured_positions = configured_positions(graph, config);
        let configured_appearances = configured_appearances(graph, config);
        let mut key_counts = BTreeMap::<String, usize>::new();
        for node in graph.nodes.values() {
            *key_counts.entry(node_layout_key(node)).or_default() += 1;
        }

        config.node_positions = graph
            .nodes
            .values()
            .map(|node| {
                let position = self
                    .local_positions
                    .get(&node.id)
                    .copied()
                    .or_else(|| configured_positions.get(&node.id).copied())
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
                    let position = self
                        .local_positions
                        .get(&node.id)
                        .copied()
                        .or_else(|| configured_positions.get(&node.id).copied())
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

    fn project_ports(&self, graph: &Graph, node: &Node) -> Vec<PortGroupView> {
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
            });
            if let Some(key) = key {
                group_index.insert(key, index);
            }
        }
        groups
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

pub(crate) fn node_type_label(node_type: NodeType) -> &'static str {
    match node_type {
        NodeType::PipeWire => "PipeWire node",
        NodeType::Effect => "Effect node",
        NodeType::AlsaMidi => "ALSA MIDI node",
        NodeType::Unknown => "Unknown node",
    }
}

pub(crate) fn node_type_color(node_type: NodeType) -> [u8; 4] {
    match node_type {
        NodeType::PipeWire => [63, 82, 101, 255],
        NodeType::Effect => [82, 117, 176, 255],
        NodeType::AlsaMidi => [138, 93, 159, 255],
        NodeType::Unknown => [112, 112, 112, 255],
    }
}

pub(crate) fn port_type_color(port_type: PortType) -> [u8; 4] {
    match port_type {
        PortType::Audio => [87, 199, 133, 255],
        PortType::Video => [78, 157, 230, 255],
        PortType::MidiJack => [227, 93, 106, 255],
        PortType::MidiAlsa => [169, 121, 209, 255],
        PortType::Unknown => [165, 165, 165, 255],
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

pub(crate) fn node_layout_key(node: &Node) -> String {
    let kind = match node.node_type {
        NodeType::PipeWire => "PipeWire",
        NodeType::Effect => "Effect",
        NodeType::AlsaMidi => "AlsaMidi",
        NodeType::Unknown => "Unknown",
    };
    format!("{kind}:{}", node.name)
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

        let snapshot = state.snapshot_with_meters(&graph, &config, &meters, MeterState::Waiting);
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
}
