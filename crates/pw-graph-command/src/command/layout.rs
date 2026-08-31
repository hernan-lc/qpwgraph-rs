//! Moving nodes, which is undoable like any other command.

use super::*;

/// Applies a node-position transaction, used for drag and arrange undo.
pub struct MoveNodesCommand {
    pub(super) before: Vec<(NodeId, [f32; 2])>,
    pub(super) after: Vec<(NodeId, [f32; 2])>,
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
