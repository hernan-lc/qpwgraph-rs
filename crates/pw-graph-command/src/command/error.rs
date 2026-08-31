//! What a command can refuse to do, and why.

use super::*;

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
    #[error("{operation} failed and could not be rolled back: {cause}; {stranded} item(s) were left changed")]
    PartiallyApplied {
        operation: &'static str,
        cause: String,
        stranded: usize,
    },
}
