use super::components::document_button;
use eframe::egui;
use eframe::egui::{Color32, RichText, Stroke, Ui};
use pw_graph_backend::MeterPolicy;
use pw_graph_ui::{MediaFilter, UiDocument};

pub(super) const PANEL_FILL: Color32 = Color32::from_rgb(25, 29, 36);
pub(super) const SECTION_FILL: Color32 = Color32::from_rgb(30, 35, 43);
pub(super) const SECTION_STROKE: Color32 = Color32::from_rgb(59, 70, 84);
pub(super) const NAV_RAIL_WIDTH: f32 = 76.0;
pub(super) const FULL_PANEL_MARGIN: f32 = 8.0;

pub(super) fn apply_panel_text_scale(ui: &mut Ui, scale: f32) {
    let scale = scale.clamp(0.80, 2.0);
    for font_id in ui.style_mut().text_styles.values_mut() {
        font_id.size *= scale;
    }
    ui.spacing_mut().item_spacing = egui::vec2(8.0, 7.0);
    ui.spacing_mut().button_padding = egui::vec2(8.0, 5.0);
}

pub(super) fn panel_section(ui: &mut Ui, title: String, contents: impl FnOnce(&mut Ui)) {
    let available_width = ui.available_width();
    egui::Frame::group(ui.style())
        .fill(SECTION_FILL)
        .stroke(Stroke::new(1.0_f32, SECTION_STROKE))
        .inner_margin(9.0)
        .show(ui, |ui| {
            ui.set_min_width((available_width - 18.0).max(0.0));
            ui.label(
                RichText::new(title)
                    .strong()
                    .color(Color32::from_rgb(205, 216, 230)),
            );
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

pub(super) fn show_backdrop(ctx: &egui::Context, id_source: &str) -> bool {
    show_backdrop_rect(ctx, id_source, ctx.screen_rect())
}

pub(super) fn show_backdrop_rect(ctx: &egui::Context, id_source: &str, rect: egui::Rect) -> bool {
    let backdrop_id = egui::Id::new(("modal-backdrop", id_source));
    // Keep the modal window above its backdrop no matter what. Clicking the
    // backdrop (to dismiss the dialog) makes egui call `move_to_top` on that
    // layer, and the reordering persists in memory — so reopening the dialog
    // would otherwise draw the backdrop over the window. Registering the
    // window as a sublayer of the backdrop re-inserts it directly above the
    // backdrop at the end of every frame, overriding any stale order.
    ctx.set_sublayer(
        egui::LayerId::new(egui::Order::Foreground, backdrop_id),
        egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new(("modal-window", id_source)),
        ),
    );
    egui::Area::new(backdrop_id)
        .order(egui::Order::Foreground)
        .fixed_pos(rect.min)
        .show(ctx, |ui| {
            let mut sense = egui::Sense::click();
            // The backdrop must receive pointer clicks, but it is not a real
            // control and must not interrupt Tab traversal inside the modal.
            sense.focusable = false;
            // Keep the backdrop as an invisible hit target. The modal still
            // owns the foreground layer and the backdrop can dismiss it, but
            // opening a dialog no longer dims the graph behind it.
            let (response, _painter) = ui.allocate_painter(rect.size(), sense);
            response
        })
        .inner
        .clicked()
}

pub(super) fn preferences_rect(ctx: &egui::Context) -> egui::Rect {
    let screen = ctx.screen_rect();
    let left = screen.left() + NAV_RAIL_WIDTH + FULL_PANEL_MARGIN;
    let top = screen.top() + FULL_PANEL_MARGIN;
    let width = (screen.width() - NAV_RAIL_WIDTH - FULL_PANEL_MARGIN * 2.0).max(240.0);
    let height = (screen.height() - FULL_PANEL_MARGIN * 2.0).max(260.0);
    egui::Rect::from_min_size(egui::pos2(left, top), egui::vec2(width, height))
}

pub(super) fn full_panel_window(
    id_source: &str,
    title: String,
    rect: egui::Rect,
) -> egui::Window<'static> {
    egui::Window::new(title)
        .id(egui::Id::new(("modal-window", id_source)))
        .collapsible(false)
        .resizable(false)
        .fixed_pos(rect.min)
        .fixed_size(rect.size())
        .order(egui::Order::Foreground)
}

/// Shared chrome for the remaining compact dialog: fixed size, centered,
/// non-collapsible, and always on top.
pub(super) fn modal_window(
    id_source: &str,
    title: String,
    default_width: f32,
) -> egui::Window<'static> {
    egui::Window::new(title)
        .id(egui::Id::new(("modal-window", id_source)))
        .collapsible(false)
        .resizable(false)
        .default_width(default_width)
        .max_width(default_width)
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
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
