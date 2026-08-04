use super::*;
use egui;
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
fn slider_respects_explicit_width() {
    let mut document = UiDocument::new();
    let ctx = egui::Context::default();
    let mut slider_width = 0.0;
    ctx.begin_pass(egui::RawInput::default());
    egui::CentralPanel::default().show(&ctx, |ui| {
        slider_width = document
            .slider(
                ui,
                SliderProps::new("sized-slider", "")
                    .size(180.0, 26.0)
                    .show_value(false),
            )
            .rect
            .width();
    });
    let _ = ctx.end_pass();

    assert!((slider_width - 180.0).abs() < f32::EPSILON);
}

#[test]
fn number_input_keeps_the_embedded_stepper_inside_its_width() {
    let mut document = UiDocument::new();
    let ctx = egui::Context::default();
    let mut number_width = 0.0;
    ctx.begin_pass(egui::RawInput::default());
    egui::CentralPanel::default().show(&ctx, |ui| {
        number_width = document
            .number_input(
                ui,
                NumberInputProps::new("sized-number", "")
                    .value(0.8)
                    .range(0.8, 2.0)
                    .step(0.05)
                    .size(118.0, 24.0),
            )
            .rect
            .width();
    });
    let _ = ctx.end_pass();

    assert!((number_width - 118.0).abs() < f32::EPSILON);
    assert_eq!(document.number("sized-number"), Some(0.8));
}

#[test]
fn dialog_registers_and_renders_with_a_stable_id() {
    let mut document = UiDocument::new();
    let ctx = egui::Context::default();
    ctx.begin_pass(egui::RawInput::default());
    let response = document.dialog(
        &ctx,
        DialogProps::centered("settings-dialog", "Settings", 420.0),
        |ui, _document| {
            ui.label("Dialog content");
        },
    );
    let _ = ctx.end_pass();

    assert!(response.shown);
    assert_eq!(
        document.get_element_by_id("settings-dialog").unwrap().kind,
        ElementKind::Dialog
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
