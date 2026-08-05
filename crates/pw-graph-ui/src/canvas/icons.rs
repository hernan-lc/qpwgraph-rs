use crate::components::Icon;
use egui::{Color32, Image, Vec2};

/// Canvas-only rendering helper for a bundled [`Icon`]. The artwork registry
/// lives in [`crate::components`] so the canvas and the component layer can
/// never disagree about which SVG a named icon maps to.
pub(super) fn image(icon: Icon, size: Vec2, color: Color32) -> Image<'static> {
    Image::new(icon.source()).fit_to_exact_size(size).tint(color)
}
