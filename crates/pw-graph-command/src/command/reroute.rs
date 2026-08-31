//! Moving one end of an existing link to a different pin.

use super::*;

/// Move one end of an existing link to a different port.
///
/// Dragging an edge is a single user action, so it is a single undoable one:
/// disconnecting and reconnecting as two commands would put a broken
/// intermediate state on the undo stack, and undoing once would leave the
/// graph disconnected rather than back where it started.
pub struct RerouteLinkCommand {
    pub(super) link_id: LinkId,
    /// Port the dragged end was dropped on. Its direction decides which end of
    /// the link it replaces, so the caller does not have to say.
    pub(super) new_port: PortId,
    /// The link's endpoints before and after, captured while executing so an
    /// undo survives the backend renumbering its ports.
    pub(super) old_keys: Option<EndpointPair>,
    pub(super) new_keys: Option<EndpointPair>,
    pub(super) applied: bool,
    /// Whether *this command* created the new link, as opposed to finding one
    /// already there. Undo must only remove a link it made: tearing down a
    /// connection somebody else established would be a silent, unrelated
    /// change the user never asked for.
    pub(super) created_new: bool,
}

impl RerouteLinkCommand {
    pub fn new(link_id: LinkId, new_port: PortId) -> Self {
        Self {
            link_id,
            new_port,
            old_keys: None,
            new_keys: None,
            applied: false,
            created_new: false,
        }
    }

    /// Resolve which end moves, and what the link becomes.
    pub(super) fn resolve(
        &self,
        driver: &dyn GraphDriver,
    ) -> Result<(EndpointPair, EndpointPair), CommandError> {
        let graph = driver.graph();
        let link = graph
            .link(self.link_id)
            .ok_or(CommandError::MissingUndoLink)?;
        let target = graph
            .port(self.new_port)
            .ok_or(CommandError::MissingUndoLink)?;
        let old = stable_pair(graph, link.output_port, link.input_port)
            .ok_or(CommandError::MissingUndoLink)?;
        // A source replaces the source end, a sink replaces the sink end. That
        // keeps the link's direction intact whichever end was dragged.
        let (output, input) = if target.direction.is_source() {
            (self.new_port, link.input_port)
        } else {
            (link.output_port, self.new_port)
        };
        if output == link.output_port && input == link.input_port {
            // Dropped back where it started.
            return Err(CommandError::MissingUndoLink);
        }
        let new = stable_pair(graph, output, input).ok_or(CommandError::MissingUndoLink)?;
        Ok((old, new))
    }
}

impl Command for RerouteLinkCommand {
    fn name(&self) -> &'static str {
        "Reroute"
    }

    fn description(&self) -> String {
        pair_description(self.name(), &self.new_keys)
    }

    fn execute(&mut self, driver: &mut dyn GraphDriver) -> Result<(), CommandError> {
        driver.refresh()?;
        let (old, new) = match (self.old_keys.clone(), self.new_keys.clone()) {
            // A redo replays the endpoints captured the first time round.
            (Some(old), Some(new)) => (old, new),
            _ => self.resolve(driver)?,
        };

        // A restored link may receive a new backend ID. Resolve the current
        // link by its stable endpoints instead of relying on the ID captured
        // by the original execution.
        let old_link = driver
            .graph()
            .find_link_by_keys(&old.0, &old.1)
            .ok_or(CommandError::MissingUndoLink)?;
        if !driver.is_link_mutable(old_link.id) {
            return Err(BackendError::Unsupported(
                "this link is observed and cannot be rerouted".into(),
            )
            .into());
        }

        // Connect first would briefly leave the source feeding two inputs, and
        // some backends refuse a second link from the same port, so the old one
        // goes first and is restored if the new connection cannot be made.
        // Avoid creating suppression state if a concurrent change removed the
        // old route between the lookup above and this mutation. Preserve the
        // normal suppression behavior only when this command actually
        // removed a link.
        let removed_old = driver
            .disconnect_by_key_if_present_without_suppression(&old.0, &old.1)?
            .is_some();
        if removed_old {
            driver.suppress_connection(&old.0, &old.1);
        }
        match driver.connect_by_key_if_missing(&new.0, &new.1) {
            // `None` means the target link already existed. Recording that
            // distinction is what stops undo from deleting it.
            Ok(created) => {
                self.created_new = created.is_some();
                self.old_keys = Some(old);
                self.new_keys = Some(new);
                self.applied = true;
                Ok(())
            }
            Err(error) => {
                if removed_old {
                    // Restoring the old route is the whole point of removing it
                    // last; if even that fails the graph is left with neither
                    // route, and saying so is better than reporting the
                    // original error as if nothing had changed.
                    if let Err(restore) = driver.connect_by_key_if_missing(&old.0, &old.1) {
                        return Err(CommandError::PartiallyApplied {
                            operation: "Reroute",
                            cause: format!(
                                "{error}; restoring the previous route failed: {restore}"
                            ),
                            stranded: 1,
                        });
                    }
                }
                Err(error.into())
            }
        }
    }

    fn undo(&mut self, driver: &mut dyn GraphDriver) -> Result<(), CommandError> {
        if !self.applied {
            return Ok(());
        }
        let new = self
            .new_keys
            .as_ref()
            .cloned()
            .ok_or(CommandError::MissingUndoLink)?;
        let old = self
            .old_keys
            .as_ref()
            .cloned()
            .ok_or(CommandError::MissingUndoLink)?;
        let removed_new = if self.created_new {
            // The backend may report `None` when the route disappeared
            // concurrently. It is a rollback operation, so a missing route
            // must not create a new suppression rule.
            let removed =
                driver.disconnect_by_key_if_present_without_suppression(&new.0, &new.1)?;
            if removed.is_some() {
                driver.allow_connection(&new.0, &new.1);
            }
            removed.is_some()
        } else {
            false
        };
        if let Err(error) = driver.connect_by_key_if_missing(&old.0, &old.1) {
            // Undo has already taken the new route down; leaving the graph
            // with neither route and reporting a plain backend error would
            // hide that. Put the new route back if we can.
            if removed_new {
                match driver.connect_by_key_if_missing(&new.0, &new.1) {
                    // The undo did not happen, but the graph is back where it
                    // was before it started, so an ordinary backend error is
                    // the honest report.
                    Ok(_) => return Err(error.into()),
                    // Both levels failed: neither the route the undo was
                    // restoring nor the one it removed to make room is in the
                    // graph. Swallowing this second error — as this used to —
                    // reported a plain backend failure for a graph that had
                    // lost a connection outright.
                    Err(rollback) => {
                        return Err(CommandError::PartiallyApplied {
                            operation: "Reroute",
                            cause: format!(
                                "restoring the original route failed: {error}; \
                                 and the rerouted connection could not be put \
                                 back either: {rollback}"
                            ),
                            stranded: 2,
                        })
                    }
                }
            }
            return Err(CommandError::PartiallyApplied {
                operation: "Reroute",
                cause: error.to_string(),
                stranded: 1,
            });
        }
        self.applied = false;
        self.created_new = false;
        Ok(())
    }
}
