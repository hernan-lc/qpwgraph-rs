use super::super::{
    CommonProps, Element, ElementId, ElementKind, EventType, Form, FormValues, OptionItem, Theme,
    ThemeMode, ThemeToken, UiDocument, UiEvent, Value,
};
use egui::Color32;
use std::collections::{BTreeMap, VecDeque};

impl Default for UiDocument {
    fn default() -> Self {
        Self {
            elements: BTreeMap::new(),
            listeners: BTreeMap::new(),
            pending_events: VecDeque::new(),
            next_listener_id: 1,
            theme: Theme::default(),
        }
    }
}

impl UiDocument {
    /// Creates an empty document.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the active theme.
    pub fn theme(&self) -> &Theme {
        &self.theme
    }

    /// Returns the active theme mode.
    pub fn theme_mode(&self) -> ThemeMode {
        self.theme.mode
    }

    /// Sets the active theme mode (Dark or Light).
    pub fn set_theme_mode(&mut self, mode: ThemeMode) {
        if self.theme.mode != mode {
            self.theme = Theme::new(mode);
        }
    }

    /// Toggles between Dark and Light themes and returns the new mode.
    pub fn toggle_theme(&mut self) -> ThemeMode {
        let new_mode = self.theme.mode.toggled();
        self.set_theme_mode(new_mode);
        new_mode
    }

    /// Resolves a semantic theme token to a [`Color32`].
    pub fn theme_color(&self, token: ThemeToken) -> Color32 {
        self.theme.color(token)
    }

    /// Clears per-frame flags and starts a new UI frame.
    pub fn begin_frame(&mut self) {
        for element in self.elements.values_mut() {
            element.changed = false;
            element.clicked = false;
        }
    }

    /// Returns the element with the given ID, like document.getElementById on
    /// the web.
    pub fn get_element_by_id(&self, id: impl AsRef<str>) -> Option<&Element> {
        self.elements.get(id.as_ref())
    }

    /// Short alias for get_element_by_id.
    pub fn get(&self, id: impl AsRef<str>) -> Option<&Element> {
        self.get_element_by_id(id)
    }

    /// Mutable element lookup for advanced state changes.
    pub fn get_element_by_id_mut(&mut self, id: impl AsRef<str>) -> Option<&mut Element> {
        self.elements.get_mut(id.as_ref())
    }

    /// Removes an element and its listeners. This is useful for dynamic form
    /// rows that no longer exist in the current view.
    pub fn remove_element_by_id(&mut self, id: impl AsRef<str>) -> Option<Element> {
        let id = ElementId::new(id.as_ref());
        self.listeners
            .retain(|(listener_id, _), _| listener_id != &id);
        self.pending_events.retain(|event| event.id != id);
        self.elements.remove(&id)
    }

    /// Iterates over every registered element.
    pub fn elements(&self) -> impl Iterator<Item = &Element> {
        self.elements.values()
    }

    /// Iterates over every (element_id, value) pair.
    pub fn values(&self) -> impl Iterator<Item = (&ElementId, &Value)> {
        self.elements
            .iter()
            .map(|(id, element)| (id, &element.value))
    }

    /// Gets one current value.
    pub fn value(&self, id: impl AsRef<str>) -> Option<&Value> {
        self.get_element_by_id(id).map(|element| &element.value)
    }

    /// Gets one current boolean value.
    pub fn checked(&self, id: impl AsRef<str>) -> Option<bool> {
        self.value(id).and_then(Value::as_bool)
    }

    /// Gets one current text value.
    pub fn text(&self, id: impl AsRef<str>) -> Option<&str> {
        self.value(id).and_then(Value::as_str)
    }

    /// Gets one current numeric value.
    pub fn number(&self, id: impl AsRef<str>) -> Option<f64> {
        self.value(id).and_then(Value::as_number)
    }

    /// Sets a registered value without emitting an event. This is useful for
    /// synchronizing document state with application configuration.
    pub fn set_value(&mut self, id: impl AsRef<str>, value: impl Into<Value>) -> bool {
        let Some(element) = self.elements.get_mut(id.as_ref()) else {
            return false;
        };
        let value = value.into();
        if element.value == value {
            return false;
        }
        element.value = value;
        true
    }

    /// Sets a registered value and queues a change event.
    pub fn set_value_and_emit(
        &mut self,
        id: impl AsRef<str>,
        value: impl Into<Value>,
        event_type: EventType,
    ) -> bool {
        let id = ElementId::new(id.as_ref());
        let value = value.into();
        let Some(element) = self.elements.get_mut(&id) else {
            return false;
        };
        if element.value == value {
            return false;
        }
        let previous = std::mem::replace(&mut element.value, value.clone());
        element.changed = true;
        self.queue_event(UiEvent {
            id,
            event_type,
            value,
            previous_value: Some(previous),
        });
        true
    }

    /// Returns whether an element changed during the current frame.
    pub fn changed(&self, id: impl AsRef<str>) -> bool {
        self.get_element_by_id(id).is_some_and(Element::changed)
    }

    /// Returns whether an element was clicked during the current frame.
    pub fn clicked(&self, id: impl AsRef<str>) -> bool {
        self.get_element_by_id(id).is_some_and(Element::clicked)
    }

    /// Returns a query handle for a form.
    pub fn form(&self, id: impl Into<ElementId>) -> Form<'_> {
        Form {
            document: self,
            id: id.into(),
        }
    }

    /// Collects the current values of all elements whose form prop matches
    /// form_id.
    pub fn form_values(&self, form_id: impl AsRef<str>) -> FormValues {
        FormValues {
            values: self
                .iter_form_values(form_id)
                .map(|(id, value)| (id.clone(), value.clone()))
                .collect(),
        }
    }

    /// Iterates over form values without allocating.
    pub fn iter_form_values(
        &self,
        form_id: impl AsRef<str>,
    ) -> impl Iterator<Item = (&ElementId, &Value)> {
        let form_id = ElementId::new(form_id.as_ref());
        self.elements
            .iter()
            .filter(move |(_, element)| element.form.as_ref() == Some(&form_id))
            .map(|(id, element)| (id, &element.value))
    }

    pub(in crate::components) fn prepare(
        &mut self,
        common: &CommonProps,
        kind: ElementKind,
        default_value: Value,
        options: Vec<OptionItem>,
    ) -> Value {
        debug_assert!(
            !common.id.is_empty(),
            "UiDocument controls should have a non-empty stable ID"
        );
        let id = common.id.clone();
        let element = self
            .elements
            .entry(id)
            .or_insert_with(|| Element::new(common, kind, default_value.clone(), options.clone()));
        if element.kind != kind {
            *element = Element::new(common, kind, default_value, options);
        } else {
            element.label = common.label.clone();
            element.options = options;
            element.style = common.style.clone();
            element.enabled = common.enabled;
            element.visible = common.visible;
            element.form = common.form.clone();
            element.changed = false;
            element.clicked = false;
            // A button represents a momentary action, not retained form data.
            if kind == ElementKind::Button {
                element.value = Value::Bool(false);
            }
        }
        element.value.clone()
    }
}
