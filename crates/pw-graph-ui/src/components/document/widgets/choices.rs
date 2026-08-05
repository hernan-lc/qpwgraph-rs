use super::super::super::{
    ElementKind, EventType, OptionItem, RadioGroupProps, SelectProps, ThemeToken, UiDocument, Value,
};
use super::shared::{labelled, with_common};
use super::{finish_control, prepare_control};
use egui::{Response, Ui};

impl UiDocument {
    /// Renders a drop-down selector and emits a change event when a new option
    /// is selected.
    pub fn select(&mut self, ui: &mut Ui, props: SelectProps) -> Response {
        let (id, before) = prepare_control(
            self,
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
        let response = with_common(ui, &props.common, &self.theme, |ui| {
            let width = style
                .width
                .unwrap_or_else(|| (ui.available_width() - 4.0).max(120.0));
            let mut changed = false;
            let combo_response = egui::ComboBox::new(("ui-document-select", id.clone()), label)
                .selected_text(selected_text)
                .width(width)
                .show_ui(ui, |ui| {
                    for option in &options {
                        let selection = selectable_option(ui, selected == option.value, option);
                        if selection.clicked() {
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
        finish_control(
            self,
            id,
            before,
            Value::String(selected),
            response,
            &[EventType::Change],
        )
    }

    /// Renders a radio-button group and emits a change event when a new option
    /// is selected.
    pub fn radio_group(&mut self, ui: &mut Ui, props: RadioGroupProps) -> Response {
        let (id, before) = prepare_control(
            self,
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
        let horizontal = props.horizontal;
        let text_color = self.theme.color(ThemeToken::TextPrimary);
        let response = with_common(ui, &props.common, &self.theme, |ui| {
            labelled(ui, label.as_deref(), text_color, |ui| {
                let mut combined: Option<Response> = None;
                let mut selected_changed = false;
                let mut draw_options = |ui: &mut Ui| {
                    for option in &options {
                        let item_response = radio_option(ui, selected == option.value, option);
                        if item_response.clicked() {
                            selected = option.value.clone();
                            selected_changed = true;
                        }
                        combined = Some(match combined.take() {
                            Some(previous) => previous.union(item_response),
                            None => item_response,
                        });
                    }
                };
                let container_response = if horizontal {
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
        finish_control(
            self,
            id,
            before,
            Value::String(selected),
            response,
            &[EventType::Change],
        )
    }
}

fn selectable_option(ui: &mut Ui, selected: bool, option: &OptionItem) -> Response {
    if option.disabled {
        ui.add_enabled_ui(false, |ui| ui.selectable_label(selected, &option.label))
            .inner
    } else {
        ui.selectable_label(selected, &option.label)
    }
}

fn radio_option(ui: &mut Ui, selected: bool, option: &OptionItem) -> Response {
    if option.disabled {
        ui.add_enabled_ui(false, |ui| ui.radio(selected, &option.label))
            .inner
    } else {
        ui.radio(selected, &option.label)
    }
}
