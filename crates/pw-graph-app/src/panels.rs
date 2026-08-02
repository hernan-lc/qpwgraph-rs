use crate::app::QpwgraphApp;
use crate::icons::{
    icon_button, icon_checkbox, icon_heading, icon_label, sidebar_icon_button,
    sidebar_icon_button_enabled, sidebar_icon_toggle_button, sidebar_nav_icon_button, Icon,
};
use egui::{Color32, RichText, Stroke, Ui};
use pw_graph_backend::MeterPolicy;
use pw_graph_command::RenameCommand;
use pw_graph_core::{NodeId, PortId};
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

fn panel_header(ui: &mut Ui, icon: Icon, title: String, hint: String) {
    ui.add_space(4.0);
    icon_heading(ui, icon, title);
    ui.add_space(1.0);
    ui.label(RichText::new(hint).weak());
    ui.add_space(5.0);
    ui.separator();
    ui.add_space(4.0);
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

fn stat_card(ui: &mut Ui, label: String, value: String) {
    egui::Frame::none()
        .fill(Color32::from_rgb(36, 43, 53))
        .rounding(5.0)
        .inner_margin(egui::Margin::symmetric(9.0, 6.0))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new(value).strong().size(18.0));
                ui.label(RichText::new(label).small().weak());
            });
        });
}

fn color_swatch(ui: &mut Ui, color: Color32, label: String) {
    egui::Frame::none()
        .fill(Color32::from_rgb(36, 43, 53))
        .rounding(5.0)
        .inner_margin(egui::Margin::symmetric(9.0, 6.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let (rect, _response) =
                    ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                ui.painter().rect_filled(rect, 2.5, color);
                ui.label(label);
            });
        });
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

fn level_db(value: f32) -> f32 {
    (20.0 * value.max(0.000001).log10()).clamp(-120.0, 0.0)
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
];

fn meter_policy_key(policy: MeterPolicy) -> &'static str {
    match policy {
        MeterPolicy::Disabled => "meters.off",
        MeterPolicy::OnDemand => "meters.on_demand",
        MeterPolicy::Always => "meters.always",
    }
}

/// The two panels that live docked next to the canvas because their content
/// is live data you want visible while working, not a one-off setting.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub(crate) enum AppScreen {
    #[default]
    Graph,
    Patchbay,
}

/// Tabs inside the Preferences modal, which holds the settings you configure
/// once rather than watch while working the canvas.
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
            let (response, painter) = ui.allocate_painter(rect.size(), sense);
            painter.rect_filled(response.rect, 0.0, Color32::from_black_alpha(120));
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

/// Shared chrome for every dialog: fixed size, centered, non-collapsible,
/// always on top. Individual dialogs only supply their title, width, and
/// body — keeps the three modals (shortcuts, preferences, diagnostics) from
/// re-declaring the same half-dozen builder calls each.
fn modal_window(id_source: &str, title: String, default_width: f32) -> egui::Window<'static> {
    egui::Window::new(title)
        .id(egui::Id::new(("modal-window", id_source)))
        .collapsible(false)
        .resizable(false)
        .default_width(default_width)
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
                .default_width(NAV_RAIL_WIDTH)
                .frame(egui::Frame::none().fill(PANEL_FILL).inner_margin(6.0))
                .show(ctx, |ui| {
                    apply_panel_text_scale(ui, self.config.panel_text_scale);
                    self.show_navigation(ui)
                });
        }

        if self.any_modal_open() || !self.dock_open {
            return;
        }
        egui::SidePanel::right("screen_panel")
            .default_width(370.0)
            .width_range(310.0..=520.0)
            .frame(egui::Frame::none().fill(PANEL_FILL).inner_margin(10.0))
            .show(ctx, |ui| {
                apply_panel_text_scale(ui, self.config.panel_text_scale);
                let scroll_id = ("inspector-scroll", self.screen, self.dock_scroll_epoch);
                fresh_scroll_area(scroll_id, ui.available_height())
                    .auto_shrink([false, false])
                    .show(ui, |ui| match self.screen {
                        AppScreen::Graph => self.show_graph_screen(ui),
                        AppScreen::Patchbay => self.show_patchbay_screen(ui),
                    });
            });
    }

    fn show_navigation(&mut self, ui: &mut Ui) {
        ui.vertical_centered(|ui| {
            let docks = [
                (AppScreen::Graph, Icon::Graph, "screen.graph"),
                (AppScreen::Patchbay, Icon::Patchbay, "screen.patchbay"),
            ];
            for (screen, icon, label) in docks {
                let help_key = match screen {
                    AppScreen::Graph => "help.navigation_graph",
                    AppScreen::Patchbay => "help.navigation_patchbay",
                };
                let selected = self.dock_open && self.screen == screen;
                if sidebar_nav_icon_button(
                    ui,
                    label,
                    icon,
                    selected,
                    self.t(label),
                    self.t(help_key),
                ) {
                    if selected {
                        self.dock_open = false;
                    } else {
                        self.screen = screen;
                        self.dock_open = true;
                        self.show_preferences = false;
                        self.show_shortcuts = false;
                        self.dock_scroll_epoch = self.dock_scroll_epoch.wrapping_add(1);
                    }
                }
            }
            ui.separator();
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
                    self.preferences_scroll_epoch = self.preferences_scroll_epoch.wrapping_add(1);
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
    }

    fn show_sidebar_actions(&mut self, ui: &mut Ui) {
        if self.config.toolbar {
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

    fn show_graph_screen(&mut self, ui: &mut Ui) {
        panel_header(
            ui,
            Icon::Graph,
            self.t("screen.graph"),
            self.t("screen.graph_hint"),
        );
        let (node_count, port_count, link_count) = self.canvas.visible_counts(self.driver.graph());
        let node_count = node_count.to_string();
        let port_count = port_count.to_string();
        let link_count = link_count.to_string();
        panel_section(ui, self.t("inspector.overview"), |ui| {
            ui.horizontal_wrapped(|ui| {
                stat_card(ui, self.t("inspector.nodes_short"), node_count);
                stat_card(ui, self.t("inspector.ports_short"), port_count);
                stat_card(ui, self.t("inspector.links_short"), link_count);
            });
        });
        if let Some(selected_link) = self.canvas.selected_link() {
            panel_section(ui, self.t("inspector.selected_link"), |ui| {
                if icon_button(
                    ui,
                    "graph.disconnect-selected",
                    Icon::Delete,
                    self.t("toolbar.disconnect"),
                    self.t("help.disconnect_link"),
                ) {
                    self.disconnect(selected_link);
                }
            });
        }

        panel_section(ui, self.t("inspector.audio_metering"), |ui| {
            self.show_meter_controls(ui);
        });

        if let Some(pinned_port) = self.canvas.pinned_meter {
            panel_section(ui, self.t("inspector.audio_monitor"), |ui| {
                self.show_meter_monitor(ui, pinned_port);
            });
        }

        if let Some(selected_node) = self.canvas.selected_node {
            panel_section(ui, self.t("inspector.rename"), |ui| {
                self.show_rename_control(ui, selected_node);
            });
        }

        panel_section(ui, self.t("inspector.layout"), |ui| {
            if ui
                .button(self.t("inspector.arrange_nodes"))
                .on_hover_text(self.t("help.arrange_nodes"))
                .clicked()
            {
                self.arrange_nodes();
            }

            let sort_by_name = self.config.sort_type != "id";
            let sort_by_name_before = sort_by_name;
            let mut sort_by_name_choice = sort_by_name;
            let sort_ports_response = egui::ComboBox::from_label(self.t("inspector.sort_ports"))
                .selected_text(if sort_by_name {
                    self.t("sort.name")
                } else {
                    self.t("sort.id")
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut sort_by_name_choice, true, self.t("sort.name"));
                    ui.selectable_value(&mut sort_by_name_choice, false, self.t("sort.id"));
                });
            sort_ports_response
                .response
                .on_hover_text(self.t("help.sort_ports"));
            if sort_by_name_choice != sort_by_name_before {
                self.config.sort_type = if sort_by_name_choice { "name" } else { "id" }.into();
            }
            let descending = self.config.sort_order == "descending";
            let mut descending_choice = descending;
            let sort_order_response = egui::ComboBox::from_label(self.t("inspector.sort_order"))
                .selected_text(if descending {
                    self.t("sort.descending")
                } else {
                    self.t("sort.ascending")
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut descending_choice, false, self.t("sort.ascending"));
                    ui.selectable_value(&mut descending_choice, true, self.t("sort.descending"));
                });
            sort_order_response
                .response
                .on_hover_text(self.t("help.sort_order"));
            if descending_choice != descending {
                self.config.sort_order = if descending_choice {
                    "descending"
                } else {
                    "ascending"
                }
                .into();
            }
        });
    }

    fn show_media_filter_sidebar(&mut self, ui: &mut Ui) {
        ui.separator();
        let current_filter = MediaFilter::parse(&self.config.media_filter);
        let mut selected_filter = current_filter;
        let response = egui::ComboBox::from_id_salt("sidebar-media-filter")
            .selected_text(self.t(media_filter_key(current_filter)))
            .width(48.0)
            .show_ui(ui, |ui| {
                for filter in MediaFilter::ALL {
                    ui.selectable_value(
                        &mut selected_filter,
                        filter,
                        self.t(media_filter_key(filter)),
                    );
                }
            });
        response.response.on_hover_text(format!(
            "{}\n{}",
            self.t("toolbar.media_filter"),
            self.t("help.media_filter")
        ));
        if selected_filter != current_filter {
            self.config.media_filter = selected_filter.as_str().into();
            self.canvas.media_filter = selected_filter;
        }
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
                    let searchable = format!("{} {}", entry.keys, description).to_lowercase();
                    searchable
                        .contains(&query)
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
        if icon_button(
            ui,
            "meters.reset",
            Icon::Refresh,
            self.t("inspector.audio_reset"),
            self.t("help.audio_reset"),
        ) {
            self.reset_audio_config();
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

    fn show_meter_monitor(&mut self, ui: &mut Ui, port_id: PortId) {
        let Some(port) = self.driver.graph().port(port_id) else {
            self.canvas.pinned_meter = None;
            return;
        };
        let node_id = port.node_id;
        let port_name = port.name.clone();
        let node_name = self
            .driver
            .graph()
            .node(node_id)
            .map(|node| node.name.clone())
            .unwrap_or_else(|| node_id.to_string());
        ui.label(RichText::new(format!("{node_name} / {port_name}")).strong());
        ui.label(RichText::new(self.t("canvas.audio_meter_node")).weak());
        if let Some(reading) = self.canvas.meters.get(&node_id).copied() {
            if reading.available {
                let stale = reading.age_ms > 750;
                ui.add(
                    egui::ProgressBar::new(reading.rms.clamp(0.0, 1.0))
                        .desired_width(ui.available_width())
                        .text(format!(
                            "{}  {:.1} dB",
                            self.t("canvas.audio_meter_rms"),
                            level_db(reading.rms)
                        )),
                );
                let peak_hold = self.canvas.meter_peak_hold(node_id, reading.peak);
                ui.add(
                    egui::ProgressBar::new(peak_hold.clamp(0.0, 1.0))
                        .desired_width(ui.available_width())
                        .text(format!(
                            "{}  {:.1} dB",
                            self.t("canvas.audio_meter_peak_hold"),
                            level_db(peak_hold)
                        )),
                );
                ui.label(
                    RichText::new(if stale {
                        self.t("canvas.audio_meter_stale")
                    } else {
                        self.t("canvas.audio_meter_live")
                    })
                    .weak(),
                );
                ui.label(
                    RichText::new(self.tf(
                        "canvas.audio_meter_age",
                        &[("age", reading.age_ms.to_string())],
                    ))
                    .small()
                    .weak(),
                );
            } else {
                ui.label(RichText::new(self.t("canvas.audio_meter_unavailable")).weak());
            }
        } else {
            ui.label(RichText::new(self.t("canvas.audio_meter_unavailable")).weak());
        }
        if ui
            .button(self.t("inspector.audio_monitor_unpin"))
            .on_hover_text(self.t("help.audio_monitor_unpin"))
            .clicked()
        {
            self.canvas.pinned_meter = None;
        }
    }

    fn show_rename_control(&mut self, ui: &mut Ui, selected_node: NodeId) {
        let current_name = self
            .driver
            .graph()
            .node(selected_node)
            .map(|node| node.name.clone());
        let Some(current_name) = current_name else {
            return;
        };
        ui.label(RichText::new(self.t("inspector.selected_node")).weak());
        if self.rename_node != Some(selected_node) {
            self.rename_node = Some(selected_node);
            self.rename_buffer = current_name.clone();
        }
        let response = ui.add(
            egui::TextEdit::singleline(&mut self.rename_buffer)
                .id(egui::Id::new(("rename-node", selected_node))),
        );
        if response.lost_focus()
            && ui.input(|input| input.key_pressed(egui::Key::Enter))
            && self.rename_buffer != current_name
            && !self.rename_buffer.trim().is_empty()
        {
            let edited_name = self.rename_buffer.clone();
            match self.commands.execute(
                Box::new(RenameCommand::new(selected_node, current_name, edited_name)),
                self.driver.as_mut(),
            ) {
                Ok(()) => self.status = self.t("status.renamed"),
                Err(error) => {
                    self.status = self.tf("status.rename_failed", &[("error", error.to_string())])
                }
            }
        }
    }

    fn show_patchbay_screen(&mut self, ui: &mut Ui) {
        panel_header(
            ui,
            Icon::Patchbay,
            self.t("screen.patchbay"),
            self.t("screen.patchbay_hint"),
        );
        panel_section(ui, self.t("inspector.live_links"), |ui| {
            let links: Vec<_> = self.driver.graph().links.values().cloned().collect();
            if links.is_empty() {
                ui.label(RichText::new(self.t("inspector.no_live_links")).weak());
            }
            for link in links {
                let (output_node, output_port) = self
                    .driver
                    .graph()
                    .port(link.output_port)
                    .map(|port| {
                        (
                            self.driver
                                .graph()
                                .node(port.node_id)
                                .map(|node| node.name.clone())
                                .unwrap_or_else(|| port.node_id.to_string()),
                            port.name.clone(),
                        )
                    })
                    .unwrap_or_else(|| {
                        (link.output_port.to_string(), link.output_port.to_string())
                    });
                let (input_node, input_port) = self
                    .driver
                    .graph()
                    .port(link.input_port)
                    .map(|port| {
                        (
                            self.driver
                                .graph()
                                .node(port.node_id)
                                .map(|node| node.name.clone())
                                .unwrap_or_else(|| port.node_id.to_string()),
                            port.name.clone(),
                        )
                    })
                    .unwrap_or_else(|| (link.input_port.to_string(), link.input_port.to_string()));
                let link_summary = self.tf(
                    "patchbay.link_summary",
                    &[
                        ("output_node", output_node),
                        ("output_port", output_port),
                        ("input_node", input_node),
                        ("input_port", input_port),
                    ],
                );
                ui.push_id(("live-link", link.id), |ui| {
                    ui.horizontal(|ui| {
                        let row_height = ui.spacing().interact_size.y;
                        let spacing = ui.spacing().item_spacing.x;
                        let pinned_width = 78.0;
                        let delete_width = 34.0;
                        let label_width =
                            (ui.available_width() - pinned_width - delete_width - spacing * 2.0)
                                .max(48.0);
                        let summary_response = ui.add_sized(
                            [label_width, row_height],
                            egui::Label::new(RichText::new(link_summary.clone()).weak()).truncate(),
                        );
                        summary_response.on_hover_ui(|ui| {
                            ui.set_max_width(520.0);
                            ui.add(egui::Label::new(link_summary.clone()).wrap());
                        });
                        let mut pinned = self
                            .patchbay
                            .connections
                            .iter()
                            .find(|connection| {
                                connection.output_port == link.output_port
                                    && connection.input_port == link.input_port
                            })
                            .is_some_and(|connection| connection.pinned);
                        if ui
                            .add_sized(
                                [pinned_width, row_height],
                                egui::Checkbox::new(&mut pinned, self.t("inspector.pinned")),
                            )
                            .changed()
                        {
                            if let Some(connection) =
                                self.patchbay.connections.iter_mut().find(|connection| {
                                    connection.output_port == link.output_port
                                        && connection.input_port == link.input_port
                                })
                            {
                                connection.pinned = pinned;
                            } else {
                                self.patchbay.add_graph_connection(
                                    self.driver.graph(),
                                    link.output_port,
                                    link.input_port,
                                    pinned,
                                );
                            }
                        }
                        if icon_button(
                            ui,
                            "disconnect",
                            Icon::Delete,
                            self.t("toolbar.disconnect"),
                            self.t("help.disconnect_link"),
                        ) {
                            self.disconnect(link.id);
                        }
                    });
                });
            }
        });
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
