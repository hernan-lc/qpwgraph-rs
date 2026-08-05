use super::super::super::{
    ButtonProps, CheckboxProps, ElementKind, EventType, LabelProps, SwitchProps, UiDocument, Value,
};
use super::shared::{add_sized, switch_widget, with_common};
use super::{finish_control, prepare_control};
use egui::{vec2, Response, Ui};

impl UiDocument {
    /// Renders a label.
    pub fn label(&mut self, ui: &mut Ui, props: LabelProps) -> Response {
        let (id, _) = prepare_control(
            self,
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
        with_common(ui, &props.common, &self.theme, |ui| ui.label(props.text))
    }

    /// Renders a push button and emits a click event when activated.
    pub fn button(&mut self, ui: &mut Ui, props: ButtonProps) -> Response {
        let (id, _) = prepare_control(
            self,
            &props.common,
            ElementKind::Button,
            Value::Bool(false),
            vec![],
        );
        let style = props.common.style.clone();
        let text = props.text.clone();
        let action_value = props
            .action_value
            .map(Value::String)
            .unwrap_or(Value::Bool(true));
        let theme = self.theme.clone();
        let response = with_common(ui, &props.common, &self.theme, |ui| {
            let mut button = egui::Button::new(text);
            if let Some(fill) = style.resolve_fill(&theme) {
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
            self.record_button_click(&id, action_value);
        } else {
            self.observe_focus(&id, &response);
        }
        response
    }

    /// Renders a checkbox and emits a change event when its value toggles.
    pub fn checkbox(&mut self, ui: &mut Ui, props: CheckboxProps) -> Response {
        let (id, before) = prepare_control(
            self,
            &props.common,
            ElementKind::Checkbox,
            Value::Bool(props.checked),
            vec![],
        );
        let mut checked = before.as_bool().unwrap_or(props.checked);
        let style = props.common.style.clone();
        let label = props.common.label.clone();
        let response = with_common(ui, &props.common, &self.theme, |ui| {
            let checkbox = egui::Checkbox::new(&mut checked, label.unwrap_or_default());
            add_sized(ui, &style, checkbox)
        });
        finish_control(
            self,
            id,
            before,
            Value::Bool(checked),
            response,
            &[EventType::Change],
        )
    }

    /// Renders a switch/toggle and emits a change event when its value toggles.
    pub fn switch(&mut self, ui: &mut Ui, props: SwitchProps) -> Response {
        let (id, before) = prepare_control(
            self,
            &props.common,
            ElementKind::Switch,
            Value::Bool(props.checked),
            vec![],
        );
        let mut checked = before.as_bool().unwrap_or(props.checked);
        let label = props.common.label.clone();
        let style = props.common.style.clone();
        let response = with_common(ui, &props.common, &self.theme, |ui| {
            switch_widget(ui, &id, &mut checked, label.as_deref(), &style, &self.theme)
        });
        finish_control(
            self,
            id,
            before,
            Value::Bool(checked),
            response,
            &[EventType::Change],
        )
    }
}
