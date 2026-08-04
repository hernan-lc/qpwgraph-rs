use super::{
    ButtonProps, CheckboxProps, CommonProps, Element, ElementId, ElementKind, EventType, Form,
    FormValues, LabelProps, Listener, ListenerId, NumberInputProps, OptionItem, RadioGroupProps,
    SelectProps, SliderProps, Style, SwitchProps, TextInputProps, UiDocument, UiEvent, Value,
};
use egui::{vec2, Color32, Frame, Margin, Response, Sense, Stroke, Ui, Vec2};
use std::collections::{BTreeMap, VecDeque};

impl Default for UiDocument {
    fn default() -> Self {
        Self {
            elements: BTreeMap::new(),
            listeners: BTreeMap::new(),
            pending_events: VecDeque::new(),
            next_listener_id: 1,
        }
    }
}

impl UiDocument {
    /// Creates an empty document.
    pub fn new() -> Self {
        Self::default()
    }

    /// Clears per-frame flags and starts a new UI frame.
    pub fn begin_frame(&mut self) {
        for element in self.elements.values_mut() {
            element.changed = false;
            element.clicked = false;
        }
    }

    /// Returns the element with the given ID, like
    /// `document.getElementById(...)` on the web.
    pub fn get_element_by_id(&self, id: impl AsRef<str>) -> Option<&Element> {
        self.elements.get(id.as_ref())
    }

    /// Short alias for [`Self::get_element_by_id`].
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

    /// Iterates over every `(element_id, value)` pair.
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

    /// Collects the current values of all elements whose `form` prop matches
    /// `form_id`.
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

    /// Registers a listener for an element event.
    pub fn on(
        &mut self,
        id: impl Into<ElementId>,
        event_type: EventType,
        callback: impl FnMut(&UiEvent) + 'static,
    ) -> ListenerId {
        let listener_id = ListenerId(self.next_listener_id);
        self.next_listener_id = self.next_listener_id.wrapping_add(1).max(1);
        self.listeners
            .entry((id.into(), event_type))
            .or_default()
            .push(Listener {
                id: listener_id,
                callback: Box::new(callback),
            });
        listener_id
    }

    /// Web-style alias for [`Self::on`].
    pub fn add_event_listener(
        &mut self,
        id: impl Into<ElementId>,
        event_type: EventType,
        callback: impl FnMut(&UiEvent) + 'static,
    ) -> ListenerId {
        self.on(id, event_type, callback)
    }

    /// Registers a change listener.
    pub fn on_change(
        &mut self,
        id: impl Into<ElementId>,
        callback: impl FnMut(&UiEvent) + 'static,
    ) -> ListenerId {
        self.on(id, EventType::Change, callback)
    }

    /// Registers an input listener.
    pub fn on_input(
        &mut self,
        id: impl Into<ElementId>,
        callback: impl FnMut(&UiEvent) + 'static,
    ) -> ListenerId {
        self.on(id, EventType::Input, callback)
    }

    /// Registers a click listener.
    pub fn on_click(
        &mut self,
        id: impl Into<ElementId>,
        callback: impl FnMut(&UiEvent) + 'static,
    ) -> ListenerId {
        self.on(id, EventType::Click, callback)
    }

    /// Registers the result of a custom widget that is rendered outside the
    /// built-in component set. This lets icon buttons and card controls keep
    /// their custom painting while still participating in document lookup and
    /// click listeners.
    pub fn record_click(&mut self, id: impl AsRef<str>, clicked: bool) -> bool {
        let id = ElementId::new(id.as_ref());
        let common = CommonProps::new(id.clone());
        self.prepare(&common, ElementKind::Button, Value::Bool(false), vec![]);
        if clicked {
            self.record_button_click(&id, Value::Bool(true));
        }
        clicked
    }

    /// Removes a previously registered listener.
    pub fn remove_event_listener(&mut self, listener_id: ListenerId) -> bool {
        let mut removed = false;
        let mut empty_keys = Vec::new();
        for (key, listeners) in &mut self.listeners {
            let before = listeners.len();
            listeners.retain(|listener| listener.id != listener_id);
            removed |= listeners.len() != before;
            if listeners.is_empty() {
                empty_keys.push(key.clone());
            }
        }
        for key in empty_keys {
            self.listeners.remove(&key);
        }
        removed
    }

    /// Queues and immediately dispatches a programmatic event.
    pub fn dispatch_event(&mut self, event: UiEvent) {
        self.queue_event(event);
        self.dispatch_pending_events();
    }

    /// Returns pending events without removing them.
    pub fn pending_events(&self) -> impl Iterator<Item = &UiEvent> {
        self.pending_events.iter()
    }

    /// Dispatches all queued events to listeners in insertion order.
    pub fn dispatch_pending_events(&mut self) {
        while let Some(event) = self.pending_events.pop_front() {
            let key = (event.id.clone(), event.event_type);
            let Some(mut listeners) = self.listeners.remove(&key) else {
                continue;
            };
            for listener in &mut listeners {
                (listener.callback)(&event);
            }
            self.listeners.entry(key).or_default().extend(listeners);
        }
    }

    /// Renders a label.
    pub fn label(&mut self, ui: &mut Ui, props: LabelProps) -> Response {
        let id = props.common.id.clone();
        self.prepare(
            &props.common,
            ElementKind::Label,
            Value::String(props.text.clone()),
            vec![],
        );
        // Labels are static declarations, so their current value follows the
        // latest text instead of retaining the first declaration.
        if let Some(element) = self.elements.get_mut(&id) {
            element.value = Value::String(props.text.clone());
        }
        with_common(ui, &props.common, |ui| ui.label(props.text))
    }

    /// Renders a push button and emits [`EventType::Click`] when activated.
    pub fn button(&mut self, ui: &mut Ui, props: ButtonProps) -> Response {
        let id = props.common.id.clone();
        self.prepare(
            &props.common,
            ElementKind::Button,
            Value::Bool(false),
            vec![],
        );
        let style = props.common.style.clone();
        let text = props.text.clone();
        let response = with_common(ui, &props.common, |ui| {
            let mut button = egui::Button::new(text);
            if let Some(fill) = style.fill {
                button = button.fill(fill);
            }
            if let Some(stroke) = style.stroke {
                button = button.stroke(stroke);
            }
            if let Some(rounding) = style.rounding {
                button = button.rounding(rounding);
            }
            if style.width.is_some() || style.height.is_some() {
                let size = vec2(
                    style.width.unwrap_or(0.0),
                    style.height.unwrap_or(ui.spacing().interact_size.y),
                );
                button = button.min_size(size);
            }
            ui.add(button)
        });
        if response.clicked() {
            let value = props
                .action_value
                .map(Value::String)
                .unwrap_or(Value::Bool(true));
            self.record_button_click(&id, value);
        } else {
            self.observe_focus(&id, &response);
        }
        response
    }

    /// Renders a checkbox and emits `change` when its value toggles.
    pub fn checkbox(&mut self, ui: &mut Ui, props: CheckboxProps) -> Response {
        let id = props.common.id.clone();
        let before = self.prepare(
            &props.common,
            ElementKind::Checkbox,
            Value::Bool(props.checked),
            vec![],
        );
        let mut checked = before.as_bool().unwrap_or(props.checked);
        let style = props.common.style.clone();
        let label = props.common.label.clone();
        let response = with_common(ui, &props.common, |ui| {
            let checkbox = egui::Checkbox::new(&mut checked, label.unwrap_or_default());
            if style.width.is_some() || style.height.is_some() {
                ui.add_sized(
                    vec2(
                        style.width.unwrap_or(ui.available_width()),
                        style.height.unwrap_or(ui.spacing().interact_size.y),
                    ),
                    checkbox,
                )
            } else {
                ui.add(checkbox)
            }
        });
        self.observe(
            &id,
            &before,
            Value::Bool(checked),
            &response,
            &[EventType::Change],
        );
        response
    }

    /// Renders a switch/toggle and emits `change` when its value toggles.
    pub fn switch(&mut self, ui: &mut Ui, props: SwitchProps) -> Response {
        let id = props.common.id.clone();
        let before = self.prepare(
            &props.common,
            ElementKind::Switch,
            Value::Bool(props.checked),
            vec![],
        );
        let mut checked = before.as_bool().unwrap_or(props.checked);
        let label = props.common.label.clone();
        let style = props.common.style.clone();
        let response = with_common(ui, &props.common, |ui| {
            switch_widget(ui, &mut checked, label.as_deref(), &style)
        });
        self.observe(
            &id,
            &before,
            Value::Bool(checked),
            &response,
            &[EventType::Change],
        );
        response
    }

    /// Renders a text input and emits both `input` and `change` while editing.
    pub fn text_input(&mut self, ui: &mut Ui, props: TextInputProps) -> Response {
        let id = props.common.id.clone();
        let before = self.prepare(
            &props.common,
            ElementKind::TextInput,
            Value::String(props.value.clone()),
            vec![],
        );
        let mut text = before
            .as_str()
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| props.value.clone());
        let style = props.common.style.clone();
        let label = props.common.label.clone();
        let hint = props.hint.clone();
        let response = with_common(ui, &props.common, |ui| {
            labelled(ui, label.as_deref(), |ui| {
                let mut editor = if props.multiline {
                    egui::TextEdit::multiline(&mut text)
                } else {
                    egui::TextEdit::singleline(&mut text)
                };
                if let Some(hint) = hint {
                    editor = editor.hint_text(hint);
                }
                editor = editor.password(props.password);
                if let Some(width) = style.width {
                    editor = editor.desired_width(width);
                }
                if let Some(height) = style.height {
                    let row_height = ui.spacing().interact_size.y.max(1.0);
                    editor = editor.desired_rows((height / row_height).round().max(1.0) as usize);
                }
                ui.add(editor)
            })
        });
        self.observe(
            &id,
            &before,
            Value::String(text),
            &response,
            &[EventType::Input, EventType::Change],
        );
        response
    }

    /// Renders a draggable numeric input and emits `change` when its value
    /// changes.
    pub fn number_input(&mut self, ui: &mut Ui, props: NumberInputProps) -> Response {
        let id = props.common.id.clone();
        let before = self.prepare(
            &props.common,
            ElementKind::NumberInput,
            Value::Number(props.value),
            vec![],
        );
        let mut value = before.as_number().unwrap_or(props.value);
        let style = props.common.style.clone();
        let label = props.common.label.clone();
        let minimum = props.minimum;
        let maximum = props.maximum;
        let step = props.step;
        let prefix = props.prefix.clone();
        let suffix = props.suffix.clone();
        let response = with_common(ui, &props.common, |ui| {
            labelled(ui, label.as_deref(), |ui| {
                let mut input = egui::DragValue::new(&mut value);
                let (minimum, maximum) = normalize_optional_range(minimum, maximum);
                if minimum.is_finite() || maximum.is_finite() {
                    input = input.range(minimum..=maximum);
                }
                if step.is_finite() && step.abs() > f64::EPSILON {
                    input = input.speed(step.abs());
                }
                if !prefix.is_empty() {
                    input = input.prefix(prefix);
                }
                if !suffix.is_empty() {
                    input = input.suffix(suffix);
                }
                if let Some(width) = style.width {
                    ui.add_sized(vec2(width, ui.spacing().interact_size.y), input)
                } else {
                    ui.add(input)
                }
            })
        });
        self.observe(
            &id,
            &before,
            Value::Number(value),
            &response,
            &[EventType::Change],
        );
        response
    }

    /// Renders a ranged slider and emits `change` when its value changes.
    pub fn slider(&mut self, ui: &mut Ui, props: SliderProps) -> Response {
        let id = props.common.id.clone();
        let before = self.prepare(
            &props.common,
            ElementKind::Slider,
            Value::Number(props.value),
            vec![],
        );
        let mut value = before.as_number().unwrap_or(props.value);
        let style = props.common.style.clone();
        let label = props.common.label.clone();
        let (minimum, maximum) = normalize_range(props.minimum, props.maximum);
        let step = props.step;
        let show_value = props.show_value;
        let prefix = props.prefix.clone();
        let suffix = props.suffix.clone();
        let response = with_common(ui, &props.common, |ui| {
            labelled(ui, label.as_deref(), |ui| {
                let mut slider =
                    egui::Slider::new(&mut value, minimum..=maximum).show_value(show_value);
                if let Some(step) =
                    step.filter(|step| step.is_finite() && step.abs() > f64::EPSILON)
                {
                    slider = slider.step_by(step.abs());
                }
                if !prefix.is_empty() {
                    slider = slider.prefix(prefix);
                }
                if !suffix.is_empty() {
                    slider = slider.suffix(suffix);
                }
                if let Some(width) = style.width {
                    ui.add_sized(vec2(width, ui.spacing().interact_size.y), slider)
                } else {
                    ui.add(slider)
                }
            })
        });
        self.observe(
            &id,
            &before,
            Value::Number(value),
            &response,
            &[EventType::Change],
        );
        response
    }

    /// Renders a drop-down selector and emits `change` when a new option is
    /// selected.
    pub fn select(&mut self, ui: &mut Ui, props: SelectProps) -> Response {
        let id = props.common.id.clone();
        let before = self.prepare(
            &props.common,
            ElementKind::Select,
            Value::String(props.selected.clone()),
            props.options.clone(),
        );
        let mut selected = before
            .as_str()
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| props.selected.clone());
        let options = props.options.clone();
        let style = props.common.style.clone();
        let label = props.common.label.clone().unwrap_or_default();
        let selected_text = options
            .iter()
            .find(|option| option.value == selected)
            .map(|option| option.label.clone())
            .unwrap_or_else(|| selected.clone());
        let response = with_common(ui, &props.common, |ui| {
            let width = style
                .width
                .unwrap_or_else(|| (ui.available_width() - 4.0).max(120.0));
            let mut changed = false;
            let combo_response = egui::ComboBox::new(("ui-document-select", id.clone()), label)
                .selected_text(selected_text)
                .width(width)
                .show_ui(ui, |ui| {
                    for option in &options {
                        let selection = if option.disabled {
                            ui.add_enabled_ui(false, |ui| {
                                ui.selectable_label(selected == option.value, &option.label)
                            })
                            .inner
                        } else {
                            ui.selectable_label(selected == option.value, &option.label)
                        };
                        if selection.clicked() && !option.disabled {
                            selected = option.value.clone();
                            changed = true;
                        }
                    }
                })
                .response;
            let mut combo_response = combo_response;
            if changed {
                combo_response.mark_changed();
            }
            combo_response
        });
        self.observe(
            &id,
            &before,
            Value::String(selected),
            &response,
            &[EventType::Change],
        );
        response
    }

    /// Renders a radio-button group and emits `change` when a new option is
    /// selected.
    pub fn radio_group(&mut self, ui: &mut Ui, props: RadioGroupProps) -> Response {
        let id = props.common.id.clone();
        let before = self.prepare(
            &props.common,
            ElementKind::RadioGroup,
            Value::String(props.selected.clone()),
            props.options.clone(),
        );
        let mut selected = before
            .as_str()
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| props.selected.clone());
        let options = props.options.clone();
        let label = props.common.label.clone();
        let response = with_common(ui, &props.common, |ui| {
            labelled(ui, label.as_deref(), |ui| {
                let mut combined: Option<Response> = None;
                let mut selected_changed = false;
                let mut draw_options = |ui: &mut Ui| {
                    for option in &options {
                        let item_response = if option.disabled {
                            ui.add_enabled_ui(false, |ui| {
                                ui.radio(selected == option.value, &option.label)
                            })
                            .inner
                        } else {
                            ui.radio(selected == option.value, &option.label)
                        };
                        if item_response.clicked() && !option.disabled {
                            selected = option.value.clone();
                            selected_changed = true;
                        }
                        combined = Some(match combined.take() {
                            Some(previous) => previous.union(item_response),
                            None => item_response,
                        });
                    }
                };
                let container_response = if props.horizontal {
                    ui.horizontal(&mut draw_options).response
                } else {
                    ui.vertical(&mut draw_options).response
                };
                let mut response = combined.unwrap_or(container_response);
                if selected_changed {
                    response.mark_changed();
                }
                response
            })
        });
        self.observe(
            &id,
            &before,
            Value::String(selected),
            &response,
            &[EventType::Change],
        );
        response
    }

    fn prepare(
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

    fn observe(
        &mut self,
        id: &ElementId,
        before: &Value,
        after: Value,
        response: &Response,
        event_types: &[EventType],
    ) {
        self.observe_focus(id, response);
        if response.changed() && before != &after {
            if let Some(element) = self.elements.get_mut(id) {
                element.value = after.clone();
                element.changed = true;
            }
            for event_type in event_types {
                self.queue_event(UiEvent {
                    id: id.clone(),
                    event_type: *event_type,
                    value: after.clone(),
                    previous_value: Some(before.clone()),
                });
            }
        }
        if response.lost_focus() {
            let value = self.value(id).cloned().unwrap_or(Value::None);
            self.queue_event(UiEvent::new(id.clone(), EventType::Blur, value));
        }
    }

    fn observe_focus(&mut self, id: &ElementId, response: &Response) {
        if response.gained_focus() {
            let value = self.value(id).cloned().unwrap_or(Value::None);
            self.queue_event(UiEvent::new(id.clone(), EventType::Focus, value));
        }
    }

    fn record_button_click(&mut self, id: &ElementId, value: Value) {
        if let Some(element) = self.elements.get_mut(id) {
            element.value = value.clone();
            element.changed = true;
            element.clicked = true;
        }
        self.queue_event(UiEvent::new(id.clone(), EventType::Click, value));
    }

    fn queue_event(&mut self, event: UiEvent) {
        self.pending_events.push_back(event);
    }
}

/// Alias that reads naturally in application code that thinks in terms of a
/// DOM rather than an egui document.
pub type Document = UiDocument;

fn normalize_range(minimum: f64, maximum: f64) -> (f64, f64) {
    let minimum = if minimum.is_finite() { minimum } else { 0.0 };
    let maximum = if maximum.is_finite() { maximum } else { 1.0 };
    if minimum <= maximum {
        (minimum, maximum)
    } else {
        (maximum, minimum)
    }
}

fn normalize_optional_range(minimum: Option<f64>, maximum: Option<f64>) -> (f64, f64) {
    let minimum = minimum
        .filter(|value| value.is_finite())
        .unwrap_or(f64::NEG_INFINITY);
    let maximum = maximum
        .filter(|value| value.is_finite())
        .unwrap_or(f64::INFINITY);
    if minimum <= maximum {
        (minimum, maximum)
    } else {
        (maximum, minimum)
    }
}

fn labelled(
    ui: &mut Ui,
    label: Option<&str>,
    render: impl FnOnce(&mut Ui) -> Response,
) -> Response {
    if let Some(label) = label.filter(|label| !label.is_empty()) {
        ui.horizontal(|ui| {
            ui.label(label);
            render(ui)
        })
        .inner
    } else {
        render(ui)
    }
}

fn with_common(
    ui: &mut Ui,
    common: &CommonProps,
    render: impl FnOnce(&mut Ui) -> Response,
) -> Response {
    if !common.visible {
        return ui.allocate_exact_size(Vec2::ZERO, Sense::hover()).1;
    }

    let style = common.style.clone();
    let draw_style = style.clone();
    let enabled = common.enabled;
    let draw = move |ui: &mut Ui| {
        if let Some(width) = draw_style.width {
            ui.set_width(width);
        }
        if let Some(height) = draw_style.height {
            ui.set_height(height);
        }
        if let Some(text_color) = draw_style.text_color {
            ui.visuals_mut().override_text_color = Some(text_color);
        }
        if enabled {
            render(ui)
        } else {
            ui.add_enabled_ui(false, render).inner
        }
    };

    let mut response = if style.has_frame() {
        let mut frame = Frame::none();
        if let Some(fill) = style.fill {
            frame = frame.fill(fill);
        }
        if let Some(stroke) = style.stroke {
            frame = frame.stroke(stroke);
        }
        if let Some(rounding) = style.rounding {
            frame = frame.rounding(rounding);
        }
        if let Some(inner_margin) = style.inner_margin {
            frame = frame.inner_margin(Margin::same(inner_margin));
        }
        frame.show(ui, draw).inner
    } else {
        ui.scope(draw).inner
    };
    if let Some(tooltip) = &common.tooltip {
        response = response.on_hover_text(tooltip.clone());
    }
    response
}

fn switch_widget(ui: &mut Ui, checked: &mut bool, label: Option<&str>, style: &Style) -> Response {
    let track_size = vec2(36.0, 20.0).max(vec2(0.0, ui.spacing().interact_size.y));
    let on_fill = style.fill.unwrap_or(Color32::from_rgb(42, 169, 244));
    let off_fill = Color32::from_rgb(76, 84, 96);
    let border = style
        .stroke
        .unwrap_or_else(|| Stroke::new(1.0_f32, Color32::from_white_alpha(70)));
    ui.horizontal(|ui| {
        let (rect, mut response) = ui.allocate_exact_size(track_size, Sense::click());
        if response.clicked() {
            *checked = !*checked;
            response.mark_changed();
        }
        let fill = if *checked { on_fill } else { off_fill };
        ui.painter().rect(rect, track_size.y / 2.0, fill, border);
        let radius = (track_size.y - 6.0).max(4.0) / 2.0;
        let knob_x = if *checked {
            rect.right() - 3.0 - radius
        } else {
            rect.left() + 3.0 + radius
        };
        ui.painter()
            .circle_filled(egui::pos2(knob_x, rect.center().y), radius, Color32::WHITE);
        if let Some(label) = label.filter(|label| !label.is_empty()) {
            ui.label(label);
        }
        response
    })
    .inner
}
