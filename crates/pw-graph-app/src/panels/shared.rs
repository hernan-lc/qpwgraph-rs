use super::components::document_button;
use crate::app::QpwgraphApp;
use eframe::egui;
use eframe::egui::{Color32, RichText, Stroke, Ui};
use pw_graph_backend::MeterPolicy;
use pw_graph_ui::{DialogProps, DialogResponse, MediaFilter, UiDocument};
use pw_graph_ui::{Theme, ThemeToken};

/// Resolves the panel background using the active theme.
pub(super) fn panel_fill(theme: &Theme) -> Color32 {
    theme.color(ThemeToken::Background)
}
/// Resolves the section surface color using the active theme.
pub(super) fn section_fill(theme: &Theme) -> Color32 {
    theme.color(ThemeToken::Surface)
}
/// Resolves the section border color using the active theme.
pub(super) fn section_stroke_color(theme: &Theme) -> Color32 {
    theme.color(ThemeToken::Border)
}
pub(super) const NAV_RAIL_WIDTH: f32 = 76.0;

pub(super) fn apply_panel_text_scale(ui: &mut Ui, scale: f32) {
    let scale = scale.clamp(0.80, 2.0);
    for font_id in ui.style_mut().text_styles.values_mut() {
        font_id.size *= scale;
    }
    ui.spacing_mut().item_spacing = egui::vec2(8.0, 7.0);
    ui.spacing_mut().button_padding = egui::vec2(8.0, 5.0);
}

pub(super) fn panel_section(
    ui: &mut Ui,
    title: String,
    theme: &Theme,
    contents: impl FnOnce(&mut Ui),
) {
    let available_width = ui.available_width();
    let fill = section_fill(theme);
    let border = section_stroke_color(theme);
    let title_color = theme.color(ThemeToken::TextPrimary);
    egui::Frame::group(ui.style())
        .fill(fill)
        .stroke(Stroke::new(1.0_f32, border))
        .inner_margin(9.0)
        .show(ui, |ui| {
            ui.set_min_width((available_width - 18.0).max(0.0));
            ui.label(RichText::new(title).strong().color(title_color));
            ui.add_space(5.0);
            contents(ui);
        });
    ui.add_space(8.0);
}

pub(super) fn media_filter_key(filter: MediaFilter) -> &'static str {
    match filter {
        MediaFilter::All => "filter.all",
        MediaFilter::Audio => "filter.audio",
        MediaFilter::Video => "filter.video",
        MediaFilter::Midi => "filter.midi",
    }
}

pub(super) fn meter_policy_key(policy: MeterPolicy) -> &'static str {
    match policy {
        MeterPolicy::Disabled => "meters.off",
        MeterPolicy::OnDemand => "meters.on_demand",
        MeterPolicy::Always => "meters.always",
    }
}

pub(super) fn show_close_button(
    document: &mut UiDocument,
    ui: &mut Ui,
    id: &str,
    label: String,
) -> bool {
    let mut clicked = false;
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if document_button(document, ui, id, label, true) {
            clicked = true;
        }
    });
    clicked
}

pub(super) fn show_centered_dialog(
    document: &mut UiDocument,
    ctx: &egui::Context,
    id_source: &str,
    title: String,
    width: f32,
    contents: impl FnOnce(&mut Ui, &mut UiDocument),
) -> DialogResponse {
    document.dialog(
        ctx,
        DialogProps::centered(id_source, title, width),
        contents,
    )
}

impl QpwgraphApp {
    /// Runs a retained modal dialog on top of the shared UI document.
    ///
    /// Every modal in the app does the same dance: temporarily take the shared
    /// document so the dialog body can hand it out mutably while still touching
    /// app state, then put it back. This helper owns that take/restore pair and
    /// reports whether the backdrop was clicked so callers can treat a backdrop
    /// click like an explicit close request.
    pub(super) fn run_dialog(
        &mut self,
        ctx: &egui::Context,
        id_source: &str,
        title: String,
        width: f32,
        body: impl FnOnce(&mut Self, &mut Ui, &mut UiDocument),
    ) -> bool {
        let mut document = std::mem::take(&mut self.ui_document);
        let response = show_centered_dialog(
            &mut document,
            ctx,
            id_source,
            title,
            width,
            |ui, document| body(self, ui, document),
        );
        self.ui_document = document;
        response.backdrop_clicked
    }
}

/// A scroll area that always opens back at the top: reopening a dialog (or
/// switching tabs inside it) must not inherit whatever offset a previous
/// scroll session left behind under the same persisted id, or content above
/// the leftover offset silently reads as "missing".
pub(super) fn fresh_scroll_area(
    id_salt: impl std::hash::Hash,
    max_height: f32,
) -> egui::ScrollArea {
    egui::ScrollArea::vertical()
        .id_salt(id_salt)
        .max_height(max_height)
        .auto_shrink([false, true])
}

#[cfg(test)]
mod tests {
    use super::{media_filter_key, meter_policy_key};
    use pw_graph_backend::MeterPolicy;
    use pw_graph_ui::MediaFilter;

    #[test]
    fn panel_filter_and_meter_keys_match_translation_catalog_names() {
        assert_eq!(media_filter_key(MediaFilter::All), "filter.all");
        assert_eq!(media_filter_key(MediaFilter::Midi), "filter.midi");
        assert_eq!(meter_policy_key(MeterPolicy::Disabled), "meters.off");
        assert_eq!(meter_policy_key(MeterPolicy::OnDemand), "meters.on_demand");
        assert_eq!(meter_policy_key(MeterPolicy::Always), "meters.always");
    }
}
