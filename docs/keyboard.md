# The keyboard contract

Everything the application binds to a key is declared once, in
`crates/pw-graph-slint/src/shortcuts.rs`. That table drives the F1 help dialog,
the conflict-detection tests and the routing tests, so the list the user reads
and the listener that answers keys cannot drift apart unnoticed.

## Rules

- **`Ctrl` means Command on macOS.** Slint's `control` modifier is already the
  portable application-shortcut modifier: Ctrl on Windows and Linux, Command on
  macOS. Bindings say `Ctrl+S` and get `Cmd+S` on macOS for free; the help
  dialog prints whichever the platform actually has.
- **Meta/Super is never an application modifier.** `meta` is the Windows key on
  Windows, Super on Linux, and physical Control on macOS. It belongs to the
  window manager. Treating it as a second spelling of Ctrl is what made
  `Win+Arrow` pan the graph while the compositor was snapping the window.
- **Alt is excluded everywhere.** A binding matches its exact modifier set, so
  `Alt+R` and `Ctrl+Alt+S` stay available to the desktop.
- **Text editing wins.** While a text input has focus only global shortcuts
  fire; a typed `r`, `a` or digit belongs to the field.
- **Modals suppress the canvas.** While a dialog is open the graph behind it
  cannot be refreshed, arranged, filtered or edited. The relay panel is
  deliberately not a modal — it is a side panel, so the canvas keymap survives
  it and only its own text fields suppress typing.
- **Only navigation and zoom auto-repeat.** Holding an arrow pans repeatedly;
  holding `T` toggles the thumbnail view exactly once, and holding `Ctrl+S`
  writes the configuration exactly once.
- **Escape operates on the topmost layer**, one layer per press:

  ```text
  QR dialog -> node appearance -> effect setup draft
            -> Effects / Shortcuts / History / Preferences
            -> relay panel -> canvas gesture
  ```

  Dismissing a QR code no longer tears down the relay panel underneath it.

## How routing works

`ui/main.slint` holds one `FocusScope` — `app_focus` — that wraps the entire
window: rail, workspace, dialogs and status bar. This matters because Slint
delivers a key event to the focused element and then bubbles it up the *parent*
chain, never sideways. A scope wrapping only the canvas stops receiving keys the
moment a `LineEdit` is clicked, and its siblings never see them. As the common
ancestor, `app_focus` still receives every key a focused text field or dialog
rejects, which is what makes F1, Escape and Ctrl+F work from inside the
shortcuts dialog's own search box.

Three properties on `MainWindow` express the policy, so no handler re-derives it:

| Property | Meaning |
| --- | --- |
| `modal-open` | A dialog owns the keyboard |
| `text-editing` | A text input has focus |
| `canvas-shortcuts-enabled` | Neither of the above — graph commands may fire |

The node canvas is drawn by a `TouchArea`, which consumes the press before any
ancestor `FocusScope` can react to it. It therefore reports `focus-requested()`
on pointer-down and `main.slint` returns focus to `app_focus`, so clicking back
onto the graph restores the keymap after a text field was used.

Custom buttons (`AppButton`, `RailButton`) are `Rectangle`s rather than Slint's
`Button`, so each carries its own `FocusScope`: they take Tab focus, show a
focus ring, and activate on Enter and Space.

## Coverage

`src/bridge/keyboard.rs` drives the real `MainWindow` through the Slint testing
backend and asserts on the `action` callback — it tests *which element receives
a key*, which is the part that used to break silently. It covers typing in text
fields, Super and Alt combinations, modal suppression, arrow step sizes,
auto-repeat, Ctrl+F inside the shortcuts dialog, focus returning to the canvas,
and Escape precedence.

`src/shortcuts.rs` adds table-level tests: no two bindings share a normalized
combination within a scope, no binding uses a window-manager modifier, and
nothing but navigation and zoom is marked repeatable.
