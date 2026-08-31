//! Undoable graph operations.
//!
//! | Module | Owns |
//! | --- | --- |
//! | [`stack`] | the undo/redo stack and the `Command` trait |
//! | [`connect`] | creating links, one pair or a whole group |
//! | [`disconnect`] | removing one, a chosen set, or every mutable link |
//! | [`reroute`] | moving one end of an existing link |
//! | [`layout`] | moving nodes |
//! | [`transaction`] | the rollback rules every command above shares |
//! | [`error`] | what a command can refuse to do |

use pw_graph_backend::{BackendError, GraphDriver};
use pw_graph_core::{Graph, Link, LinkId, NodeId, PortId, PortKey};
use thiserror::Error;

mod connect;
mod disconnect;
mod error;
mod layout;
mod reroute;
mod stack;
mod transaction;

#[cfg(test)]
mod tests;

// One 1,500-line file before; `pub(super)` keeps the reach a bare item
// had there, which is private to `command`.
pub use self::connect::*;
pub use self::disconnect::*;
pub use self::error::*;
pub use self::layout::*;
pub use self::reroute::*;
pub use self::stack::*;
use self::transaction::*;
