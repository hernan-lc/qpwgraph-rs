use super::{ElementId, OptionItem, Theme, ThemeToken};
use egui::{Color32, Frame, Margin, Stroke};

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
    /// Symbolic text token override.
    pub text_token: Option<ThemeToken>,
    /// Background fill. For controls that paint their own background this is
    /// also passed to the widget when possible.
    pub fill: Option<Color32>,
    /// Symbolic fill token override.
    pub fill_token: Option<ThemeToken>,
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
        self.text_token = None;
        self
    }

    /// Sets text color using a symbolic theme token.
    pub fn text_token(mut self, token: ThemeToken) -> Self {
        self.text_token = Some(token);
        self.text_color = None;
        self
    }

    /// Sets the background fill.
    pub fn fill(mut self, color: Color32) -> Self {
        self.fill = Some(color);
        self.fill_token = None;
        self
    }

    /// Sets background fill using a symbolic theme token.
    pub fn fill_token(mut self, token: ThemeToken) -> Self {
        self.fill_token = Some(token);
        self.fill = None;
        self
    }

    /// Resolves text color against a theme.
    pub fn resolve_text_color(&self, theme: &Theme) -> Option<Color32> {
        self.text_color
            .or_else(|| self.text_token.map(|token| theme.color(token)))
    }

    /// Resolves fill color against a theme.
    pub fn resolve_fill(&self, theme: &Theme) -> Option<Color32> {
        self.fill
            .or_else(|| self.fill_token.map(|token| theme.color(token)))
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

    /// Builds a themed [`egui::Frame`] from the style's resolved values.
    ///
    /// This is the single place that translates a [`Style`] into frame
    /// chrome, so components that paint their own surface (cards, dialogs,
    /// framed inputs) share one fill/stroke/rounding/margin policy.
    pub fn to_frame(&self, theme: &Theme) -> Frame {
        let mut frame = Frame::none();
        // Resolve fill: explicit color wins, otherwise fall back to theme token.
        if let Some(fill) = self.resolve_fill(theme) {
            frame = frame.fill(fill);
        }
        if let Some(stroke) = self.stroke {
            frame = frame.stroke(stroke);
        }
        if let Some(rounding) = self.rounding {
            frame = frame.rounding(rounding);
        }
        if let Some(inner_margin) = self.inner_margin {
            frame = frame.inner_margin(Margin::same(inner_margin));
        }
        frame
    }

    pub(super) fn has_frame(&self) -> bool {
        self.fill.is_some()
            || self.fill_token.is_some()
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

    /// Sets text color.
    pub fn text_color(mut self, color: Color32) -> Self {
        self.style = self.style.text_color(color);
        self
    }

    /// Sets text color using a symbolic theme token.
    pub fn text_token(mut self, token: ThemeToken) -> Self {
        self.style = self.style.text_token(token);
        self
    }

    /// Sets background fill.
    pub fn fill(mut self, color: Color32) -> Self {
        self.style = self.style.fill(color);
        self
    }

    /// Sets background fill using a symbolic theme token.
    pub fn fill_token(mut self, token: ThemeToken) -> Self {
        self.style = self.style.fill_token(token);
        self
    }

    /// Sets border stroke.
    pub fn stroke(mut self, stroke: Stroke) -> Self {
        self.style = self.style.stroke(stroke);
        self
    }

    /// Sets corner rounding.
    pub fn rounding(mut self, radius: f32) -> Self {
        self.style = self.style.rounding(radius);
        self
    }

    /// Sets inner margin.
    pub fn inner_margin(mut self, margin: f32) -> Self {
        self.style = self.style.inner_margin(margin);
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

            /// Sets text color.
            pub fn text_color(mut self, color: egui::Color32) -> Self {
                self.common = self.common.text_color(color);
                self
            }

            /// Sets text color using a symbolic theme token.
            pub fn text_token(mut self, token: ThemeToken) -> Self {
                self.common = self.common.text_token(token);
                self
            }

            /// Sets background fill.
            pub fn fill(mut self, color: egui::Color32) -> Self {
                self.common = self.common.fill(color);
                self
            }

            /// Sets background fill using a symbolic theme token.
            pub fn fill_token(mut self, token: ThemeToken) -> Self {
                self.common = self.common.fill_token(token);
                self
            }

            /// Sets border stroke.
            pub fn stroke(mut self, stroke: egui::Stroke) -> Self {
                self.common = self.common.stroke(stroke);
                self
            }

            /// Sets corner rounding.
            pub fn rounding(mut self, radius: f32) -> Self {
                self.common = self.common.rounding(radius);
                self
            }

            /// Sets inner margin.
            pub fn inner_margin(mut self, margin: f32) -> Self {
                self.common = self.common.inner_margin(margin);
                self
            }
        }
    };
}

/// Generates the `selected`/`options`/`option` builders shared by every
/// select-like props type. The "first enabled option wins when nothing is
/// selected yet" rule must be one definition so a control cannot pick a
/// disabled option as its default.
macro_rules! impl_selectable_options {
    ($type:ty) => {
        impl $type {
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
}

impl_selectable_options!(SelectProps);

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

    /// Lays the radio buttons out horizontally.
    pub fn horizontal(mut self, horizontal: bool) -> Self {
        self.horizontal = horizontal;
        self
    }
}

impl_selectable_options!(RadioGroupProps);

impl Default for RadioGroupProps {
    fn default() -> Self {
        Self::new("", "")
    }
}

impl_common_builders!(RadioGroupProps);
