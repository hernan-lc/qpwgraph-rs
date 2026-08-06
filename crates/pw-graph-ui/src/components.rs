//! DOM-like, reusable controls for the `egui` UI.
//!
//! The implementation is split by responsibility:
//!
//! - [`ids`] contains element IDs, values, options, and element kinds.
//! - [`props`] contains shared styles and control property builders.
//! - [`events`] contains event, element, and form data types.
//! - [`document`] contains retained state and widget rendering.
//! - [`dialog`] contains reusable modal dialog chrome and backdrop handling.
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

mod containers;
mod dialog;
mod document;
mod events;
mod icons;
mod ids;
mod indicators;
mod props;
#[cfg(test)]
mod tests;
mod theme;

use std::collections::{BTreeMap, VecDeque};

use egui::{Align, Layout, Ui};
use events::Listener;

pub use containers::{CardProps, DisclosureProps, StepItem, StepsProps, TabItem, TabsProps};
pub use dialog::{DialogPlacement, DialogProps, DialogResponse};
pub use document::Document;
pub use events::{Element, EventType, Form, FormValues, ListenerId, UiEvent};
pub use icons::{icon_image, Icon, IconSource};
pub use ids::{ElementId, ElementKind, OptionItem, Value};
pub use indicators::{record_custom_click, BadgeProps, IconButtonProps, MeterProps};
pub use props::{
    ButtonProps, CheckboxProps, CommonProps, LabelProps, NumberInputProps, RadioGroupProps,
    SelectProps, SliderProps, Style, SwitchProps, TextInputProps,
};
pub use theme::{Theme, ThemeMode, ThemePalette, ThemeToken};

/// Renders a setting row with descriptive content on the leading side and
/// its control aligned to the trailing edge.
///
/// The row deliberately accepts closures instead of a fixed label type. This
/// keeps the layout reusable for a plain node label, a title plus explanation,
/// or an icon-labelled application setting while leaving the actual control
/// owned by UiDocument.
pub fn setting_row<T>(
    ui: &mut Ui,
    leading: impl FnOnce(&mut Ui),
    trailing: impl FnOnce(&mut Ui) -> T,
) -> T {
    let available_width = ui.available_width();
    ui.horizontal(|ui| {
        ui.set_min_width(available_width);
        leading(ui);
        ui.with_layout(Layout::right_to_left(Align::Center), trailing)
            .inner
    })
    .inner
}

/// Narrowest description column worth keeping beside a control. Below this a
/// label and its explanation are unreadable as a column, so the row stacks.
const MIN_LEADING_WIDTH: f32 = 116.0;

/// A [`setting_row`] that knows how wide its control wants to be, and so can
/// keep the two sides from colliding.
///
/// Plain `setting_row` lets the description take its natural width and then
/// draws the control right-to-left over whatever is left, which in a docked
/// side panel means a wide select painted on top of its own explanation. Here
/// the description is given an explicit column bounded by what the control
/// leaves over — so it wraps instead of growing — and when even that column
/// would be too narrow to read, the control drops onto its own line beneath.
///
/// Callers pass the control's intended width; it is clamped to the row, so a
/// control can never be wider than the panel it lives in.
pub fn setting_row_sized<T>(
    ui: &mut Ui,
    trailing_width: f32,
    leading: impl FnOnce(&mut Ui),
    trailing: impl FnOnce(&mut Ui) -> T,
) -> T {
    let available_width = ui.available_width();
    let spacing = ui.spacing().item_spacing.x;
    let trailing_width = trailing_width.min(available_width);
    let leading_width = available_width - trailing_width - spacing;
    if leading_width >= MIN_LEADING_WIDTH {
        ui.horizontal(|ui| {
            ui.set_min_width(available_width);
            ui.allocate_ui_with_layout(
                egui::vec2(leading_width, 0.0),
                Layout::left_to_right(Align::Min),
                leading,
            );
            ui.with_layout(Layout::right_to_left(Align::Center), trailing)
                .inner
        })
        .inner
    } else {
        ui.vertical(|ui| {
            ui.set_min_width(available_width);
            ui.allocate_ui_with_layout(
                egui::vec2(available_width, 0.0),
                Layout::left_to_right(Align::Min),
                leading,
            );
            ui.with_layout(Layout::right_to_left(Align::Center), trailing)
                .inner
        })
        .inner
    }
}

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
    theme: Theme,
}
