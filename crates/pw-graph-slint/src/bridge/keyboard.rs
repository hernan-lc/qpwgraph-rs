//! Keyboard-routing tests.
//!
//! These drive the real `MainWindow` through the Slint testing backend and
//! assert on the `action` callback, so they cover the part that used to break
//! silently: *which* element receives a key, not what the handler then does.
//! The contract they lock down is written out in `crate::shortcuts` and
//! `docs/keyboard.md`.

use super::tests::demo_application;
use super::{actions::handle_action, MainWindow};
use slint::platform::{Key, PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, LogicalSize, SharedString};
use std::cell::RefCell;
use std::rc::Rc;

/// The graph starts at the right edge of the icon rail; the workspace search
/// field sits 16px further in and is 30px tall.
const SEARCH_FIELD: LogicalPosition = LogicalPosition { x: 150.0, y: 27.0 };
/// A point on the empty canvas, well clear of the search field and the rail.
const CANVAS_POINT: LogicalPosition = LogicalPosition { x: 700.0, y: 500.0 };

/// `init_no_event_loop` panics when the thread already has a platform, and a
/// single test may need more than one window.
fn init_backend() {
    thread_local! {
        static READY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    READY.with(|ready| {
        if !ready.replace(true) {
            i_slint_backend_testing::init_no_event_loop();
        }
    });
}

struct KeyHarness {
    window: MainWindow,
    actions: Rc<RefCell<Vec<String>>>,
}

impl KeyHarness {
    fn new() -> Self {
        init_backend();
        let window = MainWindow::new().unwrap();
        window.window().set_size(LogicalSize::new(1400.0, 900.0));
        let actions: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let sink = actions.clone();
        window.on_action(move |action| sink.borrow_mut().push(action.to_string()));
        let harness = Self { window, actions };
        harness.dispatch(WindowEvent::WindowActiveChanged(true));
        harness
    }

    fn dispatch(&self, event: WindowEvent) {
        self.window.window().dispatch_event(event);
        slint::platform::update_timers_and_animations();
    }

    fn click(&self, position: LogicalPosition) {
        self.dispatch(WindowEvent::PointerMoved { position });
        self.dispatch(WindowEvent::PointerPressed {
            position,
            button: PointerEventButton::Left,
        });
        self.dispatch(WindowEvent::PointerReleased {
            position,
            button: PointerEventButton::Left,
        });
    }

    fn down(&self, text: impl Into<SharedString>) {
        self.dispatch(WindowEvent::KeyPressed { text: text.into() });
    }

    fn up(&self, text: impl Into<SharedString>) {
        self.dispatch(WindowEvent::KeyReleased { text: text.into() });
    }

    /// A single press/release of `text` with no modifier held.
    fn press(&self, text: impl Into<SharedString>) {
        let text = text.into();
        self.down(text.clone());
        self.up(text);
    }

    /// A press/release of `text` while `modifiers` are held down, entered the
    /// way a real keyboard enters them: the modifier keys go down first.
    fn press_with(&self, modifiers: &[Key], text: impl Into<SharedString>) {
        for modifier in modifiers {
            self.down(*modifier);
        }
        self.press(text);
        for modifier in modifiers.iter().rev() {
            self.up(*modifier);
        }
    }

    /// A key held down long enough for the platform to auto-repeat it
    /// `extra` times after the initial press.
    fn hold(&self, text: impl Into<SharedString>, extra: usize) {
        let text = text.into();
        self.down(text.clone());
        for _ in 0..extra {
            self.dispatch(WindowEvent::KeyPressRepeated { text: text.clone() });
        }
        self.up(text);
    }

    fn take_actions(&self) -> Vec<String> {
        std::mem::take(&mut *self.actions.borrow_mut())
    }

    fn count(&self, action: &str) -> usize {
        self.actions
            .borrow()
            .iter()
            .filter(|entry| *entry == action)
            .count()
    }
}

/// Baseline: with nothing focused but the workspace, the canvas keymap works.
/// Every "must not fire" test below is only meaningful next to this one.
#[test]
fn canvas_shortcuts_fire_when_the_workspace_owns_the_keyboard() {
    let harness = KeyHarness::new();
    harness.press("r");
    harness.press("a");
    harness.press("t");
    harness.press("1");
    assert_eq!(
        harness.take_actions(),
        ["refresh", "arrange", "toggle-thumbnail", "filter-audio"]
    );
}

/// The regression that started all this: a click into any text field used to
/// take the keymap away for good, because the listener was a *sibling* of the
/// field rather than an ancestor.
#[test]
fn globals_still_reach_the_app_from_inside_a_text_field() {
    let harness = KeyHarness::new();
    harness.click(SEARCH_FIELD);
    assert!(
        harness.window.get_text_editing(),
        "clicking the search field must focus its text input"
    );

    harness.press(Key::F1);
    harness.press(Key::Escape);
    assert_eq!(harness.take_actions(), ["shortcuts", "escape"]);
}

/// Graph letters and digits belong to the field being typed into.
#[test]
fn typing_in_a_text_field_never_reaches_the_graph() {
    let harness = KeyHarness::new();
    harness.click(SEARCH_FIELD);
    for text in ["r", "a", "t", "0", "1", "2", "3", "+", "-"] {
        harness.press(text);
    }
    harness.press(Key::Delete);
    harness.press(Key::Backspace);
    assert!(
        harness.take_actions().is_empty(),
        "typing must not command the graph"
    );
}

/// Ctrl+Z is undo on the canvas and text editing inside a field, which the
/// text-editing guard is what makes possible.
#[test]
fn ctrl_z_undoes_the_graph_only_outside_a_text_field() {
    let harness = KeyHarness::new();
    harness.press_with(&[Key::Control], "z");
    assert_eq!(harness.take_actions(), ["undo"]);

    harness.click(SEARCH_FIELD);
    harness.press_with(&[Key::Control], "z");
    assert!(harness.take_actions().is_empty());
}

/// `meta` is the window manager's modifier — the Windows/Super key, and
/// physical Control on macOS. Treating it as Ctrl made Win+Arrow pan the graph
/// while the compositor was snapping the window.
#[test]
fn super_combinations_do_not_invoke_application_commands() {
    let harness = KeyHarness::new();
    let pan_x = harness.window.get_pan_x();
    let pan_y = harness.window.get_pan_y();

    for text in ["s", "o", "z", "y", "f"] {
        harness.press_with(&[Key::Meta], text);
    }
    for arrow in [
        Key::LeftArrow,
        Key::RightArrow,
        Key::UpArrow,
        Key::DownArrow,
    ] {
        harness.press_with(&[Key::Meta], arrow);
    }

    assert!(harness.take_actions().is_empty(), "Super is not Ctrl");
    assert_eq!(harness.window.get_pan_x(), pan_x);
    assert_eq!(harness.window.get_pan_y(), pan_y);
}

/// Alt used to slip through because the conditions only tested `control`.
#[test]
fn alt_modified_keys_are_not_graph_commands() {
    let harness = KeyHarness::new();
    for text in ["r", "a", "t", "1"] {
        harness.press_with(&[Key::Alt], text);
    }
    harness.press_with(&[Key::Control, Key::Alt], "s");
    harness.press_with(&[Key::Alt], Key::LeftArrow);
    assert!(harness.take_actions().is_empty());
}

/// Shift and Ctrl select the pan step; the more specific combination must not
/// be shadowed by the plainer one.
#[test]
fn arrow_pan_steps_scale_with_the_modifier() {
    let harness = KeyHarness::new();
    let origin = harness.window.get_pan_x();

    harness.press(Key::RightArrow);
    assert_eq!(harness.window.get_pan_x() - origin, 48.0);

    harness.press_with(&[Key::Shift], Key::RightArrow);
    assert_eq!(harness.window.get_pan_x() - origin, 48.0 + 96.0);

    harness.press_with(&[Key::Control], Key::RightArrow);
    assert_eq!(harness.window.get_pan_x() - origin, 48.0 + 96.0 + 192.0);
}

/// A modal owns the keyboard. Canvas commands must not reach the graph drawn
/// behind it, but the globals that dismiss it still have to work.
#[test]
fn an_open_modal_suppresses_canvas_shortcuts_but_not_the_globals() {
    let harness = KeyHarness::new();
    harness.window.set_show_preferences(true);
    assert!(harness.window.get_modal_open());
    assert!(!harness.window.get_canvas_shortcuts_enabled());

    for text in ["r", "a", "t", "0", "1", "2", "3"] {
        harness.press(text);
    }
    harness.press(Key::Delete);
    harness.press(Key::Backspace);
    harness.press(Key::RightArrow);
    harness.press_with(&[Key::Control], "z");
    harness.press_with(&[Key::Control], "s");
    assert!(
        harness.take_actions().is_empty(),
        "the graph behind a modal must stay untouched"
    );

    harness.press(Key::Escape);
    harness.press(Key::F1);
    assert_eq!(harness.take_actions(), ["escape", "shortcuts"]);
}

/// The relay panel is a side panel, not a modal: the canvas keymap survives it.
#[test]
fn the_relay_side_panel_leaves_canvas_shortcuts_alone() {
    let harness = KeyHarness::new();
    harness.window.set_show_relay(true);
    assert!(!harness.window.get_modal_open());
    harness.press("r");
    assert_eq!(harness.take_actions(), ["refresh"]);
}

/// Holding a toggle used to fire it once per auto-repeat, which walked the undo
/// history or rewrote the configuration dozens of times.
#[test]
fn held_toggles_and_file_commands_do_not_auto_repeat() {
    let harness = KeyHarness::new();
    harness.hold("t", 9);
    harness.hold("r", 9);
    harness.hold(Key::F1, 9);

    self::assert_single(&harness, "toggle-thumbnail");
    self::assert_single(&harness, "refresh");
    self::assert_single(&harness, "shortcuts");

    let harness = KeyHarness::new();
    harness.down(Key::Control);
    harness.hold("s", 9);
    harness.up(Key::Control);
    self::assert_single(&harness, "save-config");
}

/// Panning and zooming are the two commands where repeating is the point.
#[test]
fn held_navigation_keys_repeat() {
    let harness = KeyHarness::new();
    let origin = harness.window.get_pan_x();
    harness.hold(Key::RightArrow, 3);
    assert_eq!(harness.window.get_pan_x() - origin, 4.0 * 48.0);

    harness.hold("+", 3);
    assert_eq!(harness.count("zoom-in"), 4);
}

/// Ctrl+F is documented as "focus the shortcut search", and the dialog opens
/// with that very field focused — so the binding has to survive its own effect.
#[test]
fn ctrl_f_reaches_the_shortcut_search_from_within_the_dialog() {
    let harness = KeyHarness::new();
    harness.window.set_show_shortcuts(true);
    slint::platform::update_timers_and_animations();

    harness.click(SEARCH_FIELD);
    harness.press_with(&[Key::Control], "f");
    assert!(
        harness.window.get_text_editing(),
        "Ctrl+F must leave a text input focused"
    );
    assert!(harness.take_actions().is_empty());
}

/// Clicking back onto the graph has to return the keymap to the canvas.
#[test]
fn clicking_the_canvas_returns_the_keymap_to_the_graph() {
    let harness = KeyHarness::new();
    harness.click(SEARCH_FIELD);
    assert!(harness.window.get_text_editing());

    harness.click(CANVAS_POINT);
    assert!(!harness.window.get_text_editing());
    harness.press("r");
    assert_eq!(harness.take_actions(), ["refresh"]);
}

fn assert_single(harness: &KeyHarness, action: &str) {
    assert_eq!(
        harness.count(action),
        1,
        "{action} fired {} times while the key was held",
        harness.count(action)
    );
}

// --- Escape precedence ---------------------------------------------------
// Escape cancels the topmost active layer only. It used to close every overlay
// at once, so dismissing a QR code also tore down the relay panel under it.

#[test]
fn escape_closes_the_qr_dialog_and_leaves_the_relay_panel_open() {
    init_backend();
    let window = MainWindow::new().unwrap();
    let mut application = demo_application();
    window.set_show_relay(true);
    window.set_show_qr(true);

    handle_action(&window, &mut application, "escape");
    assert!(!window.get_show_qr());
    assert!(window.get_show_relay(), "the panel underneath must survive");

    handle_action(&window, &mut application, "escape");
    assert!(!window.get_show_relay());
}

#[test]
fn escape_abandons_an_effect_draft_before_closing_the_effects_dialog() {
    init_backend();
    let window = MainWindow::new().unwrap();
    let mut application = demo_application();
    window.set_show_effects(true);
    window.set_effect_configuring(true);

    handle_action(&window, &mut application, "escape");
    assert!(!window.get_effect_configuring());
    assert!(
        window.get_show_effects(),
        "the dialog stays for a second Esc"
    );

    handle_action(&window, &mut application, "escape");
    assert!(!window.get_show_effects());
}

#[test]
fn escape_closes_one_layer_at_a_time() {
    init_backend();
    let window = MainWindow::new().unwrap();
    let mut application = demo_application();
    window.set_show_preferences(true);
    window.set_show_node_editor(true);

    handle_action(&window, &mut application, "escape");
    assert!(!window.get_show_node_editor());
    assert!(window.get_show_preferences());

    handle_action(&window, &mut application, "escape");
    assert!(!window.get_show_preferences());
}
