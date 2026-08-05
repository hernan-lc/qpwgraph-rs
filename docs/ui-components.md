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

## Structure components

Value controls answer "what is this set to". The structure components answer
"what shape is this panel": a grouping surface, a collapsible section, a tab
strip, a step header. They keep panels declarative — a panel says *disclosure*
rather than assembling a chevron, a click target, an indent, and a retained
`bool` at each call site — and they keep spacing, accent colours, and icon
weights identical everywhere they appear.

| Component | Props | Retained value |
| --- | --- | --- |
| `card` | `CardProps` | none (a surface) |
| `disclosure` | `DisclosureProps` | `Bool` — open state |
| `tabs` | `TabsProps` + `TabItem` | `String` — selected tab |
| `steps` | `StepsProps` + `StepItem` | `Number` — current step |
| `badge` | `BadgeProps` | `String` — badge text |
| `meter` | `MeterProps` | `Number` — current level |
| `icon_button` | `IconButtonProps` | `Bool` — momentary click |
| `activity` | — | spinner plus status line |

`card`, `disclosure`, and `dialog` hand the document back to their body
closure, so containers nest without the caller juggling a second borrow:

```rust
use pw_graph_ui::{CardProps, DisclosureProps, TabItem, TabsProps};

let tab = document.tabs(
    ui,
    TabsProps::new(
        "relay.panel.tabs",
        [
            TabItem::new("connections", "Connections").badge_count(session_count),
            TabItem::new("host", "Host"),
        ],
    )
    .selected("connections"),
);

document.disclosure(
    ui,
    DisclosureProps::new("relay.advanced", "Advanced settings").summary("Opus · 10 ms"),
    |ui, document| {
        document.card(ui, CardProps::new("relay.advanced.card"), |ui, document| {
            // nested controls
        });
    },
);
```

`TabItem::badge_count` omits the badge entirely at zero, so a count never
renders as an empty pill.

### Tabs or steps?

Both present a set of parts, but they claim different things and are not
interchangeable:

- **Tabs** say every destination is available right now, in any order. Use
  them for peer views of the same subject.
- **Steps** say the task has an order and later parts are *not yet reachable*.
  Completed steps show a check, the current one is filled, and the rest stay
  dimmed. `steps` is not clickable unless `navigable` is set, and even then
  only completed or current steps respond.

The relay panel uses both: a tab strip for Connections/Host, which are equally
available, and a stepper inside the Host tab, where sharing an address is
meaningless until the host is actually listening.

## Icons

Components never draw glyph characters such as `▾` or `✓` for an affordance.
Text glyphs depend on the loaded font, render at a different weight from real
icons beside them, and cannot be tinted separately from their label. Every
affordance takes an SVG through `IconSource`:

- `IconSource::Builtin(Icon::…)` uses artwork bundled with `pw-graph-ui`
  (`ChevronDown`, `ChevronRight`, `Check`, `Info`, `More`, `Volume`,
  `VolumeMuted`, `ArrowUp`, `ArrowDown`).
- `IconSource::Custom(ImageSource)` uses application artwork. In this
  workspace `crate::icons::Icon::source()` converts an app icon into one, so
  shared components render the application's own set.

`icon_image(&icon, size, color)` is the shared helper: it sizes exactly rather
than fitting, so icons in a row share a baseline whatever each drawing's
aspect ratio is, and tints white artwork to any colour.

Because `ImageSource` is only `Clone`, props that carry an icon
(`IconButtonProps`, `TabItem`, `TabsProps`) derive `Clone` alone rather than
the `Debug + PartialEq` the value-carrying props derive.

## Custom-painted controls

Widgets that paint themselves — canvas node chrome, effect cards — still
belong in the document so form queries and click listeners see them. Call
`record_custom_click(&mut document, id, clicked)` after painting; the control
keeps its bespoke appearance and gains a stable ID, a retained value, and
click events like any built-in.
