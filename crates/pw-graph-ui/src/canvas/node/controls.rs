use crate::{CanvasAction, GraphCanvas, NodeAppearance, PortId};
use egui::{pos2, vec2, Color32, ProgressBar, Rect, RichText, Ui};
use pw_graph_core::Node;
use pw_graph_i18n::I18n;

use super::super::icons::{self, NodeIcon};
use super::helpers::level_db;

impl GraphCanvas {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_node_header_controls(
        &mut self,
        ui: &mut Ui,
        node: &Node,
        header: Rect,
        appearance: &NodeAppearance,
        can_collapse: bool,
        accent: Color32,
        actions: &mut Vec<CanvasAction>,
        i18n: &I18n,
        info: &str,
    ) {
        let mut name_draft = self
            .node_name_drafts
            .get(&node.id)
            .cloned()
            .unwrap_or_else(|| {
                appearance
                    .custom_name
                    .clone()
                    .unwrap_or_else(|| node.name.clone())
            });
        let mut working_appearance = appearance.clone();
        let mut appearance_changed = false;
        let info_rect = Rect::from_min_size(
            pos2(
                header.right() - 54.0 * self.zoom,
                header.top() + 9.0 * self.zoom,
            ),
            vec2(20.0, 22.0) * self.zoom,
        );
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(info_rect)
                .id_salt(("node-info", node.id)),
            |ui| {
                ui.add_sized(
                    info_rect.size(),
                    egui::Button::image(icons::image(
                        NodeIcon::Info,
                        vec2(14.0, 14.0) * self.zoom,
                        Color32::from_rgb(192, 204, 219),
                    ))
                    .frame(false),
                )
                .on_hover_text(info);
            },
        );

        let menu_rect = Rect::from_min_size(
            pos2(
                header.right() - 28.0 * self.zoom,
                header.top() + 9.0 * self.zoom,
            ),
            vec2(22.0, 22.0) * self.zoom,
        );
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(menu_rect)
                .id_salt(("node-options", node.id)),
            |ui| {
                ui.menu_image_button(
                    icons::image(
                        NodeIcon::More,
                        vec2(12.0, 12.0) * self.zoom,
                        ui.visuals().text_color(),
                    ),
                    |ui| {
                        if can_collapse {
                            if ui
                                .button(i18n.text(if appearance.collapsed {
                                    "canvas.expand_node"
                                } else {
                                    "canvas.collapse_node"
                                }))
                                .clicked()
                            {
                                working_appearance.collapsed = !working_appearance.collapsed;
                                appearance_changed = true;
                                ui.close_menu();
                            }
                            ui.separator();
                        }

                        ui.label(i18n.text("canvas.node_name"));
                        let name_response = ui.text_edit_singleline(&mut name_draft);
                        let submit_name = name_response.lost_focus()
                            && ui.input(|input| input.key_pressed(egui::Key::Enter));
                        if ui.button(i18n.text("canvas.apply_name")).clicked() || submit_name {
                            let name = name_draft.trim();
                            working_appearance.custom_name = if name.is_empty() {
                                None
                            } else if name == node.name {
                                None
                            } else {
                                Some(name.to_owned())
                            };
                            appearance_changed = true;
                            ui.close_menu();
                        }
                        if ui.button(i18n.text("canvas.reset_name")).clicked() {
                            name_draft = node.name.clone();
                            working_appearance.custom_name = None;
                            appearance_changed = true;
                            ui.close_menu();
                        }

                        ui.separator();
                        ui.horizontal(|ui| {
                            ui.label(i18n.text("canvas.node_color"));
                            let mut color = working_appearance
                                .color
                                .unwrap_or_else(|| accent.to_array());
                            if ui
                                .color_edit_button_srgba_unmultiplied(&mut color)
                                .changed()
                            {
                                working_appearance.color = Some(color);
                                appearance_changed = true;
                            }
                        });
                        if ui.button(i18n.text("canvas.reset_color")).clicked() {
                            working_appearance.color = None;
                            appearance_changed = true;
                            ui.close_menu();
                        }
                    },
                );
            },
        );
        self.node_name_drafts.insert(node.id, name_draft);
        if appearance_changed {
            actions.push(CanvasAction::SetNodeAppearance {
                node: node.id,
                appearance: working_appearance,
            });
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_node_audio_controls(
        &mut self,
        ui: &mut Ui,
        node: &Node,
        node_rect: Rect,
        header: Rect,
        monitor_port: Option<PortId>,
        accent: Color32,
        actions: &mut Vec<CanvasAction>,
        i18n: &I18n,
    ) {
        let control_rect = Rect::from_min_size(
            pos2(
                node_rect.left() + 8.0 * self.zoom,
                header.bottom() + 5.0 * self.zoom,
            ),
            vec2(
                node_rect.width() - 16.0 * self.zoom,
                (super::AUDIO_CONTROLS_HEIGHT - 8.0) * self.zoom,
            ),
        );
        let previous = self.node_audio_state(node.id);
        let mut state = previous;
        let control_scale = self.zoom.max(0.75);
        let pointer_over_controls = ui
            .input(|input| input.pointer.hover_pos())
            .is_some_and(|pointer| control_rect.contains(pointer));
        if pointer_over_controls {
            self.hovered_meter_node = Some(node.id);
        }
        let meter = monitor_port
            .and_then(|port| self.port_meters.get(&port).copied())
            .or_else(|| self.meters.get(&node.id).copied());
        let monitor_pinned = monitor_port.is_some_and(|port| self.pinned_meter == Some(port));

        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(control_rect)
                .id_salt(("node-audio-controls", node.id)),
            |ui| {
                ui.vertical_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 5.0 * control_scale;
                    let mute_button_size = vec2(28.0, 28.0) * control_scale;
                    let value_width = 36.0 * control_scale;
                    let slider_width = (ui.available_width()
                        - mute_button_size.x
                        - value_width
                        - 10.0 * control_scale)
                        .max(28.0 * control_scale);
                    ui.add_sized(
                        vec2(slider_width, 20.0 * control_scale),
                        egui::Slider::new(&mut state.volume, 0.0..=1.5).show_value(false),
                    );
                    ui.add_sized(
                        vec2(value_width, 20.0 * control_scale),
                        egui::Label::new(
                            RichText::new(format!("{:.0}%", state.volume * 100.0))
                                .size(10.0 * control_scale)
                                .color(accent),
                        ),
                    );
                    let icon = if state.muted {
                        NodeIcon::VolumeMuted
                    } else {
                        NodeIcon::Volume
                    };
                    let tooltip = i18n.text(if state.muted {
                        "canvas.unmute_node"
                    } else {
                        "canvas.mute_node"
                    });
                    let mute_response = ui
                        .add_sized(
                            mute_button_size,
                            egui::Button::image(icons::image(
                                icon,
                                vec2(16.0, 16.0) * control_scale,
                                ui.visuals().text_color(),
                            ))
                            .frame(false),
                        )
                        .on_hover_text(tooltip);
                    if mute_response.clicked() {
                        state.muted = !state.muted;
                    }

                    ui.add_space(2.0 * control_scale);
                    ui.horizontal(|ui| {
                        let (level, level_text, fill) = match meter {
                            Some(reading) if reading.available => {
                                let stale = reading.age_ms > 750;
                                let value = reading.peak.clamp(0.0, 1.0);
                                let text = format!("{:.0} dB", level_db(reading.rms));
                                let color = if stale {
                                    Color32::from_rgb(204, 163, 90)
                                } else {
                                    accent
                                };
                                (value, text, color)
                            }
                            Some(_) => (0.0, "-- dB".into(), Color32::from_gray(100)),
                            None => (0.0, "-- dB".into(), Color32::from_gray(100)),
                        };
                        let label_color = if meter.is_some_and(|reading| reading.available) {
                            accent
                        } else {
                            Color32::from_rgb(158, 172, 189)
                        };
                        ui.label(
                            RichText::new(i18n.text("canvas.audio_monitor"))
                                .size(9.0 * control_scale)
                                .color(label_color),
                        );
                        let pin_size = vec2(22.0, 20.0) * control_scale;
                        let text_width = 38.0 * control_scale;
                        let meter_width = (ui.available_width() - pin_size.x - text_width)
                            .max(34.0 * control_scale);
                        ui.add_sized(
                            vec2(meter_width, 12.0 * control_scale),
                            ProgressBar::new(level).fill(fill),
                        );
                        ui.add_sized(
                            vec2(text_width, 16.0 * control_scale),
                            egui::Label::new(
                                RichText::new(level_text)
                                    .size(9.0 * control_scale)
                                    .color(label_color),
                            ),
                        );
                        let monitor_tooltip = i18n.text(if monitor_pinned {
                            "canvas.audio_meter_unpin"
                        } else {
                            "canvas.audio_meter_pin"
                        });
                        let monitor_response = ui
                            .add_sized(
                                pin_size,
                                egui::Button::image(icons::image(
                                    NodeIcon::Monitor,
                                    vec2(13.0, 13.0) * control_scale,
                                    if monitor_pinned {
                                        accent
                                    } else {
                                        Color32::from_rgb(183, 196, 212)
                                    },
                                ))
                                .frame(false)
                                .fill(if monitor_pinned {
                                    Color32::from_rgba_unmultiplied(
                                        accent.r(),
                                        accent.g(),
                                        accent.b(),
                                        38,
                                    )
                                } else {
                                    Color32::TRANSPARENT
                                }),
                            )
                            .on_hover_text(monitor_tooltip);
                        if monitor_response.clicked() {
                            if let Some(port) = monitor_port {
                                self.pinned_meter = if monitor_pinned { None } else { Some(port) };
                            }
                        }
                    });
                });
            },
        );

        if state.muted != previous.muted {
            actions.push(CanvasAction::SetNodeMute {
                node: node.id,
                muted: state.muted,
            });
        }
        if (state.volume - previous.volume).abs() > f32::EPSILON {
            actions.push(CanvasAction::SetNodeVolume {
                node: node.id,
                volume: state.volume,
            });
        }
    }
}
