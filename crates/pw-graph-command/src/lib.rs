//! Undoable graph mutations.
//!
//! Every mutation the UI can perform goes through a `Command` so it can be
//! undone, and so its rollback rules live next to the mutation itself.

mod command;

pub use command::*;
