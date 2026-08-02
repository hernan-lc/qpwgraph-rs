use pw_graph_backend::GraphDriver;
use pw_graph_config::AppConfig;
use pw_graph_core::{Node, NodeType};
use std::collections::BTreeMap;

pub(super) fn node_layout_key(node: &Node) -> String {
    let node_type = match node.node_type {
        NodeType::PipeWire => "PipeWire",
        NodeType::AlsaMidi => "AlsaMidi",
        NodeType::Unknown => "Unknown",
    };
    format!("{node_type}:{}", node.name)
}

/// Restore the user's last layout when possible and fill in missing nodes with
/// the deterministic default arrangement. Numeric IDs remain a fast path for
/// compatibility; the name key handles IDs that PipeWire reassigns later.
pub(super) fn restore_node_positions(driver: &mut dyn GraphDriver, config: &AppConfig) {
    let graph = driver.graph();
    let defaults = graph.default_node_positions();
    let mut key_counts = BTreeMap::new();
    for node in graph.nodes.values() {
        *key_counts.entry(node_layout_key(node)).or_insert(0_usize) += 1;
    }
    let positions: Vec<_> = graph
        .nodes
        .values()
        .map(|node| {
            let key = node_layout_key(node);
            let by_id = config.node_positions.get(&node.id.0.to_string()).copied();
            let by_name = if key_counts.get(&key) == Some(&1) {
                config.node_positions_by_name.get(&key).copied()
            } else {
                None
            };
            let position = by_id
                .or(by_name)
                .or_else(|| defaults.get(&node.id).copied())
                .unwrap_or(node.position);
            (node.id, position)
        })
        .collect();
    for (node, position) in positions {
        let _ = driver.set_node_position(node, position);
    }
}
