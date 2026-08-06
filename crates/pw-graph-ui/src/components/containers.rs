//! Container components: surfaces that hold other controls.
//!
//! These are the structural half of the DOM-like layer. Where `basic`,
//! `inputs`, and `choices` render individual values, these render *shape* —
//! a card, a collapsible section, a tab strip, a stepper — so panels describe
//! their structure declaratively instead of hand-assembling frames, chevrons,
//! and selection colours at every call site.
//!
//! Each one keeps its state in the document under its stable ID, so a
//! disclosure stays open, a tab strip remembers its selection, and both are
//! observable through `value`/`on_change` like any other control.

use super::icons::{icon_image, Icon, IconSource};
use super::theme::ThemeToken;
use super::{CommonProps, ElementId, ElementKind, EventType, Style, UiDocument, Value};
use egui::{Align, Color32, Frame, Layout, Margin, Response, RichText, Sense, Stroke, Ui, Vec2};

/// Icon size used by container chrome (chevrons, step marks).
const CHROME_ICON_SIZE: f32 = 14.0;
/// Diameter of a step marker.
const STEP_MARKER_SIZE: f32 = 22.0;

/// Properties for [`UiDocument::card`].
///
/// A card is a plain visual grouping: fill, radius, padding, and an optional
/// leading accent bar. It carries no value of its own.
#[derive(Clone, Debug, PartialEq)]
pub struct CardProps {
    pub common: CommonProps,
    /// Colour of the accent bar drawn down the leading edge, when set.
    pub accent: Option<Color32>,
    /// Space reserved between stacked cards.
    pub gap: f32,
}

impl CardProps {
    /// Creates a card with the layer's default surface styling.
    /// The fill is resolved from [`ThemeToken::Surface`] at render time.
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            common: CommonProps::new(id).style(
                Style::default()
                    .fill_token(ThemeToken::Surface)
                    .rounding(6.0)
                    .inner_margin(8.0),
            ),
            accent: None,
            gap: 3.0,
        }
    }

    /// Draws an accent bar down the card's leading edge.
    pub fn accent(mut self, color: Color32) -> Self {
        self.accent = Some(color);
        self
    }

    /// Sets or clears the accent bar.
    pub fn accent_option(mut self, color: Option<Color32>) -> Self {
        self.accent = color;
        self
    }

    /// Sets the background fill.
    pub fn fill(mut self, color: Color32) -> Self {
        self.common.style = self.common.style.clone().fill(color);
        self
    }

    /// Sets the space reserved above the card.
    pub fn gap(mut self, gap: f32) -> Self {
        self.gap = gap.max(0.0);
        self
    }
}

/// Properties for [`UiDocument::disclosure`].
///
/// The open state is retained in the document as a boolean, so the caller
/// does not have to own a flag per section.
#[derive(Clone, Debug, PartialEq)]
pub struct DisclosureProps {
    pub common: CommonProps,
    /// Header text.
    pub title: String,
    /// Open state used the first time the section is registered.
    pub default_open: bool,
    /// Optional trailing summary shown on the header, dimmed.
    pub summary: Option<String>,
}

impl DisclosureProps {
    /// Creates a closed disclosure with the given header.
    pub fn new(id: impl Into<ElementId>, title: impl Into<String>) -> Self {
        Self {
            common: CommonProps::new(id),
            title: title.into(),
            default_open: false,
            summary: None,
        }
    }

    /// Sets the state used on first registration.
    pub fn default_open(mut self, open: bool) -> Self {
        self.default_open = open;
        self
    }

    /// Adds a dimmed trailing summary to the header.
    pub fn summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = Some(summary.into());
        self
    }

    /// Sets a tooltip on the header.
    pub fn tooltip(mut self, tooltip: impl Into<String>) -> Self {
        self.common = self.common.tooltip(tooltip);
        self
    }
}

/// One entry in a [`TabsProps`] strip.
#[derive(Clone)]
pub struct TabItem {
    /// Stable value reported as the strip's selection.
    pub value: String,
    /// Visible text.
    pub label: String,
    /// Optional count rendered as a badge after the label.
    pub badge: Option<String>,
    /// Optional leading icon.
    pub icon: Option<IconSource>,
    /// Disabled tabs stay visible but cannot be selected.
    pub disabled: bool,
}

impl TabItem {
    /// Creates an enabled tab.
    pub fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            badge: None,
            icon: None,
            disabled: false,
        }
    }

    /// Adds a trailing badge, typically a count.
    pub fn badge(mut self, badge: impl Into<String>) -> Self {
        self.badge = Some(badge.into());
        self
    }

    /// Adds a trailing badge only when `count` is non-zero.
    pub fn badge_count(self, count: usize) -> Self {
        if count == 0 {
            self
        } else {
            self.badge(count.to_string())
        }
    }

    /// Adds a leading icon.
    pub fn icon(mut self, icon: impl Into<IconSource>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// Marks the tab as disabled.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// Properties for [`UiDocument::tabs`].
#[derive(Clone)]
pub struct TabsProps {
    pub common: CommonProps,
    /// Tab entries, in display order.
    pub items: Vec<TabItem>,
    /// Selection used the first time the strip is registered.
    pub selected: String,
    /// Colour of the underline under the active tab.
    pub accent: Color32,
}

impl TabsProps {
    /// Creates a tab strip. The first item is selected by default.
    pub fn new(id: impl Into<ElementId>, items: impl IntoIterator<Item = TabItem>) -> Self {
        let items: Vec<TabItem> = items.into_iter().collect();
        let selected = items
            .first()
            .map(|item| item.value.clone())
            .unwrap_or_default();
        Self {
            common: CommonProps::new(id),
            items,
            selected,
            accent: Color32::from_rgb(96, 165, 250),
        }
    }

    /// Sets the selection used on first registration.
    pub fn selected(mut self, selected: impl Into<String>) -> Self {
        self.selected = selected.into();
        self
    }

    /// Sets the active-tab underline colour.
    pub fn accent(mut self, accent: Color32) -> Self {
        self.accent = accent;
        self
    }
}

/// One entry in a [`StepsProps`] progress header.
#[derive(Clone, Debug, PartialEq)]
pub struct StepItem {
    /// Visible text under or beside the marker.
    pub label: String,
    /// Whether this step is finished; finished steps show a check icon.
    pub done: bool,
}

impl StepItem {
    /// Creates an unfinished step.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            done: false,
        }
    }

    /// Marks the step finished.
    pub fn done(mut self, done: bool) -> Self {
        self.done = done;
        self
    }
}

/// Properties for [`UiDocument::steps`].
///
/// A stepper reports *where a multi-part task stands* — which is different
/// from a tab strip, where every destination is equally available at any
/// time. Steps are therefore not clickable by default: a step becomes
/// reachable only once it is done or is the current one.
#[derive(Clone, Debug, PartialEq)]
pub struct StepsProps {
    pub common: CommonProps,
    /// Steps in order.
    pub items: Vec<StepItem>,
    /// Zero-based index of the step in progress.
    pub current: usize,
    /// Whether finished and current steps can be clicked to navigate back.
    pub navigable: bool,
    /// Colour used for the current step and completed markers.
    pub accent: Color32,
}

impl StepsProps {
    /// Creates a stepper positioned at its first step.
    pub fn new(id: impl Into<ElementId>, items: impl IntoIterator<Item = StepItem>) -> Self {
        Self {
            common: CommonProps::new(id),
            items: items.into_iter().collect(),
            current: 0,
            navigable: true,
            accent: Color32::from_rgb(96, 165, 250),
        }
    }

    /// Sets the step in progress.
    pub fn current(mut self, current: usize) -> Self {
        self.current = current;
        self
    }

    /// Enables or disables click-to-navigate on reachable steps.
    pub fn navigable(mut self, navigable: bool) -> Self {
        self.navigable = navigable;
        self
    }

    /// Sets the accent colour.
    pub fn accent(mut self, accent: Color32) -> Self {
        self.accent = accent;
        self
    }
}

impl UiDocument {
    /// Renders a card surface and its contents.
    ///
    /// The document is handed back to the body so a card can contain further
    /// components without the caller juggling a second borrow.
    pub fn card<R>(
        &mut self,
        ui: &mut Ui,
        props: CardProps,
        add_contents: impl FnOnce(&mut Ui, &mut UiDocument) -> R,
    ) -> Option<R> {
        self.prepare(&props.common, ElementKind::Card, Value::None, vec![]);
        if !props.common.visible {
            return None;
        }
        if props.gap > 0.0 {
            ui.add_space(props.gap);
        }
        let style = props.common.style.clone();
        let rounding = style.rounding.unwrap_or(0.0);
        let frame = style.to_frame(&self.theme).show(ui, |ui| {
            if let Some(width) = style.width {
                ui.set_width(width);
            }
            add_contents(ui, self)
        });
        if let Some(accent) = props.accent {
            // Painted after the frame so it sits on top of the fill, and
            // inset by the corner radius so it follows the rounded edge
            // instead of poking out of it.
            let rect = frame.response.rect;
            let bar = egui::Rect::from_min_max(
                egui::pos2(rect.left(), rect.top() + rounding),
                egui::pos2(rect.left() + 2.5, rect.bottom() - rounding),
            );
            ui.painter().rect_filled(bar, 0.0, accent);
        }
        Some(frame.inner)
    }

    /// Renders a collapsible section, returning its contents' result when open.
    ///
    /// The header is a full-width click target with an SVG chevron, matching
    /// how platform settings panes behave: the whole row toggles, not just
    /// the arrow.
    pub fn disclosure<R>(
        &mut self,
        ui: &mut Ui,
        props: DisclosureProps,
        add_contents: impl FnOnce(&mut Ui, &mut UiDocument) -> R,
    ) -> Option<R> {
        let id = props.common.id.clone();
        let before = self.prepare(
            &props.common,
            ElementKind::Disclosure,
            Value::Bool(props.default_open),
            vec![],
        );
        if !props.common.visible {
            return None;
        }
        let mut open = before.as_bool().unwrap_or(props.default_open);
        let enabled = props.common.enabled;
        let title = props.title.clone();
        let summary = props.summary.clone();

        let text_primary = self.theme.color(ThemeToken::TextPrimary);
        let text_weak = self.theme.color(ThemeToken::TextWeak);
        let mut response = ui
            .scope(|ui| {
                let row = ui.horizontal(|ui| {
                    ui.set_min_width(ui.available_width());
                    ui.add(icon_image(
                        &Icon::disclosure(open).into(),
                        CHROME_ICON_SIZE,
                        text_primary,
                    ));
                    ui.label(RichText::new(title).strong().color(text_primary));
                    if let Some(summary) = summary {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(RichText::new(summary).small().color(text_weak));
                        });
                    }
                });
                // Interact with the whole row so the header behaves like one
                // control rather than a chevron next to some text.
                ui.interact(
                    row.response.rect,
                    ui.make_persistent_id(("ui-document-disclosure", id.clone())),
                    if enabled {
                        Sense::click()
                    } else {
                        Sense::hover()
                    },
                )
            })
            .inner;
        if response.clicked() {
            open = !open;
            response.mark_changed();
        }
        if let Some(tooltip) = &props.common.tooltip {
            response = response.on_hover_text(tooltip.clone());
        }
        self.observe(
            &id,
            &before,
            Value::Bool(open),
            &response,
            &[EventType::Change],
        );
        if !open {
            return None;
        }
        Some(
            ui.indent(("ui-document-disclosure-body", id), |ui| {
                add_contents(ui, self)
            })
            .inner,
        )
    }

    /// Renders a tab strip and returns the selected value.
    ///
    /// Selection is retained by ID, so the caller can read it back with
    /// `document.text(id)` instead of threading a field through its state.
    pub fn tabs(&mut self, ui: &mut Ui, props: TabsProps) -> String {
        let id = props.common.id.clone();
        let before = self.prepare(
            &props.common,
            ElementKind::Tabs,
            Value::String(props.selected.clone()),
            vec![],
        );
        let mut selected = before
            .as_str()
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| props.selected.clone());
        // A retained selection can name a tab that no longer exists after a
        // dynamic list changes; fall back rather than render nothing active.
        if !props.items.iter().any(|item| item.value == selected) {
            selected = props.selected.clone();
        }
        if !props.common.visible {
            return selected;
        }

        let mut clicked = None;
        let response = ui
            .horizontal(|ui| {
                for item in &props.items {
                    let active = item.value == selected;
                    let item_response = self.tab_button(ui, item, active, props.accent);
                    if item_response.clicked() && !item.disabled {
                        clicked = Some(item.value.clone());
                    }
                }
            })
            .response;
        if let Some(value) = clicked {
            selected = value;
        }
        let mut response = response;
        if before.as_str() != Some(selected.as_str()) {
            response.mark_changed();
        }
        self.observe(
            &id,
            &before,
            Value::String(selected.clone()),
            &response,
            &[EventType::Change],
        );
        selected
    }

    fn tab_button(
        &mut self,
        ui: &mut Ui,
        item: &TabItem,
        active: bool,
        accent: Color32,
    ) -> Response {
        let text_color = if active {
            self.theme.color(ThemeToken::TextPrimary)
        } else {
            self.theme.color(ThemeToken::TextSecondary)
        };
        let hover_fill = self.theme.color(ThemeToken::SurfaceHover);
        let response = ui
            .scope(|ui| {
                let inner = Frame::none()
                    .inner_margin(Margin::symmetric(8.0, 4.0))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            if let Some(icon) = &item.icon {
                                ui.add(icon_image(icon, CHROME_ICON_SIZE, text_color));
                            }
                            ui.label(RichText::new(&item.label).color(text_color));
                            if let Some(badge) = &item.badge {
                                badge_chip(ui, badge, accent);
                            }
                        });
                    })
                    .response;
                let sense = if item.disabled {
                    Sense::hover()
                } else {
                    Sense::click()
                };
                let response = ui.interact(
                    inner.rect,
                    ui.make_persistent_id(("ui-document-tab", &item.value)),
                    sense,
                );
                if active {
                    // Underline rather than a filled pill: it marks the
                    // active tab without competing with the content below.
                    let rect = inner.rect;
                    ui.painter().hline(
                        rect.x_range(),
                        rect.bottom() - 1.0,
                        Stroke::new(2.0_f32, accent),
                    );
                } else if response.hovered() {
                    ui.painter().rect_filled(inner.rect, 4.0, hover_fill);
                }
                response
            })
            .inner;
        response
    }

    /// Renders a step progress header and returns the step the user selected,
    /// or `None` when nothing was clicked this frame.
    ///
    /// Completed steps show a check icon; the current one shows its number on
    /// an accent marker; later ones stay dimmed and unreachable, because a
    /// stepper's whole purpose is to say which parts are not yet available.
    pub fn steps(&mut self, ui: &mut Ui, props: StepsProps) -> Option<usize> {
        let id = props.common.id.clone();
        let current = props.current.min(props.items.len().saturating_sub(1));
        let before = self.prepare(
            &props.common,
            ElementKind::Steps,
            Value::Number(current as f64),
            vec![],
        );
        if !props.common.visible || props.items.is_empty() {
            return None;
        }
        let mut selected = None;
        let response = ui
            .horizontal(|ui| {
                for (index, item) in props.items.iter().enumerate() {
                    if index > 0 {
                        step_connector(ui, index <= current, props.accent);
                    }
                    let reachable =
                        props.navigable && props.common.enabled && (item.done || index <= current);
                    if step_marker(ui, &id, index, item, current, reachable, props.accent) {
                        selected = Some(index);
                    }
                }
            })
            .response;
        self.observe(
            &id,
            &before,
            Value::Number(selected.unwrap_or(current) as f64),
            &response,
            &[EventType::Change],
        );
        selected
    }
}

/// Small pill used for tab counts. Kept private: the public equivalent is
/// [`super::BadgeProps`], which this deliberately mirrors in appearance.
fn badge_chip(ui: &mut Ui, text: &str, fill: Color32) {
    Frame::none()
        .fill(fill.gamma_multiply(0.28))
        .rounding(7.0)
        .inner_margin(Margin::symmetric(5.0, 1.0))
        .show(ui, |ui| {
            ui.label(RichText::new(text).small().color(fill));
        });
}

fn step_connector(ui: &mut Ui, filled: bool, accent: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(18.0, STEP_MARKER_SIZE), Sense::hover());
    let color = if filled {
        accent
    } else {
        ui.visuals().weak_text_color().gamma_multiply(0.5)
    };
    ui.painter()
        .hline(rect.x_range(), rect.center().y, Stroke::new(1.5_f32, color));
}

fn step_marker(
    ui: &mut Ui,
    id: &ElementId,
    index: usize,
    item: &StepItem,
    current: usize,
    reachable: bool,
    accent: Color32,
) -> bool {
    let active = index == current;
    let inactive_color = ui.visuals().weak_text_color();
    let color = if item.done || active {
        accent
    } else {
        inactive_color
    };
    let number_on_active = ui.visuals().extreme_bg_color;
    let response = ui
        .horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(STEP_MARKER_SIZE), Sense::hover());
            let painter = ui.painter();
            if active {
                painter.circle_filled(rect.center(), STEP_MARKER_SIZE / 2.0, color);
            } else {
                painter.circle_stroke(
                    rect.center(),
                    STEP_MARKER_SIZE / 2.0,
                    Stroke::new(1.5_f32, color),
                );
            }
            if item.done && !active {
                let icon_rect =
                    egui::Rect::from_center_size(rect.center(), Vec2::splat(CHROME_ICON_SIZE));
                icon_image(&Icon::Check.into(), CHROME_ICON_SIZE, color).paint_at(ui, icon_rect);
            } else {
                let number_color = if active { number_on_active } else { color };
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    (index + 1).to_string(),
                    egui::FontId::proportional(11.0),
                    number_color,
                );
            }
            ui.label(RichText::new(&item.label).small().color(color));
        })
        .response;
    if !reachable {
        return false;
    }
    ui.interact(
        response.rect,
        ui.make_persistent_id(("ui-document-step", id, index)),
        Sense::click(),
    )
    .clicked()
}
