use crate::app::QpwgraphApp;
use crate::icons::{
    icon_button, icon_checkbox, icon_label, sidebar_icon_button, sidebar_icon_button_enabled,
    sidebar_icon_toggle_button, sidebar_nav_icon_button, Icon,
};
use egui::{Color32, RichText, Stroke, Ui};
use pw_graph_backend::MeterPolicy;
use pw_graph_i18n::Locale;
use pw_graph_ui::{ConnectMode, MediaFilter};

const PANEL_FILL: Color32 = Color32::from_rgb(25, 29, 36);
const SECTION_FILL: Color32 = Color32::from_rgb(30, 35, 43);
const SECTION_STROKE: Color32 = Color32::from_rgb(59, 70, 84);
const NAV_RAIL_WIDTH: f32 = 76.0;
const FULL_PANEL_MARGIN: f32 = 8.0;

fn apply_panel_text_scale(ui: &mut Ui, scale: f32) {
    let scale = scale.clamp(0.80, 2.0);
    for font_id in ui.style_mut().text_styles.values_mut() {
        font_id.size *= scale;
    }
    ui.spacing_mut().item_spacing = egui::vec2(8.0, 7.0);
    ui.spacing_mut().button_padding = egui::vec2(8.0, 5.0);
}

fn panel_section(ui: &mut Ui, title: String, contents: impl FnOnce(&mut Ui)) {
    let available_width = ui.available_width();
    egui::Frame::group(ui.style())
        .fill(SECTION_FILL)
        .stroke(Stroke::new(1.0_f32, SECTION_STROKE))
        .inner_margin(9.0)
        .show(ui, |ui| {
            ui.set_min_width((available_width - 18.0).max(0.0));
            ui.label(
                RichText::new(title)
                    .strong()
                    .color(Color32::from_rgb(205, 216, 230)),
            );
            ui.add_space(5.0);
            contents(ui);
        });
    ui.add_space(8.0);
}

fn scale_slider(ui: &mut Ui, id: &str, value: &mut f32, label: String, help: String) {
    ui.push_id(("text-scale", id), |ui| {
        ui.horizontal(|ui| {
            ui.label(label);
            let response = ui.add(
                egui::Slider::new(value, 0.80..=2.0)
                    .step_by(0.05)
                    .show_value(false),
            );
            response.on_hover_text(help);
            ui.label(RichText::new(format!("{:.0}%", *value * 100.0)).weak());
        });
    });
}

fn media_filter_key(filter: MediaFilter) -> &'static str {
    match filter {
        MediaFilter::All => "filter.all",
        MediaFilter::Audio => "filter.audio",
        MediaFilter::Video => "filter.video",
        MediaFilter::Midi => "filter.midi",
    }
}

fn shortcut_row(ui: &mut Ui, keys: &str, description: String) {
    ui.label(RichText::new(keys).strong().monospace());
    ui.label(description);
    ui.end_row();
}

fn shortcut_matches_query(keys: &str, description: &str, query: &str) -> bool {
    query.is_empty()
        || keys.to_lowercase().contains(query)
        || description.to_lowercase().contains(query)
}

struct ShortcutEntry {
    keys: &'static str,
    description_key: &'static str,
}

const SHORTCUT_ENTRIES: &[ShortcutEntry] = &[
    ShortcutEntry {
        keys: "F1",
        description_key: "shortcuts.help",
    },
    ShortcutEntry {
        keys: "Esc",
        description_key: "shortcuts.close_cancel",
    },
    ShortcutEntry {
        keys: "Delete / Backspace",
        description_key: "shortcuts.delete_link",
    },
    ShortcutEntry {
        keys: "Ctrl/Cmd+Z",
        description_key: "shortcuts.undo",
    },
    ShortcutEntry {
        keys: "Ctrl/Cmd+Shift+Z",
        description_key: "shortcuts.redo",
    },
    ShortcutEntry {
        keys: "Ctrl/Cmd+Y",
        description_key: "shortcuts.redo",
    },
    ShortcutEntry {
        keys: "Ctrl/Cmd+S",
        description_key: "shortcuts.save_config",
    },
    ShortcutEntry {
        keys: "Ctrl/Cmd+Shift+S",
        description_key: "shortcuts.save_patchbay",
    },
    ShortcutEntry {
        keys: "Ctrl/Cmd+O",
        description_key: "shortcuts.load_patchbay",
    },
    ShortcutEntry {
        keys: "R",
        description_key: "shortcuts.refresh",
    },
    ShortcutEntry {
        keys: "A",
        description_key: "shortcuts.arrange",
    },
    ShortcutEntry {
        keys: "T",
        description_key: "shortcuts.thumbnail",
    },
    ShortcutEntry {
        keys: "Arrow keys",
        description_key: "shortcuts.pan_keyboard",
    },
    ShortcutEntry {
        keys: "0",
        description_key: "shortcuts.filter_all",
    },
    ShortcutEntry {
        keys: "1",
        description_key: "shortcuts.filter_audio",
    },
    ShortcutEntry {
        keys: "2",
        description_key: "shortcuts.filter_video",
    },
    ShortcutEntry {
        keys: "3",
        description_key: "shortcuts.filter_midi",
    },
    ShortcutEntry {
        keys: "+ / -",
        description_key: "shortcuts.zoom",
    },
    ShortcutEntry {
        keys: "Scroll",
        description_key: "shortcuts.scroll_pan",
    },
    ShortcutEntry {
        keys: "Shift+Scroll",
        description_key: "shortcuts.scroll_pan_horizontal",
    },
    ShortcutEntry {
        keys: "Ctrl/Cmd+Scroll",
        description_key: "shortcuts.scroll_zoom",
    },
];

fn meter_policy_key(policy: MeterPolicy) -> &'static str {
    match policy {
        MeterPolicy::Disabled => "meters.off",
        MeterPolicy::OnDemand => "meters.on_demand",
        MeterPolicy::Always => "meters.always",
    }
}

/// Tabs inside the Preferences modal, which holds settings you configure once
/// rather than watch while working the canvas.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub(crate) enum PreferencesTab {
    #[default]
    Interface,
    Patchbay,
}

fn show_backdrop(ctx: &egui::Context, id_source: &str) -> bool {
    show_backdrop_rect(ctx, id_source, ctx.screen_rect())
}

fn show_backdrop_rect(ctx: &egui::Context, id_source: &str, rect: egui::Rect) -> bool {
    let backdrop_id = egui::Id::new(("modal-backdrop", id_source));
    // Keep the modal window above its backdrop no matter what. Clicking the
    // backdrop (to dismiss the dialog) makes egui call `move_to_top` on that
    // layer, and the reordering persists in memory — so reopening the dialog
    // would otherwise draw the backdrop over the window. Registering the
    // window as a sublayer of the backdrop re-inserts it directly above the
    // backdrop at the end of every frame, overriding any stale order.
    ctx.set_sublayer(
        egui::LayerId::new(egui::Order::Foreground, backdrop_id),
        egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new(("modal-window", id_source)),
        ),
    );
    egui::Area::new(backdrop_id)
        .order(egui::Order::Foreground)
        .fixed_pos(rect.min)
        .show(ctx, |ui| {
            let mut sense = egui::Sense::click();
            // The backdrop must receive pointer clicks, but it is not a real
            // control and must not interrupt Tab traversal inside the modal.
            sense.focusable = false;
            // Keep the backdrop as an invisible hit target. The modal still
            // owns the foreground layer and the backdrop can dismiss it, but
            // opening a dialog no longer dims the graph behind it.
            let (response, _painter) = ui.allocate_painter(rect.size(), sense);
            response
        })
        .inner
        .clicked()
}

fn preferences_rect(ctx: &egui::Context) -> egui::Rect {
    let screen = ctx.screen_rect();
    let left = screen.left() + NAV_RAIL_WIDTH + FULL_PANEL_MARGIN;
    let top = screen.top() + FULL_PANEL_MARGIN;
    let width = (screen.width() - NAV_RAIL_WIDTH - FULL_PANEL_MARGIN * 2.0).max(240.0);
    let height = (screen.height() - FULL_PANEL_MARGIN * 2.0).max(260.0);
    egui::Rect::from_min_size(egui::pos2(left, top), egui::vec2(width, height))
}

fn full_panel_window(id_source: &str, title: String, rect: egui::Rect) -> egui::Window<'static> {
    egui::Window::new(title)
        .id(egui::Id::new(("modal-window", id_source)))
        .collapsible(false)
        .resizable(false)
        .fixed_pos(rect.min)
        .fixed_size(rect.size())
        .order(egui::Order::Foreground)
}

/// Shared chrome for the remaining compact dialog: fixed size, centered,
/// non-collapsible, and always on top.
fn modal_window(id_source: &str, title: String, default_width: f32) -> egui::Window<'static> {
    egui::Window::new(title)
        .id(egui::Id::new(("modal-window", id_source)))
        .collapsible(false)
        .resizable(false)
        .default_width(default_width)
        .max_width(default_width)
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
}

/// A scroll area that always opens back at the top: reopening a dialog (or
/// switching tabs inside it) must not inherit whatever offset a previous
/// scroll session left behind under the same persisted id, or content above
/// the leftover offset silently reads as "missing".
fn fresh_scroll_area(id_salt: impl std::hash::Hash, max_height: f32) -> egui::ScrollArea {
    egui::ScrollArea::vertical()
        .id_salt(id_salt)
        .max_height(max_height)
        .auto_shrink([false, true])
}

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

    pub(crate) fn show_shortcuts_modal(&mut self, ctx: &egui::Context) {
        if !self.show_shortcuts {
            return;
        }
        if show_backdrop(ctx, "shortcuts") {
            self.close_shortcuts();
            return;
        }
        modal_window("shortcuts", self.t("shortcuts.title"), 560.0).show(ctx, |ui| {
            ui.label(RichText::new(self.t("shortcuts.hint")).weak());
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(self.t("shortcuts.search")).strong());
                let clear_width = if self.shortcut_search.is_empty() {
                    0.0
                } else {
                    ui.spacing().button_padding.x * 2.0 + 42.0
                };
                let search_width = (ui.available_width() - clear_width).max(140.0);
                let search_hint = self.t("shortcuts.search_hint");
                let search_response = ui.add_sized(
                    [search_width, ui.spacing().interact_size.y],
                    egui::TextEdit::singleline(&mut self.shortcut_search)
                        .id(egui::Id::new("shortcuts-search"))
                        .hint_text(search_hint),
                );
                if self.shortcut_focus_search
                    || ui.input(|input| input.modifiers.command && input.key_pressed(egui::Key::F))
                {
                    search_response.request_focus();
                    self.shortcut_focus_search = false;
                }
                if !self.shortcut_search.is_empty()
                    && ui.small_button(self.t("shortcuts.clear_search")).clicked()
                {
                    self.shortcut_search.clear();
                    self.shortcut_focus_search = true;
                }
            });
            ui.add_space(6.0);

            let query = self.shortcut_search.trim().to_lowercase();
            let matching_entries: Vec<_> = SHORTCUT_ENTRIES
                .iter()
                .filter_map(|entry| {
                    let description = self.t(entry.description_key);
                    shortcut_matches_query(entry.keys, &description, &query)
                        .then_some((entry.keys, description))
                })
                .collect();
            ui.label(
                RichText::new(self.tf(
                    "shortcuts.result_count",
                    &[("count", matching_entries.len().to_string())],
                ))
                .small()
                .weak(),
            );
            fresh_scroll_area(("shortcuts-scroll", self.shortcut_scroll_epoch), 420.0).show(
                ui,
                |ui| {
                    if matching_entries.is_empty() {
                        ui.label(RichText::new(self.t("shortcuts.no_results")).weak());
                    } else {
                        egui::Grid::new("shortcuts-grid")
                            .num_columns(2)
                            .spacing(egui::vec2(18.0, 7.0))
                            .show(ui, |ui| {
                                for (keys, description) in matching_entries {
                                    shortcut_row(ui, keys, description);
                                }
                            });
                    }
                },
            );
            ui.add_space(10.0);
            if self.show_close_button(ui) {
                self.close_shortcuts();
            }
        });
    }

    pub(crate) fn show_history_modal(&mut self, ctx: &egui::Context) {
        if !self.show_history {
            return;
        }
        if show_backdrop(ctx, "history") {
            self.show_history = false;
            return;
        }
        modal_window("history", self.t("history.title"), 520.0).show(ctx, |ui| {
            ui.label(RichText::new(self.t("history.hint")).weak());
            ui.add_space(8.0);
            ui.label(RichText::new(self.t("history.undoable")).strong());
            let undo_history = self.commands.undo_history();
            if undo_history.is_empty() {
                ui.label(RichText::new(self.t("history.empty")).weak());
            } else {
                for (index, entry) in undo_history.iter().enumerate() {
                    ui.label(format!("{}. {}", index + 1, entry));
                }
            }
            ui.add_space(8.0);
            ui.label(RichText::new(self.t("history.redoable")).strong());
            let redo_history = self.commands.redo_history();
            if redo_history.is_empty() {
                ui.label(RichText::new(self.t("history.empty")).weak());
            } else {
                for (index, entry) in redo_history.iter().enumerate() {
                    ui.label(format!("{}. {}", index + 1, entry));
                }
            }
            ui.add_space(10.0);
            if self.show_close_button(ui) {
                self.show_history = false;
            }
        });
    }

    /// Right-aligned Close button shared by every modal footer.
    fn show_close_button(&self, ui: &mut Ui) -> bool {
        let mut clicked = false;
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button(self.t("shortcuts.close")).clicked() {
                clicked = true;
            }
        });
        clicked
    }

    fn show_meter_controls(&mut self, ui: &mut Ui) {
        let current = MeterPolicy::parse(&self.config.audio_meters);
        let mut selected = current;
        if icon_button(
            ui,
            "meters.reset",
            Icon::Refresh,
            self.t("inspector.audio_reset"),
            self.t("help.audio_reset"),
        ) {
            self.reset_audio_config();
        }
        let response = egui::ComboBox::from_label(self.t("inspector.audio_metering_policy"))
            .selected_text(self.t(meter_policy_key(current)))
            .show_ui(ui, |ui| {
                for policy in MeterPolicy::ALL {
                    let label = self.t(meter_policy_key(policy));
                    ui.selectable_value(&mut selected, policy, label);
                }
            });
        response
            .response
            .on_hover_text(self.t("help.audio_metering_policy"));
        if selected != current {
            self.config.audio_meters = selected.as_str().to_owned();
        }

        ui.label(
            RichText::new(self.t(match selected {
                MeterPolicy::Disabled => "meters.off_hint",
                MeterPolicy::OnDemand => "meters.on_demand_hint",
                MeterPolicy::Always => "meters.always_hint",
            }))
            .small()
            .weak(),
        );

        ui.add_space(2.0);
    }

    pub(crate) fn show_preferences_modal(&mut self, ctx: &egui::Context) {
        if !self.show_preferences {
            return;
        }
        let rect = preferences_rect(ctx);
        if show_backdrop_rect(ctx, "preferences", rect) {
            self.show_preferences = false;
            return;
        }
        full_panel_window("preferences", self.t("preferences.title"), rect).show(ctx, |ui| {
            apply_panel_text_scale(ui, self.config.panel_text_scale);
            ui.horizontal(|ui| {
                for (tab, label_key) in [
                    (PreferencesTab::Interface, "screen.interface"),
                    (PreferencesTab::Patchbay, "inspector.patchbay_options"),
                ] {
                    if ui
                        .selectable_label(self.preferences_tab == tab, self.t(label_key))
                        .clicked()
                        && self.preferences_tab != tab
                    {
                        self.preferences_tab = tab;
                        self.preferences_scroll_epoch =
                            self.preferences_scroll_epoch.wrapping_add(1);
                    }
                }
            });
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(6.0);
            let scroll_id = (
                "preferences-scroll",
                self.preferences_tab,
                self.preferences_scroll_epoch,
            );
            fresh_scroll_area(scroll_id, ui.available_height().max(0.0))
                .auto_shrink([false, false])
                .show(ui, |ui| match self.preferences_tab {
                    PreferencesTab::Interface => self.show_preferences_interface_tab(ui),
                    PreferencesTab::Patchbay => self.show_preferences_patchbay_tab(ui),
                });
            ui.add_space(8.0);
            if self.show_close_button(ui) {
                self.show_preferences = false;
            }
        });
    }

    fn show_preferences_patchbay_tab(&mut self, ui: &mut Ui) {
        panel_section(ui, self.t("inspector.patchbay_options"), |ui| {
            let exclusive_label = self.t("inspector.exclusive");
            let exclusive_help = self.t("help.exclusive");
            icon_checkbox(
                ui,
                "patchbay.exclusive",
                &mut self.config.patchbay_exclusive,
                Icon::Exclusive,
                exclusive_label,
                exclusive_help,
            );
            let auto_disconnect_label = self.t("inspector.auto_disconnect");
            let auto_disconnect_help = self.t("help.auto_disconnect");
            icon_checkbox(
                ui,
                "patchbay.auto_disconnect",
                &mut self.config.patchbay_auto_disconnect,
                Icon::AutoDisconnect,
                auto_disconnect_label,
                auto_disconnect_help,
            );
            let auto_pin_label = self.t("inspector.auto_pin");
            let auto_pin_help = self.t("help.auto_pin");
            icon_checkbox(
                ui,
                "patchbay.auto_pin",
                &mut self.config.patchbay_auto_pin,
                Icon::Pin,
                auto_pin_label,
                auto_pin_help,
            );
            let patchbay_activated_before = self.config.patchbay_activated;
            let patchbay_activated_label = self.t("inspector.patchbay_activated");
            let patchbay_activated_help = self.t("help.patchbay_activated");
            icon_checkbox(
                ui,
                "patchbay.activated",
                &mut self.config.patchbay_activated,
                Icon::Timer,
                patchbay_activated_label,
                patchbay_activated_help,
            );
            if self.config.patchbay_activated && !patchbay_activated_before {
                self.activate_patchbay();
            }
        });

        let current_path = self.patchbay_file.display().to_string();
        let choose_directory = self.t("patchbay.choose_directory");
        let recent_label = self.t("patchbay.recent_files");
        let profile_label = self.t("patchbay.profile");
        let save_profile_label = self.t("patchbay.save_profile");
        panel_section(ui, self.t("patchbay.file_options"), |ui| {
            ui.horizontal(|ui| {
                ui.label(profile_label.clone());
                ui.text_edit_singleline(&mut self.profile_name);
                if ui.button(save_profile_label.clone()).clicked() {
                    let name = self.profile_name.trim().to_owned();
                    if !name.is_empty() {
                        self.config.active_patchbay_profile = name.clone();
                        self.config
                            .patchbay_profiles
                            .insert(name, self.patchbay_file.clone());
                    }
                }
            });
            let mut selected_profile = self.config.active_patchbay_profile.clone();
            let profiles: Vec<_> = self.config.patchbay_profiles.keys().cloned().collect();
            if !profiles.is_empty() {
                egui::ComboBox::from_id_salt("patchbay-profile-list")
                    .selected_text(selected_profile.clone())
                    .show_ui(ui, |ui| {
                        for profile in &profiles {
                            ui.selectable_value(&mut selected_profile, profile.clone(), profile);
                        }
                    });
                if selected_profile != self.config.active_patchbay_profile {
                    self.config.active_patchbay_profile = selected_profile.clone();
                    self.profile_name = selected_profile.clone();
                    if let Some(path) = self
                        .config
                        .patchbay_profiles
                        .get(&selected_profile)
                        .cloned()
                    {
                        self.select_patchbay_path(path);
                        let _ = self.load_patchbay_from_current();
                    }
                }
            }
            ui.label(self.tf("patchbay.current_path", &[("path", current_path.clone())]));
            if ui.button(choose_directory.clone()).clicked() {
                self.choose_patchbay_directory();
            }
            if !self.config.recent_patchbay_paths.is_empty() {
                let mut selected = self.patchbay_file.clone();
                egui::ComboBox::from_label(recent_label.clone())
                    .selected_text(selected.display().to_string())
                    .show_ui(ui, |ui| {
                        for path in &self.config.recent_patchbay_paths {
                            ui.selectable_value(
                                &mut selected,
                                path.clone(),
                                path.display().to_string(),
                            );
                        }
                    });
                if selected != self.patchbay_file {
                    self.use_recent_patchbay(selected);
                }
            }
        });

        let remove_rule = self.t("patchbay.remove_rule");
        let output_label = self.t("patchbay.output");
        let input_label = self.t("patchbay.input");
        let pinned_label = self.t("inspector.pinned");
        panel_section(ui, self.t("patchbay.connections"), |ui| {
            if self.patchbay.connections.is_empty() {
                ui.label(RichText::new(self.t("patchbay.no_connections")).weak());
            }
            let mut remove_index = None;
            for (index, connection) in self.patchbay.connections.iter_mut().enumerate() {
                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(output_label.clone());
                        ui.text_edit_singleline(&mut connection.output_node);
                        ui.label("/");
                        ui.text_edit_singleline(&mut connection.output_name);
                    });
                    ui.horizontal(|ui| {
                        ui.label(input_label.clone());
                        ui.text_edit_singleline(&mut connection.input_node);
                        ui.label("/");
                        ui.text_edit_singleline(&mut connection.input_name);
                    });
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut connection.pinned, pinned_label.clone());
                        if ui.button(remove_rule.clone()).clicked() {
                            remove_index = Some(index);
                        }
                    });
                });
            }
            if let Some(index) = remove_index {
                self.patchbay.connections.remove(index);
            }
        });
    }

    fn show_preferences_interface_tab(&mut self, ui: &mut Ui) {
        let current_locale = self.i18n.locale();
        let mut selected_locale = current_locale;
        panel_section(ui, self.t("inspector.configuration"), |ui| {
            ui.horizontal(|ui| {
                icon_label(ui, Icon::Language, self.t("language.label"));
                let response = egui::ComboBox::from_label(self.t("language.label"))
                    .selected_text(selected_locale.native_name())
                    .show_ui(ui, |ui| {
                        for locale in Locale::ALL {
                            ui.selectable_value(&mut selected_locale, locale, locale.native_name());
                        }
                    });
                response.response.on_hover_text(self.t("help.language"));
            });
            ui.add_space(2.0);
            if icon_button(
                ui,
                "configuration.save",
                Icon::Save,
                self.t("inspector.save_configuration"),
                self.t("help.save_configuration"),
            ) {
                self.save_config_now();
            }
            ui.label(
                RichText::new(self.tf(
                    "inspector.config_path",
                    &[("path", self.config_file.display().to_string())],
                ))
                .small()
                .weak(),
            );
        });
        if selected_locale != current_locale {
            self.i18n.set_locale(selected_locale);
            self.config.language = selected_locale.code().to_owned();
            self.status = self.t("status.language_changed");
        }

        panel_section(ui, self.t("inspector.interface"), |ui| {
            let toolbar_label = self.t("inspector.toolbar_visible");
            let toolbar_help = self.t("help.toolbar_visible");
            icon_checkbox(
                ui,
                "interface.toolbar",
                &mut self.config.toolbar,
                Icon::Toolbar,
                toolbar_label,
                toolbar_help,
            );
            let statusbar_label = self.t("inspector.statusbar_visible");
            let statusbar_help = self.t("help.statusbar_visible");
            icon_checkbox(
                ui,
                "interface.statusbar",
                &mut self.config.statusbar,
                Icon::Statusbar,
                statusbar_label,
                statusbar_help,
            );
            let patchbay_toolbar_label = self.t("inspector.patchbay_toolbar_visible");
            let patchbay_toolbar_help = self.t("help.patchbay_toolbar_visible");
            icon_checkbox(
                ui,
                "interface.patchbay_toolbar",
                &mut self.config.patchbay_toolbar,
                Icon::Patchbay,
                patchbay_toolbar_label,
                patchbay_toolbar_help,
            );
        });

        panel_section(ui, self.t("inspector.behavior"), |ui| {
            let repel_label = self.t("inspector.repel_overlaps");
            let repel_help = self.t("help.repel_overlaps");
            icon_checkbox(
                ui,
                "behavior.repel",
                &mut self.config.repel_overlapping_nodes,
                Icon::Repel,
                repel_label,
                repel_help,
            );
            let through_label = self.t("inspector.connect_through");
            let through_help = self.t("help.connect_through");
            icon_checkbox(
                ui,
                "behavior.connect_through",
                &mut self.config.connect_through_nodes,
                Icon::Connect,
                through_label,
                through_help,
            );
            let thumbnail_label = self.t("inspector.thumbnail_view");
            let thumbnail_help = self.t("help.thumbnail_view");
            icon_checkbox(
                ui,
                "behavior.thumbnail",
                &mut self.canvas.thumbnail_mode,
                Icon::Thumbnail,
                thumbnail_label,
                thumbnail_help,
            );
        });

        panel_section(ui, self.t("inspector.audio_metering"), |ui| {
            self.show_meter_controls(ui);
        });

        panel_section(ui, self.t("inspector.typography"), |ui| {
            if ui
                .small_button(self.t("inspector.typography_recommended"))
                .on_hover_text(self.t("help.typography_recommended"))
                .clicked()
            {
                self.config.ui_text_scale = 1.10;
                self.config.panel_text_scale = 1.20;
                self.config.node_text_scale = 1.15;
            }
            let ui_text_label = self.t("inspector.ui_text_scale");
            let ui_text_help = self.t("help.ui_text_scale");
            scale_slider(
                ui,
                "ui",
                &mut self.config.ui_text_scale,
                ui_text_label,
                ui_text_help,
            );
            let panel_text_label = self.t("inspector.panel_text_scale");
            let panel_text_help = self.t("help.panel_text_scale");
            scale_slider(
                ui,
                "panels",
                &mut self.config.panel_text_scale,
                panel_text_label,
                panel_text_help,
            );
            let node_text_label = self.t("inspector.node_text_scale");
            let node_text_help = self.t("help.node_text_scale");
            scale_slider(
                ui,
                "nodes",
                &mut self.config.node_text_scale,
                node_text_label,
                node_text_help,
            );
        });
    }
}

#[cfg(test)]
mod tests {
    use super::shortcut_matches_query;

    #[test]
    fn shortcut_search_matches_translated_descriptions() {
        assert!(shortcut_matches_query(
            "Ctrl/Cmd+Z",
            "Deshacer el último cambio",
            "deshacer"
        ));
        assert!(!shortcut_matches_query(
            "Ctrl/Cmd+Z",
            "Deshacer el último cambio",
            "volumen"
        ));
    }
}
