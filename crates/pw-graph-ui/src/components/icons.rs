//! Icon artwork for the component layer.
//!
//! Components never draw glyph characters such as `▾` or `✓`. Text glyphs
//! depend on whatever font happens to be loaded, render at inconsistent
//! weights next to real icons, and cannot be tinted independently of the
//! label beside them. Every affordance therefore takes an SVG.
//!
//! [`Icon`] covers the artwork this crate ships for its own components.
//! Applications with their own icon set pass [`IconSource::Custom`] with any
//! [`ImageSource`], so a component's appearance is never limited to the
//! built-ins.

use egui::{include_image, Color32, Image, ImageSource, Vec2};

/// Icons bundled with the component layer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Icon {
    /// Disclosure in its open state.
    ChevronDown,
    /// Disclosure in its closed state.
    ChevronRight,
    /// Completion mark, used by finished steps.
    Check,
    Info,
    More,
    Volume,
    VolumeMuted,
    ArrowUp,
    ArrowDown,
}

impl Icon {
    /// The bundled artwork for this icon.
    pub fn source(self) -> ImageSource<'static> {
        match self {
            Self::ChevronDown => include_image!("../../assets/icons/chevron_down.svg"),
            Self::ChevronRight => include_image!("../../assets/icons/chevron_right.svg"),
            Self::Check => include_image!("../../assets/icons/check.svg"),
            Self::Info => include_image!("../../assets/icons/info.svg"),
            Self::More => include_image!("../../assets/icons/more.svg"),
            Self::Volume => include_image!("../../assets/icons/volume.svg"),
            Self::VolumeMuted => include_image!("../../assets/icons/volume_muted.svg"),
            Self::ArrowUp => include_image!("../../assets/icons/arrow_up.svg"),
            Self::ArrowDown => include_image!("../../assets/icons/arrow_down.svg"),
        }
    }

    /// The disclosure chevron matching an open/closed state.
    pub fn disclosure(open: bool) -> Self {
        if open {
            Self::ChevronDown
        } else {
            Self::ChevronRight
        }
    }
}

/// Artwork for a component that shows an icon.
///
/// `ImageSource` is only `Clone`, so props carrying an icon derive `Clone`
/// alone rather than the `Debug + PartialEq` the value-carrying props use.
#[derive(Clone)]
pub enum IconSource {
    /// One of this crate's bundled icons.
    Builtin(Icon),
    /// Application-supplied artwork, usually an `include_image!` SVG.
    Custom(ImageSource<'static>),
}

impl IconSource {
    /// Resolves to the underlying image source.
    pub fn source(&self) -> ImageSource<'static> {
        match self {
            Self::Builtin(icon) => icon.source(),
            Self::Custom(source) => source.clone(),
        }
    }
}

impl From<Icon> for IconSource {
    fn from(icon: Icon) -> Self {
        Self::Builtin(icon)
    }
}

impl From<ImageSource<'static>> for IconSource {
    fn from(source: ImageSource<'static>) -> Self {
        Self::Custom(source)
    }
}

/// Builds a square, tinted image sized to `size` logical points.
///
/// SVG artwork in this project is drawn in white so a tint reproduces any
/// colour exactly; sizing is exact rather than "fit" so icons in a row share
/// a baseline regardless of each drawing's aspect ratio.
pub fn icon_image(icon: &IconSource, size: f32, color: Color32) -> Image<'static> {
    Image::new(icon.source())
        .fit_to_exact_size(Vec2::splat(size))
        .tint(color)
}
