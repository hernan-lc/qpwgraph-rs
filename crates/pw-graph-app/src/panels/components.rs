//! Panel adapters around the shared `pw-graph-ui` document components.
//!
//! These helpers keep application-owned configuration synchronized with the
//! retained document while leaving each panel concise. Custom icon controls
//! use `UiDocument::record_click`, so their existing visual treatment still
//! participates in the same ID/event system.

#![allow(dead_code)]

use crate::icons::{
    icon_button as draw_icon_button, icon_button_enabled as draw_icon_button_enabled,
    sidebar_icon_button as draw_sidebar_icon_button,
    sidebar_icon_button_enabled as draw_sidebar_icon_button_enabled,
    sidebar_icon_toggle_button as draw_sidebar_icon_toggle_button,
    sidebar_nav_icon_button as draw_sidebar_nav_icon_button, Icon,
};
use eframe::egui::{Response, RichText, Ui};
use pw_graph_ui::{
    setting_row, ButtonProps, CheckboxProps, EventType, NumberInputProps, OptionItem, SelectProps,
    SliderProps, SwitchProps, TextInputProps, UiDocument, Value,
};

pub(super) fn modal_hint(ui: &mut Ui, text: String) {
    ui.label(RichText::new(text).weak());
}

pub(super) fn modal_step_heading(ui: &mut Ui, step: usize, current: usize, label: String) {
    let text = format!("{}. {label}", step + 1);
    ui.label(if step == current {
        RichText::new(text).strong()
    } else {
        RichText::new(text).weak()
    });
}

fn sync_value(document: &mut UiDocument, id: &str, value: Value) {
    if document.value(id) != Some(&value) {
        document.set_value(id, value);
    }
}

pub(super) fn document_button(
    document: &mut UiDocument,
    ui: &mut Ui,
    id: &str,
    text: String,
    enabled: bool,
) -> bool {
    document
        .button(ui, ButtonProps::new(id, text).enabled(enabled))
        .clicked()
}

fn document_step_button(
    document: &mut UiDocument,
    ui: &mut Ui,
    id: &str,
    label: &str,
    tooltip: String,
) -> bool {
    document
        .button(
            ui,
            ButtonProps::new(id, label)
                .size(28.0, 18.0)
                .tooltip(tooltip),
        )
        .clicked()
}

pub(super) fn document_checkbox(
    document: &mut UiDocument,
    ui: &mut Ui,
    id: &str,
    current: bool,
    label: String,
    tooltip: Option<String>,
) -> bool {
    sync_value(document, id, Value::Bool(current));
    document.checkbox(
        ui,
        CheckboxProps::new(id, label)
            .checked(current)
            .tooltip_option(tooltip),
    );
    document.checked(id).unwrap_or(current)
}

pub(super) fn document_switch(
    document: &mut UiDocument,
    ui: &mut Ui,
    id: &str,
    current: bool,
    label: String,
    tooltip: Option<String>,
) -> bool {
    sync_value(document, id, Value::Bool(current));
    document.switch(
        ui,
        SwitchProps::new(id, label)
            .checked(current)
            .tooltip_option(tooltip),
    );
    document.checked(id).unwrap_or(current)
}

pub(super) fn document_icon_checkbox(
    document: &mut UiDocument,
    ui: &mut Ui,
    id: &str,
    current: bool,
    icon: Icon,
    label: String,
    explanation: String,
) -> bool {
    ui.horizontal(|ui| {
        // Keep the icon affordance from the existing panel design, while the
        // actual value and change event belong to UiDocument.
        crate::icons::icon_label(ui, icon, explanation.clone());
        document_checkbox(document, ui, id, current, label, Some(explanation))
    })
    .inner
}

/// A settings-specific composition of the shared switch widget.
///
/// Preferences need more context than a bare checkbox: the icon identifies
/// the setting family, the description explains the consequence, and the
/// control should line up on the trailing edge of every row. This belongs in
/// the application layer because it is layout for a settings page rather than
/// a new primitive that the document component library should know about.
pub(super) fn document_setting_switch(
    document: &mut UiDocument,
    ui: &mut Ui,
    id: &str,
    current: bool,
    icon: Icon,
    label: String,
    explanation: String,
) -> bool {
    let checked = setting_row(
        ui,
        |ui| {
            crate::icons::icon_label(ui, icon, explanation.clone());
            ui.add_space(8.0);
            ui.vertical(|ui| {
                ui.label(RichText::new(label).strong());
                ui.label(RichText::new(explanation.clone()).small().weak());
            });
        },
        |ui| {
            document_switch(
                document,
                ui,
                id,
                current,
                String::new(),
                Some(explanation.clone()),
            )
        },
    );
    ui.add_space(2.0);
    checked
}

#[allow(clippy::too_many_arguments)]
pub(super) fn document_setting_select<I>(
    document: &mut UiDocument,
    ui: &mut Ui,
    id: &str,
    current: &str,
    icon: Option<Icon>,
    label: String,
    explanation: String,
    options: I,
    width: f32,
) -> String
where
    I: IntoIterator<Item = OptionItem>,
{
    let selected = setting_row(
        ui,
        |ui| {
            if let Some(icon) = icon {
                crate::icons::icon_label(ui, icon, explanation.clone());
                ui.add_space(8.0);
            }
            ui.vertical(|ui| {
                ui.label(RichText::new(label).strong());
                ui.label(RichText::new(explanation.clone()).small().weak());
            });
        },
        |ui| {
            document_select_sized(
                document,
                ui,
                id,
                current,
                String::new(),
                options,
                Some(width),
            )
        },
    );
    ui.add_space(2.0);
    selected
}

pub(super) fn document_setting_switch_plain(
    document: &mut UiDocument,
    ui: &mut Ui,
    id: &str,
    current: bool,
    label: String,
    explanation: String,
) -> bool {
    let checked = setting_row(
        ui,
        |ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new(label).strong());
                if !explanation.is_empty() {
                    ui.label(RichText::new(explanation.clone()).small().weak());
                }
            });
        },
        |ui| {
            document_switch(
                document,
                ui,
                id,
                current,
                String::new(),
                Some(explanation.clone()),
            )
        },
    );
    ui.add_space(2.0);
    checked
}

pub(super) fn document_text_input(
    document: &mut UiDocument,
    ui: &mut Ui,
    id: &str,
    current: &str,
    label: String,
    hint: Option<String>,
) -> (Response, String) {
    document_text_input_sized(document, ui, id, current, label, hint, None)
}

pub(super) fn document_text_input_sized(
    document: &mut UiDocument,
    ui: &mut Ui,
    id: &str,
    current: &str,
    label: String,
    hint: Option<String>,
    width: Option<f32>,
) -> (Response, String) {
    sync_value(document, id, Value::String(current.to_owned()));
    let mut props = TextInputProps::new(id, label).value(current);
    if let Some(hint) = hint {
        props = props.hint(hint);
    }
    if let Some(width) = width {
        props = props.width(width);
    }
    let response = document.text_input(ui, props);
    let value = document.text(id).unwrap_or(current).to_owned();
    (response, value)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn document_slider(
    document: &mut UiDocument,
    ui: &mut Ui,
    id: &str,
    current: f32,
    minimum: f32,
    maximum: f32,
    step: f64,
    label: String,
    show_value: bool,
    tooltip: Option<String>,
) -> (Response, f32) {
    document_slider_sized(
        document, ui, id, current, minimum, maximum, step, label, show_value, tooltip, None,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn document_slider_sized(
    document: &mut UiDocument,
    ui: &mut Ui,
    id: &str,
    current: f32,
    minimum: f32,
    maximum: f32,
    step: f64,
    label: String,
    show_value: bool,
    tooltip: Option<String>,
    width: Option<f32>,
) -> (Response, f32) {
    sync_value(document, id, Value::Number(f64::from(current)));
    let mut props = SliderProps::new(id, label)
        .value(f64::from(current))
        .range(f64::from(minimum), f64::from(maximum))
        .step(step)
        .show_value(show_value)
        .tooltip_option(tooltip);
    if let Some(width) = width {
        props = props.width(width);
    }
    let response = document.slider(ui, props);
    let value = document.number(id).unwrap_or(f64::from(current)) as f32;
    (response, value)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn document_setting_slider(
    document: &mut UiDocument,
    ui: &mut Ui,
    id: &str,
    current: f32,
    minimum: f32,
    maximum: f32,
    step: f64,
    label: String,
    explanation: String,
    width: f32,
) -> f32 {
    let value = setting_row(
        ui,
        |ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new(label).strong());
                if !explanation.is_empty() {
                    ui.label(RichText::new(explanation.clone()).small().weak());
                }
            });
        },
        |ui| {
            let (_, next) = document_slider_sized(
                document,
                ui,
                id,
                current,
                minimum,
                maximum,
                step,
                String::new(),
                true,
                Some(explanation.clone()),
                Some(width),
            );
            next
        },
    );
    ui.add_space(2.0);
    value
}

#[allow(clippy::too_many_arguments)]
pub(super) fn document_setting_number(
    document: &mut UiDocument,
    ui: &mut Ui,
    id: &str,
    current: f32,
    minimum: f32,
    maximum: f32,
    step: f64,
    label: String,
    explanation: String,
    width: f32,
) -> f32 {
    let mut value = current as f64;
    let increment_id = format!("{id}.increment");
    let decrement_id = format!("{id}.decrement");
    setting_row(
        ui,
        |ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new(label).strong());
                ui.label(RichText::new(explanation.clone()).small().weak());
            });
        },
        |ui| {
            ui.horizontal(|ui| {
                let (_, edited) = document_number_input_sized(
                    document,
                    ui,
                    id,
                    current as f64,
                    minimum as f64,
                    maximum as f64,
                    step,
                    String::new(),
                    Some(width),
                    Some(explanation.clone()),
                );
                value = edited;
                ui.vertical(|ui| {
                    let incremented =
                        document_step_button(document, ui, &increment_id, "▲", explanation.clone());
                    let decremented =
                        document_step_button(document, ui, &decrement_id, "▼", explanation.clone());
                    if incremented && !decremented {
                        value += step;
                    } else if decremented && !incremented {
                        value -= step;
                    }
                });
            });
        },
    );
    value = value.clamp(minimum as f64, maximum as f64);
    if document
        .number(id)
        .is_some_and(|number| (number - value).abs() > f64::EPSILON)
    {
        document.set_value_and_emit(id, Value::Number(value), EventType::Change);
    }
    ui.add_space(2.0);
    value as f32
}

#[allow(clippy::too_many_arguments)]
fn document_number_input_sized(
    document: &mut UiDocument,
    ui: &mut Ui,
    id: &str,
    current: f64,
    minimum: f64,
    maximum: f64,
    step: f64,
    label: String,
    width: Option<f32>,
    tooltip: Option<String>,
) -> (Response, f64) {
    sync_value(document, id, Value::Number(current));
    let mut props = NumberInputProps::new(id, label)
        .value(current)
        .range(minimum, maximum)
        .step(step)
        .tooltip_option(tooltip);
    if let Some(width) = width {
        props = props.width(width);
    }
    let response = document.number_input(ui, props);
    let value = document.number(id).unwrap_or(current);
    (response, value)
}

pub(super) fn document_select<I>(
    document: &mut UiDocument,
    ui: &mut Ui,
    id: &str,
    current: &str,
    label: String,
    options: I,
) -> String
where
    I: IntoIterator<Item = OptionItem>,
{
    document_select_sized(document, ui, id, current, label, options, None)
}

pub(super) fn document_select_sized<I>(
    document: &mut UiDocument,
    ui: &mut Ui,
    id: &str,
    current: &str,
    label: String,
    options: I,
    width: Option<f32>,
) -> String
where
    I: IntoIterator<Item = OptionItem>,
{
    sync_value(document, id, Value::String(current.to_owned()));
    let mut props = SelectProps::new(id, label)
        .selected(current)
        .options(options);
    if let Some(width) = width {
        props = props.width(width);
    }
    document.select(ui, props);
    document.text(id).unwrap_or(current).to_owned()
}

pub(super) fn document_icon_button(
    document: &mut UiDocument,
    ui: &mut Ui,
    id: &str,
    icon: Icon,
    label: String,
    explanation: String,
) -> bool {
    let clicked = draw_icon_button(ui, id, icon, label, explanation);
    document.record_click(id, clicked)
}

pub(super) fn document_icon_button_enabled(
    document: &mut UiDocument,
    ui: &mut Ui,
    id: &str,
    icon: Icon,
    label: String,
    explanation: String,
    enabled: bool,
) -> bool {
    let clicked = draw_icon_button_enabled(ui, id, icon, label, explanation, enabled);
    document.record_click(id, clicked)
}

pub(super) fn document_sidebar_icon_button(
    document: &mut UiDocument,
    ui: &mut Ui,
    id: &str,
    icon: Icon,
    label: String,
    explanation: String,
) -> bool {
    let clicked = draw_sidebar_icon_button(ui, id, icon, label, explanation);
    document.record_click(id, clicked)
}

pub(super) fn document_sidebar_icon_button_enabled(
    document: &mut UiDocument,
    ui: &mut Ui,
    id: &str,
    icon: Icon,
    label: String,
    explanation: String,
    enabled: bool,
) -> bool {
    let clicked = draw_sidebar_icon_button_enabled(ui, id, icon, label, explanation, enabled);
    document.record_click(id, clicked)
}

pub(super) fn document_sidebar_icon_toggle_button(
    document: &mut UiDocument,
    ui: &mut Ui,
    id: &str,
    icon: Icon,
    selected: bool,
    label: String,
    explanation: String,
) -> bool {
    let clicked = draw_sidebar_icon_toggle_button(ui, id, icon, selected, label, explanation);
    document.record_click(id, clicked)
}

pub(super) fn document_sidebar_nav_icon_button(
    document: &mut UiDocument,
    ui: &mut Ui,
    id: &str,
    icon: Icon,
    selected: bool,
    label: String,
    explanation: String,
) -> bool {
    let clicked = draw_sidebar_nav_icon_button(ui, id, icon, selected, label, explanation);
    document.record_click(id, clicked)
}

pub(super) fn document_selectable_label(
    document: &mut UiDocument,
    ui: &mut Ui,
    id: &str,
    selected: bool,
    label: &str,
    tooltip: String,
) -> bool {
    let clicked = ui
        .selectable_label(selected, label)
        .on_hover_text(tooltip)
        .clicked();
    document.record_click(id, clicked)
}
