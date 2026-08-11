use crate::model::ConnectMode;
use pw_graph_core::{Direction, PortKey};
use std::time::Instant;

use super::app::{set_connection_feedback, PreviewApp};

pub(crate) fn handle_link_requested(preview: &mut PreviewApp, start_id: i32, end_id: i32) {
    if start_id != end_id {
        preview.pending_connection_pin = None;
        if preview.view.connect_mode == ConnectMode::Easy {
            // A rendered pin stands for a whole channel group here, so the
            // drag connects every channel it holds, not just the first one.
            easy_connect_pin_pair(preview, start_id, end_id);
        } else {
            connect_pin_pair(preview, start_id, end_id);
        }
        return;
    }

    match preview.pending_connection_pin.take() {
        None => {
            preview.pending_connection_pin = Some(start_id);
            set_connection_feedback(
                preview,
                "Connection started: click a destination pin or drag",
                false,
            );
        }
        Some(pending) if pending == start_id => {
            set_connection_feedback(preview, "Connection cancelled", false);
        }
        Some(pending) if preview.view.connect_mode == ConnectMode::Easy => {
            easy_connect_pin_pair(preview, pending, start_id);
        }
        Some(pending) => connect_pin_pair(preview, pending, start_id),
    }
}

pub(crate) fn connect_pin_pair(preview: &mut PreviewApp, start_id: i32, end_id: i32) {
    if start_id == end_id {
        set_connection_feedback(preview, "Connection cancelled", false);
        return;
    }

    let Some(start_port) = preview.view.ids.port_id(start_id) else {
        set_connection_feedback(
            preview,
            format!("Connection failed: source pin {start_id} is no longer available"),
            true,
        );
        return;
    };
    let Some(end_port) = preview.view.ids.port_id(end_id) else {
        set_connection_feedback(
            preview,
            format!("Connection failed: destination pin {end_id} is no longer available"),
            true,
        );
        return;
    };

    let (output, input) = {
        let graph = preview.source.graph();
        let Some(start) = graph.port(start_port) else {
            set_connection_feedback(preview, "Connection failed: source pin disappeared", true);
            return;
        };
        let Some(end) = graph.port(end_port) else {
            set_connection_feedback(
                preview,
                "Connection failed: destination pin disappeared",
                true,
            );
            return;
        };
        match (start.direction, end.direction) {
            (Direction::Source, Direction::Sink) => (start_port, end_port),
            (Direction::Sink, Direction::Source) => (end_port, start_port),
            _ => {
                set_connection_feedback(
                    preview,
                    "Connection failed: connect one output pin to one input pin",
                    true,
                );
                return;
            }
        }
    };

    let Some((output, input)) = ({
        let graph = preview.source.graph();
        graph.port_key(output).zip(graph.port_key(input))
    }) else {
        set_connection_feedback(
            preview,
            "Connection failed: pin identity is unavailable",
            true,
        );
        return;
    };

    match preview.source.connect_by_key_if_missing(&output, &input) {
        Ok(created) => match refresh_connection_graph(preview) {
            Ok(()) => {
                let message = if created {
                    "Connection created"
                } else {
                    "Connection already exists"
                };
                set_connection_feedback(preview, message, false);
            }
            Err(error) => set_connection_feedback(
                preview,
                format!("Connection succeeded, but graph refresh failed: {error}"),
                true,
            ),
        },
        Err(error) => set_connection_feedback(preview, format!("Connection failed: {error}"), true),
    }
}

pub(crate) fn refresh_connection_graph(preview: &mut PreviewApp) -> Result<(), String> {
    preview.source.refresh()?;
    preview.last_refresh = Instant::now();
    Ok(())
}

pub(crate) fn easy_connect_nodes(
    preview: &mut PreviewApp,
    source_id: i32,
    x: f32,
    y: f32,
    target_pin_id: i32,
) {
    let Some(source_node) = preview.view.ids.node_id(source_id) else {
        set_connection_feedback(preview, "Easy connect source is no longer available", true);
        return;
    };
    // Prefer the actual rendered pin under the release. This is authoritative
    // even when a transformed/captured TouchArea reports imperfect card-local
    // coordinates at the edge of another node.
    let target_from_pin = preview
        .view
        .ids
        .port_id(target_pin_id)
        .and_then(|port| preview.source.graph().port(port))
        .map(|port| port.node_id)
        .filter(|node| *node != source_node);
    if target_from_pin.is_some() {
        // The drop landed on a rendered pin, so fill just that group instead
        // of everything the destination card exposes.
        let port_keys = {
            let graph = preview.source.graph();
            preview
                .view
                .matching_group_to_node_pairs(graph, target_pin_id, source_node)
                .into_iter()
                .filter_map(|(output, input)| {
                    Some((graph.port_key(output)?, graph.port_key(input)?))
                })
                .collect::<Vec<_>>()
        };
        if !port_keys.is_empty() {
            apply_easy_pairs(preview, port_keys);
            return;
        }
    }
    let Some(target_node) = target_from_pin.or_else(|| {
        // A card-body drop has no pin identity. Keep coordinate hit-testing as
        // its fallback, including the small margin occupied by edge pins.
        preview
            .view
            .node_at(&preview.snapshot, x, y, source_node)
            .or_else(|| {
                preview
                    .view
                    .node_at_with_margin(&preview.snapshot, x, y, source_node, 12.0)
            })
    }) else {
        set_connection_feedback(
            preview,
            "Easy connect cancelled: drop onto another node",
            true,
        );
        return;
    };
    easy_connect_node_pair(preview, source_node, target_node);
}

pub(crate) fn easy_connect_from_pin(preview: &mut PreviewApp, source_pin_id: i32, x: f32, y: f32) {
    let Some(source_node) = preview
        .view
        .ids
        .port_id(source_pin_id)
        .and_then(|port| preview.source.graph().port(port))
        .map(|port| port.node_id)
    else {
        set_connection_feedback(
            preview,
            "Easy connect source pin is no longer available",
            true,
        );
        return;
    };
    let Some(source_id) = preview.view.ids.node(source_node) else {
        set_connection_feedback(preview, "Easy connect source is no longer available", true);
        return;
    };
    // The drag began on a pin, so connect that group's channels rather than
    // everything the destination card happens to expose. Whole-card pairing
    // stays as the fallback when the group has nothing to face.
    let target_node = preview
        .view
        .node_at(&preview.snapshot, x, y, source_node)
        .or_else(|| {
            preview
                .view
                .node_at_with_margin(&preview.snapshot, x, y, source_node, 12.0)
        });
    if let Some(target_node) = target_node {
        let port_keys = {
            let graph = preview.source.graph();
            preview
                .view
                .matching_group_to_node_pairs(graph, source_pin_id, target_node)
                .into_iter()
                .filter_map(|(output, input)| {
                    Some((graph.port_key(output)?, graph.port_key(input)?))
                })
                .collect::<Vec<_>>()
        };
        if !port_keys.is_empty() {
            preview.pending_connection_pin = None;
            apply_easy_pairs(preview, port_keys);
            return;
        }
    }
    easy_connect_nodes(preview, source_id, x, y, 0);
}

fn easy_connect_pin_pair(preview: &mut PreviewApp, source_pin_id: i32, target_pin_id: i32) {
    let nodes = {
        let graph = preview.source.graph();
        preview
            .view
            .ids
            .port_id(source_pin_id)
            .and_then(|source| graph.port(source))
            .map(|port| port.node_id)
            .zip(
                preview
                    .view
                    .ids
                    .port_id(target_pin_id)
                    .and_then(|target| graph.port(target))
                    .map(|port| port.node_id),
            )
    };
    let Some((source_node, target_node)) = nodes else {
        set_connection_feedback(preview, "Easy connect pin is no longer available", true);
        return;
    };
    if source_node == target_node {
        set_connection_feedback(preview, "Connection cancelled: select another node", false);
        return;
    }

    // Pair the channels inside the two groups the user actually aimed at.
    let port_keys = {
        let graph = preview.source.graph();
        preview
            .view
            .matching_pin_pairs(graph, source_pin_id, target_pin_id)
            .into_iter()
            .filter_map(|(output, input)| Some((graph.port_key(output)?, graph.port_key(input)?)))
            .collect::<Vec<_>>()
    };
    if port_keys.is_empty() {
        set_connection_feedback(
            preview,
            "Easy connect needs one output pin and one input pin",
            true,
        );
        return;
    }
    apply_easy_pairs(preview, port_keys);
}

fn easy_connect_node_pair(
    preview: &mut PreviewApp,
    source_node: pw_graph_core::NodeId,
    target_node: pw_graph_core::NodeId,
) {
    let port_keys = {
        let graph = preview.source.graph();
        preview
            .view
            .matching_port_pairs(graph, source_node, target_node)
            .into_iter()
            .filter_map(|(output, input)| Some((graph.port_key(output)?, graph.port_key(input)?)))
            .collect::<Vec<_>>()
    };
    if port_keys.is_empty() {
        set_connection_feedback(
            preview,
            "Easy connect found no compatible output/input ports",
            true,
        );
        return;
    }
    apply_easy_pairs(preview, port_keys);
}

/// Create every pair an Easy-mode gesture resolved to, then report what
/// happened as a single message.
fn apply_easy_pairs(preview: &mut PreviewApp, port_keys: Vec<(PortKey, PortKey)>) {
    let mut connected = 0usize;
    let mut already_connected = 0usize;
    for (output, input) in port_keys {
        match preview.source.connect_by_key_if_missing(&output, &input) {
            Ok(true) => connected += 1,
            Ok(false) => already_connected += 1,
            Err(error) => {
                let _ = refresh_connection_graph(preview);
                set_connection_feedback(
                    preview,
                    format!("Easy connect created {connected} connection(s), then failed: {error}"),
                    true,
                );
                return;
            }
        }
    }
    match refresh_connection_graph(preview) {
        Ok(()) => {
            let message = if connected == 0 {
                format!("Easy connect: {already_connected} connection(s) already exist")
            } else if already_connected == 0 {
                format!("Easy connect created {connected} connection(s)")
            } else {
                format!(
                    "Easy connect created {connected} connection(s); {already_connected} already exist"
                )
            };
            set_connection_feedback(preview, message, false);
        }
        Err(error) => {
            set_connection_feedback(
                preview,
                format!("Easy connect succeeded, but graph refresh failed: {error}"),
                true,
            );
        }
    }
}

pub(crate) fn delete_selected_connections(preview: &mut PreviewApp) {
    let keys = {
        let graph = preview.source.graph();
        preview
            .view
            .selected_links
            .iter()
            .filter_map(|id| {
                let link = graph.link(*id)?;
                Some((
                    graph.port_key(link.output_port)?,
                    graph.port_key(link.input_port)?,
                ))
            })
            .collect::<Vec<_>>()
    };
    if keys.is_empty() {
        set_connection_feedback(preview, "Select a connection before deleting", true);
        return;
    }

    let mut removed = 0usize;
    for (output, input) in keys {
        match preview.source.disconnect_by_key_if_present(&output, &input) {
            Ok(true) => removed += 1,
            Ok(false) => {}
            Err(error) => {
                let _ = refresh_connection_graph(preview);
                set_connection_feedback(
                    preview,
                    format!("Removed {removed} connection(s), then failed: {error}"),
                    true,
                );
                return;
            }
        }
    }
    preview.view.clear_selection();
    match refresh_connection_graph(preview) {
        Ok(()) => {
            set_connection_feedback(preview, format!("Removed {removed} connection(s)"), false)
        }
        Err(error) => set_connection_feedback(
            preview,
            format!("Connection removed, but graph refresh failed: {error}"),
            true,
        ),
    }
}
