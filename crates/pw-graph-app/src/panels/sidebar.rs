//! Navigation rail and compact graph controls.

use super::shared::{
    apply_panel_text_scale, fresh_scroll_area, media_filter_key, NAV_RAIL_WIDTH, PANEL_FILL,
};
use crate::app::QpwgraphApp;
use crate::icons::{
    sidebar_icon_button, sidebar_icon_button_enabled, sidebar_icon_toggle_button,
    sidebar_nav_icon_button, Icon,
};
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
                if sidebar_nav_icon_button(
                    ui,
                    "nav.preferences",
                    Icon::Settings,
                    self.show_preferences,
                    self.t("nav.preferences"),
                    self.t("help.navigation_preferences"),
                ) {
                    if self.show_preferences {
                        self.show_preferences = false;
                    } else {
                        self.show_preferences = true;
                        self.show_shortcuts = false;
                        self.preferences_scroll_epoch =
                            self.preferences_scroll_epoch.wrapping_add(1);
                    }
                }
                if sidebar_nav_icon_button(
                    ui,
                    "nav.shortcuts",
                    Icon::Help,
                    self.show_shortcuts,
                    self.t("nav.shortcuts"),
                    self.t("help.navigation_shortcuts"),
                ) {
                    self.toggle_shortcuts();
                }
                self.show_sidebar_actions(ui);
            });
        });
    }

    fn show_sidebar_actions(&mut self, ui: &mut Ui) {
        let search_placeholder = self.t("search.placeholder");
        let search = ui.add(
            egui::TextEdit::singleline(&mut self.config.graph_search)
                .desired_width(58.0)
                .hint_text(search_placeholder),
        );
        search.on_hover_text(self.t("search.help"));
        ui.add_space(4.0);
        if sidebar_icon_toggle_button(
            ui,
            "sidebar.minimap",
            Icon::Minimap,
            self.canvas.minimap_visible,
            self.t("toolbar.minimap"),
            self.t("help.minimap"),
        ) {
            self.canvas.minimap_visible = !self.canvas.minimap_visible;
        }
        if self.config.toolbar {
            if sidebar_icon_button(
                ui,
                "sidebar.refresh",
                Icon::Refresh,
                self.t("toolbar.refresh"),
                self.t("help.refresh"),
            ) {
                self.refresh_graph();
            }
            if sidebar_icon_button_enabled(
                ui,
                "sidebar.undo",
                Icon::Undo,
                self.t("toolbar.undo"),
                self.t("help.undo"),
                self.commands.can_undo(),
            ) {
                self.undo();
            }
            if sidebar_icon_button_enabled(
                ui,
                "sidebar.redo",
                Icon::Redo,
                self.t("toolbar.redo"),
                self.t("help.redo"),
                self.commands.can_redo(),
            ) {
                self.redo();
            }
            if sidebar_icon_button(
                ui,
                "sidebar.history",
                Icon::Timer,
                self.t("toolbar.history"),
                self.t("help.history"),
            ) {
                self.toggle_history();
            }
            let easy = self.canvas.connect_mode == ConnectMode::Easy;
            let connect_mode_label = self.t(if easy {
                "toolbar.connect_mode_easy"
            } else {
                "toolbar.connect_mode_advanced"
            });
            let connect_mode_help = self.t(if easy {
                "help.connect_mode_easy"
            } else {
                "help.connect_mode_advanced"
            });
            if sidebar_icon_toggle_button(
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
            if sidebar_icon_button(
                ui,
                "sidebar.arrange",
                Icon::Repel,
                self.t("inspector.arrange_nodes"),
                self.t("help.arrange_nodes"),
            ) {
                self.arrange_nodes();
            }
            self.show_sort_controls(ui);
            if self.canvas.selected_link().is_some()
                && sidebar_icon_button(
                    ui,
                    "sidebar.disconnect-selected",
                    Icon::Delete,
                    self.t("toolbar.disconnect"),
                    self.t("help.disconnect_link"),
                )
            {
                let links = self.canvas.selected_links(self.driver.graph());
                self.disconnect_many(links);
            }
            if !self.driver.graph().links.is_empty()
                && sidebar_icon_button(
                    ui,
                    "sidebar.disconnect-all",
                    Icon::Delete,
                    self.t("toolbar.disconnect_all"),
                    self.t("help.disconnect_all"),
                )
            {
                self.disconnect_all();
            }
        }
        if self.config.patchbay_toolbar {
            if sidebar_icon_button(
                ui,
                "sidebar.save",
                Icon::Save,
                self.t("toolbar.save_patchbay"),
                self.t("help.save_patchbay"),
            ) {
                self.save_patchbay();
            }
            if sidebar_icon_button(
                ui,
                "sidebar.load",
                Icon::Load,
                self.t("toolbar.load_patchbay"),
                self.t("help.load_patchbay"),
            ) {
                self.load_patchbay();
            }
            if sidebar_icon_button(
                ui,
                "sidebar.snapshot",
                Icon::Snapshot,
                self.t("toolbar.snapshot"),
                self.t("help.snapshot"),
            ) {
                self.snapshot_patchbay();
            }
            if sidebar_icon_button(
                ui,
                "sidebar.activate",
                Icon::Activate,
                self.t("toolbar.activate"),
                self.t("help.activate"),
            ) {
                self.activate_patchbay();
            }
        }
        self.show_media_filter_sidebar(ui);
    }

    /// Compact sort-by/sort-order toggles for the always-visible sidebar:
    /// icon only, current state and full explanation on hover. A `ComboBox`
    /// with an inline label doesn't fit the narrow rail width and spills
    /// text past the panel edge, so these cycle on click like every other
    /// icon button here instead.
    fn show_sort_controls(&mut self, ui: &mut Ui) {
        let sort_by_name = self.config.sort_type != "id";
        let sort_label = self.t(if sort_by_name { "sort.name" } else { "sort.id" });
        if sidebar_icon_toggle_button(
            ui,
            "sidebar.sort_by",
            Icon::Sort,
            sort_by_name,
            format!("{}: {}", self.t("inspector.sort_ports"), sort_label),
            self.t("help.sort_ports"),
        ) {
            self.config.sort_type = if sort_by_name { "id" } else { "name" }.into();
        }

        let descending = self.config.sort_order == "descending";
        let order_label = self.t(if descending {
            "sort.descending"
        } else {
            "sort.ascending"
        });
        if sidebar_icon_toggle_button(
            ui,
            "sidebar.sort_order",
            Icon::SortDirection,
            descending,
            format!("{}: {}", self.t("inspector.sort_order"), order_label),
            self.t("help.sort_order"),
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
        let filter_label = self.t(media_filter_key(current_filter));
        if sidebar_icon_button(
            ui,
            "sidebar.media_filter",
            Icon::Filter,
            format!("{}: {}", self.t("toolbar.media_filter"), filter_label),
            self.t("help.media_filter"),
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
                    let response = ui.selectable_label(current_filter == filter, key);
                    if response
                        .on_hover_text(format!("{}: {}", key, self.t(media_filter_key(filter))))
                        .clicked()
                    {
                        self.set_media_filter(filter);
                    }
                }
            });
        });
    }
}
