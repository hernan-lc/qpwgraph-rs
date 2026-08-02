use egui::{pos2, vec2, Color32, Painter, Pos2, Rect, Sense, Shape, Stroke, Ui, Vec2};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Icon {
    Activate,
    AutoDisconnect,
    Brand,
    Connect,
    Delete,
    Diagnostics,
    Exclusive,
    Graph,
    Language,
    Load,
    Patchbay,
    Pin,
    Refresh,
    Repel,
    Redo,
    Save,
    Settings,
    Snapshot,
    Statusbar,
    Thumbnail,
    Timer,
    Toolbar,
    Undo,
}

const ICON_BUTTON_SIZE: Vec2 = vec2(34.0, 30.0);
const ICON_STROKE_WIDTH: f32 = 1.6;

pub(crate) fn icon_button(
    ui: &mut Ui,
    id: &str,
    icon: Icon,
    label: String,
    explanation: String,
) -> bool {
    icon_button_enabled(ui, id, icon, label, explanation, true)
}

pub(crate) fn icon_button_enabled(
    ui: &mut Ui,
    id: &str,
    icon: Icon,
    label: String,
    explanation: String,
    enabled: bool,
) -> bool {
    ui.push_id(("icon-button", id), |ui| {
        let sense = if enabled {
            Sense::click()
        } else {
            Sense::hover()
        };
        let (rect, response) = ui.allocate_exact_size(ICON_BUTTON_SIZE, sense);
        let response = response.on_hover_text(format!("{label}\n{explanation}"));
        let visuals = if enabled {
            ui.style().interact(&response)
        } else {
            ui.style().noninteractive()
        };
        ui.painter()
            .rect(rect, 4.0, visuals.bg_fill, visuals.bg_stroke);
        paint_icon(
            ui.painter(),
            rect.shrink(7.0),
            icon,
            visuals.fg_stroke.color,
        );
        response.clicked()
    })
    .inner
}

pub(crate) fn nav_icon_button(
    ui: &mut Ui,
    id: &str,
    icon: Icon,
    selected: bool,
    label: String,
    explanation: String,
) -> bool {
    ui.push_id(("navigation-icon", id), |ui| {
        let (rect, response) = ui.allocate_exact_size(vec2(42.0, 38.0), Sense::click());
        let response = response.on_hover_text(format!("{label}\n{explanation}"));
        let visuals = ui.style().interact_selectable(&response, selected);
        ui.painter()
            .rect(rect, 5.0, visuals.bg_fill, visuals.bg_stroke);
        paint_icon(
            ui.painter(),
            rect.shrink(10.0),
            icon,
            visuals.fg_stroke.color,
        );
        response.clicked()
    })
    .inner
}

pub(crate) fn icon_checkbox(
    ui: &mut Ui,
    id: &str,
    value: &mut bool,
    icon: Icon,
    label: String,
    explanation: String,
) -> bool {
    ui.push_id(("icon-checkbox", id), |ui| {
        ui.horizontal(|ui| {
            let (rect, response) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
            let response = response.on_hover_text(explanation.clone());
            paint_icon(ui.painter(), rect.shrink(3.0), icon, Color32::LIGHT_BLUE);
            let checkbox_response = ui.checkbox(value, label);
            let changed = checkbox_response.changed();
            checkbox_response.on_hover_text(explanation);
            let _ = response;
            changed
        })
        .inner
    })
    .inner
}

pub(crate) fn icon_heading(ui: &mut Ui, icon: Icon, title: String) {
    ui.horizontal(|ui| {
        let (rect, response) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::hover());
        let response = response.on_hover_text(title.clone());
        paint_icon(
            ui.painter(),
            rect.shrink(3.0),
            icon,
            ui.visuals().text_color(),
        );
        let _ = response;
        ui.heading(title);
    });
}

pub(crate) fn icon_label(ui: &mut Ui, icon: Icon, tooltip: String) {
    let (rect, response) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
    let response = response.on_hover_text(tooltip);
    paint_icon(
        ui.painter(),
        rect.shrink(2.0),
        icon,
        ui.visuals().text_color(),
    );
    let _ = response;
}

pub(crate) fn paint_icon(painter: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let stroke = Stroke::new(ICON_STROKE_WIDTH, color);
    let center = rect.center();
    let radius = rect.width().min(rect.height()) * 0.34;

    match icon {
        Icon::Activate => {
            painter.add(Shape::convex_polygon(
                vec![
                    pos2(
                        rect.left() + rect.width() * 0.30,
                        rect.top() + rect.height() * 0.18,
                    ),
                    pos2(rect.right() - rect.width() * 0.22, center.y),
                    pos2(
                        rect.left() + rect.width() * 0.30,
                        rect.bottom() - rect.height() * 0.18,
                    ),
                ],
                color,
                Stroke::NONE,
            ));
        }
        Icon::AutoDisconnect => {
            painter.line_segment(
                [
                    pos2(rect.left() + 1.0, center.y),
                    pos2(center.x - 2.0, center.y),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    pos2(center.x + 2.0, center.y),
                    pos2(rect.right() - 1.0, center.y),
                ],
                stroke,
            );
            painter.circle_stroke(pos2(center.x - 3.0, center.y), 3.0, stroke);
            painter.circle_stroke(pos2(center.x + 3.0, center.y), 3.0, stroke);
            painter.line_segment(
                [
                    pos2(center.x - 1.5, center.y - 4.5),
                    pos2(center.x + 1.5, center.y + 4.5),
                ],
                stroke,
            );
        }
        Icon::Brand => {
            painter.circle_stroke(center, radius, stroke);
            painter.line_segment(
                [
                    pos2(center.x + radius * 0.15, center.y + radius * 0.15),
                    pos2(rect.right(), rect.bottom()),
                ],
                stroke,
            );
        }
        Icon::Connect => {
            painter.circle_stroke(pos2(rect.left() + 3.0, center.y), 3.0, stroke);
            painter.circle_stroke(pos2(rect.right() - 3.0, center.y), 3.0, stroke);
            painter.line_segment(
                [
                    pos2(rect.left() + 6.0, center.y),
                    pos2(rect.right() - 6.0, center.y),
                ],
                stroke,
            );
        }
        Icon::Delete => {
            painter.line_segment(
                [
                    pos2(rect.left() + 3.0, rect.top() + 3.0),
                    pos2(rect.right() - 3.0, rect.bottom() - 3.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    pos2(rect.right() - 3.0, rect.top() + 3.0),
                    pos2(rect.left() + 3.0, rect.bottom() - 3.0),
                ],
                stroke,
            );
        }
        Icon::Diagnostics => {
            painter.circle_stroke(center, radius, stroke);
            painter.circle_filled(pos2(center.x, center.y - radius * 0.48), 1.2, color);
            painter.line_segment(
                [
                    pos2(center.x, center.y - radius * 0.12),
                    pos2(center.x, center.y + radius * 0.56),
                ],
                stroke,
            );
        }
        Icon::Exclusive => {
            let points = vec![
                pos2(center.x, rect.top() + 1.0),
                pos2(rect.right() - 1.0, center.y),
                pos2(center.x, rect.bottom() - 1.0),
                pos2(rect.left() + 1.0, center.y),
            ];
            painter.add(Shape::convex_polygon(points, Color32::TRANSPARENT, stroke));
        }
        Icon::Graph => {
            let a = pos2(rect.left() + 3.0, rect.top() + 4.0);
            let b = pos2(rect.right() - 3.0, rect.top() + 4.0);
            let c = pos2(center.x, rect.bottom() - 3.0);
            painter.line_segment([a, c], stroke);
            painter.line_segment([b, c], stroke);
            painter.circle_filled(a, 2.5, color);
            painter.circle_filled(b, 2.5, color);
            painter.circle_filled(c, 2.5, color);
        }
        Icon::Language => {
            painter.circle_stroke(center, radius, stroke);
            painter.line_segment(
                [
                    pos2(center.x - radius, center.y),
                    pos2(center.x + radius, center.y),
                ],
                stroke,
            );
            painter.add(Shape::line(
                vec![
                    pos2(center.x, center.y - radius),
                    pos2(center.x - radius * 0.42, center.y),
                    pos2(center.x, center.y + radius),
                ],
                stroke,
            ));
            painter.add(Shape::line(
                vec![
                    pos2(center.x, center.y - radius),
                    pos2(center.x + radius * 0.42, center.y),
                    pos2(center.x, center.y + radius),
                ],
                stroke,
            ));
        }
        Icon::Load => {
            painter.rect_stroke(rect.shrink(2.0), 2.0, stroke);
            painter.arrow(center + vec2(0.0, -radius), vec2(0.0, radius * 1.4), stroke);
        }
        Icon::Patchbay => {
            painter.circle_filled(pos2(rect.left() + 3.0, center.y), 2.5, color);
            painter.circle_filled(pos2(rect.right() - 3.0, center.y), 2.5, color);
            painter.arrow(
                pos2(rect.left() + 6.0, center.y - 2.5),
                vec2(rect.width() - 12.0, 0.0),
                stroke,
            );
            painter.arrow(
                pos2(rect.right() - 6.0, center.y + 2.5),
                vec2(-(rect.width() - 12.0), 0.0),
                stroke,
            );
        }
        Icon::Pin => {
            painter.line_segment(
                [
                    pos2(center.x, rect.top() + 1.0),
                    pos2(center.x, rect.bottom() - 1.0),
                ],
                stroke,
            );
            painter.add(Shape::convex_polygon(
                vec![
                    pos2(center.x - 4.0, rect.top() + 4.0),
                    pos2(center.x + 4.0, rect.top() + 4.0),
                    pos2(center.x + 2.5, center.y + 1.0),
                    pos2(center.x - 2.5, center.y + 1.0),
                ],
                color,
                Stroke::NONE,
            ));
        }
        Icon::Refresh => {
            draw_arc(painter, center, radius, 0.45, 5.35, stroke);
            painter.arrow(
                pos2(center.x + radius * 0.74, center.y - radius * 0.67),
                vec2(radius * 0.03, radius * 0.42),
                stroke,
            );
        }
        Icon::Repel => {
            painter.arrow(center, vec2(-radius, -radius), stroke);
            painter.arrow(center, vec2(radius, -radius), stroke);
            painter.arrow(center, vec2(-radius, radius), stroke);
            painter.arrow(center, vec2(radius, radius), stroke);
            painter.circle_filled(center, 1.5, color);
        }
        Icon::Redo => {
            painter.add(Shape::line(
                vec![
                    pos2(rect.left() + 3.0, center.y + 4.0),
                    pos2(center.x, center.y + 4.0),
                    pos2(center.x + 4.0, center.y),
                    pos2(center.x, center.y - 4.0),
                    pos2(rect.left() + 3.0, center.y - 4.0),
                ],
                stroke,
            ));
            painter.arrow(pos2(center.x + 4.0, center.y), vec2(5.0, 0.0), stroke);
        }
        Icon::Save => {
            let body = rect.shrink(2.0);
            painter.rect_stroke(body, 2.0, stroke);
            painter.rect_filled(
                Rect::from_min_max(
                    pos2(body.left() + 3.0, body.top() + 2.0),
                    pos2(body.right() - 3.0, body.top() + 6.0),
                ),
                1.0,
                color,
            );
            painter.circle_stroke(pos2(center.x, body.bottom() - 4.0), 2.5, stroke);
        }
        Icon::Settings => {
            painter.circle_stroke(center, radius * 0.45, stroke);
            for index in 0..8 {
                let angle = index as f32 * std::f32::consts::TAU / 8.0;
                let inner = center + vec2(angle.cos(), angle.sin()) * (radius * 0.72);
                let outer = center + vec2(angle.cos(), angle.sin()) * radius;
                painter.line_segment([inner, outer], stroke);
            }
        }
        Icon::Snapshot => {
            painter.circle_stroke(center, radius, stroke);
            painter.circle_stroke(center, radius * 0.44, stroke);
            painter.line_segment(
                [
                    pos2(center.x, rect.top() + 1.0),
                    pos2(center.x, center.y - radius * 0.55),
                ],
                stroke,
            );
        }
        Icon::Statusbar => {
            let body = rect.shrink(2.0);
            painter.rect_stroke(body, 2.0, stroke);
            painter.line_segment(
                [
                    pos2(body.left(), body.bottom() - 5.0),
                    pos2(body.right(), body.bottom() - 5.0),
                ],
                stroke,
            );
        }
        Icon::Thumbnail => {
            let body = rect.shrink(2.0);
            painter.rect_stroke(body, 2.0, stroke);
            painter.line_segment(
                [pos2(center.x, body.top()), pos2(center.x, body.bottom())],
                stroke,
            );
            painter.line_segment(
                [pos2(body.left(), center.y), pos2(body.right(), center.y)],
                stroke,
            );
        }
        Icon::Timer => {
            painter.circle_stroke(center, radius, stroke);
            painter.line_segment([center, pos2(center.x, center.y - radius * 0.58)], stroke);
            painter.line_segment(
                [
                    center,
                    pos2(center.x + radius * 0.48, center.y + radius * 0.26),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    pos2(center.x - 2.0, rect.top() + 1.0),
                    pos2(center.x + 2.0, rect.top() + 1.0),
                ],
                stroke,
            );
        }
        Icon::Toolbar => {
            for offset in [-4.0, 0.0, 4.0] {
                painter.line_segment(
                    [
                        pos2(rect.left() + 1.0, center.y + offset),
                        pos2(rect.right() - 1.0, center.y + offset),
                    ],
                    stroke,
                );
            }
        }
        Icon::Undo => {
            painter.add(Shape::line(
                vec![
                    pos2(rect.right() - 3.0, center.y + 4.0),
                    pos2(center.x, center.y + 4.0),
                    pos2(center.x - 4.0, center.y),
                    pos2(center.x, center.y - 4.0),
                    pos2(rect.right() - 3.0, center.y - 4.0),
                ],
                stroke,
            ));
            painter.arrow(pos2(center.x - 4.0, center.y), vec2(-5.0, 0.0), stroke);
        }
    }
}

fn draw_arc(painter: &Painter, center: Pos2, radius: f32, start: f32, end: f32, stroke: Stroke) {
    let points = (0..=20)
        .map(|index| {
            let progress = index as f32 / 20.0;
            let angle = start + (end - start) * progress;
            center + vec2(angle.cos(), angle.sin()) * radius
        })
        .collect();
    painter.add(Shape::line(points, stroke));
}
