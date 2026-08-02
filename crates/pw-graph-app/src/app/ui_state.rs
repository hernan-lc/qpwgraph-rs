use super::QpwgraphApp;
use eframe::egui;
use pw_graph_ui::MediaFilter;

impl QpwgraphApp {
    pub(crate) fn any_modal_open(&self) -> bool {
        self.show_shortcuts
            || self.show_history
            || self.show_preferences
            || self.effect_gallery.is_some()
    }

    pub(crate) fn toggle_shortcuts(&mut self) {
        if self.show_shortcuts {
            self.show_shortcuts = false;
            self.shortcut_focus_search = false;
            return;
        }
        self.show_shortcuts = true;
        self.show_preferences = false;
        self.show_history = false;
        self.effect_gallery = None;
        self.shortcut_search.clear();
        self.shortcut_focus_search = true;
        self.shortcut_scroll_epoch = self.shortcut_scroll_epoch.wrapping_add(1);
    }

    pub(crate) fn close_shortcuts(&mut self) {
        self.show_shortcuts = false;
        self.shortcut_focus_search = false;
    }

    pub(crate) fn toggle_history(&mut self) {
        self.show_history = !self.show_history;
        if self.show_history {
            self.show_shortcuts = false;
            self.show_preferences = false;
            self.effect_gallery = None;
        }
    }

    pub(crate) fn set_media_filter(&mut self, filter: MediaFilter) {
        self.canvas.media_filter = filter;
        self.config.media_filter = filter.as_str().into();
    }

    pub(super) fn apply_ui_text_scale(&self, ctx: &egui::Context) {
        let scale = self.config.ui_text_scale.clamp(0.80, 2.0);
        let default_text_styles = egui::Style::default().text_styles;
        ctx.style_mut(|style| {
            for (text_style, font_id) in &default_text_styles {
                let mut scaled_font = font_id.clone();
                scaled_font.size *= scale;
                style.text_styles.insert(text_style.clone(), scaled_font);
            }
        });
    }
}
