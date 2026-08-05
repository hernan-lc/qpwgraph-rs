//! Status components: badges, level meters, icon buttons, and spinners.
//!
//! These carry the small pieces of state a panel repeats everywhere — "this
//! is connected", "audio is flowing", "this action is a refresh". Centralising
//! them keeps a status pill the same size and weight in every panel, which is
//! most of what makes a set of screens look like one product.

use super::icons::{icon_image, IconSource};
use super::{CommonProps, ElementId, ElementKind, UiDocument, Value};
use egui::{Color32, Frame, Margin, Response, RichText, Sense, Ui, Vec2};

/// Default edge length of an icon button's clickable square.
const ICON_BUTTON_SIZE: f32 = 26.0;
/// Default glyph size inside an icon button.
const ICON_GLYPH_SIZE: f32 = 15.0;

/// Properties for [`UiDocument::badge`]: a small status pill.
#[derive(Clone, Debug, PartialEq)]
pub struct BadgeProps {
    pub common: CommonProps,
    /// Text inside the pill.
    pub text: String,
    /// Pill colour. The background is a dimmed version of it, so one colour
    /// defines the whole badge.
    pub color: Color32,
}

impl BadgeProps {
    /// Creates a neutral badge.
    pub fn new(id: impl Into<ElementId>, text: impl Into<String>) -> Self {
        Self {
            common: CommonProps::new(id),
            text: text.into(),
            color: Color32::from_rgb(148, 163, 184),
        }
    }

    /// Sets the badge colour.
    pub fn color(mut self, color: Color32) -> Self {
        self.color = color;
        self
    }

    /// Sets a tooltip.
    pub fn tooltip(mut self, tooltip: impl Into<String>) -> Self {
        self.common = self.common.tooltip(tooltip);
        self
    }
}

/// Properties for [`UiDocument::meter`]: a horizontal level bar.
#[derive(Clone, Debug, PartialEq)]
pub struct MeterProps {
    pub common: CommonProps,
    /// Level in `0.0..=1.0`. `None` draws the empty track, which keeps rows a
    /// stable width while a source has not reported yet.
    pub level: Option<f32>,
    /// Fill colour of the active portion.
    pub color: Color32,
    /// Whether to apply a square-root curve, which makes ordinary speech
    /// levels legible instead of pinned near zero.
    pub perceptual: bool,
}

impl MeterProps {
    /// Creates a meter with no reading.
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            common: CommonProps::new(id).size(52.0, 6.0),
            level: None,
            color: Color32::from_rgb(96, 190, 130),
            perceptual: true,
        }
    }

    /// Sets the current level.
    pub fn level(mut self, level: f32) -> Self {
        self.level = Some(level);
        self
    }

    /// Sets the level from an optional reading.
    pub fn level_option(mut self, level: Option<f32>) -> Self {
        self.level = level;
        self
    }

    /// Sets the bar colour.
    pub fn color(mut self, color: Color32) -> Self {
        self.color = color;
        self
    }

    /// Enables or disables the perceptual curve.
    pub fn perceptual(mut self, perceptual: bool) -> Self {
        self.perceptual = perceptual;
        self
    }

    /// Sets a tooltip.
    pub fn tooltip(mut self, tooltip: impl Into<String>) -> Self {
        self.common = self.common.tooltip(tooltip);
        self
    }

    /// Sets the bar size.
    pub fn size(mut self, width: f32, height: f32) -> Self {
        self.common = self.common.size(width, height);
        self
    }
}

/// Properties for [`UiDocument::icon_button`].
#[derive(Clone)]
pub struct IconButtonProps {
    pub common: CommonProps,
    /// Artwork for the button.
    pub icon: IconSource,
    /// Edge length of the clickable square.
    pub button_size: f32,
    /// Glyph size drawn inside that square.
    pub icon_size: f32,
    /// Icon tint. Defaults to the theme's text colour.
    pub tint: Option<Color32>,
    /// Whether the button paints a hover/press background.
    pub frameless: bool,
}

impl IconButtonProps {
    /// Creates an icon button with the layer's default sizing.
    pub fn new(id: impl Into<ElementId>, icon: impl Into<IconSource>) -> Self {
        Self {
            common: CommonProps::new(id),
            icon: icon.into(),
            button_size: ICON_BUTTON_SIZE,
            icon_size: ICON_GLYPH_SIZE,
            tint: None,
            frameless: false,
        }
    }

    /// Sets the clickable square's edge length.
    pub fn button_size(mut self, size: f32) -> Self {
        self.button_size = size.max(1.0);
        self
    }

    /// Sets the glyph size.
    pub fn icon_size(mut self, size: f32) -> Self {
        self.icon_size = size.max(1.0);
        self
    }

    /// Sets the icon tint.
    pub fn tint(mut self, tint: Color32) -> Self {
        self.tint = Some(tint);
        self
    }

    /// Removes the hover/press background.
    pub fn frameless(mut self, frameless: bool) -> Self {
        self.frameless = frameless;
        self
    }

    /// Enables or disables the button.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.common = self.common.enabled(enabled);
        self
    }

    /// Sets a tooltip. Icon-only controls should always carry one: the icon
    /// is the whole label otherwise.
    pub fn tooltip(mut self, tooltip: impl Into<String>) -> Self {
        self.common = self.common.tooltip(tooltip);
        self
    }
}

impl UiDocument {
    /// Renders a status badge.
    pub fn badge(&mut self, ui: &mut Ui, props: BadgeProps) -> Response {
        let id = props.common.id.clone();
        self.prepare(
            &props.common,
            ElementKind::Badge,
            Value::String(props.text.clone()),
            vec![],
        );
        if let Some(element) = self.elements.get_mut(&id) {
            element.value = Value::String(props.text.clone());
        }
        if !props.common.visible {
            return ui.allocate_exact_size(Vec2::ZERO, Sense::hover()).1;
        }
        let mut response = Frame::none()
            .fill(props.color.gamma_multiply(0.28))
            .rounding(7.0)
            .inner_margin(Margin::symmetric(6.0, 1.0))
            .show(ui, |ui| {
                ui.label(RichText::new(props.text).small().color(props.color));
            })
            .response;
        if let Some(tooltip) = &props.common.tooltip {
            response = response.on_hover_text(tooltip.clone());
        }
        response
    }

    /// Renders a level meter.
    pub fn meter(&mut self, ui: &mut Ui, props: MeterProps) -> Response {
        let id = props.common.id.clone();
        self.prepare(
            &props.common,
            ElementKind::Meter,
            Value::Number(props.level.unwrap_or(0.0) as f64),
            vec![],
        );
        if let Some(element) = self.elements.get_mut(&id) {
            element.value = Value::Number(props.level.unwrap_or(0.0) as f64);
        }
        if !props.common.visible {
            return ui.allocate_exact_size(Vec2::ZERO, Sense::hover()).1;
        }
        let size = Vec2::new(
            props.common.style.width.unwrap_or(52.0),
            props.common.style.height.unwrap_or(6.0),
        );
        let (rect, mut response) = ui.allocate_exact_size(size, Sense::hover());
        let rounding = props.common.style.rounding.unwrap_or(size.y / 2.0);
        let painter = ui.painter();
        painter.rect_filled(rect, rounding, ui.visuals().extreme_bg_color);
        if let Some(level) = props.level {
            let level = level.clamp(0.0, 1.0);
            let level = if props.perceptual {
                level.sqrt()
            } else {
                level
            };
            if level > f32::EPSILON {
                let filled =
                    egui::Rect::from_min_size(rect.min, Vec2::new(rect.width() * level, size.y));
                painter.rect_filled(filled, rounding, props.color);
            }
        }
        if let Some(tooltip) = &props.common.tooltip {
            response = response.on_hover_text(tooltip.clone());
        }
        response
    }

    /// Renders an SVG icon button and emits a click event when activated.
    pub fn icon_button(&mut self, ui: &mut Ui, props: IconButtonProps) -> bool {
        let id = props.common.id.clone();
        self.prepare(
            &props.common,
            ElementKind::IconButton,
            Value::Bool(false),
            vec![],
        );
        if !props.common.visible {
            return false;
        }
        let (rect, mut response) = ui.allocate_exact_size(
            Vec2::splat(props.button_size),
            if props.common.enabled {
                Sense::click()
            } else {
                Sense::hover()
            },
        );
        let visuals = if props.common.enabled {
            *ui.style().interact(&response)
        } else {
            *ui.style().noninteractive()
        };
        if !props.frameless
            && props.common.enabled
            && (response.hovered() || response.is_pointer_button_down_on())
        {
            ui.painter().rect_filled(rect, 5.0, visuals.bg_fill);
        }
        let tint = props.tint.unwrap_or(visuals.fg_stroke.color);
        let tint = if props.common.enabled {
            tint
        } else {
            tint.gamma_multiply(0.4)
        };
        let icon_rect = egui::Rect::from_center_size(rect.center(), Vec2::splat(props.icon_size));
        icon_image(&props.icon, props.icon_size, tint).paint_at(ui, icon_rect);
        if let Some(tooltip) = &props.common.tooltip {
            response = response.on_hover_text(tooltip.clone());
        }
        let clicked = response.clicked();
        if clicked {
            self.record_button_click(&id, Value::Bool(true));
        } else {
            self.observe_focus(&id, &response);
        }
        clicked
    }

    /// Renders a spinner with a trailing status line.
    ///
    /// Panels that scan for something need to say so continuously; a static
    /// "searching" label reads as stalled the moment nothing has appeared for
    /// a few seconds.
    pub fn activity(&mut self, ui: &mut Ui, id: impl Into<ElementId>, text: impl Into<String>) {
        let common = CommonProps::new(id);
        let text = text.into();
        self.prepare(
            &common,
            ElementKind::Label,
            Value::String(text.clone()),
            vec![],
        );
        ui.horizontal(|ui| {
            ui.add(egui::Spinner::new().size(13.0));
            ui.label(RichText::new(text).small().weak());
        });
    }
}

/// Emits a click event for a control the application painted itself.
///
/// This is the escape hatch for bespoke widgets: they stay visually custom
/// while still appearing in the document, so form queries and click listeners
/// see them like any built-in control.
pub fn record_custom_click(document: &mut UiDocument, id: impl Into<ElementId>, clicked: bool) {
    let id = id.into();
    let common = CommonProps::new(id.clone());
    document.prepare(&common, ElementKind::Button, Value::Bool(false), vec![]);
    if clicked {
        document.record_button_click(&id, Value::Bool(true));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meter_props_carry_an_optional_reading() {
        let props = MeterProps::new("meter");
        assert_eq!(props.level, None);
        assert_eq!(props.level(0.5).level, Some(0.5));
    }

    #[test]
    fn badge_defaults_to_a_neutral_colour() {
        let props = BadgeProps::new("badge", "3");
        assert_eq!(props.text, "3");
        assert_eq!(props.color(Color32::RED).color, Color32::RED);
    }
}
