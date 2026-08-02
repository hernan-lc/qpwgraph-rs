use egui::Color32;
use pw_graph_core::{Direction, Node, NodeType, Port, PortType};
use pw_graph_i18n::I18n;

use super::super::ports::{port_role, PortRole};

pub(super) fn level_db(value: f32) -> f32 {
    (20.0 * value.max(0.000001).log10()).clamp(-120.0, 0.0)
}

pub(super) fn dominant_port<'a>(ports: &[&'a Port]) -> Option<&'a Port> {
    let mut counts: std::collections::HashMap<PortType, usize> = std::collections::HashMap::new();
    for port in ports {
        if port.port_type != PortType::Unknown {
            *counts.entry(port.port_type).or_insert(0) += 1;
        }
    }
    ports
        .iter()
        .copied()
        .filter(|port| port.port_type != PortType::Unknown)
        .max_by_key(|port| {
            let role_priority = match port_role(port) {
                PortRole::Monitor => 2,
                PortRole::Output => 1,
                PortRole::Input => 0,
            };
            (
                counts.get(&port.port_type).copied().unwrap_or_default(),
                role_priority,
                port.id,
            )
        })
}

pub(super) fn node_color(node_type: NodeType) -> Color32 {
    match node_type {
        NodeType::PipeWire => Color32::from_rgb(91, 172, 224),
        NodeType::AlsaMidi => Color32::from_rgb(180, 128, 220),
        NodeType::Unknown => Color32::from_rgb(153, 163, 175),
    }
}

pub(super) fn node_tooltip(node: &Node, ports: &[&Port], i18n: &I18n) -> String {
    let inputs = ports
        .iter()
        .filter(|port| port.direction == Direction::Sink)
        .count();
    let outputs = ports
        .iter()
        .filter(|port| port.direction == Direction::Source)
        .count();
    i18n.format(
        "canvas.node_tooltip",
        &[
            ("type", node_type_label(node.node_type, i18n)),
            ("name", node.name.clone()),
            ("inputs", inputs.to_string()),
            ("outputs", outputs.to_string()),
        ],
    )
}

pub(super) fn node_type_label(node_type: NodeType, i18n: &I18n) -> String {
    match node_type {
        NodeType::PipeWire => i18n.text("canvas.node_type_pipewire"),
        NodeType::AlsaMidi => i18n.text("canvas.node_type_alsa_midi"),
        NodeType::Unknown => i18n.text("canvas.node_type_unknown"),
    }
}
