use egui::Color32;
use pw_graph_core::{Direction, Node, NodeType, Port, PortType};
use pw_graph_i18n::I18n;

use super::super::ports::{port_role, PortRole};

pub(super) fn level_db(value: f32) -> f32 {
    (20.0 * value.max(0.000001).log10()).clamp(-120.0, 0.0)
}

pub(super) fn format_level_db(value: f32) -> String {
    if !value.is_finite() || value <= 0.000001 {
        "−∞ dB".into()
    } else {
        format!("{:.0} dB", level_db(value))
    }
}

/// Convert a linear amplitude to a readable −60..0 dBFS meter position.
/// Drawing the normalized sample value directly makes nearly every useful
/// audio level look empty (for example −30 dBFS is only 3.2% linearly).
pub(super) fn meter_fraction(value: f32) -> f32 {
    ((level_db(value) + 60.0) / 60.0).clamp(0.0, 1.0)
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

#[cfg(test)]
mod tests {
    use super::{format_level_db, level_db, meter_fraction};

    #[test]
    fn converts_linear_amplitude_to_dbfs() {
        assert!((level_db(1.0) - 0.0).abs() < 0.001);
        assert!((level_db(0.1) + 20.0).abs() < 0.001);
        assert_eq!(format_level_db(0.0), "−∞ dB");
    }

    #[test]
    fn maps_the_visible_meter_range_logarithmically() {
        assert_eq!(meter_fraction(0.001), 0.0);
        assert!((meter_fraction(0.01) - (1.0 / 3.0)).abs() < 0.001);
        assert_eq!(meter_fraction(1.0), 1.0);
    }
}
