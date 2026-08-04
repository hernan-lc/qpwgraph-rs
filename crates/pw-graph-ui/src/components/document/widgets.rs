//! Built-in controls backed by the retained UI document.
//!
//! Controls are grouped by interaction style:
//!
//! - basic contains labels, buttons, checkboxes, and switches.
//! - inputs contains text and numeric editors.
//! - choices contains select boxes and radio groups.
//! - shared contains layout, styling, and control lifecycle helpers.

mod basic;
mod choices;
mod inputs;
mod shared;

use super::super::{CommonProps, ElementId, ElementKind, EventType, OptionItem, UiDocument, Value};
use egui::Response;

pub(super) fn prepare_control(
    document: &mut UiDocument,
    common: &CommonProps,
    kind: ElementKind,
    default_value: Value,
    options: Vec<OptionItem>,
) -> (ElementId, Value) {
    let id = common.id.clone();
    let before = document.prepare(common, kind, default_value, options);
    (id, before)
}

pub(super) fn finish_control(
    document: &mut UiDocument,
    id: ElementId,
    before: Value,
    after: Value,
    response: Response,
    event_types: &[EventType],
) -> Response {
    document.observe(&id, &before, after, &response, event_types);
    response
}
