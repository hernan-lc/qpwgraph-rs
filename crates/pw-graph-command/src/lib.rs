//! Undoable graph operations.

use pw_graph_backend::{BackendError, GraphDriver};
use pw_graph_core::{Graph, Link, LinkId, NodeId, PortId, PortKey};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CommandError {
    #[error(transparent)]
    Backend(#[from] BackendError),
    #[error("command has no link to undo")]
    MissingUndoLink,
    /// A group operation failed partway *and* the rollback could not put the
    /// graph back. This is the one outcome the user has to be told about
    /// explicitly: the command is not on the undo stack (it never completed),
    /// so nothing else will offer to repair it.
    #[error("{operation} failed and could not be rolled back: {cause}; {stranded} connection(s) were left changed")]
    PartiallyApplied {
        operation: &'static str,
        cause: String,
        stranded: usize,
    },
}

/// A link's two endpoints, identified by stable name rather than by id.
type EndpointPair = (PortKey, PortKey);

fn stable_pair(graph: &Graph, output: PortId, input: PortId) -> Option<EndpointPair> {
    Some((graph.port_key(output)?, graph.port_key(input)?))
}

/// Collapse repeated endpoint pairs while preserving order.
fn dedup_pairs(pairs: impl IntoIterator<Item = (PortKey, PortKey)>) -> Vec<(PortKey, PortKey)> {
    let mut unique = Vec::new();
    for pair in pairs {
        if !unique.contains(&pair) {
            unique.push(pair);
        }
    }
    unique
}

fn pair_description(name: &str, keys: &Option<(PortKey, PortKey)>) -> String {
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
fn disconnect_keys(
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
fn rollback_disconnects(
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

/// Reconnect every previously removed pair, returning the restored links.
///
/// An undo that fails partway is reported as partially applied rather than as
/// a bare backend error: the command stays on the undo stack, but the caller
/// needs to know the graph is now in neither the before nor the after state.
fn restore_keys(
    driver: &mut dyn GraphDriver,
    operation: &'static str,
    removed_keys: &[(PortKey, PortKey)],
) -> Result<Vec<Link>, CommandError> {
    let mut restored = Vec::with_capacity(removed_keys.len());
    for (index, (output, input)) in removed_keys.iter().enumerate() {
        match driver.connect_by_key_if_missing(output, input) {
            Ok(Some(link)) => restored.push(link),
            Ok(None) => {}
            Err(error) => {
                return Err(CommandError::PartiallyApplied {
                    operation,
                    cause: error.to_string(),
                    stranded: removed_keys.len() - index,
                })
            }
        }
    }
    Ok(restored)
}

/// Apply node positions transactionally, rolling back to `rollback` positions
/// if any node rejects its target. Used by move execute and undo, which are
/// mirror images of each other.
fn apply_positions(
    driver: &mut dyn GraphDriver,
    targets: &[(NodeId, [f32; 2])],
    rollback: &[(NodeId, [f32; 2])],
) -> Result<(), CommandError> {
    let mut applied = Vec::new();
    for (node, position) in targets {
        match driver.set_node_position(*node, *position) {
            Ok(()) => applied.push(*node),
            Err(error) => {
                for applied_node in applied.iter().rev() {
                    if let Some((_, before)) = rollback.iter().find(|(id, _)| id == applied_node) {
                        let _ = driver.set_node_position(*applied_node, *before);
                    }
                }
                return Err(error.into());
            }
        }
    }
    Ok(())
}

pub trait Command {
    fn name(&self) -> &'static str;
    fn description(&self) -> String {
        self.name().to_owned()
    }
    fn execute(&mut self, driver: &mut dyn GraphDriver) -> Result<(), CommandError>;
    fn undo(&mut self, driver: &mut dyn GraphDriver) -> Result<(), CommandError>;
}

fn port_description(port: &PortKey) -> String {
    format!("{} / {}", port.node_name, port.port_name)
}

pub struct CommandStack {
    undo_stack: Vec<Box<dyn Command>>,
    redo_stack: Vec<Box<dyn Command>>,
}

impl Default for CommandStack {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandStack {
    pub fn new() -> Self {
        Self {
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        }
    }

    pub fn execute(
        &mut self,
        mut command: Box<dyn Command>,
        driver: &mut dyn GraphDriver,
    ) -> Result<(), CommandError> {
        command.execute(driver)?;
        self.undo_stack.push(command);
        self.redo_stack.clear();
        Ok(())
    }

    pub fn undo(&mut self, driver: &mut dyn GraphDriver) -> Result<bool, CommandError> {
        let Some(mut command) = self.undo_stack.pop() else {
            return Ok(false);
        };
        if let Err(error) = command.undo(driver) {
            self.undo_stack.push(command);
            return Err(error);
        }
        self.redo_stack.push(command);
        Ok(true)
    }

    pub fn redo(&mut self, driver: &mut dyn GraphDriver) -> Result<bool, CommandError> {
        let Some(mut command) = self.redo_stack.pop() else {
            return Ok(false);
        };
        if let Err(error) = command.execute(driver) {
            self.redo_stack.push(command);
            return Err(error);
        }
        self.undo_stack.push(command);
        Ok(true)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    pub fn undo_label(&self) -> Option<&'static str> {
        self.undo_stack.last().map(|command| command.name())
    }

    pub fn redo_label(&self) -> Option<&'static str> {
        self.redo_stack.last().map(|command| command.name())
    }

    pub fn undo_history(&self) -> Vec<String> {
        self.undo_stack
            .iter()
            .rev()
            .map(|command| command.description())
            .collect()
    }

    pub fn redo_history(&self) -> Vec<String> {
        self.redo_stack
            .iter()
            .rev()
            .map(|command| command.description())
            .collect()
    }
}

pub struct ConnectCommand {
    src: PortId,
    dst: PortId,
    link: Option<Link>,
    keys: Option<(PortKey, PortKey)>,
}

/// Connects a group of compatible ports as one undoable action.
pub struct ConnectManyCommand {
    pairs: Vec<(PortId, PortId)>,
    keys: Vec<(PortKey, PortKey)>,
    links: Vec<Link>,
    created_keys: Vec<(PortKey, PortKey)>,
}

impl ConnectManyCommand {
    pub fn new(pairs: Vec<(PortId, PortId)>) -> Self {
        Self {
            pairs,
            keys: Vec::new(),
            links: Vec::new(),
            created_keys: Vec::new(),
        }
    }

    pub fn with_keys(pairs: Vec<(PortId, PortId)>, keys: Vec<(PortKey, PortKey)>) -> Self {
        let keys = dedup_pairs(keys);
        Self {
            pairs,
            keys,
            links: Vec::new(),
            created_keys: Vec::new(),
        }
    }
}

impl Command for ConnectManyCommand {
    fn name(&self) -> &'static str {
        "Connect group"
    }

    fn description(&self) -> String {
        format!(
            "{} ({} pairs)",
            self.name(),
            self.keys.len().max(self.pairs.len())
        )
    }

    fn execute(&mut self, driver: &mut dyn GraphDriver) -> Result<(), CommandError> {
        driver.refresh()?;
        if self.keys.is_empty() {
            self.keys = dedup_pairs(
                self.pairs
                    .iter()
                    .filter_map(|(src, dst)| stable_pair(driver.graph(), *src, *dst)),
            );
        }
        self.links.clear();
        self.created_keys.clear();
        for (output, input) in &self.keys {
            match driver.connect_by_key_if_missing(output, input) {
                Ok(Some(link)) => {
                    self.links.push(link);
                    self.created_keys.push((output.clone(), input.clone()));
                }
                Ok(None) => {}
                Err(error) => {
                    for (created_output, created_input) in self.created_keys.iter().rev() {
                        let _ = driver.disconnect_by_key_if_present(created_output, created_input);
                        driver.allow_connection(created_output, created_input);
                    }
                    self.links.clear();
                    self.created_keys.clear();
                    return Err(error.into());
                }
            }
        }
        Ok(())
    }

    fn undo(&mut self, driver: &mut dyn GraphDriver) -> Result<(), CommandError> {
        for (output, input) in self.created_keys.iter().rev() {
            let _ = driver.disconnect_by_key_if_present(output, input)?;
        }
        self.links.clear();
        self.created_keys.clear();
        Ok(())
    }
}

impl ConnectCommand {
    pub fn new(src: PortId, dst: PortId) -> Self {
        Self {
            src,
            dst,
            link: None,
            keys: None,
        }
    }

    pub fn from_keys(output: PortKey, input: PortKey) -> Self {
        Self {
            src: PortId::default(),
            dst: PortId::default(),
            link: None,
            keys: Some((output, input)),
        }
    }
}

impl Command for ConnectCommand {
    fn name(&self) -> &'static str {
        "Connect"
    }

    fn description(&self) -> String {
        pair_description(self.name(), &self.keys)
    }

    fn execute(&mut self, driver: &mut dyn GraphDriver) -> Result<(), CommandError> {
        driver.refresh()?;
        let keys = self
            .keys
            .clone()
            .or_else(|| stable_pair(driver.graph(), self.src, self.dst));
        let Some((output, input)) = keys else {
            return Ok(());
        };
        self.keys = Some((output.clone(), input.clone()));
        self.link = driver.connect_by_key_if_missing(&output, &input)?;
        Ok(())
    }

    fn undo(&mut self, driver: &mut dyn GraphDriver) -> Result<(), CommandError> {
        if self.link.is_none() {
            return Ok(());
        }
        let (output, input) = self.keys.as_ref().ok_or(CommandError::MissingUndoLink)?;
        let _ = driver.disconnect_by_key_if_present(output, input)?;
        self.link = None;
        Ok(())
    }
}

/// Move one end of an existing link to a different port.
///
/// Dragging an edge is a single user action, so it is a single undoable one:
/// disconnecting and reconnecting as two commands would put a broken
/// intermediate state on the undo stack, and undoing once would leave the
/// graph disconnected rather than back where it started.
pub struct RerouteLinkCommand {
    link_id: LinkId,
    /// Port the dragged end was dropped on. Its direction decides which end of
    /// the link it replaces, so the caller does not have to say.
    new_port: PortId,
    /// The link's endpoints before and after, captured while executing so an
    /// undo survives the backend renumbering its ports.
    old_keys: Option<EndpointPair>,
    new_keys: Option<EndpointPair>,
    applied: bool,
    /// Whether *this command* created the new link, as opposed to finding one
    /// already there. Undo must only remove a link it made: tearing down a
    /// connection somebody else established would be a silent, unrelated
    /// change the user never asked for.
    created_new: bool,
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
    fn resolve(
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
        if !driver.is_link_mutable(self.link_id) {
            return Err(BackendError::Unsupported(
                "this link is observed and cannot be rerouted".into(),
            )
            .into());
        }
        let (old, new) = match (self.old_keys.clone(), self.new_keys.clone()) {
            // A redo replays the endpoints captured the first time round.
            (Some(old), Some(new)) => (old, new),
            _ => self.resolve(driver)?,
        };
        // Connect first would briefly leave the source feeding two inputs, and
        // some backends refuse a second link from the same port, so the old one
        // goes first and is restored if the new connection cannot be made.
        let removed_old = driver
            .disconnect_by_key_if_present(&old.0, &old.1)?
            .is_some();
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
            driver
                .disconnect_by_key_if_present(&new.0, &new.1)?
                .is_some()
        } else {
            false
        };
        if let Err(error) = driver.connect_by_key_if_missing(&old.0, &old.1) {
            // Undo has already taken the new route down; leaving the graph
            // with neither route and reporting a plain backend error would
            // hide that. Put the new route back if we can.
            if removed_new {
                let _ = driver.connect_by_key_if_missing(&new.0, &new.1);
                return Err(error.into());
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

pub struct DisconnectCommand {
    link: Option<Link>,
    link_id: LinkId,
    keys: Option<(PortKey, PortKey)>,
    removed: bool,
}

/// Disconnect every live link as one undoable operation.
pub struct DisconnectAllCommand {
    links: Vec<Link>,
    keys: Vec<(PortKey, PortKey)>,
    removed_keys: Vec<(PortKey, PortKey)>,
}

pub struct DisconnectManyCommand {
    link_ids: Vec<LinkId>,
    keys: Vec<(PortKey, PortKey)>,
    removed_keys: Vec<(PortKey, PortKey)>,
    links: Vec<Link>,
}

impl DisconnectManyCommand {
    pub fn new(link_ids: Vec<LinkId>) -> Self {
        Self {
            link_ids,
            keys: Vec::new(),
            removed_keys: Vec::new(),
            links: Vec::new(),
        }
    }

    pub fn from_links(graph: &Graph, links: Vec<Link>) -> Self {
        let keys = links
            .iter()
            .filter_map(|link| stable_pair(graph, link.output_port, link.input_port))
            .collect();
        Self {
            link_ids: links.iter().map(|link| link.id).collect(),
            keys,
            removed_keys: Vec::new(),
            links,
        }
    }
}

impl Command for DisconnectManyCommand {
    fn name(&self) -> &'static str {
        "Disconnect group"
    }

    fn description(&self) -> String {
        format!("{} ({} links)", self.name(), self.keys.len())
    }

    fn execute(&mut self, driver: &mut dyn GraphDriver) -> Result<(), CommandError> {
        driver.refresh()?;
        if self.keys.is_empty() {
            self.keys = self
                .link_ids
                .iter()
                .filter_map(|id| driver.graph().link(*id))
                .filter_map(|link| stable_pair(driver.graph(), link.output_port, link.input_port))
                .collect();
        }
        self.links = disconnect_keys(driver, self.name(), &self.keys, &mut self.removed_keys)?;
        Ok(())
    }

    fn undo(&mut self, driver: &mut dyn GraphDriver) -> Result<(), CommandError> {
        self.links = restore_keys(driver, self.name(), &self.removed_keys)?;
        Ok(())
    }
}

impl DisconnectAllCommand {
    pub fn new() -> Self {
        Self {
            links: Vec::new(),
            keys: Vec::new(),
            removed_keys: Vec::new(),
        }
    }
}

impl Default for DisconnectAllCommand {
    fn default() -> Self {
        Self::new()
    }
}

impl Command for DisconnectAllCommand {
    fn name(&self) -> &'static str {
        "Disconnect all"
    }

    fn description(&self) -> String {
        format!("{} ({} links)", self.name(), self.keys.len())
    }

    fn execute(&mut self, driver: &mut dyn GraphDriver) -> Result<(), CommandError> {
        driver.refresh()?;
        self.keys = driver
            .graph()
            .links
            .values()
            .filter(|link| driver.is_link_mutable(link.id))
            .filter_map(|link| stable_pair(driver.graph(), link.output_port, link.input_port))
            .collect();
        self.links = disconnect_keys(driver, self.name(), &self.keys, &mut self.removed_keys)?;
        Ok(())
    }

    fn undo(&mut self, driver: &mut dyn GraphDriver) -> Result<(), CommandError> {
        self.links = restore_keys(driver, self.name(), &self.removed_keys)?;
        Ok(())
    }
}

impl DisconnectCommand {
    pub fn new(link_id: LinkId) -> Self {
        Self {
            link: None,
            link_id,
            keys: None,
            removed: false,
        }
    }

    pub fn from_link(graph: &Graph, link: Link) -> Self {
        Self {
            link_id: link.id,
            keys: stable_pair(graph, link.output_port, link.input_port),
            link: Some(link),
            removed: false,
        }
    }
}

impl Command for DisconnectCommand {
    fn name(&self) -> &'static str {
        "Disconnect"
    }

    fn description(&self) -> String {
        pair_description(self.name(), &self.keys)
    }

    fn execute(&mut self, driver: &mut dyn GraphDriver) -> Result<(), CommandError> {
        self.removed = false;
        driver.refresh()?;
        if self.keys.is_none() {
            self.keys = driver
                .graph()
                .link(self.link_id)
                .and_then(|link| stable_pair(driver.graph(), link.output_port, link.input_port));
        }
        let Some((output, input)) = self.keys.as_ref() else {
            return Ok(());
        };
        if let Some(link) = driver.graph().find_link_by_keys(output, input) {
            if !driver.is_link_mutable(link.id) {
                return Ok(());
            }
        }
        if let Some(link) = driver.disconnect_by_key_if_present(output, input)? {
            self.link_id = link.id;
            self.link = Some(link);
            self.removed = true;
        }
        Ok(())
    }

    fn undo(&mut self, driver: &mut dyn GraphDriver) -> Result<(), CommandError> {
        if !self.removed {
            return Ok(());
        }
        let (output, input) = self.keys.as_ref().ok_or(CommandError::MissingUndoLink)?;
        if let Some(restored) = driver.connect_by_key_if_missing(output, input)? {
            self.link_id = restored.id;
            self.link = Some(restored);
        } else if let Some(restored) = driver.graph().find_link_by_keys(output, input) {
            self.link_id = restored.id;
            self.link = Some(restored);
        }
        self.removed = false;
        Ok(())
    }
}

/// Applies a node-position transaction, used for drag and arrange undo.
pub struct MoveNodesCommand {
    before: Vec<(NodeId, [f32; 2])>,
    after: Vec<(NodeId, [f32; 2])>,
}

impl MoveNodesCommand {
    pub fn new(before: Vec<(NodeId, [f32; 2])>, after: Vec<(NodeId, [f32; 2])>) -> Self {
        Self { before, after }
    }
}

impl Command for MoveNodesCommand {
    fn name(&self) -> &'static str {
        "Move nodes"
    }

    fn description(&self) -> String {
        format!("{} ({})", self.name(), self.after.len())
    }

    fn execute(&mut self, driver: &mut dyn GraphDriver) -> Result<(), CommandError> {
        apply_positions(driver, &self.after, &self.before)
    }

    fn undo(&mut self, driver: &mut dyn GraphDriver) -> Result<(), CommandError> {
        apply_positions(driver, &self.before, &self.after)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pw_graph_backend::InMemoryDriver;

    #[test]
    fn connect_undo_redo_round_trip() {
        let mut driver = InMemoryDriver::demo();
        let mut commands = CommandStack::new();
        commands
            .execute(
                Box::new(ConnectCommand::new(PortId(1), PortId(3))),
                &mut driver,
            )
            .unwrap();
        assert!(commands.can_undo());
        assert_eq!(commands.undo_history().len(), 1);
        assert!(commands.undo_history()[0].starts_with("Connect:"));
        assert!(commands.redo_history().is_empty());
        assert_eq!(driver.graph().links.len(), 1);
        commands.undo(&mut driver).unwrap();
        assert!(driver.graph().links.is_empty());
        assert!(commands.undo_history().is_empty());
        assert_eq!(commands.redo_history().len(), 1);
        commands.redo(&mut driver).unwrap();
        assert_eq!(driver.graph().links.len(), 1);
    }

    #[test]
    fn connecting_an_existing_pair_is_a_noop_for_undo() {
        let mut driver = InMemoryDriver::demo();
        let mut commands = CommandStack::new();
        commands
            .execute(
                Box::new(ConnectCommand::new(PortId(1), PortId(3))),
                &mut driver,
            )
            .unwrap();
        commands
            .execute(
                Box::new(ConnectCommand::new(PortId(1), PortId(3))),
                &mut driver,
            )
            .unwrap();
        assert_eq!(driver.graph().links.len(), 1);
        commands.undo(&mut driver).unwrap();
        assert_eq!(driver.graph().links.len(), 1);
        commands.undo(&mut driver).unwrap();
        assert!(driver.graph().links.is_empty());
    }

    #[test]
    fn disconnect_undo_redo_round_trip() {
        let mut driver = InMemoryDriver::demo();
        let link = driver.connect(PortId(1), PortId(3)).unwrap();
        let mut commands = CommandStack::new();
        commands
            .execute(Box::new(DisconnectCommand::new(link.id)), &mut driver)
            .unwrap();
        assert!(driver.graph().links.is_empty());
        commands.undo(&mut driver).unwrap();
        assert_eq!(driver.graph().links.len(), 1);
        commands.redo(&mut driver).unwrap();
        assert!(driver.graph().links.is_empty());
    }

    #[test]
    fn disconnect_all_undo_redo_round_trip() {
        let mut driver = InMemoryDriver::demo();
        driver.connect(PortId(1), PortId(3)).unwrap();
        driver.connect(PortId(2), PortId(4)).unwrap();
        let mut commands = CommandStack::new();

        commands
            .execute(Box::new(DisconnectAllCommand::new()), &mut driver)
            .unwrap();
        assert!(driver.graph().links.is_empty());
        commands.undo(&mut driver).unwrap();
        assert_eq!(driver.graph().links.len(), 2);
        commands.redo(&mut driver).unwrap();
        assert!(driver.graph().links.is_empty());
    }

    #[test]
    fn disconnect_commands_leave_observed_links_in_place() {
        let mut driver = InMemoryDriver::demo();
        let link = driver.connect(PortId(1), PortId(3)).unwrap();
        driver.mark_link_observed(link.id);
        let mut commands = CommandStack::new();

        commands
            .execute(Box::new(DisconnectAllCommand::new()), &mut driver)
            .unwrap();
        assert!(driver.graph().link(link.id).is_some());
        commands.undo(&mut driver).unwrap();
        assert!(driver.graph().link(link.id).is_some());

        commands
            .execute(Box::new(DisconnectCommand::new(link.id)), &mut driver)
            .unwrap();
        assert!(driver.graph().link(link.id).is_some());
    }

    #[test]
    fn disconnect_many_is_one_undoable_operation() {
        let mut driver = InMemoryDriver::demo();
        let first = driver.connect(PortId(1), PortId(3)).unwrap();
        let second = driver.connect(PortId(2), PortId(4)).unwrap();
        let mut commands = CommandStack::new();

        commands
            .execute(
                Box::new(DisconnectManyCommand::new(vec![first.id, second.id])),
                &mut driver,
            )
            .unwrap();
        assert!(driver.graph().links.is_empty());
        commands.undo(&mut driver).unwrap();
        assert_eq!(driver.graph().links.len(), 2);
        commands.redo(&mut driver).unwrap();
        assert!(driver.graph().links.is_empty());
    }

    #[test]
    fn connect_many_is_one_undoable_operation() {
        let mut driver = InMemoryDriver::demo();
        let mut commands = CommandStack::new();
        commands
            .execute(
                Box::new(ConnectManyCommand::new(vec![
                    (PortId(1), PortId(3)),
                    (PortId(2), PortId(4)),
                ])),
                &mut driver,
            )
            .unwrap();
        assert_eq!(driver.graph().links.len(), 2);
        commands.undo(&mut driver).unwrap();
        assert!(driver.graph().links.is_empty());
    }

    #[test]
    fn a_failed_group_disconnect_leaves_the_graph_untouched() {
        // The regression this guards: the first links were removed, the error
        // propagated, and `CommandStack::execute` then refused to record the
        // command — so the user lost connections with no undo available.
        let mut driver = InMemoryDriver::demo();
        let first = driver.connect(PortId(1), PortId(3)).unwrap();
        let second = driver.connect(PortId(2), PortId(4)).unwrap();
        driver.fail_disconnect_of(second.id);
        let mut commands = CommandStack::new();

        let error = commands
            .execute(
                Box::new(DisconnectManyCommand::new(vec![first.id, second.id])),
                &mut driver,
            )
            .expect_err("the group disconnect must fail");
        assert!(matches!(error, CommandError::Backend(_)));
        assert_eq!(
            driver.graph().links.len(),
            2,
            "a failed group disconnect must roll back the links it removed"
        );
        assert!(!commands.can_undo());
    }

    #[test]
    fn reroute_undo_keeps_a_connection_it_did_not_create() {
        // Rerouting onto a pair that already exists must not give undo licence
        // to delete somebody else's connection.
        let mut driver = InMemoryDriver::demo();
        let moving = driver.connect(PortId(1), PortId(3)).unwrap();
        let existing = driver.connect(PortId(2), PortId(4)).unwrap();
        let mut commands = CommandStack::new();

        // Drag the source end of `moving` onto port 2, which makes it the same
        // pair as `existing`.
        commands
            .execute(
                Box::new(RerouteLinkCommand::new(moving.id, PortId(2))),
                &mut driver,
            )
            .unwrap();
        assert!(driver.graph().link(existing.id).is_some());

        commands.undo(&mut driver).unwrap();
        assert!(
            driver
                .graph()
                .find_link_by_keys(
                    &driver.graph().port_key(PortId(2)).unwrap(),
                    &driver.graph().port_key(PortId(4)).unwrap()
                )
                .is_some(),
            "undo must not delete a pre-existing connection it did not create"
        );
    }

    #[test]
    fn a_failed_reroute_restores_the_original_route() {
        let mut driver = InMemoryDriver::demo();
        let link = driver.connect(PortId(1), PortId(3)).unwrap();
        driver.fail_connect_of(PortId(2), PortId(3));
        let mut commands = CommandStack::new();

        assert!(commands
            .execute(
                Box::new(RerouteLinkCommand::new(link.id, PortId(2))),
                &mut driver
            )
            .is_err());
        assert_eq!(
            driver.graph().links.len(),
            1,
            "the original route must come back when the new one is refused"
        );
    }

    #[test]
    fn move_nodes_undo_redo_round_trip() {
        let mut driver = InMemoryDriver::demo();
        let before = driver.graph().node(NodeId(1)).unwrap().position;
        let after = [300.0, 200.0];
        let mut commands = CommandStack::new();
        commands
            .execute(
                Box::new(MoveNodesCommand::new(
                    vec![(NodeId(1), before)],
                    vec![(NodeId(1), after)],
                )),
                &mut driver,
            )
            .unwrap();
        assert_eq!(driver.graph().node(NodeId(1)).unwrap().position, after);
        commands.undo(&mut driver).unwrap();
        assert_eq!(driver.graph().node(NodeId(1)).unwrap().position, before);
        commands.redo(&mut driver).unwrap();
        assert_eq!(driver.graph().node(NodeId(1)).unwrap().position, after);
    }
}
