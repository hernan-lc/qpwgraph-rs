use egui::{include_image, vec2, Color32, Image, ImageSource, Rect, Sense, Ui, Vec2};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Icon {
    Activate,
    AutoDisconnect,
    Close,
    Connect,
    Delete,
    #[cfg(feature = "relay")]
    DeviceDesktop,
    #[cfg(feature = "relay")]
    DeviceGeneric,
    #[cfg(feature = "relay")]
    DevicePhone,
    Effects,
    Exclusive,
    Filter,
    Help,
    Language,
    Load,
    Minimap,
    Patchbay,
    Pin,
    #[cfg(feature = "relay")]
    QrCode,
    Refresh,
    #[cfg(feature = "relay")]
    Relay,
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

impl Icon {
    /// The icon as a `pw-graph-ui` component icon source, so shared
    /// components render the application's artwork instead of shipping a
    /// duplicate set of their own.
    pub(crate) fn source(self) -> pw_graph_ui::IconSource {
        pw_graph_ui::IconSource::Custom(icon_source(self))
    }
}

fn icon_source(icon: Icon) -> ImageSource<'static> {
    match icon {
        Icon::Activate => include_image!("../assets/icons/activate.svg"),
        Icon::AutoDisconnect => include_image!("../assets/icons/auto_disconnect.svg"),
        Icon::Close => include_image!("../assets/icons/close.svg"),
        Icon::Connect => include_image!("../assets/icons/connect.svg"),
        Icon::Delete => include_image!("../assets/icons/delete.svg"),
        #[cfg(feature = "relay")]
        Icon::DeviceDesktop => include_image!("../assets/icons/device_desktop.svg"),
        #[cfg(feature = "relay")]
        Icon::DeviceGeneric => include_image!("../assets/icons/device_generic.svg"),
        #[cfg(feature = "relay")]
        Icon::DevicePhone => include_image!("../assets/icons/device_phone.svg"),
        Icon::Effects => include_image!("../assets/icons/effects.svg"),
        Icon::Exclusive => include_image!("../assets/icons/exclusive.svg"),
        Icon::Filter => include_image!("../assets/icons/filter.svg"),
        Icon::Help => include_image!("../assets/icons/help.svg"),
        Icon::Language => include_image!("../assets/icons/language.svg"),
        Icon::Load => include_image!("../assets/icons/load.svg"),
        Icon::Minimap => include_image!("../assets/icons/minimap.svg"),
        Icon::Patchbay => include_image!("../assets/icons/patchbay.svg"),
        Icon::Pin => include_image!("../assets/icons/pin.svg"),
        #[cfg(feature = "relay")]
        Icon::QrCode => include_image!("../assets/icons/qr.svg"),
        Icon::Refresh => include_image!("../assets/icons/refresh.svg"),
        #[cfg(feature = "relay")]
        Icon::Relay => include_image!("../assets/icons/relay.svg"),
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
/// Draws one icon button: a hoverable square with the icon centered, reporting
/// whether it was clicked. All three public buttons (plain, enabled-aware,
/// selectable toggle, nav) share this body — they differ only in the id salt,
/// corner radius, and how the visual state is derived from `selected`/`enabled`.
#[allow(clippy::too_many_arguments)]
fn draw_icon_button(
    ui: &mut Ui,
    id: &str,
    id_salt: &str,
    size: Vec2,
    icon_inset: f32,
    corner_radius: f32,
    selected: Option<bool>,
    enabled: bool,
    icon: Icon,
    label: String,
    explanation: String,
) -> bool {
    ui.push_id((id_salt, id), |ui| {
        let clickable = selected.is_some() || enabled;
        let sense = if clickable {
            Sense::click()
        } else {
            Sense::hover()
        };
        let (rect, response) = ui.allocate_exact_size(size, sense);
        let response = response.on_hover_text(format!("{label}\n{explanation}"));
        let visuals = match selected {
            Some(selected) => ui.style().interact_selectable(&response, selected),
            None if enabled => *ui.style().interact(&response),
            None => *ui.style().noninteractive(),
        };
        ui.painter()
            .rect(rect, corner_radius, visuals.bg_fill, visuals.bg_stroke);
        paint_icon(ui, rect.shrink(icon_inset), icon, visuals.fg_stroke.color);
        response.clicked()
    })
    .inner
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
    draw_icon_button(
        ui,
        id,
        "icon-button",
        size,
        icon_inset,
        4.0,
        None,
        enabled,
        icon,
        label,
        explanation,
    )
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
    draw_icon_button(
        ui,
        id,
        "icon-toggle-button",
        size,
        icon_inset,
        4.0,
        Some(selected),
        true,
        icon,
        label,
        explanation,
    )
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
    draw_icon_button(
        ui,
        id,
        "navigation-icon",
        size,
        icon_inset,
        5.0,
        Some(selected),
        true,
        icon,
        label,
        explanation,
    )
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
