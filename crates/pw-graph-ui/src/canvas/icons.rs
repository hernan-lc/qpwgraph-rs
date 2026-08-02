use egui::{include_image, Color32, Image, ImageSource, Vec2};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum NodeIcon {
    Info,
    More,
    Monitor,
    Volume,
    VolumeMuted,
}

fn source(icon: NodeIcon) -> ImageSource<'static> {
    match icon {
        NodeIcon::Info => include_image!("../../assets/icons/info.svg"),
        NodeIcon::More => include_image!("../../assets/icons/more.svg"),
        NodeIcon::Monitor => include_image!("../../assets/icons/monitor.svg"),
        NodeIcon::Volume => include_image!("../../assets/icons/volume.svg"),
        NodeIcon::VolumeMuted => include_image!("../../assets/icons/volume_muted.svg"),
    }
}

pub(super) fn image(icon: NodeIcon, size: Vec2, color: Color32) -> Image<'static> {
    Image::new(source(icon)).fit_to_exact_size(size).tint(color)
}
