//! The keyboard contract, in one place.
//!
//! Every shortcut the application answers to is declared once in [`SHORTCUTS`]
//! and nowhere else. The table drives the help dialog, the conflict-detection
//! tests and the routing tests, so the listener in `ui/main.slint` and the list
//! the user reads cannot drift apart unnoticed.
//!
//! Rules the table encodes (see also `docs/keyboard.md`):
//!
//! * `Ctrl` means Ctrl on Windows/Linux and Command on macOS — that is exactly
//!   what Slint's `control` modifier reports. `Meta`/`Super` (the Windows key,
//!   physical Control on macOS) is never an application modifier.
//! * Text editing wins: while a text input has focus only [`Scope::Global`]
//!   shortcuts fire.
//! * A modal suppresses every [`Scope::Canvas`] shortcut behind it.
//! * Only navigation and zoom may auto-repeat.

/// Which layer of the UI answers a shortcut.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Scope {
    /// Reaches the application from anywhere, including a focused text field
    /// and an open modal.
    Global,
    /// Drives the node graph. Suppressed while a modal is open or a text input
    /// has focus.
    Canvas,
    /// A mouse gesture rather than a key. Listed for the help dialog; it has no
    /// place in key-conflict detection.
    Pointer,
}

/// One entry of the keyboard contract.
///
/// `action`, `scope` and `repeat` document the contract for the routing and
/// conflict tests; only `keys` and `description` are read at runtime, by the
/// help dialog.
#[derive(Debug, Clone, Copy)]
#[cfg_attr(not(test), allow(dead_code))]
pub struct Shortcut {
    /// The action string sent to `handle_action`, or `""` when the binding is
    /// handled entirely inside the `.slint` listener (panning) or by a pointer
    /// gesture.
    pub action: &'static str,
    /// Human-readable combination, also the text shown in the help dialog.
    pub keys: &'static str,
    /// i18n key of the description column.
    pub description: &'static str,
    pub scope: Scope,
    /// Whether holding the key down repeats the action.
    pub repeat: bool,
}

const fn s(
    action: &'static str,
    keys: &'static str,
    description: &'static str,
    scope: Scope,
    repeat: bool,
) -> Shortcut {
    Shortcut {
        action,
        keys,
        description,
        scope,
        repeat,
    }
}

/// The single source of truth. Order is the order shown in the help dialog.
pub const SHORTCUTS: &[Shortcut] = &[
    s("shortcuts", "F1", "shortcuts.help", Scope::Global, false),
    s(
        "escape",
        "Esc",
        "shortcuts.close_cancel",
        Scope::Global,
        false,
    ),
    s("", "Ctrl+F", "shortcuts.search_hint", Scope::Global, false),
    s(
        "delete-selection",
        "Delete / Backspace",
        "shortcuts.delete_link",
        Scope::Canvas,
        false,
    ),
    s("undo", "Ctrl+Z", "shortcuts.undo", Scope::Canvas, false),
    s(
        "redo",
        "Ctrl+Shift+Z",
        "shortcuts.redo",
        Scope::Canvas,
        false,
    ),
    s("redo", "Ctrl+Y", "shortcuts.redo", Scope::Canvas, false),
    s(
        "save-config",
        "Ctrl+S",
        "shortcuts.save_config",
        Scope::Canvas,
        false,
    ),
    s(
        "save-patchbay",
        "Ctrl+Shift+S",
        "shortcuts.save_patchbay",
        Scope::Canvas,
        false,
    ),
    s(
        "load-patchbay",
        "Ctrl+O",
        "shortcuts.load_patchbay",
        Scope::Canvas,
        false,
    ),
    s("refresh", "R", "shortcuts.refresh", Scope::Canvas, false),
    s("arrange", "A", "shortcuts.arrange", Scope::Canvas, false),
    s(
        "toggle-thumbnail",
        "T",
        "shortcuts.thumbnail",
        Scope::Canvas,
        false,
    ),
    s(
        "",
        "Arrow keys",
        "shortcuts.pan_keyboard",
        Scope::Canvas,
        true,
    ),
    s(
        "filter-all",
        "0",
        "shortcuts.filter_all",
        Scope::Canvas,
        false,
    ),
    s(
        "filter-audio",
        "1",
        "shortcuts.filter_audio",
        Scope::Canvas,
        false,
    ),
    s(
        "filter-video",
        "2",
        "shortcuts.filter_video",
        Scope::Canvas,
        false,
    ),
    s(
        "filter-midi",
        "3",
        "shortcuts.filter_midi",
        Scope::Canvas,
        false,
    ),
    s("", "+ / -", "shortcuts.zoom", Scope::Canvas, true),
    s("", "Scroll", "shortcuts.scroll_pan", Scope::Pointer, false),
    s(
        "",
        "Shift+Scroll",
        "shortcuts.scroll_pan_horizontal",
        Scope::Pointer,
        false,
    ),
    s(
        "",
        "Ctrl+Scroll",
        "shortcuts.scroll_zoom",
        Scope::Pointer,
        false,
    ),
];

/// Canonical form of a combination, so `Ctrl+Shift+Z` and `shift+ctrl+z`
/// compare equal. Modifiers are sorted; the key keeps its position last.
///
/// Returns `None` for entries that name a family of keys rather than one
/// combination (`Arrow keys`, `Delete / Backspace`); those cannot collide with
/// a single binding and are excluded from conflict detection.
#[cfg_attr(not(test), allow(dead_code))]
pub fn normalize(keys: &str) -> Option<String> {
    if keys.contains('/') || keys.eq_ignore_ascii_case("arrow keys") {
        return None;
    }
    let mut modifiers: Vec<String> = Vec::new();
    let mut key = String::new();
    for part in keys.split('+') {
        let part = part.trim();
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" | "cmd" | "command" => modifiers.push("ctrl".into()),
            "shift" => modifiers.push("shift".into()),
            "alt" | "option" => modifiers.push("alt".into()),
            "meta" | "super" | "win" => modifiers.push("meta".into()),
            other => key = other.to_string(),
        }
    }
    modifiers.sort();
    modifiers.dedup();
    modifiers.push(key);
    Some(modifiers.join("+"))
}

/// The combination as the user should read it. Slint's `control` modifier is
/// the Command key on macOS, so the help dialog says so instead of printing a
/// modifier that platform does not have.
pub fn display_keys(keys: &str) -> String {
    if cfg!(target_os = "macos") {
        keys.replace("Ctrl", "Cmd")
    } else {
        keys.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Two bindings must never claim the same combination inside one scope.
    /// Across scopes it is legal and intended: `Esc` is global, and a canvas
    /// binding of the same name would still be shadowed by precedence.
    #[test]
    fn no_two_shortcuts_share_a_combination_within_a_scope() {
        let mut seen: HashMap<(Scope, String), &'static str> = HashMap::new();
        for shortcut in SHORTCUTS {
            if shortcut.scope == Scope::Pointer {
                continue;
            }
            let Some(combo) = normalize(shortcut.keys) else {
                continue;
            };
            if let Some(previous) = seen.insert((shortcut.scope, combo.clone()), shortcut.keys) {
                panic!(
                    "{combo:?} is bound twice in {:?}: {previous} and {}",
                    shortcut.scope, shortcut.keys
                );
            }
        }
    }

    /// Meta/Super is the window manager's modifier. If it ever appears in the
    /// table, `Win+Arrow` style collisions are back.
    #[test]
    fn no_shortcut_uses_the_meta_modifier() {
        for shortcut in SHORTCUTS {
            let lowered = shortcut.keys.to_ascii_lowercase();
            assert!(
                !lowered.contains("meta") && !lowered.contains("super") && !lowered.contains("win"),
                "{} uses a window-manager modifier",
                shortcut.keys
            );
        }
    }

    /// Only navigation and zoom may auto-repeat: a repeating toggle or a
    /// repeating "save configuration" is always a bug.
    #[test]
    fn only_navigation_and_zoom_repeat() {
        for shortcut in SHORTCUTS.iter().filter(|entry| entry.repeat) {
            assert!(
                matches!(shortcut.action, "" | "zoom-in" | "zoom-out"),
                "{} must not auto-repeat",
                shortcut.keys
            );
        }
    }

    /// The three modifier models the application has to survive: on Windows
    /// and Linux `Ctrl` is Ctrl and Super belongs to the desktop; on macOS
    /// Slint reports Command as `control` and physical Control as `meta`. The
    /// table is written in the portable spelling, so Cmd and Ctrl must
    /// normalize to the same binding while Super/Win/Meta stays distinct.
    #[test]
    fn the_three_modifier_models_agree_on_one_binding() {
        assert_eq!(normalize("Cmd+S"), normalize("Ctrl+S"));
        assert_eq!(normalize("Command+S"), normalize("Control+S"));
        assert_ne!(normalize("Super+S"), normalize("Ctrl+S"));
        assert_eq!(normalize("Win+S"), normalize("Meta+S"));
        assert_eq!(normalize("Super+S"), normalize("Meta+S"));
    }

    /// The help dialog names the modifier the platform actually has.
    #[test]
    fn the_help_dialog_spells_the_modifier_for_this_platform() {
        let shown = display_keys("Ctrl+Shift+S");
        if cfg!(target_os = "macos") {
            assert_eq!(shown, "Cmd+Shift+S");
        } else {
            assert_eq!(shown, "Ctrl+Shift+S");
        }
    }

    #[test]
    fn normalize_is_order_insensitive() {
        assert_eq!(normalize("Ctrl+Shift+Z"), normalize("shift+ctrl+z"));
        assert_eq!(normalize("Cmd+S"), normalize("Ctrl+S"));
        assert_ne!(normalize("Ctrl+Z"), normalize("Ctrl+Shift+Z"));
        assert_eq!(normalize("Delete / Backspace"), None);
        assert_eq!(normalize("Arrow keys"), None);
    }
}
