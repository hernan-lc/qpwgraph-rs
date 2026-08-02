//! Port grouping and connection-matching logic shared by node and link
//! rendering.
//!
//! Advanced mode always renders one row per port. Easy mode collapses ports
//! that share a base name and only differ by a recognized channel suffix
//! (FL/FR, L/R, a full 5.1 set, ...) into a single row, so a stereo pair
//! shows up once instead of twice; dragging that row connects every channel
//! it represents in one gesture.

use super::names::display_port_name;
use crate::ConnectMode;
use egui::Color32;
use pw_graph_core::{Direction, Graph, Node, Port, PortId, PortType};
use pw_graph_i18n::I18n;
use std::collections::HashMap;

const CHANNEL_DELIMITERS: [char; 5] = ['_', '-', ' ', ':', '.'];
const CHANNEL_TOKENS: [&str; 34] = [
    "FL", "FR", "RL", "RR", "SL", "SR", "FC", "RC", "LFE", "MONO", "LEFT", "RIGHT", "L", "R", "C",
    "FLC", "FRC", "TC", "TFL", "TFR", "TFC", "TRL", "TRR", "TRC", "BFL", "BFR", "BFC", "BL", "BR",
    "BC", "BLC", "BRC", "TBL", "TBR",
];

/// A row of one or more ports rendered together. Advanced mode only ever
/// produces single-port groups; Easy mode may merge several.
pub(crate) struct PortGroup<'a> {
    pub label: String,
    pub direction: Direction,
    pub port_type: PortType,
    pub ports: Vec<&'a Port>,
}

impl<'a> PortGroup<'a> {
    fn new(ports: Vec<&'a Port>, label: String) -> Self {
        let representative = ports[0];
        Self {
            label,
            direction: representative.direction,
            port_type: representative.port_type,
            ports,
        }
    }

    pub(crate) fn representative(&self) -> &Port {
        self.ports[0]
    }

    pub(crate) fn contains(&self, port_id: PortId) -> bool {
        self.ports.iter().any(|port| port.id == port_id)
    }
}

/// Groups `ports` (already in display order) into rows for rendering.
pub(crate) fn grouped_rows<'a>(mode: ConnectMode, ports: &[&'a Port]) -> Vec<Vec<&'a Port>> {
    if mode != ConnectMode::Easy {
        return ports.iter().map(|port| vec![*port]).collect();
    }
    let mut rows: Vec<Vec<&Port>> = Vec::new();
    let mut row_by_key: HashMap<(Direction, PortType, String), usize> = HashMap::new();
    for port in ports {
        if let Some(key) = channel_group_key(port) {
            if let Some(&row) = row_by_key.get(&key) {
                rows[row].push(port);
                continue;
            }
            let row_index = rows.len();
            rows.push(vec![*port]);
            row_by_key.insert(key, row_index);
        } else {
            rows.push(vec![*port]);
        }
    }
    rows
}

/// Builds the labeled, ordered groups a node renders in `mode`. Only a
/// group that actually merged sibling ports gets the shortened "base name"
/// label; a lone port always keeps its full display name.
pub(crate) fn display_groups<'a>(
    mode: ConnectMode,
    ports: Vec<&'a Port>,
    i18n: &I18n,
) -> Vec<PortGroup<'a>> {
    grouped_rows(mode, &ports)
        .into_iter()
        .map(|group| {
            let label = if group.len() > 1 {
                channel_base_name(group[0])
                    .filter(|base| !base.is_empty())
                    .map(|base| display_port_name(base, i18n))
                    .unwrap_or_else(|| i18n.text("canvas.channel_group_label"))
            } else {
                display_port_name(&group[0].name, i18n)
            };
            PortGroup::new(group, label)
        })
        .collect()
}

fn channel_group_key(port: &Port) -> Option<(Direction, PortType, String)> {
    if port.port_type != PortType::Audio {
        return None;
    }
    let base = channel_base_name(port)?;
    Some((port.direction, port.port_type, base.to_ascii_lowercase()))
}

/// Finds the shared bus name for an audio port. Prefer the semantic channel
/// position supplied by the backend; otherwise recognize a conservative
/// trailing channel token (FL, FR, L, R, LFE, ...) in the display name. `None`
/// means the port should stay on its own row.
fn channel_base_name(port: &Port) -> Option<&str> {
    let name = port.name.as_str();

    // PipeWire exposes the semantic channel position separately from the
    // display name. When it is present, trust the backend metadata and allow
    // names such as `output_1` whose suffix is not itself `FL`/`FR`. The name
    // still supplies the shared bus prefix. Unknown/empty metadata falls
    // through to the conservative display-name parser below.
    if let Some(channel) = port.channel.as_deref() {
        if is_backend_channel_position(channel) {
            if let Some(position) =
                name.rfind(|character: char| CHANNEL_DELIMITERS.contains(&character))
            {
                let (base, _) = name.split_at(position);
                if !base.is_empty() {
                    return Some(base);
                }
            }
            return if is_channel_token(name) {
                Some("")
            } else if name.is_empty() {
                None
            } else {
                Some(name)
            };
        }
    }

    if let Some(position) = name.rfind(|character: char| CHANNEL_DELIMITERS.contains(&character)) {
        let (base, rest) = name.split_at(position);
        let token = &rest[1..];
        if !base.is_empty() && is_channel_token(token) {
            return Some(base);
        }
    }
    // Bare names like "FL"/"R" have no delimiter to split on, but the whole
    // name is itself the channel token.
    is_channel_token(name).then_some("")
}

fn is_channel_token(token: &str) -> bool {
    let token = token.trim();
    CHANNEL_TOKENS
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(token))
        || token
            .strip_prefix("AUX")
            .is_some_and(|suffix| !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()))
}

fn is_backend_channel_position(channel: &str) -> bool {
    let channel = channel.trim();
    !channel.is_empty()
        && !matches!(
            channel.to_ascii_uppercase().as_str(),
            "UNKNOWN" | "UNDEFINED" | "NONE" | "NA"
        )
}

pub(crate) fn ports_compatible(a: PortType, b: PortType) -> bool {
    a == b || a == PortType::Unknown || b == PortType::Unknown
}

/// Pairs outputs and inputs by channel position when PipeWire exposes it. A
/// channel-aware score prevents a reversed registry order from wiring FL to
/// FR; only ports without channel metadata fall back to display order.
pub(crate) fn pair_ports(outputs: &[&Port], inputs: &[&Port]) -> Vec<(PortId, PortId)> {
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

fn channels_can_pair(output: &Port, input: &Port) -> bool {
    match (channel_identity(output), channel_identity(input)) {
        (Some(output), Some(input)) => output == input,
        _ => true,
    }
}

fn channel_pair_score(output: &Port, input: &Port) -> u8 {
    match (channel_identity(output), channel_identity(input)) {
        (Some(output), Some(input)) if output == input => 100,
        (Some(_), Some(_)) => 0,
        (Some(_), None) | (None, Some(_)) => 20,
        (None, None) => 10,
    }
}

fn name_pair_score(output: &Port, input: &Port) -> u8 {
    match (channel_base_name(output), channel_base_name(input)) {
        (Some(output), Some(input)) if !output.is_empty() && output.eq_ignore_ascii_case(input) => {
            10
        }
        _ => 0,
    }
}

fn channel_identity(port: &Port) -> Option<String> {
    let raw = port
        .channel
        .as_deref()
        .filter(|channel| is_backend_channel_position(channel))
        .or_else(|| trailing_channel_token(&port.name))?;
    Some(normalize_channel(raw))
}

fn trailing_channel_token(name: &str) -> Option<&str> {
    let position = name.rfind(|character: char| CHANNEL_DELIMITERS.contains(&character))?;
    let token = &name[position + 1..];
    is_channel_token(token).then_some(token)
}

fn normalize_channel(channel: &str) -> String {
    let compact: String = channel
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_uppercase)
        .collect();
    match compact.as_str() {
        "FRONTLEFT" | "LEFT" | "L" => "FL".into(),
        "FRONTRIGHT" | "RIGHT" | "R" => "FR".into(),
        "REARLEFT" | "BACKLEFT" => "RL".into(),
        "REARRIGHT" | "BACKRIGHT" => "RR".into(),
        "SIDELEFT" => "SL".into(),
        "SIDERIGHT" => "SR".into(),
        "CENTER" | "FC" | "C" => "C".into(),
        "LOWFREQUENCY" => "LFE".into(),
        _ => compact,
    }
}

pub(crate) fn link_exists(graph: &Graph, output: PortId, input: PortId) -> bool {
    graph
        .links
        .values()
        .any(|link| link.output_port == output && link.input_port == input)
}

/// The visual role of a port within its media family.
///
/// Monitor ports are detected from their name because PipeWire represents
/// them as ordinary input/output ports. They intentionally get their own
/// shade so a monitor path remains recognizable without introducing a new
/// media category.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PortRole {
    Input,
    Output,
    Monitor,
}

pub(crate) fn port_role(port: &Port) -> PortRole {
    if port.name.to_ascii_lowercase().contains("monitor") {
        PortRole::Monitor
    } else if port.direction.is_source() {
        PortRole::Output
    } else {
        PortRole::Input
    }
}

/// Color palette shared by port dots, node accents and the minimap.
///
/// Each media type keeps its own hue family. Within that family, input is the
/// darkest shade, output is brighter, and monitor is the lightest shade.
pub(crate) fn port_color(port_type: PortType, role: PortRole) -> Color32 {
    match (port_type, role) {
        (PortType::Audio, PortRole::Input) => Color32::from_rgb(44, 151, 96),
        (PortType::Audio, PortRole::Output) => Color32::from_rgb(82, 207, 133),
        (PortType::Audio, PortRole::Monitor) => Color32::from_rgb(139, 231, 177),
        (PortType::Video, PortRole::Input) => Color32::from_rgb(43, 125, 202),
        (PortType::Video, PortRole::Output) => Color32::from_rgb(91, 181, 244),
        (PortType::Video, PortRole::Monitor) => Color32::from_rgb(151, 213, 255),
        (PortType::MidiJack, PortRole::Input) => Color32::from_rgb(186, 57, 87),
        (PortType::MidiJack, PortRole::Output) => Color32::from_rgb(237, 108, 128),
        (PortType::MidiJack, PortRole::Monitor) => Color32::from_rgb(255, 161, 177),
        (PortType::MidiAlsa, PortRole::Input) => Color32::from_rgb(128, 78, 172),
        (PortType::MidiAlsa, PortRole::Output) => Color32::from_rgb(190, 132, 225),
        (PortType::MidiAlsa, PortRole::Monitor) => Color32::from_rgb(220, 177, 245),
        (PortType::Unknown, PortRole::Input) => Color32::from_rgb(116, 127, 141),
        (PortType::Unknown, PortRole::Output) => Color32::from_rgb(177, 188, 202),
        (PortType::Unknown, PortRole::Monitor) => Color32::from_rgb(214, 222, 232),
    }
}

/// Slightly muted version of the port palette for connection lines.
pub(crate) fn link_color(port_type: PortType, role: PortRole) -> Color32 {
    match (port_type, role) {
        (PortType::Audio, PortRole::Input) => Color32::from_rgb(38, 126, 80),
        (PortType::Audio, PortRole::Output) => Color32::from_rgb(62, 173, 109),
        (PortType::Audio, PortRole::Monitor) => Color32::from_rgb(105, 194, 145),
        (PortType::Video, PortRole::Input) => Color32::from_rgb(37, 105, 170),
        (PortType::Video, PortRole::Output) => Color32::from_rgb(69, 147, 204),
        (PortType::Video, PortRole::Monitor) => Color32::from_rgb(112, 177, 218),
        (PortType::MidiJack, PortRole::Input) => Color32::from_rgb(151, 48, 72),
        (PortType::MidiJack, PortRole::Output) => Color32::from_rgb(198, 83, 105),
        (PortType::MidiJack, PortRole::Monitor) => Color32::from_rgb(220, 125, 145),
        (PortType::MidiAlsa, PortRole::Input) => Color32::from_rgb(104, 62, 141),
        (PortType::MidiAlsa, PortRole::Output) => Color32::from_rgb(157, 105, 191),
        (PortType::MidiAlsa, PortRole::Monitor) => Color32::from_rgb(187, 145, 213),
        (PortType::Unknown, PortRole::Input) => Color32::from_rgb(93, 103, 116),
        (PortType::Unknown, PortRole::Output) => Color32::from_rgb(143, 155, 171),
        (PortType::Unknown, PortRole::Monitor) => Color32::from_rgb(178, 188, 202),
    }
}

pub(crate) fn port_type_label(port_type: PortType, i18n: &I18n) -> String {
    match port_type {
        PortType::Audio => i18n.text("port.audio"),
        PortType::Video => i18n.text("port.video"),
        PortType::MidiJack => i18n.text("port.pw_midi"),
        PortType::MidiAlsa => i18n.text("port.alsa_midi"),
        PortType::Unknown => i18n.text("canvas.unknown"),
    }
}

pub(crate) fn port_tooltip(node: &Node, port: &Port, i18n: &I18n) -> String {
    let direction = if port.direction == Direction::Source {
        i18n.text("canvas.output")
    } else {
        i18n.text("canvas.input")
    };
    i18n.format(
        "canvas.port_tooltip",
        &[
            ("direction", direction),
            ("type", port_type_label(port.port_type, i18n)),
            ("port", port.name.clone()),
            ("node", node.name.clone()),
            (
                "help",
                if port.direction == Direction::Source {
                    i18n.text("canvas.output_help")
                } else {
                    i18n.text("canvas.input_help")
                },
            ),
        ],
    )
}

/// Tooltip for a rendered row: a single-port group reuses [`port_tooltip`]
/// verbatim, while a merged group lists every channel it represents.
pub(crate) fn port_group_tooltip(node: &Node, group: &PortGroup, i18n: &I18n) -> String {
    if group.ports.len() == 1 {
        return port_tooltip(node, group.ports[0], i18n);
    }
    let direction = if group.direction == Direction::Source {
        i18n.text("canvas.output")
    } else {
        i18n.text("canvas.input")
    };
    let channels = group
        .ports
        .iter()
        .map(|port| display_port_name(&port.name, i18n))
        .collect::<Vec<_>>()
        .join(", ");
    i18n.format(
        "canvas.port_group_tooltip",
        &[
            ("direction", direction),
            ("type", port_type_label(group.port_type, i18n)),
            ("port", group.label.clone()),
            ("node", node.name.clone()),
            ("channels", channels),
            (
                "help",
                if group.direction == Direction::Source {
                    i18n.text("canvas.output_group_help")
                } else {
                    i18n.text("canvas.input_group_help")
                },
            ),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use pw_graph_core::{LinkId, NodeId, NodeType};

    #[test]
    fn port_role_distinguishes_direction_and_monitor_names() {
        let input = Port::new(
            PortId(1),
            NodeId(1),
            "capture_FL",
            Direction::Sink,
            PortType::Audio,
        );
        let output = Port::new(
            PortId(2),
            NodeId(1),
            "playback_FL",
            Direction::Source,
            PortType::Audio,
        );
        let monitor = Port::new(
            PortId(3),
            NodeId(1),
            "Monitor_FL",
            Direction::Source,
            PortType::Audio,
        );

        assert_eq!(port_role(&input), PortRole::Input);
        assert_eq!(port_role(&output), PortRole::Output);
        assert_eq!(port_role(&monitor), PortRole::Monitor);
    }

    #[test]
    fn media_palette_keeps_role_shades_in_the_same_family() {
        let input = port_color(PortType::Audio, PortRole::Input);
        let output = port_color(PortType::Audio, PortRole::Output);
        let monitor = port_color(PortType::Audio, PortRole::Monitor);

        assert_ne!(input, output);
        assert_ne!(output, monitor);
        assert!(input.g() < output.g());
        assert!(output.g() < monitor.g());
    }

    fn stereo_pair_graph() -> Graph {
        let mut graph = Graph::default();
        graph
            .add_node(Node::new(NodeId(1), "Source", NodeType::PipeWire))
            .unwrap();
        graph
            .add_node(Node::new(NodeId(2), "Sink", NodeType::PipeWire))
            .unwrap();
        graph
            .add_port(Port::new(
                PortId(10),
                NodeId(1),
                "out_L",
                Direction::Source,
                PortType::Audio,
            ))
            .unwrap();
        graph
            .add_port(Port::new(
                PortId(11),
                NodeId(1),
                "out_R",
                Direction::Source,
                PortType::Audio,
            ))
            .unwrap();
        graph
            .add_port(Port::new(
                PortId(20),
                NodeId(2),
                "in_L",
                Direction::Sink,
                PortType::Audio,
            ))
            .unwrap();
        graph
            .add_port(Port::new(
                PortId(21),
                NodeId(2),
                "in_R",
                Direction::Sink,
                PortType::Audio,
            ))
            .unwrap();
        graph
    }

    #[test]
    fn pair_ports_zips_outputs_to_inputs_in_order() {
        let graph = stereo_pair_graph();
        let source = graph.node(NodeId(1)).unwrap();
        let target = graph.node(NodeId(2)).unwrap();
        let outputs: Vec<&Port> = source
            .ports
            .iter()
            .filter_map(|id| graph.port(*id))
            .filter(|port| port.direction == Direction::Source)
            .collect();
        let inputs: Vec<&Port> = target
            .ports
            .iter()
            .filter_map(|id| graph.port(*id))
            .filter(|port| port.direction == Direction::Sink)
            .collect();
        assert_eq!(
            pair_ports(&outputs, &inputs),
            vec![(PortId(10), PortId(20)), (PortId(11), PortId(21))]
        );
    }

    #[test]
    fn pair_ports_prefers_matching_channels_over_registry_order() {
        let mut graph = stereo_pair_graph();
        graph.ports.get_mut(&PortId(20)).unwrap().name = "in_R".into();
        graph.ports.get_mut(&PortId(21)).unwrap().name = "in_L".into();
        let source = graph.node(NodeId(1)).unwrap();
        let target = graph.node(NodeId(2)).unwrap();
        let outputs: Vec<&Port> = source
            .ports
            .iter()
            .filter_map(|id| graph.port(*id))
            .collect();
        let inputs: Vec<&Port> = target
            .ports
            .iter()
            .filter_map(|id| graph.port(*id))
            .collect();
        assert_eq!(
            pair_ports(&outputs, &inputs),
            vec![(PortId(10), PortId(21)), (PortId(11), PortId(20))]
        );
    }

    #[test]
    fn link_exists_matches_only_the_exact_output_input_pair() {
        let mut graph = stereo_pair_graph();
        graph.add_link(LinkId(1), PortId(10), PortId(20)).unwrap();
        assert!(link_exists(&graph, PortId(10), PortId(20)));
        assert!(!link_exists(&graph, PortId(11), PortId(20)));
        assert!(!link_exists(&graph, PortId(10), PortId(21)));
    }

    #[test]
    fn advanced_mode_never_groups_ports() {
        let graph = stereo_pair_graph();
        let node = graph.node(NodeId(1)).unwrap();
        let ports: Vec<&Port> = node.ports.iter().filter_map(|id| graph.port(*id)).collect();
        let rows = grouped_rows(ConnectMode::Advanced, &ports);
        assert_eq!(rows.len(), ports.len());
    }

    #[test]
    fn easy_mode_merges_a_stereo_pair_into_one_row() {
        let graph = stereo_pair_graph();
        let node = graph.node(NodeId(1)).unwrap();
        let mut ports: Vec<&Port> = node.ports.iter().filter_map(|id| graph.port(*id)).collect();
        ports.sort_by_key(|port| port.name.clone());
        let rows = grouped_rows(ConnectMode::Easy, &ports);
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].iter().map(|port| port.id).collect::<Vec<_>>(),
            vec![PortId(10), PortId(11)]
        );
    }

    #[test]
    fn easy_mode_keeps_unrelated_ports_on_separate_rows() {
        let mut graph = Graph::default();
        graph
            .add_node(Node::new(NodeId(1), "Source", NodeType::PipeWire))
            .unwrap();
        graph
            .add_port(Port::new(
                PortId(1),
                NodeId(1),
                "input_1",
                Direction::Source,
                PortType::MidiJack,
            ))
            .unwrap();
        graph
            .add_port(Port::new(
                PortId(2),
                NodeId(1),
                "input_2",
                Direction::Source,
                PortType::MidiJack,
            ))
            .unwrap();
        let node = graph.node(NodeId(1)).unwrap();
        let ports: Vec<&Port> = node.ports.iter().filter_map(|id| graph.port(*id)).collect();
        let rows = grouped_rows(ConnectMode::Easy, &ports);
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn display_groups_labels_a_merged_pair_with_the_shared_base_name() {
        let graph = stereo_pair_graph();
        let i18n = I18n::default();
        let node = graph.node(NodeId(1)).unwrap();
        let mut ports: Vec<&Port> = node.ports.iter().filter_map(|id| graph.port(*id)).collect();
        ports.sort_by_key(|port| port.name.clone());
        let groups = display_groups(ConnectMode::Easy, ports, &i18n);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].label, "Out");
        assert_eq!(groups[0].ports.len(), 2);
    }

    #[test]
    fn backend_channel_metadata_groups_numeric_port_suffixes() {
        let mut graph = Graph::default();
        graph
            .add_node(Node::new(NodeId(1), "Source", NodeType::PipeWire))
            .unwrap();
        graph
            .add_port(
                Port::new(
                    PortId(1),
                    NodeId(1),
                    "output_1",
                    Direction::Source,
                    PortType::Audio,
                )
                .with_channel("FL"),
            )
            .unwrap();
        graph
            .add_port(
                Port::new(
                    PortId(2),
                    NodeId(1),
                    "output_2",
                    Direction::Source,
                    PortType::Audio,
                )
                .with_channel("FR"),
            )
            .unwrap();

        let node = graph.node(NodeId(1)).unwrap();
        let ports: Vec<&Port> = node.ports.iter().filter_map(|id| graph.port(*id)).collect();
        assert_eq!(grouped_rows(ConnectMode::Easy, &ports).len(), 1);
    }

    #[test]
    fn backend_metadata_is_authoritative_for_new_channel_positions() {
        let mut first = Port::new(
            PortId(1),
            NodeId(1),
            "output_1",
            Direction::Source,
            PortType::Audio,
        )
        .with_channel("TopFrontLeft");
        let second = Port::new(
            PortId(2),
            NodeId(1),
            "output_2",
            Direction::Source,
            PortType::Audio,
        )
        .with_channel("TopFrontRight");
        assert_eq!(channel_base_name(&first), Some("output"));
        assert_eq!(channel_base_name(&second), Some("output"));

        first.channel = Some("UNKNOWN".into());
        assert_eq!(channel_base_name(&first), None);
    }

    #[test]
    fn channel_token_matching_is_case_insensitive_without_rewriting_the_name() {
        assert!(is_channel_token("fl"));
        assert!(is_channel_token(" Right "));
        assert!(is_channel_token("AUX12"));
        assert!(!is_channel_token("1"));
    }
}
