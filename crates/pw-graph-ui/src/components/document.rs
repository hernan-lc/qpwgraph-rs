//! Retained document state and built-in egui widget rendering.
//!
//! The implementation is split by responsibility:
//!
//! - state owns element registration, retained values, and form queries.
//! - events owns listeners, event delivery, and interaction bookkeeping.
//! - widgets owns built-in control rendering and shared layout helpers.

mod events;
mod state;
mod widgets;

/// Alias that reads naturally in application code that thinks in terms of a
/// DOM rather than an egui document.
pub type Document = super::UiDocument;
