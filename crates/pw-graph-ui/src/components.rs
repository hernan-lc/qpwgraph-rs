//! DOM-like, reusable controls for the `egui` UI.
//!
//! The implementation is split by responsibility:
//!
//! - [`ids`] contains element IDs, values, options, and element kinds.
//! - [`props`] contains shared styles and control property builders.
//! - [`events`] contains event, element, and form data types.
//! - [`document`] contains retained state and widget rendering.
//!
//! `egui` is an immediate-mode UI toolkit: widgets are created again on every
//! frame. [`UiDocument`] adds the small amount of retained state that is useful
//! for forms and reusable panels. Controls have stable IDs, can be looked up
//! after they are rendered, expose a [`Value`], and can emit DOM-style events.
//!
//! A typical form looks like this:
//!
//! ```no_run
//! use pw_graph_ui::components::{
//!     CheckboxProps, EventType, OptionItem, SelectProps, TextInputProps, UiDocument,
//! };
//!
//! # fn show(ui: &mut egui::Ui) {
//! let mut document = UiDocument::new();
//! document.on_change("settings.name", |event| {
//!     println!("{} changed to {}", event.id, event.value);
//! });
//!
//! document.text_input(
//!     ui,
//!     TextInputProps::new("settings.name", "Name")
//!         .value("default")
//!         .form("settings"),
//! );
//! document.checkbox(
//!     ui,
//!     CheckboxProps::new("settings.enabled", "Enabled")
//!         .checked(true)
//!         .form("settings"),
//! );
//! document.select(
//!     ui,
//!     SelectProps::new("settings.mode", "Mode")
//!         .selected("easy")
//!         .options([
//!             OptionItem::new("easy", "Easy"),
//!             OptionItem::new("advanced", "Advanced"),
//!         ])
//!         .form("settings"),
//! );
//!
//! // Dispatch listeners after all controls have been declared for the frame.
//! document.dispatch_pending_events();
//! let values = document.form_values("settings");
//! assert_eq!(values.get_string("settings.name"), Some("default"));
//! # }
//! ```

mod document;
mod events;
mod ids;
mod props;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, VecDeque};

use events::Listener;

pub use document::Document;
pub use events::{Element, EventType, Form, FormValues, ListenerId, UiEvent};
pub use ids::{ElementId, ElementKind, OptionItem, Value};
pub use props::{
    ButtonProps, CheckboxProps, CommonProps, LabelProps, NumberInputProps, RadioGroupProps,
    SelectProps, SliderProps, Style, SwitchProps, TextInputProps,
};

/// Retained DOM-like state and reusable `egui` controls.
///
/// Keep one document alongside the application or panel state. Call
/// [`Self::begin_frame`] before drawing a group of controls and
/// [`Self::dispatch_pending_events`] after drawing them. State is retained by
/// ID, so props values are defaults for first registration rather than values
/// that overwrite user input on every frame.
pub struct UiDocument {
    elements: BTreeMap<ElementId, Element>,
    listeners: BTreeMap<(ElementId, EventType), Vec<Listener>>,
    pending_events: VecDeque<UiEvent>,
    next_listener_id: u64,
}
