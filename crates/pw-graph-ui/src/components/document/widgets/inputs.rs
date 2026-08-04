use super::super::super::{
    ElementKind, EventType, NumberInputProps, SliderProps, TextInputProps, UiDocument, Value,
};
use super::shared::{add_sized, labelled, normalize_optional_range, normalize_range, with_common};
use super::{finish_control, prepare_control};
use egui::{
    include_image, vec2, Align, Button, Color32, Frame, Image, ImageSource, Layout, Response, Ui,
    Vec2,
};

const NUMBER_INPUT_STEPPER_WIDTH: f32 = 20.0;
const NUMBER_INPUT_ICON_SIZE: f32 = 9.0;

fn number_step_icon(increment: bool) -> ImageSource<'static> {
    if increment {
        include_image!("../../../../assets/icons/arrow_up.svg")
    } else {
        include_image!("../../../../assets/icons/arrow_down.svg")
    }
}

fn number_step_button(ui: &mut Ui, increment: bool, enabled: bool, size: Vec2) -> Response {
    ui.add_enabled_ui(enabled, |ui| {
        let color = ui.visuals().text_color();
        let image = Image::new(number_step_icon(increment))
            .fit_to_exact_size(vec2(NUMBER_INPUT_ICON_SIZE, NUMBER_INPUT_ICON_SIZE))
            .tint(if enabled {
                color
            } else {
                Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 120)
            });
        ui.add_sized(size, Button::image(image).rounding(2.0))
    })
    .inner
}

#[allow(clippy::too_many_arguments)]
fn number_input_surface(
    ui: &mut Ui,
    style: &super::super::super::Style,
    value: &mut f64,
    minimum: f64,
    maximum: f64,
    step: f64,
    prefix: &str,
    suffix: &str,
) -> Response {
    let width = style
        .width
        .unwrap_or_else(|| ui.spacing().interact_size.x.max(72.0));
    let height = style
        .height
        .unwrap_or_else(|| ui.spacing().interact_size.y.max(24.0));
    let stepper_width = NUMBER_INPUT_STEPPER_WIDTH.min((width * 0.3).max(16.0));
    let field_width = (width - stepper_width).max(1.0);
    let half_height = (height / 2.0).max(1.0);
    let frame_fill = style
        .fill
        .unwrap_or_else(|| ui.visuals().widgets.inactive.bg_fill);
    let frame_stroke = style
        .stroke
        .unwrap_or_else(|| ui.visuals().widgets.inactive.bg_stroke);
    let frame_rounding = style.rounding.unwrap_or(4.0);
    let step_size = if step.is_finite() && step.abs() > f64::EPSILON {
        step.abs()
    } else {
        1.0
    };

    let frame_response = Frame::none()
        .fill(frame_fill)
        .stroke(frame_stroke)
        .rounding(frame_rounding)
        .show(ui, |ui| {
            ui.allocate_ui_with_layout(
                vec2(width, height),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    let mut input = egui::DragValue::new(value);
                    if minimum.is_finite() || maximum.is_finite() {
                        input = input.range(minimum..=maximum);
                    }
                    input = input.speed(step_size);
                    if !prefix.is_empty() {
                        input = input.prefix(prefix);
                    }
                    if !suffix.is_empty() {
                        input = input.suffix(suffix);
                    }
                    let field_response = ui.add_sized(vec2(field_width, height), input);
                    let stepper = ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        let incremented = number_step_button(
                            ui,
                            true,
                            *value < maximum,
                            vec2(stepper_width, half_height),
                        )
                        .clicked();
                        let decremented = number_step_button(
                            ui,
                            false,
                            *value > minimum,
                            vec2(stepper_width, half_height),
                        )
                        .clicked();
                        (incremented, decremented)
                    });
                    (field_response, stepper.inner.0, stepper.inner.1)
                },
            )
            .inner
        });
    let (field_response, incremented, decremented) = frame_response.inner;
    // Keep the DragValue response as the primary response so focus and
    // keyboard events continue to use its stable widget ID, while the frame
    // expands hover/click geometry to include the embedded stepper.
    let mut response = field_response.union(frame_response.response);
    if incremented && !decremented {
        *value = (*value + step_size).clamp(minimum, maximum);
        response.mark_changed();
    } else if decremented && !incremented {
        *value = (*value - step_size).clamp(minimum, maximum);
        response.mark_changed();
    }
    response
}

impl UiDocument {
    /// Renders a text input and emits input and change events while editing.
    pub fn text_input(&mut self, ui: &mut Ui, props: TextInputProps) -> Response {
        let (id, before) = prepare_control(
            self,
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
        let multiline = props.multiline;
        let password = props.password;
        let response = with_common(ui, &props.common, |ui| {
            labelled(ui, label.as_deref(), |ui| {
                let mut editor = if multiline {
                    egui::TextEdit::multiline(&mut text)
                } else {
                    egui::TextEdit::singleline(&mut text)
                };
                if let Some(hint) = hint {
                    editor = editor.hint_text(hint);
                }
                editor = editor.password(password);
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
        finish_control(
            self,
            id,
            before,
            Value::String(text),
            response,
            &[EventType::Input, EventType::Change],
        )
    }

    /// Renders a draggable numeric input and emits a change event when its
    /// value changes.
    pub fn number_input(&mut self, ui: &mut Ui, props: NumberInputProps) -> Response {
        let (id, before) = prepare_control(
            self,
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
                let (minimum, maximum) = normalize_optional_range(minimum, maximum);
                number_input_surface(
                    ui, &style, &mut value, minimum, maximum, step, &prefix, &suffix,
                )
            })
        });
        finish_control(
            self,
            id,
            before,
            Value::Number(value),
            response,
            &[EventType::Change],
        )
    }

    /// Renders a ranged slider and emits a change event when its value changes.
    pub fn slider(&mut self, ui: &mut Ui, props: SliderProps) -> Response {
        let (id, before) = prepare_control(
            self,
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
                // egui's slider uses `Spacing::slider_width` for its inner
                // interaction rect instead of the outer allocation supplied
                // by `add_sized`. Keep the reusable component's explicit
                // width authoritative so fixed-size layouts do not leave a
                // misleading gap beside the slider.
                if let Some(width) = style.width {
                    ui.spacing_mut().slider_width = width;
                }
                add_sized(ui, &style, slider)
            })
        });
        finish_control(
            self,
            id,
            before,
            Value::Number(value),
            response,
            &[EventType::Change],
        )
    }
}
