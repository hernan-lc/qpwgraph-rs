//! Moving nodes: where a drag may land, and how saved positions are
//! restored onto a freshly discovered graph.

use super::*;

pub(super) const COLLISION_GAP: f32 = 18.0;

/// Resolve a user-requested drag against the exact visible card rectangles.
///
/// Every selected node receives the same returned delta, preserving the group
/// as a rigid object. Candidate positions come from all four edges of every
/// visible stationary card and are ranked by distance from the requested drop,
/// with a stable coordinate tie-breaker.
pub(crate) fn resolve_drag_delta(
    snapshot: &GraphSnapshot,
    selected: &BTreeSet<NodeId>,
    desired: [f32; 2],
    repel: bool,
) -> [f32; 2] {
    if !repel || selected.is_empty() {
        return desired;
    }

    let dragged = snapshot
        .nodes
        .iter()
        .filter(|node| selected.contains(&node.node_id))
        .collect::<Vec<_>>();
    let stationary = snapshot
        .nodes
        .iter()
        .filter(|node| !selected.contains(&node.node_id))
        .collect::<Vec<_>>();
    if dragged.is_empty() || stationary.is_empty() || drag_is_clear(&dragged, &stationary, desired)
    {
        return desired;
    }

    let mut xs = vec![desired[0]];
    let mut ys = vec![desired[1]];
    for moving in &dragged {
        for obstacle in &stationary {
            xs.push(obstacle.position[0] - COLLISION_GAP - moving.width - moving.position[0]);
            xs.push(obstacle.position[0] + obstacle.width + COLLISION_GAP - moving.position[0]);
            ys.push(obstacle.position[1] - COLLISION_GAP - moving.height - moving.position[1]);
            ys.push(obstacle.position[1] + obstacle.height + COLLISION_GAP - moving.position[1]);
        }
    }
    xs.sort_by(f32::total_cmp);
    xs.dedup_by(|left, right| left.total_cmp(right).is_eq());
    ys.sort_by(f32::total_cmp);
    ys.dedup_by(|left, right| left.total_cmp(right).is_eq());

    xs.into_iter()
        .flat_map(|x| ys.iter().copied().map(move |y| [x, y]))
        .filter(|candidate| drag_is_clear(&dragged, &stationary, *candidate))
        .min_by(|left, right| {
            drag_distance_squared(*left, desired)
                .total_cmp(&drag_distance_squared(*right, desired))
                .then_with(|| left[1].total_cmp(&right[1]))
                .then_with(|| left[0].total_cmp(&right[0]))
        })
        .unwrap_or(desired)
}

pub(super) fn drag_is_clear(
    dragged: &[&NodeView],
    stationary: &[&NodeView],
    delta: [f32; 2],
) -> bool {
    dragged.iter().all(|moving| {
        let position = [moving.position[0] + delta[0], moving.position[1] + delta[1]];
        stationary.iter().all(|obstacle| {
            !intersects(
                position,
                [moving.width, moving.height],
                obstacle.position[0] - COLLISION_GAP,
                obstacle.position[1] - COLLISION_GAP,
                obstacle.width + COLLISION_GAP * 2.0,
                obstacle.height + COLLISION_GAP * 2.0,
            )
        })
    })
}

pub(super) fn drag_distance_squared(candidate: [f32; 2], desired: [f32; 2]) -> f32 {
    let dx = candidate[0] - desired[0];
    let dy = candidate[1] - desired[1];
    dx * dx + dy * dy
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
