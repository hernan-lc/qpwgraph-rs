//! Navigation rail and compact graph controls.

use super::components::{
    document_selectable_label, document_sidebar_icon_button, document_sidebar_icon_button_enabled,
    document_sidebar_icon_toggle_button, document_sidebar_nav_icon_button,
    document_text_input_sized,
};
use super::shared::{
    apply_panel_text_scale, fresh_scroll_area, media_filter_key, NAV_RAIL_WIDTH, PANEL_FILL,
};
use crate::app::QpwgraphApp;
use crate::icons::Icon;
use eframe::egui::{self, Ui};
use pw_graph_ui::{ConnectMode, MediaFilter};

impl QpwgraphApp {
    pub(crate) fn show_gui_panels(&mut self, ctx: &egui::Context) {
        if !self.any_modal_open() || self.show_preferences {
            egui::SidePanel::left("navigation")
                .resizable(false)
                .exact_width(NAV_RAIL_WIDTH)
                .frame(egui::Frame::none().fill(PANEL_FILL).inner_margin(6.0))
                .show(ctx, |ui| {
                    apply_panel_text_scale(ui, self.config.panel_text_scale);
                    self.show_navigation(ui)
                });
        }
    }

    fn show_navigation(&mut self, ui: &mut Ui) {
        let rail_max_height = ui.available_height();
        fresh_scroll_area("nav-rail-scroll", rail_max_height).show(ui, |ui| {
            ui.vertical_centered(|ui| {
                if document_sidebar_nav_icon_button(
                    &mut self.ui_document,
                    ui,
                    "nav.preferences",
                    Icon::Settings,
                    self.show_preferences,
                    self.i18n.text("nav.preferences"),
                    self.i18n.text("help.navigation_preferences"),
                ) {
                    if self.show_preferences {
                        self.show_preferences = false;
                    } else {
                        self.show_preferences = true;
                        self.show_shortcuts = false;
                        self.effect_gallery = None;
                        self.preferences_scroll_epoch =
                            self.preferences_scroll_epoch.wrapping_add(1);
                    }
                }
                if document_sidebar_nav_icon_button(
                    &mut self.ui_document,
                    ui,
                    "nav.shortcuts",
                    Icon::Help,
                    self.show_shortcuts,
                    self.i18n.text("nav.shortcuts"),
                    self.i18n.text("help.navigation_shortcuts"),
                ) {
                    self.toggle_shortcuts();
                }
                self.show_sidebar_actions(ui);
            });
        });
    }

    fn show_sidebar_actions(&mut self, ui: &mut Ui) {
        let search_placeholder = self.i18n.text("search.placeholder");
        let (_, search_value) = document_text_input_sized(
            &mut self.ui_document,
            ui,
            "sidebar.search",
            &self.config.graph_search,
            String::new(),
            Some(search_placeholder),
            Some(58.0),
        );
        self.config.graph_search = search_value;
        ui.add_space(4.0);
        if document_sidebar_icon_toggle_button(
            &mut self.ui_document,
            ui,
            "sidebar.minimap",
            Icon::Minimap,
            self.canvas.minimap_visible,
            self.i18n.text("toolbar.minimap"),
            self.i18n.text("help.minimap"),
        ) {
            self.canvas.minimap_visible = !self.canvas.minimap_visible;
        }
        if self.config.toolbar {
            if document_sidebar_icon_button(
                &mut self.ui_document,
                ui,
                "sidebar.refresh",
                Icon::Refresh,
                self.i18n.text("toolbar.refresh"),
                self.i18n.text("help.refresh"),
            ) {
                self.refresh_graph();
            }
            if document_sidebar_icon_button_enabled(
                &mut self.ui_document,
                ui,
                "sidebar.undo",
                Icon::Undo,
                self.i18n.text("toolbar.undo"),
                self.i18n.text("help.undo"),
                self.commands.can_undo(),
            ) {
                self.undo();
            }
            if document_sidebar_icon_button_enabled(
                &mut self.ui_document,
                ui,
                "sidebar.redo",
                Icon::Redo,
                self.i18n.text("toolbar.redo"),
                self.i18n.text("help.redo"),
                self.commands.can_redo(),
            ) {
                self.redo();
            }
            if document_sidebar_icon_button(
                &mut self.ui_document,
                ui,
                "sidebar.history",
                Icon::Timer,
                self.i18n.text("toolbar.history"),
                self.i18n.text("help.history"),
            ) {
                self.toggle_history();
            }
            let easy = self.canvas.connect_mode == ConnectMode::Easy;
            let connect_mode_label = self.i18n.text(if easy {
                "toolbar.connect_mode_easy"
            } else {
                "toolbar.connect_mode_advanced"
            });
            let connect_mode_help = self.i18n.text(if easy {
                "help.connect_mode_easy"
            } else {
                "help.connect_mode_advanced"
            });
            if document_sidebar_icon_toggle_button(
                &mut self.ui_document,
                ui,
                "sidebar.connect_mode",
                Icon::Connect,
                easy,
                connect_mode_label,
                connect_mode_help,
            ) {
                self.canvas.connect_mode = if easy {
                    ConnectMode::Advanced
                } else {
                    ConnectMode::Easy
                };
            }
            if document_sidebar_icon_button(
                &mut self.ui_document,
                ui,
                "sidebar.arrange",
                Icon::Repel,
                self.i18n.text("inspector.arrange_nodes"),
                self.i18n.text("help.arrange_nodes"),
            ) {
                self.arrange_nodes();
            }
            self.show_sort_controls(ui);
            if self.canvas.selected_link().is_some()
                && document_sidebar_icon_button(
                    &mut self.ui_document,
                    ui,
                    "sidebar.disconnect-selected",
                    Icon::Delete,
                    self.i18n.text("toolbar.disconnect"),
                    self.i18n.text("help.disconnect_link"),
                )
            {
                let links = self.canvas.selected_links(self.driver.graph());
                self.disconnect_many(links);
            }
            if !self.driver.graph().links.is_empty()
                && document_sidebar_icon_button(
                    &mut self.ui_document,
                    ui,
                    "sidebar.disconnect-all",
                    Icon::Delete,
                    self.i18n.text("toolbar.disconnect_all"),
                    self.i18n.text("help.disconnect_all"),
                )
            {
                self.disconnect_all();
            }
        }
        if self.config.patchbay_toolbar {
            if document_sidebar_icon_button(
                &mut self.ui_document,
                ui,
                "sidebar.save",
                Icon::Save,
                self.i18n.text("toolbar.save_patchbay"),
                self.i18n.text("help.save_patchbay"),
            ) {
                self.save_patchbay();
            }
            if document_sidebar_icon_button(
                &mut self.ui_document,
                ui,
                "sidebar.load",
                Icon::Load,
                self.i18n.text("toolbar.load_patchbay"),
                self.i18n.text("help.load_patchbay"),
            ) {
                self.load_patchbay();
            }
            if document_sidebar_icon_button(
                &mut self.ui_document,
                ui,
                "sidebar.snapshot",
                Icon::Snapshot,
                self.i18n.text("toolbar.snapshot"),
                self.i18n.text("help.snapshot"),
            ) {
                self.snapshot_patchbay();
            }
            if document_sidebar_icon_button(
                &mut self.ui_document,
                ui,
                "sidebar.activate",
                Icon::Activate,
                self.i18n.text("toolbar.activate"),
                self.i18n.text("help.activate"),
            ) {
                self.activate_patchbay();
            }
        }
        self.show_effect_controls(ui);
        self.show_media_filter_sidebar(ui);
    }

    fn show_effect_controls(&mut self, ui: &mut Ui) {
        if document_sidebar_icon_button(
            &mut self.ui_document,
            ui,
            "sidebar.effects-add",
            Icon::Effects,
            self.i18n.text("effects.add"),
            self.i18n.text("effects.add_help"),
        ) {
            self.open_effect_gallery();
        }
    }

    /// Compact sort-by/sort-order toggles for the always-visible sidebar:
    /// icon only, current state and full explanation on hover. A `ComboBox`
    /// with an inline label doesn't fit the narrow rail width and spills
    /// text past the panel edge, so these cycle on click like every other
    /// icon button here instead.
    fn show_sort_controls(&mut self, ui: &mut Ui) {
        let sort_by_name = self.config.sort_type != "id";
        let sort_label = self
            .i18n
            .text(if sort_by_name { "sort.name" } else { "sort.id" });
        if document_sidebar_icon_toggle_button(
            &mut self.ui_document,
            ui,
            "sidebar.sort_by",
            Icon::Sort,
            sort_by_name,
            format!("{}: {}", self.i18n.text("inspector.sort_ports"), sort_label),
            self.i18n.text("help.sort_ports"),
        ) {
            self.config.sort_type = if sort_by_name { "id" } else { "name" }.into();
        }

        let descending = self.config.sort_order == "descending";
        let order_label = self.i18n.text(if descending {
            "sort.descending"
        } else {
            "sort.ascending"
        });
        if document_sidebar_icon_toggle_button(
            &mut self.ui_document,
            ui,
            "sidebar.sort_order",
            Icon::SortDirection,
            descending,
            format!(
                "{}: {}",
                self.i18n.text("inspector.sort_order"),
                order_label
            ),
            self.i18n.text("help.sort_order"),
        ) {
            self.config.sort_order = if descending {
                "ascending"
            } else {
                "descending"
            }
            .into();
        }
    }

    fn show_media_filter_sidebar(&mut self, ui: &mut Ui) {
        ui.separator();
        let current_filter = MediaFilter::parse(&self.config.media_filter);
        let filter_label = self.i18n.text(media_filter_key(current_filter));
        if document_sidebar_icon_button(
            &mut self.ui_document,
            ui,
            "sidebar.media_filter",
            Icon::Filter,
            format!(
                "{}: {}",
                self.i18n.text("toolbar.media_filter"),
                filter_label
            ),
            self.i18n.text("help.media_filter"),
        ) {
            let next_index = MediaFilter::ALL
                .iter()
                .position(|filter| *filter == current_filter)
                .map(|index| (index + 1) % MediaFilter::ALL.len())
                .unwrap_or(0);
            let selected_filter = MediaFilter::ALL[next_index];
            self.config.media_filter = selected_filter.as_str().into();
            self.canvas.media_filter = selected_filter;
        }
        ui.scope(|ui| {
            // The navigation rail is narrow; keep all four shortcuts visible
            // without letting the row stretch past its bounds.
            ui.spacing_mut().item_spacing.x = 1.0;
            ui.spacing_mut().button_padding = egui::vec2(2.0, 2.0);
            ui.spacing_mut().interact_size.x = 14.0;
            ui.horizontal(|ui| {
                for (key, filter) in [
                    ("0", MediaFilter::All),
                    ("1", MediaFilter::Audio),
                    ("2", MediaFilter::Video),
                    ("3", MediaFilter::Midi),
                ] {
                    let id = format!("sidebar.media_filter.shortcut.{key}");
                    if document_selectable_label(
                        &mut self.ui_document,
                        ui,
                        &id,
                        current_filter == filter,
                        key,
                        format!("{}: {}", key, self.i18n.text(media_filter_key(filter))),
                    ) {
                        self.set_media_filter(filter);
                    }
                }
            });
        });
    }
}
