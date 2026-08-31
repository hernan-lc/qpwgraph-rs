//! The undo/redo stack and the trait its entries implement.

use super::*;

pub trait Command {
    fn name(&self) -> &'static str;
    fn description(&self) -> String {
        self.name().to_owned()
    }
    fn execute(&mut self, driver: &mut dyn GraphDriver) -> Result<(), CommandError>;
    fn undo(&mut self, driver: &mut dyn GraphDriver) -> Result<(), CommandError>;
}

pub struct CommandStack {
    pub(super) undo_stack: Vec<Box<dyn Command>>,
    pub(super) redo_stack: Vec<Box<dyn Command>>,
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
