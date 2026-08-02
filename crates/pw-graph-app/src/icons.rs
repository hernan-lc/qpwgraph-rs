use egui::{include_image, vec2, Color32, Image, ImageSource, Rect, Sense, Ui, Vec2};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Icon {
    Activate,
    AutoDisconnect,
    Connect,
    Delete,
    Exclusive,
    Filter,
    Help,
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
    Sort,
    SortDirection,
    Statusbar,
    Thumbnail,
    Timer,
    Toolbar,
    Undo,
}

const ICON_BUTTON_SIZE: Vec2 = vec2(34.0, 30.0);
const SIDEBAR_ICON_BUTTON_SIZE: Vec2 = vec2(46.0, 42.0);
const SIDEBAR_NAV_BUTTON_SIZE: Vec2 = vec2(52.0, 48.0);

fn icon_source(icon: Icon) -> ImageSource<'static> {
    match icon {
        Icon::Activate => include_image!("../assets/icons/activate.svg"),
        Icon::AutoDisconnect => include_image!("../assets/icons/auto_disconnect.svg"),
        Icon::Connect => include_image!("../assets/icons/connect.svg"),
        Icon::Delete => include_image!("../assets/icons/delete.svg"),
        Icon::Exclusive => include_image!("../assets/icons/exclusive.svg"),
        Icon::Filter => include_image!("../assets/icons/filter.svg"),
        Icon::Help => include_image!("../assets/icons/help.svg"),
        Icon::Language => include_image!("../assets/icons/language.svg"),
        Icon::Load => include_image!("../assets/icons/load.svg"),
        Icon::Patchbay => include_image!("../assets/icons/patchbay.svg"),
        Icon::Pin => include_image!("../assets/icons/pin.svg"),
        Icon::Refresh => include_image!("../assets/icons/refresh.svg"),
        Icon::Repel => include_image!("../assets/icons/repel.svg"),
        Icon::Redo => include_image!("../assets/icons/redo.svg"),
        Icon::Save => include_image!("../assets/icons/save.svg"),
        Icon::Settings => include_image!("../assets/icons/settings.svg"),
        Icon::Snapshot => include_image!("../assets/icons/snapshot.svg"),
        Icon::Sort => include_image!("../assets/icons/sort.svg"),
        Icon::SortDirection => include_image!("../assets/icons/sort_direction.svg"),
        Icon::Statusbar => include_image!("../assets/icons/statusbar.svg"),
        Icon::Thumbnail => include_image!("../assets/icons/thumbnail.svg"),
        Icon::Timer => include_image!("../assets/icons/timer.svg"),
        Icon::Toolbar => include_image!("../assets/icons/toolbar.svg"),
        Icon::Undo => include_image!("../assets/icons/undo.svg"),
    }
}

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
    icon_button_enabled_sized(
        ui,
        id,
        icon,
        label,
        explanation,
        enabled,
        ICON_BUTTON_SIZE,
        7.0,
    )
}

pub(crate) fn sidebar_icon_button(
    ui: &mut Ui,
    id: &str,
    icon: Icon,
    label: String,
    explanation: String,
) -> bool {
    sidebar_icon_button_enabled(ui, id, icon, label, explanation, true)
}

pub(crate) fn sidebar_icon_button_enabled(
    ui: &mut Ui,
    id: &str,
    icon: Icon,
    label: String,
    explanation: String,
    enabled: bool,
) -> bool {
    icon_button_enabled_sized(
        ui,
        id,
        icon,
        label,
        explanation,
        enabled,
        SIDEBAR_ICON_BUTTON_SIZE,
        9.0,
    )
}
#[allow(clippy::too_many_arguments)]
fn icon_button_enabled_sized(
    ui: &mut Ui,
    id: &str,
    icon: Icon,
    label: String,
    explanation: String,
    enabled: bool,
    size: Vec2,
    icon_inset: f32,
) -> bool {
    ui.push_id(("icon-button", id), |ui| {
        let sense = if enabled {
            Sense::click()
        } else {
            Sense::hover()
        };
        let (rect, response) = ui.allocate_exact_size(size, sense);
        let response = response.on_hover_text(format!("{label}\n{explanation}"));
        let visuals = if enabled {
            ui.style().interact(&response)
        } else {
            ui.style().noninteractive()
        };
        ui.painter()
            .rect(rect, 4.0, visuals.bg_fill, visuals.bg_stroke);
        paint_icon(ui, rect.shrink(icon_inset), icon, visuals.fg_stroke.color);
        response.clicked()
    })
    .inner
}

pub(crate) fn sidebar_icon_toggle_button(
    ui: &mut Ui,
    id: &str,
    icon: Icon,
    selected: bool,
    label: String,
    explanation: String,
) -> bool {
    icon_toggle_button_sized(
        ui,
        id,
        icon,
        selected,
        label,
        explanation,
        SIDEBAR_ICON_BUTTON_SIZE,
        9.0,
    )
}
#[allow(clippy::too_many_arguments)]
fn icon_toggle_button_sized(
    ui: &mut Ui,
    id: &str,
    icon: Icon,
    selected: bool,
    label: String,
    explanation: String,
    size: Vec2,
    icon_inset: f32,
) -> bool {
    ui.push_id(("icon-toggle-button", id), |ui| {
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
        let response = response.on_hover_text(format!("{label}\n{explanation}"));
        let visuals = ui.style().interact_selectable(&response, selected);
        ui.painter()
            .rect(rect, 4.0, visuals.bg_fill, visuals.bg_stroke);
        paint_icon(ui, rect.shrink(icon_inset), icon, visuals.fg_stroke.color);
        response.clicked()
    })
    .inner
}

pub(crate) fn sidebar_nav_icon_button(
    ui: &mut Ui,
    id: &str,
    icon: Icon,
    selected: bool,
    label: String,
    explanation: String,
) -> bool {
    nav_icon_button_sized(
        ui,
        id,
        icon,
        selected,
        label,
        explanation,
        SIDEBAR_NAV_BUTTON_SIZE,
        12.0,
    )
}
#[allow(clippy::too_many_arguments)]
fn nav_icon_button_sized(
    ui: &mut Ui,
    id: &str,
    icon: Icon,
    selected: bool,
    label: String,
    explanation: String,
    size: Vec2,
    icon_inset: f32,
) -> bool {
    ui.push_id(("navigation-icon", id), |ui| {
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
        let response = response.on_hover_text(format!("{label}\n{explanation}"));
        let visuals = ui.style().interact_selectable(&response, selected);
        ui.painter()
            .rect(rect, 5.0, visuals.bg_fill, visuals.bg_stroke);
        paint_icon(ui, rect.shrink(icon_inset), icon, visuals.fg_stroke.color);
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
            paint_icon(ui, rect.shrink(3.0), icon, Color32::LIGHT_BLUE);
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

pub(crate) fn icon_label(ui: &mut Ui, icon: Icon, tooltip: String) {
    let (rect, response) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
    let response = response.on_hover_text(tooltip);
    paint_icon(ui, rect.shrink(2.0), icon, ui.visuals().text_color());
    let _ = response;
}

pub(crate) fn paint_icon(ui: &Ui, rect: Rect, icon: Icon, color: Color32) {
    Image::new(icon_source(icon)).tint(color).paint_at(ui, rect);
}
