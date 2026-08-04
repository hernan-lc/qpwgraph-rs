use super::super::super::{
    ElementKind, EventType, NumberInputProps, SliderProps, TextInputProps, UiDocument, Value,
};
use super::shared::{add_sized, labelled, normalize_optional_range, normalize_range, with_common};
use super::{finish_control, prepare_control};
use egui::{Response, Ui};

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
                add_sized(ui, &style, input)
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
