use std::borrow::Borrow;
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
