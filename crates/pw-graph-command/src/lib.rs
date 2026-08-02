//! Undoable graph operations.

use pw_graph_backend::{BackendError, GraphDriver};
use pw_graph_core::{Link, LinkId, NodeId, PortId};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CommandError {
    #[error(transparent)]
    Backend(#[from] BackendError),
    #[error("command has no link to undo")]
    MissingUndoLink,
}

pub trait Command {
    fn name(&self) -> &'static str;
    fn execute(&mut self, driver: &mut dyn GraphDriver) -> Result<(), CommandError>;
    fn undo(&mut self, driver: &mut dyn GraphDriver) -> Result<(), CommandError>;
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
}

pub struct ConnectCommand {
    src: PortId,
    dst: PortId,
    link: Option<Link>,
}

/// Connects a group of compatible ports as one undoable action.
pub struct ConnectManyCommand {
    pairs: Vec<(PortId, PortId)>,
    links: Vec<Link>,
}

impl ConnectManyCommand {
    pub fn new(pairs: Vec<(PortId, PortId)>) -> Self {
        Self {
            pairs,
            links: Vec::new(),
        }
    }
}

impl Command for ConnectManyCommand {
    fn name(&self) -> &'static str {
        "Connect group"
    }

    fn execute(&mut self, driver: &mut dyn GraphDriver) -> Result<(), CommandError> {
        self.links.clear();
        for (src, dst) in &self.pairs {
            match driver.connect(*src, *dst) {
                Ok(link) => self.links.push(link),
                Err(error) => {
                    for link in self.links.iter().rev() {
                        let _ = driver.disconnect(link.id);
                    }
                    self.links.clear();
                    return Err(error.into());
                }
            }
        }
        Ok(())
    }

    fn undo(&mut self, driver: &mut dyn GraphDriver) -> Result<(), CommandError> {
        for link in self.links.iter().rev() {
            driver.disconnect(link.id)?;
        }
        Ok(())
    }
}

impl ConnectCommand {
    pub fn new(src: PortId, dst: PortId) -> Self {
        Self {
            src,
            dst,
            link: None,
        }
    }
}

impl Command for ConnectCommand {
    fn name(&self) -> &'static str {
        "Connect"
    }

    fn execute(&mut self, driver: &mut dyn GraphDriver) -> Result<(), CommandError> {
        self.link = Some(driver.connect(self.src, self.dst)?);
        Ok(())
    }

    fn undo(&mut self, driver: &mut dyn GraphDriver) -> Result<(), CommandError> {
        let link = self.link.take().ok_or(CommandError::MissingUndoLink)?;
        driver.disconnect(link.id)?;
        Ok(())
    }
}

pub struct DisconnectCommand {
    link: Option<Link>,
    link_id: LinkId,
}

/// Disconnect every live link as one undoable operation.
pub struct DisconnectAllCommand {
    links: Vec<Link>,
}

pub struct DisconnectManyCommand {
    link_ids: Vec<LinkId>,
    links: Vec<Link>,
}

impl DisconnectManyCommand {
    pub fn new(link_ids: Vec<LinkId>) -> Self {
        Self {
            link_ids,
            links: Vec::new(),
        }
    }
}

impl Command for DisconnectManyCommand {
    fn name(&self) -> &'static str {
        "Disconnect group"
    }

    fn execute(&mut self, driver: &mut dyn GraphDriver) -> Result<(), CommandError> {
        self.links = if self.links.is_empty() {
            self.link_ids
                .iter()
                .filter_map(|id| driver.graph().link(*id).cloned())
                .collect()
        } else {
            self.links
                .iter()
                .filter_map(|saved| {
                    driver
                        .graph()
                        .links
                        .values()
                        .find(|live| {
                            live.output_port == saved.output_port
                                && live.input_port == saved.input_port
                        })
                        .cloned()
                })
                .collect()
        };
        let mut disconnected = Vec::with_capacity(self.links.len());
        for link in &self.links {
            match driver.disconnect(link.id) {
                Ok(link) => disconnected.push(link),
                Err(error) => {
                    for restored in disconnected.iter().rev() {
                        let _ = driver.connect(restored.output_port, restored.input_port);
                    }
                    return Err(error.into());
                }
            }
        }
        self.links = disconnected;
        Ok(())
    }

    fn undo(&mut self, driver: &mut dyn GraphDriver) -> Result<(), CommandError> {
        let mut restored = Vec::with_capacity(self.links.len());
        for link in &self.links {
            match driver.connect(link.output_port, link.input_port) {
                Ok(link) => restored.push(link),
                Err(error) => {
                    for restored_link in restored.iter().rev() {
                        let _ = driver.disconnect(restored_link.id);
                    }
                    return Err(error.into());
                }
            }
        }
        self.links = restored;
        Ok(())
    }
}

impl DisconnectAllCommand {
    pub fn new() -> Self {
        Self { links: Vec::new() }
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

    fn execute(&mut self, driver: &mut dyn GraphDriver) -> Result<(), CommandError> {
        // Re-snapshot on every execution because reconnecting during undo may
        // allocate different backend link IDs.
        self.links = driver.graph().links.values().cloned().collect();
        let mut disconnected = Vec::with_capacity(self.links.len());
        for link in &self.links {
            match driver.disconnect(link.id) {
                Ok(link) => disconnected.push(link),
                Err(error) => {
                    for restored in disconnected.iter().rev() {
                        let _ = driver.connect(restored.output_port, restored.input_port);
                    }
                    return Err(error.into());
                }
            }
        }
        self.links = disconnected;
        Ok(())
    }

    fn undo(&mut self, driver: &mut dyn GraphDriver) -> Result<(), CommandError> {
        let mut restored = Vec::with_capacity(self.links.len());
        for link in &self.links {
            match driver.connect(link.output_port, link.input_port) {
                Ok(link) => restored.push(link),
                Err(error) => {
                    for restored_link in restored.iter().rev() {
                        let _ = driver.disconnect(restored_link.id);
                    }
                    return Err(error.into());
                }
            }
        }
        self.links = restored;
        Ok(())
    }
}

impl DisconnectCommand {
    pub fn new(link_id: LinkId) -> Self {
        Self {
            link: None,
            link_id,
        }
    }
}

impl Command for DisconnectCommand {
    fn name(&self) -> &'static str {
        "Disconnect"
    }

    fn execute(&mut self, driver: &mut dyn GraphDriver) -> Result<(), CommandError> {
        self.link = Some(driver.disconnect(self.link_id)?);
        Ok(())
    }

    fn undo(&mut self, driver: &mut dyn GraphDriver) -> Result<(), CommandError> {
        let link = self
            .link
            .as_ref()
            .ok_or(CommandError::MissingUndoLink)?
            .clone();
        let restored = driver.connect(link.output_port, link.input_port)?;
        self.link_id = restored.id;
        self.link = Some(restored);
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

    fn execute(&mut self, driver: &mut dyn GraphDriver) -> Result<(), CommandError> {
        let mut applied = Vec::new();
        for (node, position) in &self.after {
            match driver.set_node_position(*node, *position) {
                Ok(()) => applied.push(*node),
                Err(error) => {
                    for applied_node in applied.iter().rev() {
                        if let Some((_, before)) =
                            self.before.iter().find(|(id, _)| id == applied_node)
                        {
                            let _ = driver.set_node_position(*applied_node, *before);
                        }
                    }
                    return Err(error.into());
                }
            }
        }
        Ok(())
    }

    fn undo(&mut self, driver: &mut dyn GraphDriver) -> Result<(), CommandError> {
        let mut applied = Vec::new();
        for (node, position) in &self.before {
            match driver.set_node_position(*node, *position) {
                Ok(()) => applied.push(*node),
                Err(error) => {
                    for applied_node in applied.iter().rev() {
                        if let Some((_, after)) =
                            self.after.iter().find(|(id, _)| id == applied_node)
                        {
                            let _ = driver.set_node_position(*applied_node, *after);
                        }
                    }
                    return Err(error.into());
                }
            }
        }
        Ok(())
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
        assert_eq!(driver.graph().links.len(), 1);
        commands.undo(&mut driver).unwrap();
        assert!(driver.graph().links.is_empty());
        commands.redo(&mut driver).unwrap();
        assert_eq!(driver.graph().links.len(), 1);
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
