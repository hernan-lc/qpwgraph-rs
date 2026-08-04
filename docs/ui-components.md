# Reusable UI components

`pw-graph-ui` includes a small retained document layer on top of `egui`. Keep
one `UiDocument` in the panel or application that owns the controls. Widgets
are declared on every frame, but their values are retained by stable IDs.

```rust
use pw_graph_ui::{
    CheckboxProps, EventType, OptionItem, SelectProps, TextInputProps, UiDocument,
};

struct SettingsPanel {
    document: UiDocument,
}

fn show_settings(panel: &mut SettingsPanel, ui: &mut egui::Ui) {
    panel.document.begin_frame();

    panel.document.text_input(
        ui,
        TextInputProps::new("settings.name", "Name")
            .value("default")
            .form("settings"),
    );
    panel.document.checkbox(
        ui,
        CheckboxProps::new("settings.enabled", "Enabled")
            .checked(true)
            .form("settings"),
    );
    panel.document.select(
        ui,
        SelectProps::new("settings.mode", "Mode")
            .selected("easy")
            .options([
                OptionItem::new("easy", "Easy"),
                OptionItem::new("advanced", "Advanced"),
            ])
            .form("settings"),
    );

    panel.document.dispatch_pending_events();

    for (id, value) in panel.document.form_values("settings").iter() {
        println!("{id} = {value}");
    }
}
```

Useful APIs include:

- `get_element_by_id`, `value`, `checked`, `text`, and `number` for DOM-style
  lookup.
- `set_value` for synchronizing a control with application configuration.
- `on`, `on_change`, `on_input`, `on_click`, and `dispatch_pending_events` for
  event listeners.
- `form("id").iter()`, `iter_form_values`, and `form_values` for collecting
  all fields.
- `Style` and the props builders (`.width()`, `.enabled()`, `.tooltip()`,
  `.style()`, and so on) for per-control defaults and overrides.

Available controls are `label`, `button`, `checkbox`, `switch`, `text_input`,
`number_input`, `slider`, `select`, and `radio_group`. `DialogProps` and
`UiDocument::dialog` provide reusable centered or fixed modal chrome, a
translucent click-to-dismiss backdrop, stable dialog IDs, and a callback that
receives both the dialog `Ui` and the document for its child controls.

```rust
use pw_graph_ui::{CheckboxProps, DialogProps};

let response = document.dialog(
    ctx,
    DialogProps::centered("settings-dialog", "Settings", 520.0),
    |ui, document| {
        document.checkbox(
            ui,
            CheckboxProps::new("settings.enabled", "Enabled")
                .checked(true)
                .form("settings"),
        );
    },
);

let should_close = response.backdrop_clicked;
```
