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

/// Greedily pairs `outputs` with `inputs` in order, matching compatible
/// port types. Shared by whole-node connects, single-group-row connects,
/// and the "connect through nodes" click shortcut.
pub(crate) fn pair_ports(outputs: &[&Port], inputs: &[&Port]) -> Vec<(PortId, PortId)> {
    let mut used = vec![false; inputs.len()];
    let mut pairs = Vec::new();
    for output in outputs {
        let candidate = inputs.iter().enumerate().position(|(index, input)| {
            !used[index] && ports_compatible(output.port_type, input.port_type)
        });
        if let Some(index) = candidate {
            used[index] = true;
            pairs.push((output.id, inputs[index].id));
        }
    }
    pairs
}

pub(crate) fn link_exists(graph: &Graph, output: PortId, input: PortId) -> bool {
    graph
        .links
        .values()
        .any(|link| link.output_port == output && link.input_port == input)
}

pub(crate) fn port_color(port_type: PortType) -> Color32 {
    match port_type {
        PortType::Audio => Color32::from_rgb(87, 199, 133),
        PortType::Video => Color32::from_rgb(78, 157, 230),
        PortType::MidiJack => Color32::from_rgb(227, 93, 106),
        PortType::MidiAlsa => Color32::from_rgb(169, 121, 209),
        PortType::Unknown => Color32::from_rgb(165, 165, 165),
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
