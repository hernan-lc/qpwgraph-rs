use super::{CommonProps, ElementKind, UiDocument, Value};
use egui::{Color32, Context, Frame, Id, LayerId, Margin, Order, Rect, Sense, Stroke, Ui, Vec2};

/// Placement strategy for a reusable dialog.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DialogPlacement {
    /// Centers the dialog in the current viewport.
    Centered { width: f32 },
    /// Pins the dialog to an application-provided rectangle.
    Fixed { rect: Rect },
}

/// Properties for [`UiDocument::dialog`].
#[derive(Clone, Debug, PartialEq)]
pub struct DialogProps {
    /// Shared identity and visual properties.
    pub common: CommonProps,
    /// Title shown in the dialog title bar.
    pub title: String,
    /// Dialog position and sizing mode.
    pub placement: DialogPlacement,
    /// Fill used for the area behind the dialog.
    pub backdrop_fill: Color32,
    /// Whether the background captures clicks and dismisses the dialog.
    pub modal: bool,
    /// Whether the dialog can be resized by the user.
    pub resizable: bool,
    /// Whether the title bar can collapse the dialog.
    pub collapsible: bool,
}

impl DialogProps {
    /// Creates a centered dialog with polished dark-theme defaults.
    pub fn centered(id: impl Into<String>, title: impl Into<String>, width: f32) -> Self {
        Self::new(id, title).placement(DialogPlacement::Centered {
            width: width.max(1.0),
        })
    }

    /// Creates a dialog fixed to an application-provided rectangle.
    pub fn fixed(id: impl Into<String>, title: impl Into<String>, rect: Rect) -> Self {
        Self::new(id, title).placement(DialogPlacement::Fixed { rect })
    }

    /// Creates a centered dialog using the default width.
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        let id = id.into();
        let style = super::Style::default()
            .fill(Color32::from_rgb(28, 33, 42))
            .stroke(Stroke::new(1.0_f32, Color32::from_rgb(75, 88, 106)))
            .rounding(8.0)
            .inner_margin(10.0);
        Self {
            common: CommonProps::new(id).style(style),
            title: title.into(),
            placement: DialogPlacement::Centered { width: 520.0 },
            backdrop_fill: Color32::from_black_alpha(150),
            modal: true,
            resizable: false,
            collapsible: false,
        }
    }

    /// Sets the placement mode.
    pub fn placement(mut self, placement: DialogPlacement) -> Self {
        self.placement = placement;
        self
    }

    /// Replaces the shared dialog style.
    pub fn style(mut self, style: super::Style) -> Self {
        self.common = self.common.style(style);
        self
    }

    /// Sets the dialog background fill.
    pub fn fill(mut self, fill: Color32) -> Self {
        self.common.style = self.common.style.clone().fill(fill);
        self
    }

    /// Sets the dialog border.
    pub fn stroke(mut self, stroke: Stroke) -> Self {
        self.common.style = self.common.style.clone().stroke(stroke);
        self
    }

    /// Sets the dialog corner radius.
    pub fn rounding(mut self, rounding: f32) -> Self {
        self.common.style = self.common.style.clone().rounding(rounding);
        self
    }

    /// Sets the dialog inner margin.
    pub fn inner_margin(mut self, margin: f32) -> Self {
        self.common.style = self.common.style.clone().inner_margin(margin);
        self
    }

    /// Sets the backdrop color.
    pub fn backdrop(mut self, fill: Color32) -> Self {
        self.backdrop_fill = fill;
        self
    }

    /// Enables or disables click-to-dismiss backdrop behavior.
    pub fn modal(mut self, modal: bool) -> Self {
        self.modal = modal;
        self
    }

    /// Enables or disables resizing.
    pub fn resizable(mut self, resizable: bool) -> Self {
        self.resizable = resizable;
        self
    }

    /// Enables or disables collapsing.
    pub fn collapsible(mut self, collapsible: bool) -> Self {
        self.collapsible = collapsible;
        self
    }
}

/// Result of showing a dialog for one frame.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DialogResponse {
    /// Whether the dialog was visible and rendered.
    pub shown: bool,
    /// Whether the user clicked outside the dialog this frame.
    pub backdrop_clicked: bool,
}

impl UiDocument {
    /// Shows a retained, modal dialog and its contents.
    ///
    /// The dialog registers itself by ID, paints a translucent backdrop behind
    /// its window, and keeps the window above that backdrop even after a click
    /// reorders foreground layers. The caller decides what a backdrop click
    /// means, usually by closing the dialog.
    pub fn dialog(
        &mut self,
        ctx: &Context,
        props: DialogProps,
        add_contents: impl FnOnce(&mut Ui, &mut UiDocument),
    ) -> DialogResponse {
        let id = props.common.id.clone();
        self.prepare(
            &props.common,
            ElementKind::Dialog,
            Value::Bool(true),
            vec![],
        );
        if !props.common.visible {
            return DialogResponse::default();
        }

        let backdrop_id = Id::new(("ui-document-dialog-backdrop", id.clone()));
        let window_id = Id::new(("ui-document-dialog-window", id));
        let backdrop_clicked = if props.modal {
            let backdrop_layer = LayerId::new(Order::Foreground, backdrop_id);
            let window_layer = LayerId::new(Order::Foreground, window_id);
            ctx.set_sublayer(backdrop_layer, window_layer);
            let viewport = ctx.screen_rect();
            egui::Area::new(backdrop_id)
                .order(Order::Foreground)
                .fixed_pos(viewport.min)
                .show(ctx, |ui| {
                    let mut sense = Sense::click();
                    // This hit target must not become part of keyboard focus traversal.
                    sense.focusable = false;
                    let (response, painter) = ui.allocate_painter(viewport.size(), sense);
                    painter.rect_filled(response.rect, 0.0, props.backdrop_fill);
                    response
                })
                .inner
                .clicked()
        } else {
            false
        };

        let mut window = egui::Window::new(props.title)
            .id(window_id)
            .collapsible(props.collapsible)
            .resizable(props.resizable)
            .order(Order::Foreground)
            .frame(dialog_frame(&props.common.style));
        match props.placement {
            DialogPlacement::Centered { width } => {
                window = window
                    .default_width(width.max(1.0))
                    .max_width(width.max(1.0))
                    .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO);
                if let Some(height) = props.common.style.height {
                    window = window.default_height(height.max(1.0));
                }
            }
            DialogPlacement::Fixed { rect } => {
                window = window.fixed_pos(rect.min).fixed_size(rect.size());
            }
        }
        let shown = window.show(ctx, |ui| add_contents(ui, self)).is_some();
        DialogResponse {
            shown,
            backdrop_clicked,
        }
    }
}

fn dialog_frame(style: &super::Style) -> Frame {
    let mut frame = Frame::none();
    if let Some(fill) = style.fill {
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
    frame
}
