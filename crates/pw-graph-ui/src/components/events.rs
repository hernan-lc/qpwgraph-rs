use super::{CommonProps, ElementId, ElementKind, OptionItem, Style, UiDocument, Value};
use std::collections::BTreeMap;

/// A DOM-style event emitted by an interactive component.
#[derive(Clone, Debug, PartialEq)]
pub struct UiEvent {
    /// Element that emitted the event.
    pub id: ElementId,
    /// Event category.
    pub event_type: EventType,
    /// Current element value.
    pub value: Value,
    /// Value before the interaction, when there was one.
    pub previous_value: Option<Value>,
}

impl UiEvent {
    /// Creates an event with no previous value.
    pub fn new(id: impl Into<ElementId>, event_type: EventType, value: impl Into<Value>) -> Self {
        Self {
            id: id.into(),
            event_type,
            value: value.into(),
            previous_value: None,
        }
    }

    /// Adds a previous value to the event.
    pub fn from_previous(mut self, previous_value: impl Into<Value>) -> Self {
        self.previous_value = Some(previous_value.into());
        self
    }
}

/// Event categories supported by [`UiDocument::on`].
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum EventType {
    /// A push button was clicked.
    Click,
    /// A committed control value changed.
    Change,
    /// Text or numeric input changed during editing.
    Input,
    /// The control received keyboard focus.
    Focus,
    /// The control lost keyboard focus.
    Blur,
    /// Reserved for application-level form submission buttons.
    Submit,
}

/// Snapshot of a registered element.
#[derive(Clone, Debug)]
pub struct Element {
    /// Stable element ID.
    pub id: ElementId,
    /// Component kind.
    pub kind: ElementKind,
    /// Current value.
    pub value: Value,
    /// Value supplied when the element was first registered.
    pub default_value: Value,
    /// Optional visible label.
    pub label: Option<String>,
    /// Current select/radio options.
    pub options: Vec<OptionItem>,
    /// Current style.
    pub style: Style,
    /// Whether the element is enabled.
    pub enabled: bool,
    /// Whether the element is visible.
    pub visible: bool,
    /// Optional form ID.
    pub form: Option<ElementId>,
    pub(super) changed: bool,
    pub(super) clicked: bool,
}

impl Element {
    pub(super) fn new(
        common: &CommonProps,
        kind: ElementKind,
        default_value: Value,
        options: Vec<OptionItem>,
    ) -> Self {
        Self {
            id: common.id.clone(),
            kind,
            value: default_value.clone(),
            default_value,
            label: common.label.clone(),
            options,
            style: common.style.clone(),
            enabled: common.enabled,
            visible: common.visible,
            form: common.form.clone(),
            changed: false,
            clicked: false,
        }
    }

    /// Returns whether this element changed during the current frame.
    pub fn changed(&self) -> bool {
        self.changed
    }

    /// Returns whether this element was clicked during the current frame.
    pub fn clicked(&self) -> bool {
        self.clicked
    }
}

/// Values collected from all elements associated with one form.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FormValues {
    pub(super) values: BTreeMap<ElementId, Value>,
}

impl FormValues {
    /// Looks up a value by its element ID.
    pub fn get(&self, id: impl AsRef<str>) -> Option<&Value> {
        self.values.get(id.as_ref())
    }

    /// Looks up a boolean value.
    pub fn get_bool(&self, id: impl AsRef<str>) -> Option<bool> {
        self.get(id).and_then(Value::as_bool)
    }

    /// Looks up a string value.
    pub fn get_string(&self, id: impl AsRef<str>) -> Option<&str> {
        self.get(id).and_then(Value::as_str)
    }

    /// Looks up a numeric value.
    pub fn get_number(&self, id: impl AsRef<str>) -> Option<f64> {
        self.get(id).and_then(Value::as_number)
    }

    /// Iterates over `(element_id, value)` pairs in ID order.
    pub fn iter(&self) -> impl Iterator<Item = (&ElementId, &Value)> {
        self.values.iter()
    }

    /// Returns the number of collected fields.
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Returns whether the form has no fields.
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Consumes the snapshot into its underlying map.
    pub fn into_inner(self) -> BTreeMap<ElementId, Value> {
        self.values
    }
}

/// A handle for querying one form without copying it until needed.
pub struct Form<'a> {
    pub(super) document: &'a UiDocument,
    pub(super) id: ElementId,
}

impl Form<'_> {
    /// Returns a snapshot of all fields in the form.
    pub fn values(&self) -> FormValues {
        self.document.form_values(&self.id)
    }

    /// Looks up one form field.
    pub fn get(&self, id: impl AsRef<str>) -> Option<&Value> {
        self.document
            .get_element_by_id(id)
            .filter(|element| element.form.as_ref() == Some(&self.id))
            .map(|element| &element.value)
    }

    /// Iterates over form fields without allocating a snapshot.
    pub fn iter(&self) -> impl Iterator<Item = (&ElementId, &Value)> {
        self.document.iter_form_values(&self.id)
    }
}

/// Opaque handle returned by event-listener registration.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ListenerId(pub(super) u64);

pub(super) struct Listener {
    pub(super) id: ListenerId,
    pub(super) callback: Box<dyn FnMut(&UiEvent)>,
}
