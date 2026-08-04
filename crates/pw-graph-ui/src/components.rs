//! DOM-like, reusable controls for the `egui` UI.
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

use egui::{vec2, Color32, Frame, Margin, Response, Sense, Stroke, Ui, Vec2};
use std::borrow::Borrow;
use std::collections::{BTreeMap, VecDeque};
use std::fmt;

/// Stable DOM-like identifier for a control.
#[derive(Clone, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ElementId(String);

impl ElementId {
    /// Creates an ID from a string-like value.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Returns the string representation used for lookup and form keys.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns whether this is an empty ID.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl AsRef<str> for ElementId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Borrow<str> for ElementId {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for ElementId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl From<&str> for ElementId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for ElementId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&String> for ElementId {
    fn from(value: &String) -> Self {
        Self::new(value.clone())
    }
}

impl From<&ElementId> for ElementId {
    fn from(value: &ElementId) -> Self {
        value.clone()
    }
}

/// Values exposed by controls and collected by forms.
///
/// Strings are used for text inputs and selected options. Numeric controls use
/// `f64` so the same form API works for integer-like and floating-point input.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// An absent value, useful for labels and controls without a value.
    None,
    /// A boolean value, used by checkboxes and switches.
    Bool(bool),
    /// A text or option value.
    String(String),
    /// A numeric value.
    Number(f64),
}

impl Value {
    /// Returns the contained boolean, if this is [`Value::Bool`].
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            _ => None,
        }
    }

    /// Returns the contained string, if this is [`Value::String`].
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }

    /// Returns the contained number, if this is [`Value::Number`].
    pub fn as_number(&self) -> Option<f64> {
        match self {
            Self::Number(value) => Some(*value),
            _ => None,
        }
    }

    /// Returns a cloned string value, if present.
    pub fn into_string(self) -> Option<String> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => Ok(()),
            Self::Bool(value) => value.fmt(formatter),
            Self::String(value) => formatter.write_str(value),
            Self::Number(value) => value.fmt(formatter),
        }
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Self::String(value.to_owned())
    }
}

impl From<&String> for Value {
    fn from(value: &String) -> Self {
        Self::String(value.clone())
    }
}

impl From<f32> for Value {
    fn from(value: f32) -> Self {
        Self::Number(f64::from(value))
    }
}

impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Self::Number(value)
    }
}

macro_rules! impl_integer_value {
    ($($type:ty),* $(,)?) => {
        $(
            impl From<$type> for Value {
                fn from(value: $type) -> Self {
                    Self::Number(value as f64)
                }
            }
        )*
    };
}

impl_integer_value!(i8, i16, i32, i64, isize, u8, u16, u32, u64, usize);

/// The kind of a registered element.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ElementKind {
    /// Non-interactive text.
    Label,
    /// A push button.
    Button,
    /// A standard checkbox.
    Checkbox,
    /// A switch/toggle rendered as a sliding track.
    Switch,
    /// A single-line or multiline text input.
    TextInput,
    /// A draggable numeric input.
    NumberInput,
    /// A ranged numeric slider.
    Slider,
    /// A drop-down option selector.
    Select,
    /// A group of mutually exclusive radio buttons.
    RadioGroup,
}

/// An option used by [`SelectProps`] and [`RadioGroupProps`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OptionItem {
    /// Stable value submitted by the form.
    pub value: String,
    /// Human-readable text shown to the user.
    pub label: String,
    /// Disabled options remain visible but cannot be selected.
    pub disabled: bool,
}

impl OptionItem {
    /// Creates an enabled option.
    pub fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            disabled: false,
        }
    }

    /// Marks the option as disabled.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// Common visual options shared by all controls.
///
/// The fields are public for quick one-off changes; builder methods are also
/// provided by every component props type.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Style {
    /// Fixed width for the component, when supported by the widget.
    pub width: Option<f32>,
    /// Fixed height for the component, when supported by the widget.
    pub height: Option<f32>,
    /// Text color override.
    pub text_color: Option<Color32>,
    /// Background fill. For controls that paint their own background this is
    /// also passed to the widget when possible.
    pub fill: Option<Color32>,
    /// Border stroke.
    pub stroke: Option<Stroke>,
    /// Frame corner radius.
    pub rounding: Option<f32>,
    /// Frame inner margin.
    pub inner_margin: Option<f32>,
}

impl Style {
    /// Sets a fixed width.
    pub fn width(mut self, width: f32) -> Self {
        self.width = Some(width.max(0.0));
        self
    }

    /// Sets a fixed height.
    pub fn height(mut self, height: f32) -> Self {
        self.height = Some(height.max(0.0));
        self
    }

    /// Sets both dimensions.
    pub fn size(mut self, width: f32, height: f32) -> Self {
        self.width = Some(width.max(0.0));
        self.height = Some(height.max(0.0));
        self
    }

    /// Sets the text color.
    pub fn text_color(mut self, color: Color32) -> Self {
        self.text_color = Some(color);
        self
    }

    /// Sets the background fill.
    pub fn fill(mut self, color: Color32) -> Self {
        self.fill = Some(color);
        self
    }

    /// Sets the border stroke.
    pub fn stroke(mut self, stroke: Stroke) -> Self {
        self.stroke = Some(stroke);
        self
    }

    /// Sets the corner radius.
    pub fn rounding(mut self, radius: f32) -> Self {
        self.rounding = Some(radius.max(0.0));
        self
    }

    /// Sets the inner frame margin.
    pub fn inner_margin(mut self, margin: f32) -> Self {
        self.inner_margin = Some(margin.max(0.0));
        self
    }

    fn has_frame(&self) -> bool {
        self.fill.is_some()
            || self.stroke.is_some()
            || self.rounding.is_some()
            || self.inner_margin.is_some()
    }
}

/// Properties shared by all reusable controls.
#[derive(Clone, Debug, PartialEq)]
pub struct CommonProps {
    /// Stable ID used by [`UiDocument::get_element_by_id`].
    pub id: ElementId,
    /// Optional visible label.
    pub label: Option<String>,
    /// Whether the control accepts interaction.
    pub enabled: bool,
    /// Whether the control is rendered.
    pub visible: bool,
    /// Visual customization.
    pub style: Style,
    /// Optional hover tooltip.
    pub tooltip: Option<String>,
    /// Optional form ID. Controls with the same form ID are collected by
    /// [`UiDocument::form_values`].
    pub form: Option<ElementId>,
}

impl CommonProps {
    /// Creates common props with enabled and visible defaults.
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            label: None,
            enabled: true,
            visible: true,
            style: Style::default(),
            tooltip: None,
            form: None,
        }
    }

    /// Sets the label.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Enables or disables the control.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Convenience inverse of [`Self::enabled`].
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.enabled = !disabled;
        self
    }

    /// Shows or hides the control.
    pub fn visible(mut self, visible: bool) -> Self {
        self.visible = visible;
        self
    }

    /// Replaces the visual style.
    pub fn style(mut self, style: Style) -> Self {
        self.style = style;
        self
    }

    /// Sets a tooltip.
    pub fn tooltip(mut self, tooltip: impl Into<String>) -> Self {
        self.tooltip = Some(tooltip.into());
        self
    }

    /// Sets or clears a tooltip.
    pub fn tooltip_option(mut self, tooltip: Option<String>) -> Self {
        self.tooltip = tooltip;
        self
    }

    /// Associates the control with a form.
    pub fn form(mut self, form: impl Into<ElementId>) -> Self {
        self.form = Some(form.into());
        self
    }

    /// Sets a fixed width without constructing a [`Style`] manually.
    pub fn width(mut self, width: f32) -> Self {
        self.style = self.style.width(width);
        self
    }

    /// Sets a fixed height without constructing a [`Style`] manually.
    pub fn height(mut self, height: f32) -> Self {
        self.style = self.style.height(height);
        self
    }

    /// Sets both dimensions without constructing a [`Style`] manually.
    pub fn size(mut self, width: f32, height: f32) -> Self {
        self.style = self.style.size(width, height);
        self
    }
}

macro_rules! impl_common_builders {
    ($type:ty) => {
        impl $type {
            /// Sets the visible label.
            pub fn label(mut self, label: impl Into<String>) -> Self {
                self.common = self.common.label(label);
                self
            }

            /// Enables or disables the control.
            pub fn enabled(mut self, enabled: bool) -> Self {
                self.common = self.common.enabled(enabled);
                self
            }

            /// Convenience inverse of [`CommonProps::enabled`].
            pub fn disabled(mut self, disabled: bool) -> Self {
                self.common = self.common.disabled(disabled);
                self
            }

            /// Shows or hides the control.
            pub fn visible(mut self, visible: bool) -> Self {
                self.common = self.common.visible(visible);
                self
            }

            /// Replaces the visual style.
            pub fn style(mut self, style: Style) -> Self {
                self.common = self.common.style(style);
                self
            }

            /// Sets a tooltip.
            pub fn tooltip(mut self, tooltip: impl Into<String>) -> Self {
                self.common = self.common.tooltip(tooltip);
                self
            }

            /// Sets or clears a tooltip.
            pub fn tooltip_option(mut self, tooltip: Option<String>) -> Self {
                self.common = self.common.tooltip_option(tooltip);
                self
            }

            /// Associates the control with a form.
            pub fn form(mut self, form: impl Into<ElementId>) -> Self {
                self.common = self.common.form(form);
                self
            }

            /// Sets a fixed width.
            pub fn width(mut self, width: f32) -> Self {
                self.common = self.common.width(width);
                self
            }

            /// Sets a fixed height.
            pub fn height(mut self, height: f32) -> Self {
                self.common = self.common.height(height);
                self
            }

            /// Sets both dimensions.
            pub fn size(mut self, width: f32, height: f32) -> Self {
                self.common = self.common.size(width, height);
                self
            }
        }
    };
}

/// Text-only component properties.
#[derive(Clone, Debug, PartialEq)]
pub struct LabelProps {
    /// Shared properties.
    pub common: CommonProps,
    /// Text to display.
    pub text: String,
}

impl LabelProps {
    /// Creates label props.
    pub fn new(id: impl Into<ElementId>, text: impl Into<String>) -> Self {
        Self {
            common: CommonProps::new(id),
            text: text.into(),
        }
    }

    /// Replaces the displayed text.
    pub fn text(mut self, text: impl Into<String>) -> Self {
        self.text = text.into();
        self
    }
}

impl Default for LabelProps {
    fn default() -> Self {
        Self::new("", "")
    }
}

impl_common_builders!(LabelProps);

/// Push-button properties.
#[derive(Clone, Debug, PartialEq)]
pub struct ButtonProps {
    /// Shared properties.
    pub common: CommonProps,
    /// Text displayed by the button.
    pub text: String,
    /// Optional value included in the click event.
    pub action_value: Option<String>,
}

impl ButtonProps {
    /// Creates button props.
    pub fn new(id: impl Into<ElementId>, text: impl Into<String>) -> Self {
        Self {
            common: CommonProps::new(id),
            text: text.into(),
            action_value: None,
        }
    }

    /// Replaces the button text.
    pub fn text(mut self, text: impl Into<String>) -> Self {
        self.text = text.into();
        self
    }

    /// Sets the value emitted on click.
    pub fn action_value(mut self, value: impl Into<String>) -> Self {
        self.action_value = Some(value.into());
        self
    }
}

impl Default for ButtonProps {
    fn default() -> Self {
        Self::new("", "")
    }
}

impl_common_builders!(ButtonProps);

/// Checkbox properties.
#[derive(Clone, Debug, PartialEq)]
pub struct CheckboxProps {
    /// Shared properties.
    pub common: CommonProps,
    /// Initial checked value. It is used when the element is first registered;
    /// subsequent state is retained by [`UiDocument`].
    pub checked: bool,
}

impl CheckboxProps {
    /// Creates checkbox props with an unchecked default.
    pub fn new(id: impl Into<ElementId>, label: impl Into<String>) -> Self {
        Self {
            common: CommonProps::new(id).label(label),
            checked: false,
        }
    }

    /// Sets the initial checked value.
    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }
}

impl Default for CheckboxProps {
    fn default() -> Self {
        Self::new("", "")
    }
}

impl_common_builders!(CheckboxProps);

/// Switch properties. Switches share the same boolean form value as
/// checkboxes but use a compact sliding visual.
#[derive(Clone, Debug, PartialEq)]
pub struct SwitchProps {
    /// Shared properties.
    pub common: CommonProps,
    /// Initial on/off value.
    pub checked: bool,
}

impl SwitchProps {
    /// Creates switch props with an off default.
    pub fn new(id: impl Into<ElementId>, label: impl Into<String>) -> Self {
        Self {
            common: CommonProps::new(id).label(label),
            checked: false,
        }
    }

    /// Sets the initial on/off value.
    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }
}

impl Default for SwitchProps {
    fn default() -> Self {
        Self::new("", "")
    }
}

impl_common_builders!(SwitchProps);

/// Text input properties.
#[derive(Clone, Debug, PartialEq)]
pub struct TextInputProps {
    /// Shared properties.
    pub common: CommonProps,
    /// Initial text.
    pub value: String,
    /// Optional placeholder.
    pub hint: Option<String>,
    /// Uses a multiline editor when true.
    pub multiline: bool,
    /// Masks the value as a password.
    pub password: bool,
}

impl TextInputProps {
    /// Creates a single-line text input.
    pub fn new(id: impl Into<ElementId>, label: impl Into<String>) -> Self {
        Self {
            common: CommonProps::new(id).label(label),
            value: String::new(),
            hint: None,
            multiline: false,
            password: false,
        }
    }

    /// Sets the initial text.
    pub fn value(mut self, value: impl Into<String>) -> Self {
        self.value = value.into();
        self
    }

    /// Sets the placeholder text.
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// Uses a multiline editor.
    pub fn multiline(mut self, multiline: bool) -> Self {
        self.multiline = multiline;
        self
    }

    /// Masks the editor value.
    pub fn password(mut self, password: bool) -> Self {
        self.password = password;
        self
    }
}

impl Default for TextInputProps {
    fn default() -> Self {
        Self::new("", "")
    }
}

impl_common_builders!(TextInputProps);

/// Numeric drag-input properties.
#[derive(Clone, Debug, PartialEq)]
pub struct NumberInputProps {
    /// Shared properties.
    pub common: CommonProps,
    /// Initial value.
    pub value: f64,
    /// Optional inclusive lower bound.
    pub minimum: Option<f64>,
    /// Optional inclusive upper bound.
    pub maximum: Option<f64>,
    /// Dragging step.
    pub step: f64,
    /// Text before the value.
    pub prefix: String,
    /// Text after the value.
    pub suffix: String,
}

impl NumberInputProps {
    /// Creates a numeric input with a zero default and a unit step.
    pub fn new(id: impl Into<ElementId>, label: impl Into<String>) -> Self {
        Self {
            common: CommonProps::new(id).label(label),
            value: 0.0,
            minimum: None,
            maximum: None,
            step: 1.0,
            prefix: String::new(),
            suffix: String::new(),
        }
    }

    /// Sets the initial value.
    pub fn value(mut self, value: f64) -> Self {
        self.value = value;
        self
    }

    /// Sets both range bounds.
    pub fn range(mut self, minimum: f64, maximum: f64) -> Self {
        self.minimum = Some(minimum);
        self.maximum = Some(maximum);
        self
    }

    /// Sets the lower bound.
    pub fn minimum(mut self, minimum: f64) -> Self {
        self.minimum = Some(minimum);
        self
    }

    /// Sets the upper bound.
    pub fn maximum(mut self, maximum: f64) -> Self {
        self.maximum = Some(maximum);
        self
    }

    /// Sets the drag step.
    pub fn step(mut self, step: f64) -> Self {
        self.step = step;
        self
    }

    /// Sets a prefix.
    pub fn prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }

    /// Sets a suffix.
    pub fn suffix(mut self, suffix: impl Into<String>) -> Self {
        self.suffix = suffix.into();
        self
    }
}

impl Default for NumberInputProps {
    fn default() -> Self {
        Self::new("", "")
    }
}

impl_common_builders!(NumberInputProps);

/// Slider properties.
#[derive(Clone, Debug, PartialEq)]
pub struct SliderProps {
    /// Shared properties.
    pub common: CommonProps,
    /// Initial value.
    pub value: f64,
    /// Inclusive lower bound.
    pub minimum: f64,
    /// Inclusive upper bound.
    pub maximum: f64,
    /// Optional slider step.
    pub step: Option<f64>,
    /// Whether egui displays the current value.
    pub show_value: bool,
    /// Text before the value.
    pub prefix: String,
    /// Text after the value.
    pub suffix: String,
}

impl SliderProps {
    /// Creates a slider in the normalized 0–1 range.
    pub fn new(id: impl Into<ElementId>, label: impl Into<String>) -> Self {
        Self {
            common: CommonProps::new(id).label(label),
            value: 0.0,
            minimum: 0.0,
            maximum: 1.0,
            step: None,
            show_value: true,
            prefix: String::new(),
            suffix: String::new(),
        }
    }

    /// Sets the initial value.
    pub fn value(mut self, value: f64) -> Self {
        self.value = value;
        self
    }

    /// Sets the inclusive range.
    pub fn range(mut self, minimum: f64, maximum: f64) -> Self {
        self.minimum = minimum;
        self.maximum = maximum;
        self
    }

    /// Sets the slider step.
    pub fn step(mut self, step: f64) -> Self {
        self.step = Some(step);
        self
    }

    /// Controls the displayed value.
    pub fn show_value(mut self, show_value: bool) -> Self {
        self.show_value = show_value;
        self
    }

    /// Sets a prefix.
    pub fn prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }

    /// Sets a suffix.
    pub fn suffix(mut self, suffix: impl Into<String>) -> Self {
        self.suffix = suffix.into();
        self
    }
}

impl Default for SliderProps {
    fn default() -> Self {
        Self::new("", "")
    }
}

impl_common_builders!(SliderProps);

/// Select/drop-down properties.
#[derive(Clone, Debug, PartialEq)]
pub struct SelectProps {
    /// Shared properties.
    pub common: CommonProps,
    /// Initial selected option value.
    pub selected: String,
    /// Options displayed in the drop-down.
    pub options: Vec<OptionItem>,
}

impl SelectProps {
    /// Creates an empty select.
    pub fn new(id: impl Into<ElementId>, label: impl Into<String>) -> Self {
        Self {
            common: CommonProps::new(id).label(label),
            selected: String::new(),
            options: Vec::new(),
        }
    }

    /// Sets the initial selected value.
    pub fn selected(mut self, selected: impl Into<String>) -> Self {
        self.selected = selected.into();
        self
    }

    /// Replaces the options.
    pub fn options<I>(mut self, options: I) -> Self
    where
        I: IntoIterator<Item = OptionItem>,
    {
        self.options = options.into_iter().collect();
        if self.selected.is_empty() {
            self.selected = self
                .options
                .iter()
                .find(|option| !option.disabled)
                .map(|option| option.value.clone())
                .unwrap_or_default();
        }
        self
    }

    /// Adds one option.
    pub fn option(mut self, value: impl Into<String>, label: impl Into<String>) -> Self {
        let option = OptionItem::new(value, label);
        if self.selected.is_empty() {
            self.selected = option.value.clone();
        }
        self.options.push(option);
        self
    }
}

impl Default for SelectProps {
    fn default() -> Self {
        Self::new("", "")
    }
}

impl_common_builders!(SelectProps);

/// Mutually exclusive radio-button group properties.
#[derive(Clone, Debug, PartialEq)]
pub struct RadioGroupProps {
    /// Shared properties.
    pub common: CommonProps,
    /// Initial selected option value.
    pub selected: String,
    /// Radio options.
    pub options: Vec<OptionItem>,
    /// Lay out radio buttons horizontally when true.
    pub horizontal: bool,
}

impl RadioGroupProps {
    /// Creates a vertical radio group.
    pub fn new(id: impl Into<ElementId>, label: impl Into<String>) -> Self {
        Self {
            common: CommonProps::new(id).label(label),
            selected: String::new(),
            options: Vec::new(),
            horizontal: false,
        }
    }

    /// Sets the initial selected value.
    pub fn selected(mut self, selected: impl Into<String>) -> Self {
        self.selected = selected.into();
        self
    }

    /// Replaces the options.
    pub fn options<I>(mut self, options: I) -> Self
    where
        I: IntoIterator<Item = OptionItem>,
    {
        self.options = options.into_iter().collect();
        if self.selected.is_empty() {
            self.selected = self
                .options
                .iter()
                .find(|option| !option.disabled)
                .map(|option| option.value.clone())
                .unwrap_or_default();
        }
        self
    }

    /// Adds one option.
    pub fn option(mut self, value: impl Into<String>, label: impl Into<String>) -> Self {
        let option = OptionItem::new(value, label);
        if self.selected.is_empty() {
            self.selected = option.value.clone();
        }
        self.options.push(option);
        self
    }

    /// Lays the radio buttons out horizontally.
    pub fn horizontal(mut self, horizontal: bool) -> Self {
        self.horizontal = horizontal;
        self
    }
}

impl Default for RadioGroupProps {
    fn default() -> Self {
        Self::new("", "")
    }
}

impl_common_builders!(RadioGroupProps);

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
    changed: bool,
    clicked: bool,
}

impl Element {
    fn new(
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
    values: BTreeMap<ElementId, Value>,
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
    document: &'a UiDocument,
    id: ElementId,
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
pub struct ListenerId(u64);

struct Listener {
    id: ListenerId,
    callback: Box<dyn FnMut(&UiEvent)>,
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
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[test]
    fn values_and_element_lookup_are_retained_by_id() {
        let mut document = UiDocument::new();
        let ctx = egui::Context::default();
        ctx.begin_pass(egui::RawInput::default());
        egui::CentralPanel::default().show(&ctx, |ctx_ui| {
            document.checkbox(
                ctx_ui,
                CheckboxProps::new("enabled", "Enabled").checked(true),
            );
        });
        let _ = ctx.end_pass();

        assert_eq!(document.checked("enabled"), Some(true));
        assert_eq!(
            document.get_element_by_id("enabled").unwrap().kind,
            ElementKind::Checkbox
        );
        assert!(!document.changed("enabled"));
        assert!(document.set_value("enabled", false));
        assert_eq!(document.checked("enabled"), Some(false));
    }

    #[test]
    fn forms_collect_all_declared_values_and_can_iterate_them() {
        let mut document = UiDocument::new();
        let ctx = egui::Context::default();
        ctx.begin_pass(egui::RawInput::default());
        egui::CentralPanel::default().show(&ctx, |ctx_ui| {
            document.text_input(
                ctx_ui,
                TextInputProps::new("settings.name", "Name")
                    .value("qpwgraph")
                    .form("settings"),
            );
            document.checkbox(
                ctx_ui,
                CheckboxProps::new("settings.enabled", "Enabled")
                    .checked(true)
                    .form("settings"),
            );
            document.slider(
                ctx_ui,
                SliderProps::new("other.volume", "Volume")
                    .value(0.5)
                    .form("other"),
            );
        });
        let _ = ctx.end_pass();

        let values = document.form_values("settings");
        assert_eq!(values.len(), 2);
        assert_eq!(values.get_string("settings.name"), Some("qpwgraph"));
        assert_eq!(values.get_bool("settings.enabled"), Some(true));
        assert_eq!(document.form("settings").iter().count(), 2);
        assert_eq!(
            document.form_values("other").get_number("other.volume"),
            Some(0.5)
        );
    }

    #[test]
    fn every_builtin_component_registers_and_renders() {
        let mut document = UiDocument::new();
        let ctx = egui::Context::default();
        ctx.begin_pass(egui::RawInput::default());
        egui::CentralPanel::default().show(&ctx, |ui| {
            document.label(ui, LabelProps::new("label", "Label"));
            document.button(ui, ButtonProps::new("button", "Button"));
            document.checkbox(ui, CheckboxProps::new("checkbox", "Checkbox"));
            document.switch(ui, SwitchProps::new("switch", "Switch"));
            document.text_input(ui, TextInputProps::new("text", "Text"));
            document.number_input(ui, NumberInputProps::new("number", "Number"));
            document.slider(ui, SliderProps::new("slider", "Slider"));
            document.select(
                ui,
                SelectProps::new("select", "Select").option("one", "One"),
            );
            document.radio_group(
                ui,
                RadioGroupProps::new("radio", "Radio").option("one", "One"),
            );
        });
        let _ = ctx.end_pass();

        assert_eq!(document.elements().count(), 9);
        assert_eq!(
            document.get_element_by_id("switch").unwrap().kind,
            ElementKind::Switch
        );
        assert_eq!(
            document.get_element_by_id("select").unwrap().options[0].value,
            "one"
        );
    }

    #[test]
    fn listeners_receive_events_and_can_be_removed() {
        let mut document = UiDocument::new();
        let events: Rc<RefCell<Vec<Value>>> = Rc::new(RefCell::new(Vec::new()));
        let received = Rc::clone(&events);
        let listener = document.on_change("field", move |event| {
            received.borrow_mut().push(event.value.clone());
        });
        document.dispatch_event(UiEvent::new("field", EventType::Change, "updated"));
        assert_eq!(
            events.as_ref().borrow().as_slice(),
            &[Value::String("updated".into())]
        );
        assert!(document.remove_event_listener(listener));
        document.dispatch_event(UiEvent::new("field", EventType::Change, "ignored"));
        assert_eq!(events.as_ref().borrow().len(), 1);
    }

    #[test]
    fn props_have_useful_defaults_and_options_are_retained() {
        let props = SelectProps::new("mode", "Mode")
            .selected("easy")
            .option("easy", "Easy")
            .option("advanced", "Advanced")
            .width(180.0)
            .disabled(true);
        assert_eq!(props.common.id.as_str(), "mode");
        assert!(!props.common.enabled);
        assert_eq!(props.options.len(), 2);
        assert_eq!(Style::default(), Style::default());
    }
}
