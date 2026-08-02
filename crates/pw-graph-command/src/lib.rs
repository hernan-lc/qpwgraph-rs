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

pub struct RenameCommand {
    node: NodeId,
    before: String,
    after: String,
}

impl RenameCommand {
    pub fn new(node: NodeId, before: impl Into<String>, after: impl Into<String>) -> Self {
        Self {
            node,
            before: before.into(),
            after: after.into(),
        }
    }
}

impl Command for RenameCommand {
    fn name(&self) -> &'static str {
        "Rename"
    }

    fn execute(&mut self, driver: &mut dyn GraphDriver) -> Result<(), CommandError> {
        driver.rename_node(self.node, self.after.clone())?;
        Ok(())
    }

    fn undo(&mut self, driver: &mut dyn GraphDriver) -> Result<(), CommandError> {
        driver.rename_node(self.node, self.before.clone())?;
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
}
