use egui::{include_image, Color32, Image, ImageSource, Vec2};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum NodeIcon {
    Expand,
    Collapse,
    More,
}

fn source(icon: NodeIcon) -> ImageSource<'static> {
    match icon {
        NodeIcon::Expand => include_image!("../../assets/icons/expand.svg"),
        NodeIcon::Collapse => include_image!("../../assets/icons/collapse.svg"),
        NodeIcon::More => include_image!("../../assets/icons/more.svg"),
    }
}

pub(super) fn image(icon: NodeIcon, size: Vec2, color: Color32) -> Image<'static> {
    Image::new(source(icon)).fit_to_exact_size(size).tint(color)
}
