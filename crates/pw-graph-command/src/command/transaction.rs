//! The atomicity rules every command shares.
//!
//! A command that touches several links has to be all-or-nothing, so each
//! one records what it removed and can put it back. These helpers are
//! deliberately explicit rather than a generic transaction engine: the
//! rollback semantics differ enough between connect, reroute and disconnect
//! that readable repetition beats a clever abstraction over them.

use super::*;

/// A link's two endpoints, identified by stable name rather than by id.
pub(super) type EndpointPair = (PortKey, PortKey);

pub(super) fn stable_pair(graph: &Graph, output: PortId, input: PortId) -> Option<EndpointPair> {
    Some((graph.port_key(output)?, graph.port_key(input)?))
}

/// Collapse repeated endpoint pairs while preserving order.
pub(super) fn dedup_pairs(
    pairs: impl IntoIterator<Item = (PortKey, PortKey)>,
) -> Vec<(PortKey, PortKey)> {
    let mut unique = Vec::new();
    for pair in pairs {
        if !unique.contains(&pair) {
            unique.push(pair);
        }
    }
    unique
}

pub(super) fn pair_description(name: &str, keys: &Option<(PortKey, PortKey)>) -> String {
    keys.as_ref()
        .map(|(output, input)| {
            format!(
                "{name}: {} → {}",
                port_description(output),
                port_description(input)
            )
        })
        .unwrap_or_else(|| name.to_owned())
}

/// Disconnect every stable pair, recording which ones were actually removed.
/// Shared by the many/all disconnect commands.
///
/// The operation is all-or-nothing. Returning early on the first failure —
/// as this used to — left the earlier links disconnected *and* kept the
/// command off the undo stack, because `CommandStack::execute` only records
/// commands that succeeded. The user was left with a partly-torn graph and no
/// way to undo it. Now a failure rolls the earlier removals back, and only a
/// failed rollback is reported as such.
pub(super) fn disconnect_keys(
    driver: &mut dyn GraphDriver,
    operation: &'static str,
    keys: &[(PortKey, PortKey)],
    removed_keys: &mut Vec<(PortKey, PortKey)>,
) -> Result<Vec<Link>, CommandError> {
    removed_keys.clear();
    let mut disconnected = Vec::with_capacity(keys.len());
    for (output, input) in keys {
        // A composite can expose observed relationships alongside mutable
        // links (Windows Core Audio sessions are the important example).
        // Stable-key commands must not turn a broad disconnect action into a
        // request to delete something the owning backend explicitly protects.
        if let Some(link) = driver.graph().find_link_by_keys(output, input) {
            if !driver.is_link_mutable(link.id) {
                continue;
            }
        }
        match driver.disconnect_by_key_if_present(output, input) {
            Ok(Some(link)) => {
                disconnected.push(link);
                removed_keys.push((output.clone(), input.clone()));
            }
            Ok(None) => {}
            Err(error) => {
                let stranded = rollback_disconnects(driver, removed_keys);
                removed_keys.clear();
                return Err(if stranded == 0 {
                    error.into()
                } else {
                    CommandError::PartiallyApplied {
                        operation,
                        cause: error.to_string(),
                        stranded,
                    }
                });
            }
        }
    }
    Ok(disconnected)
}

/// Reconnect everything a failed group disconnect had already removed.
/// Returns how many could not be restored.
pub(super) fn rollback_disconnects(
    driver: &mut dyn GraphDriver,
    removed_keys: &[(PortKey, PortKey)],
) -> usize {
    let mut stranded = 0;
    for (output, input) in removed_keys.iter().rev() {
        if driver.connect_by_key_if_missing(output, input).is_err() {
            stranded += 1;
        }
    }
    stranded
}

/// Disconnect everything a failed group connect had already created.
/// Returns how many could not be removed.
///
/// The mirror image of [`rollback_disconnects`]. `allow_connection` is only
/// meaningful for a pair that really did come back out of the graph: telling
/// the backend to stop suppressing a link that is still connected would
/// contradict the state it can see.
pub(super) fn rollback_connects(
    driver: &mut dyn GraphDriver,
    created_keys: &[(PortKey, PortKey)],
) -> usize {
    let mut stranded = 0;
    for (output, input) in created_keys.iter().rev() {
        match driver.disconnect_by_key_if_present_without_suppression(output, input) {
            Ok(Some(_)) => driver.allow_connection(output, input),
            Ok(None) => {}
            Err(_) => stranded += 1,
        }
    }
    stranded
}

/// Reconnect every previously removed pair, returning the restored links.
///
/// An undo that fails partway is reported as partially applied rather than as
/// a bare backend error: the command stays on the undo stack, but the caller
/// needs to know the graph is now in neither the before nor the after state.
pub(super) fn restore_keys(
    driver: &mut dyn GraphDriver,
    operation: &'static str,
    removed_keys: &[(PortKey, PortKey)],
) -> Result<Vec<Link>, CommandError> {
    let mut restored = Vec::with_capacity(removed_keys.len());
    let mut failures = Vec::new();
    for (output, input) in removed_keys {
        match driver.connect_by_key_if_missing(output, input) {
            Ok(Some(link)) => restored.push(link),
            Ok(None) => {}
            Err(error) => failures.push(error.to_string()),
        }
    }
    if !failures.is_empty() {
        return Err(CommandError::PartiallyApplied {
            operation,
            cause: failures.join("; "),
            stranded: failures.len(),
        });
    }
    Ok(restored)
}

/// Apply node positions transactionally, rolling back to `rollback` positions
/// if any node rejects its target. Used by move execute and undo, which are
/// mirror images of each other.
pub(super) fn apply_positions(
    driver: &mut dyn GraphDriver,
    targets: &[(NodeId, [f32; 2])],
    rollback: &[(NodeId, [f32; 2])],
) -> Result<(), CommandError> {
    let mut applied = Vec::new();
    for (node, position) in targets {
        match driver.set_node_position(*node, *position) {
            Ok(()) => applied.push(*node),
            Err(error) => {
                let mut rollback_errors = Vec::new();
                for applied_node in applied.iter().rev() {
                    if let Some((_, before)) = rollback.iter().find(|(id, _)| id == applied_node) {
                        if let Err(restore) = driver.set_node_position(*applied_node, *before) {
                            rollback_errors
                                .push(format!("node {applied_node} restore failed: {restore}"));
                        }
                    }
                }
                if !rollback_errors.is_empty() {
                    return Err(CommandError::PartiallyApplied {
                        operation: "Move nodes",
                        cause: format!("{error}; {}", rollback_errors.join("; ")),
                        stranded: rollback_errors.len(),
                    });
                }
                return Err(error.into());
            }
        }
    }
    Ok(())
}

pub(super) fn port_description(port: &PortKey) -> String {
    format!("{} / {}", port.node_name, port.port_name)
}
