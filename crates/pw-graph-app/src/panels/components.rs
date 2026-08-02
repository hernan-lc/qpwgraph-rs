//! Reusable controls for modal forms and setup screens.

use eframe::egui::{self, RichText, Ui};

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

pub(super) fn modal_checkbox(
    ui: &mut Ui,
    id: impl std::hash::Hash,
    value: &mut bool,
    label: String,
) -> bool {
    ui.push_id(id, |ui| ui.checkbox(value, label).changed())
        .inner
}

pub(super) fn modal_slider(
    ui: &mut Ui,
    id: impl std::hash::Hash,
    value: &mut f32,
    minimum: f32,
    maximum: f32,
    label: String,
    unit: &str,
) -> bool {
    ui.push_id(id, |ui| {
        ui.add(egui::Slider::new(value, minimum..=maximum).text(format!("{label} ({unit})")))
            .changed()
    })
    .inner
}

pub(super) fn modal_combo<T, I>(
    ui: &mut Ui,
    id: impl std::hash::Hash,
    label: String,
    selected_text: String,
    selected: &mut T,
    choices: I,
) where
    T: Clone + PartialEq,
    I: IntoIterator<Item = (T, String)>,
{
    ui.push_id(id, |ui| {
        ui.horizontal(|ui| {
            ui.label(label);
            egui::ComboBox::from_id_salt(ui.id().with("value"))
                .selected_text(selected_text)
                .width((ui.available_width() - 4.0).max(120.0))
                .show_ui(ui, |ui| {
                    for (value, text) in choices {
                        ui.selectable_value(selected, value, text);
                    }
                });
        });
    });
}
