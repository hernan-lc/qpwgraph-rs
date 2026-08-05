use super::super::super::theme::{Theme, ThemeToken};
use super::super::super::{CommonProps, ElementId, Style};
use egui::{vec2, Color32, Frame, Key, Margin, Response, Sense, Stroke, Ui, Vec2};

pub(super) fn normalize_range(minimum: f64, maximum: f64) -> (f64, f64) {
    let minimum = if minimum.is_finite() { minimum } else { 0.0 };
    let maximum = if maximum.is_finite() { maximum } else { 1.0 };
    if minimum <= maximum {
        (minimum, maximum)
    } else {
        (maximum, minimum)
    }
}

pub(super) fn normalize_optional_range(minimum: Option<f64>, maximum: Option<f64>) -> (f64, f64) {
    let minimum = minimum
        .filter(|value| value.is_finite())
        .unwrap_or(f64::NEG_INFINITY);
    let maximum = maximum
        .filter(|value| value.is_finite())
        .unwrap_or(f64::INFINITY);
    if minimum <= maximum {
        (minimum, maximum)
    } else {
        (maximum, minimum)
    }
}

pub(super) fn add_sized<W: egui::Widget>(ui: &mut Ui, style: &Style, widget: W) -> Response {
    if style.width.is_none() && style.height.is_none() {
        return ui.add(widget);
    }
    let width = style.width.unwrap_or_else(|| ui.available_width().max(0.0));
    let height = style.height.unwrap_or_else(|| ui.spacing().interact_size.y);
    ui.add_sized(vec2(width, height), widget)
}

pub(super) fn labelled(
    ui: &mut Ui,
    label: Option<&str>,
    text_color: Color32,
    render: impl FnOnce(&mut Ui) -> Response,
) -> Response {
    if let Some(label) = label.filter(|label| !label.is_empty()) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(label).color(text_color));
            render(ui)
        })
        .inner
    } else {
        render(ui)
    }
}

pub(super) fn with_common(
    ui: &mut Ui,
    common: &CommonProps,
    theme: &Theme,
    render: impl FnOnce(&mut Ui) -> Response,
) -> Response {
    if !common.visible {
        return ui.allocate_exact_size(Vec2::ZERO, Sense::hover()).1;
    }

    let style = common.style.clone();
    let draw_style = style.clone();
    let enabled = common.enabled;
    // Resolve text color: explicit override, then theme token, then none.
    let resolved_text_color = draw_style.resolve_text_color(theme);
    let draw = move |ui: &mut Ui| {
        if let Some(width) = draw_style.width {
            ui.set_width(width);
        }
        if let Some(height) = draw_style.height {
            ui.set_height(height);
        }
        if let Some(tc) = resolved_text_color {
            ui.visuals_mut().override_text_color = Some(tc);
        }
        if enabled {
            render(ui)
        } else {
            ui.add_enabled_ui(false, render).inner
        }
    };

    let mut response = if style.has_frame() {
        let mut frame = Frame::none();
        // Resolve fill: explicit color wins, then theme token.
        if let Some(fill) = style.resolve_fill(theme) {
            frame = frame.fill(fill);
        }
        if let Some(stroke) = style.stroke {
            frame = frame.stroke(stroke);
        }
        if let Some(rounding) = style.rounding {
            frame = frame.rounding(rounding);
        }
        if let Some(inner_margin) = style.inner_margin {
            frame = frame.inner_margin(Margin::same(inner_margin));
        }
        frame.show(ui, draw).inner
    } else {
        ui.scope(draw).inner
    };
    if let Some(tooltip) = &common.tooltip {
        response = response.on_hover_text(tooltip.clone());
    }
    response
}

pub(super) fn switch_widget(
    ui: &mut Ui,
    id: &ElementId,
    checked: &mut bool,
    label: Option<&str>,
    style: &Style,
    theme: &Theme,
) -> Response {
    let track_size = vec2(style.width.unwrap_or(36.0), style.height.unwrap_or(20.0))
        .max(vec2(0.0, ui.spacing().interact_size.y));
    let on_fill = style
        .resolve_fill(theme)
        .unwrap_or_else(|| theme.color(ThemeToken::Accent));
    let off_fill = theme.color(ThemeToken::SurfaceHover);
    ui.horizontal(|ui| {
        let widget_id = ui.make_persistent_id(("ui-document-switch", id));
        let (rect, _) = ui.allocate_exact_size(track_size, Sense::hover());
        let mut response = ui.interact(rect, widget_id, Sense::click());
        let keyboard_toggled = response.has_focus()
            && ui.input(|input| input.key_pressed(Key::Enter) || input.key_pressed(Key::Space));
        if response.clicked() || keyboard_toggled {
            *checked = !*checked;
            response.mark_changed();
            response.request_focus();
        }
        let visuals = if response.enabled() {
            ui.style().interact(&response)
        } else {
            ui.style().noninteractive()
        };
        let fill = if response.enabled() {
            if *checked {
                on_fill
            } else {
                off_fill
            }
        } else {
            visuals.bg_fill
        };
        let border = style.stroke.unwrap_or_else(|| {
            if response.enabled() && response.has_focus() {
                Stroke::new(2.0_f32, visuals.fg_stroke.color)
            } else {
                visuals.bg_stroke
            }
        });
        ui.painter().rect(rect, track_size.y / 2.0, fill, border);
        let radius = (track_size.y - 6.0).max(4.0) / 2.0;
        let knob_x = if *checked {
            rect.right() - 3.0 - radius
        } else {
            rect.left() + 3.0 + radius
        };
        ui.painter()
            .circle_filled(egui::pos2(knob_x, rect.center().y), radius, Color32::WHITE);
        if let Some(label) = label.filter(|label| !label.is_empty()) {
            ui.label(label);
        }
        response
    })
    .inner
}

#[cfg(test)]
mod tests {
    use super::{normalize_optional_range, normalize_range};

    #[test]
    fn normalize_range_orders_bounds_and_uses_finite_defaults() {
        assert_eq!(normalize_range(4.0, -2.0), (-2.0, 4.0));
        assert_eq!(normalize_range(f64::NAN, f64::INFINITY), (0.0, 1.0));
    }

    #[test]
    fn normalize_optional_range_keeps_open_ended_bounds() {
        assert_eq!(
            normalize_optional_range(None, Some(2.0)),
            (f64::NEG_INFINITY, 2.0)
        );
        assert_eq!(normalize_optional_range(Some(4.0), Some(-2.0)), (-2.0, 4.0));
    }
}
