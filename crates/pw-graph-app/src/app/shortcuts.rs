//! Global keyboard navigation and graph shortcuts.

use super::QpwgraphApp;
use eframe::egui;
use pw_graph_ui::MediaFilter;

impl QpwgraphApp {
    pub(super) fn text_input_focused(&self, ctx: &egui::Context) -> bool {
        let shortcut_search_id = self
            .show_shortcuts
            .then_some(egui::Id::new("shortcuts-search"));
        ctx.wants_keyboard_input()
            || ctx.memory(|memory| {
                memory
                    .focused()
                    .is_some_and(|focused| Some(focused) == shortcut_search_id)
            })
    }

    pub(super) fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let f1_pressed = ctx.input(|input| input.key_pressed(egui::Key::F1));
        if f1_pressed {
            self.toggle_shortcuts();
            return;
        }

        if self.any_modal_open() {
            if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
                self.close_shortcuts();
                self.show_history = false;
                self.show_preferences = false;
                self.effect_gallery = None;
            }
            return;
        }

        if self.text_input_focused(ctx) {
            return;
        }

        let (command, shift, undo, redo, save, load, arrow_left, arrow_right, arrow_up, arrow_down) =
            ctx.input(|input| {
                (
                    input.modifiers.command,
                    input.modifiers.shift,
                    input.key_pressed(egui::Key::Z),
                    input.key_pressed(egui::Key::Y),
                    input.key_pressed(egui::Key::S),
                    input.key_pressed(egui::Key::O),
                    input.key_pressed(egui::Key::ArrowLeft),
                    input.key_pressed(egui::Key::ArrowRight),
                    input.key_pressed(egui::Key::ArrowUp),
                    input.key_pressed(egui::Key::ArrowDown),
                )
            });
        if command && undo {
            if shift {
                self.redo();
            } else {
                self.undo();
            }
            return;
        }
        if command && redo {
            self.redo();
            return;
        }
        if command && save {
            if shift {
                self.save_patchbay();
            } else {
                self.save_config_now();
            }
            return;
        }
        if command && load {
            self.load_patchbay();
            return;
        }

        let pan_step = if command {
            192.0
        } else if shift {
            96.0
        } else {
            48.0
        };
        let mut pan_delta = egui::Vec2::ZERO;
        if arrow_left {
            pan_delta.x -= pan_step;
        }
        if arrow_right {
            pan_delta.x += pan_step;
        }
        if arrow_up {
            pan_delta.y -= pan_step;
        }
        if arrow_down {
            pan_delta.y += pan_step;
        }
        self.canvas.pan += pan_delta;

        if ctx.input(|input| input.key_pressed(egui::Key::R)) {
            self.refresh_graph();
        }
        if ctx.input(|input| input.key_pressed(egui::Key::A)) {
            self.arrange_nodes();
        }
        if ctx.input(|input| input.key_pressed(egui::Key::T)) {
            self.canvas.thumbnail_mode = !self.canvas.thumbnail_mode;
        }
        if ctx.input(|input| input.key_pressed(egui::Key::Num0)) {
            self.set_media_filter(MediaFilter::All);
        } else if ctx.input(|input| input.key_pressed(egui::Key::Num1)) {
            self.set_media_filter(MediaFilter::Audio);
        } else if ctx.input(|input| input.key_pressed(egui::Key::Num2)) {
            self.set_media_filter(MediaFilter::Video);
        } else if ctx.input(|input| input.key_pressed(egui::Key::Num3)) {
            self.set_media_filter(MediaFilter::Midi);
        }
        if ctx.input(|input| input.key_pressed(egui::Key::Plus)) {
            self.canvas.zoom = (self.canvas.zoom * 1.1).clamp(0.35, 2.5);
        }
        if ctx.input(|input| input.key_pressed(egui::Key::Minus)) {
            self.canvas.zoom = (self.canvas.zoom / 1.1).clamp(0.35, 2.5);
        }
    }
}
