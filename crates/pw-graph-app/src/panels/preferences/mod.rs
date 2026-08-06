//! Preferences modal: the settings you configure once and leave alone,
//! as opposed to the docked panels you watch while working the canvas.
//!
//! The modal is only a shell — a tab strip, a scroll area, and a close
//! action. Each tab's content lives in its own submodule because the two
//! tabs have nothing in common: [`interface`] is a page of independent
//! preferences read top to bottom, while [`patchbay`] is a small editor with
//! a file, a rule list, and its own [`rules`] row component.

mod interface;
mod patchbay;
mod rules;

use super::components::document_setting_switch;
use super::shared::{apply_panel_text_scale, fresh_scroll_area, show_close_button};
use crate::app::QpwgraphApp;
use crate::icons::Icon;
use eframe::egui::{self, Ui};
use pw_graph_ui::{TabItem, TabsProps, ThemeToken, UiDocument};

const PREFERENCES_DIALOG_WIDTH: f32 = 780.0;
const PREFERENCES_SCROLL_MAX_HEIGHT: f32 = 600.0;
/// Width shared by every labelled select on a Preferences page, so the
/// controls form one trailing column instead of a ragged edge.
pub(super) const PREFERENCES_SELECT_WIDTH: f32 = 260.0;
/// Filesystem paths need considerably more room than an option label.
pub(super) const PREFERENCES_PATH_SELECT_WIDTH: f32 = 520.0;

/// Tabs inside the Preferences modal, which holds settings you configure once
/// rather than watch while working the canvas.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub(crate) enum PreferencesTab {
    #[default]
    Interface,
    Patchbay,
}

/// Tab identity in the document. The enum stays the app's source of truth;
/// these map it onto the stable string values the tab strip retains.
fn preferences_tab_value(tab: PreferencesTab) -> &'static str {
    match tab {
        PreferencesTab::Interface => "interface",
        PreferencesTab::Patchbay => "patchbay",
    }
}

fn preferences_tab_from_value(value: &str) -> PreferencesTab {
    match value {
        "patchbay" => PreferencesTab::Patchbay,
        _ => PreferencesTab::Interface,
    }
}

impl QpwgraphApp {
    pub(crate) fn show_preferences_modal(&mut self, ctx: &egui::Context) {
        if !self.show_preferences {
            return;
        }
        let viewport = ctx.screen_rect();
        let dialog_width = (viewport.width() - 48.0)
            .clamp(320.0, PREFERENCES_DIALOG_WIDTH)
            .max(1.0);
        let scroll_max_height = (viewport.height() - 240.0)
            .clamp(220.0, PREFERENCES_SCROLL_MAX_HEIGHT)
            .max(1.0);
        if self.run_dialog(
            ctx,
            "preferences",
            self.i18n.text("preferences.title"),
            dialog_width,
            |app, ui, document| {
                apply_panel_text_scale(ui, app.config.panel_text_scale);
                app.show_preferences_tab_bar(document, ui);
                ui.add_space(6.0);
                ui.separator();
                ui.add_space(6.0);
                let scroll_id = (
                    "preferences-scroll",
                    app.preferences_tab,
                    app.preferences_scroll_epoch,
                );
                fresh_scroll_area(scroll_id, scroll_max_height)
                    .auto_shrink([false, false])
                    .show(ui, |ui| match app.preferences_tab {
                        PreferencesTab::Interface => {
                            app.show_preferences_interface_tab(document, ui)
                        }
                        PreferencesTab::Patchbay => app.show_preferences_patchbay_tab(document, ui),
                    });
                ui.add_space(8.0);
                if show_close_button(
                    document,
                    ui,
                    "preferences.close",
                    app.i18n.text("shortcuts.close"),
                ) {
                    app.show_preferences = false;
                }
            },
        ) {
            self.show_preferences = false;
        }
    }

    /// The same tab strip the relay panel uses, rather than a pair of bare
    /// selectable labels: two surfaces that both mean "switch page" should
    /// not need two different appearances to say it. The rule count rides on
    /// the Patchbay tab so the editor's size is visible from either page.
    fn show_preferences_tab_bar(&mut self, document: &mut UiDocument, ui: &mut Ui) {
        let accent = document.theme_color(ThemeToken::Accent);
        let rule_count = self.patchbay.connections.len();
        let selected = document.tabs(
            ui,
            TabsProps::new(
                "preferences.tabs",
                [
                    TabItem::new("interface", self.i18n.text("screen.interface"))
                        .icon(Icon::Settings.source()),
                    TabItem::new("patchbay", self.i18n.text("inspector.patchbay_options"))
                        .icon(Icon::Patchbay.source())
                        .badge_count(rule_count),
                ],
            )
            .selected(preferences_tab_value(self.preferences_tab))
            .accent(accent),
        );
        let selected = preferences_tab_from_value(&selected);
        if selected != self.preferences_tab {
            self.preferences_tab = selected;
            self.preferences_scroll_epoch = self.preferences_scroll_epoch.wrapping_add(1);
        }
    }

    /// An icon switch whose label and explanation follow the catalog's
    /// `inspector.<name>` / `help.<name>` pairing.
    ///
    /// Ten rows across the two tabs differed only in that name, and spelling
    /// both keys out at every call site buried the one word that varied under
    /// four lines of boilerplate.
    pub(super) fn preferences_switch(
        &self,
        document: &mut UiDocument,
        ui: &mut Ui,
        id: &str,
        current: bool,
        icon: Icon,
        setting_name: &str,
    ) -> bool {
        document_setting_switch(
            document,
            ui,
            id,
            current,
            icon,
            self.i18n.text(&format!("inspector.{setting_name}")),
            self.i18n.text(&format!("help.{setting_name}")),
        )
    }
}
