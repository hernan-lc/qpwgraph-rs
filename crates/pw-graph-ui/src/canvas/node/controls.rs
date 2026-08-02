use crate::{CanvasAction, GraphCanvas, MeterReading, NodeAppearance};
use egui::{
    pos2, vec2, Color32, Key, ProgressBar, Rect, Response, RichText, Sense, Stroke, Ui, WidgetInfo,
};
use pw_graph_core::Node;
use pw_graph_i18n::I18n;
use std::cell::Cell;

use super::super::icons::{self, NodeIcon};
use super::helpers::{level_db, meter_fraction};
use super::AudioInfo;

const MAX_VOLUME: f32 = 1.5;
const UNITY_TRACK_POSITION: f32 = 0.9;

/// Put 0–100% across most of the track and reserve the final 10% for boost.
/// Unity therefore sits near the right edge like a conventional audio fader,
/// while the existing 150% range remains available.
fn volume_track_position(volume: f32) -> f32 {
    let volume = volume.clamp(0.0, MAX_VOLUME);
    if volume <= 1.0 {
        volume * UNITY_TRACK_POSITION
    } else {
        UNITY_TRACK_POSITION + (volume - 1.0) / (MAX_VOLUME - 1.0) * (1.0 - UNITY_TRACK_POSITION)
    }
}

fn volume_from_track_position(position: f32) -> f32 {
    let position = position.clamp(0.0, 1.0);
    if position <= UNITY_TRACK_POSITION {
        position / UNITY_TRACK_POSITION
    } else {
        1.0 + (position - UNITY_TRACK_POSITION) / (1.0 - UNITY_TRACK_POSITION) * (MAX_VOLUME - 1.0)
    }
}

fn volume_slider(
    ui: &mut Ui,
    volume: &mut f32,
    size: egui::Vec2,
    label: &str,
    meter: Option<MeterReading>,
) -> Response {
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let scale = (size.y / 26.0).max(0.1);
    let thumb_radius = 6.0 * scale;
    let track = Rect::from_min_max(
        pos2(rect.left() + thumb_radius, rect.top()),
        pos2(rect.right() - thumb_radius, rect.bottom()),
    );

    if let Some(pointer) = (response.clicked() || response.dragged())
        .then(|| response.interact_pointer_pos())
        .flatten()
    {
        let position = ((pointer.x - track.left()) / track.width()).clamp(0.0, 1.0);
        let next = volume_from_track_position(position);
        if (*volume - next).abs() > f32::EPSILON {
            *volume = next;
            response.mark_changed();
        }
        response.request_focus();
    }

    if response.has_focus() {
        let step = ui.input(|input| {
            if input.key_pressed(Key::ArrowLeft) || input.key_pressed(Key::ArrowDown) {
                -0.01
            } else if input.key_pressed(Key::ArrowRight) || input.key_pressed(Key::ArrowUp) {
                0.01
            } else {
                0.0
            }
        });
        if step != 0.0 {
            *volume = (*volume + step).clamp(0.0, MAX_VOLUME);
            response.mark_changed();
        }
    }

    let position = volume_track_position(*volume);
    let track_y = rect.top() + 7.0 * scale;
    let thumb_x = egui::lerp(track.x_range(), position);
    let painter = ui.painter();
    painter.line_segment(
        [pos2(track.left(), track_y), pos2(track.right(), track_y)],
        Stroke::new(3.0 * scale, Color32::from_rgb(56, 64, 75)),
    );
    painter.line_segment(
        [pos2(track.left(), track_y), pos2(thumb_x, track_y)],
        Stroke::new(3.0 * scale, Color32::from_rgb(42, 169, 244)),
    );

    // Reuse the colored scale as the live level indicator. The volume fader
    // remains the blue line and white thumb above it; only the colored ticks
    // respond to the node's audio level.
    let meter_level = meter
        .filter(|reading| reading.available)
        .map(|reading| meter_fraction(reading.peak));
    let tick_top = rect.top() + 18.0 * scale;
    let tick_bottom = rect.top() + 22.0 * scale;
    for index in 0..=30 {
        let tick_position = index as f32 / 30.0;
        let base_color = if tick_position < 0.72 {
            Color32::from_rgb(57, 166, 224)
        } else if tick_position < 0.9 {
            Color32::from_rgb(220, 164, 76)
        } else {
            Color32::from_rgb(221, 75, 72)
        };
        let color = if meter_level.is_some_and(|level| tick_position <= level) {
            base_color
        } else {
            base_color.gamma_multiply(0.22)
        };
        let x = egui::lerp(track.x_range(), tick_position);
        painter.line_segment(
            [pos2(x, tick_top), pos2(x, tick_bottom)],
            Stroke::new(1.35 * scale, color),
        );
    }

    painter.circle_filled(
        pos2(thumb_x, track_y),
        thumb_radius,
        Color32::from_rgb(244, 247, 250),
    );
    painter.circle_stroke(
        pos2(thumb_x, track_y),
        thumb_radius,
        Stroke::new(1.0_f32, Color32::from_rgb(21, 27, 34)),
    );
    if response.hovered() || response.has_focus() {
        painter.circle_stroke(
            pos2(thumb_x, track_y),
            thumb_radius + 2.0 * scale,
            Stroke::new(1.0_f32, Color32::from_white_alpha(80)),
        );
    }

    response.widget_info(|| WidgetInfo::slider(ui.is_enabled(), f64::from(*volume), label));
    response
}

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
        audio_info: Option<&AudioInfo>,
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
        let pin_requested = Cell::new(false);
        let info_response = ui
            .scope_builder(
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
                },
            )
            .inner;
        if let Some(audio_info) = audio_info {
            let meter = audio_info.meter;
            let peak_hold = meter
                .filter(|reading| reading.available)
                .map(|reading| self.meter_peak_hold(node.id, reading.peak));
            let monitor_pinned = self.pinned_meter == Some(audio_info.port_id);
            info_response.on_hover_ui(|ui| {
                ui.label(RichText::new(&audio_info.port_help).strong());
                ui.separator();
                ui.label(RichText::new(i18n.text("canvas.audio_meter_title")).strong());
                match meter {
                    Some(reading) if reading.available => {
                        let state = if reading.age_ms > 750 {
                            i18n.text("canvas.audio_meter_stale")
                        } else {
                            i18n.text("canvas.audio_meter_live")
                        };
                        ui.label(RichText::new(state).weak());
                        ui.add(
                            ProgressBar::new(meter_fraction(reading.rms))
                                .desired_width(190.0)
                                .text(format!(
                                    "{}  {:.1} dB",
                                    i18n.text("canvas.audio_meter_rms"),
                                    level_db(reading.rms)
                                )),
                        );
                        if let Some(peak_hold) = peak_hold {
                            ui.add(
                                ProgressBar::new(meter_fraction(peak_hold))
                                    .desired_width(190.0)
                                    .text(format!(
                                        "{}  {:.1} dB",
                                        i18n.text("canvas.audio_meter_peak_hold"),
                                        level_db(peak_hold)
                                    )),
                            );
                        }
                        ui.label(
                            RichText::new(i18n.format(
                                "canvas.audio_meter_age",
                                &[("age", reading.age_ms.to_string())],
                            ))
                            .small()
                            .weak(),
                        );
                    }
                    Some(_) => {
                        ui.label(RichText::new(i18n.text("canvas.audio_meter_unavailable")).weak());
                    }
                    None if self.metering_disabled => {
                        ui.label(RichText::new(i18n.text("canvas.audio_meter_disabled")).weak());
                    }
                    None => {
                        ui.label(RichText::new(i18n.text("canvas.audio_meter_starting")).weak());
                    }
                }
                if ui
                    .button(if monitor_pinned {
                        i18n.text("canvas.audio_meter_pinned")
                    } else {
                        i18n.text("canvas.audio_meter_pin")
                    })
                    .clicked()
                {
                    pin_requested.set(true);
                }
            });
            if pin_requested.get() {
                self.pinned_meter = if monitor_pinned {
                    None
                } else {
                    Some(audio_info.port_id)
                };
            }
        } else {
            info_response.on_hover_text(info);
        }

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

                        if node.node_type == pw_graph_core::NodeType::Effect {
                            if ui.button(i18n.text("canvas.remove_effect")).clicked() {
                                actions.push(CanvasAction::RemoveEffect { node: node.id });
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
                            working_appearance.custom_name = if name.is_empty() || name == node.name
                            {
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
        accent: Color32,
        meter: Option<MeterReading>,
        actions: &mut Vec<CanvasAction>,
        i18n: &I18n,
    ) {
        let control_rect = Rect::from_min_size(
            pos2(
                node_rect.left() + 10.0 * self.zoom,
                header.bottom() + 6.0 * self.zoom,
            ),
            vec2(
                node_rect.width() - 20.0 * self.zoom,
                (super::AUDIO_CONTROLS_HEIGHT - 12.0) * self.zoom,
            ),
        );
        let previous = self.node_audio_state(node.id);
        let mut state = previous;
        // Controls must scale with their card. A separate minimum scale makes
        // them overlap the port rows when the canvas is zoomed out.
        let control_scale = self.zoom;
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(control_rect)
                .id_salt(("node-audio-controls", node.id)),
            |ui| {
                ui.vertical_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 5.0 * control_scale;
                    ui.spacing_mut().item_spacing.y = 3.0 * control_scale;
                    ui.horizontal(|ui| {
                        let mute_button_size = vec2(28.0, 26.0) * control_scale;
                        let slider_width =
                            (ui.available_width() - mute_button_size.x - 5.0 * control_scale)
                                .max(28.0 * control_scale);
                        let volume_label = i18n.text("canvas.volume");
                        let volume_response = volume_slider(
                            ui,
                            &mut state.volume,
                            vec2(slider_width, 26.0 * control_scale),
                            &volume_label,
                            meter,
                        );
                        volume_response
                            .on_hover_text(format!("{volume_label}: {:.0}%", state.volume * 100.0));
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
                                    vec2(15.0, 15.0) * control_scale,
                                    if state.muted {
                                        accent
                                    } else {
                                        Color32::from_rgb(224, 231, 239)
                                    },
                                ))
                                .fill(Color32::from_rgb(31, 37, 46))
                                .stroke(Stroke::new(
                                    1.0_f32,
                                    if state.muted {
                                        accent
                                    } else {
                                        Color32::from_rgb(69, 81, 98)
                                    },
                                ))
                                .rounding(6.0 * control_scale),
                            )
                            .on_hover_text(tooltip);
                        if mute_response.clicked() {
                            state.muted = !state.muted;
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

    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_node_effect_controls(
        &mut self,
        ui: &mut Ui,
        node: &Node,
        node_rect: Rect,
        header: Rect,
        accent: Color32,
        actions: &mut Vec<CanvasAction>,
        i18n: &I18n,
    ) {
        let Some(control) = self.effect_controls.get(&node.id).cloned() else {
            return;
        };
        let controls_height = self.effect_controls_height(node);
        let control_rect = Rect::from_min_size(
            pos2(
                node_rect.left() + 8.0 * self.zoom,
                header.bottom() + 5.0 * self.zoom,
            ),
            vec2(
                node_rect.width() - 16.0 * self.zoom,
                (controls_height - super::EFFECT_CONTROLS_VERTICAL_PADDING) * self.zoom,
            ),
        );
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(control_rect)
                .id_salt(("node-effect-controls", node.id)),
            |ui| {
                // `UiBuilder::max_rect` constrains layout but does not clip
                // painting. Clip the panel as a final safety net at unusual
                // zoom levels, so no widget can ever cover the port rows.
                ui.shrink_clip_rect(control_rect);
                ui.spacing_mut().item_spacing.y = 2.0 * self.zoom;
                let mut enabled = control.enabled;
                if ui
                    .checkbox(&mut enabled, i18n.text("effects.enabled"))
                    .changed()
                {
                    actions.push(CanvasAction::SetEffectEnabled {
                        node: node.id,
                        enabled,
                    });
                }
                for parameter in control.parameters {
                    if parameter.boolean {
                        let mut value = parameter.value >= 0.5;
                        if ui.checkbox(&mut value, parameter.name).changed() {
                            actions.push(CanvasAction::SetEffectParameter {
                                node: node.id,
                                parameter: parameter.id,
                                value: if value { 1.0 } else { 0.0 },
                            });
                        }
                    } else {
                        let mut value = parameter.value;
                        if ui
                            .add(
                                egui::Slider::new(
                                    &mut value,
                                    parameter.minimum..=parameter.maximum,
                                )
                                .text(format!("{} ({})", parameter.name, parameter.unit)),
                            )
                            .changed()
                        {
                            actions.push(CanvasAction::SetEffectParameter {
                                node: node.id,
                                parameter: parameter.id,
                                value,
                            });
                        }
                    }
                }
            },
        );
        ui.painter().line_segment(
            [
                pos2(
                    node_rect.left() + 8.0 * self.zoom,
                    header.bottom() + controls_height * self.zoom,
                ),
                pos2(
                    node_rect.right() - 8.0 * self.zoom,
                    header.bottom() + controls_height * self.zoom,
                ),
            ],
            Stroke::new(1.0_f32, Color32::from_rgb(52, 63, 78)),
        );
        let _ = accent;
    }
}

#[cfg(test)]
mod tests {
    use super::{volume_from_track_position, volume_track_position, UNITY_TRACK_POSITION};

    #[test]
    fn unity_volume_sits_near_the_end_of_the_track() {
        assert!((volume_track_position(1.0) - UNITY_TRACK_POSITION).abs() < f32::EPSILON);
    }

    #[test]
    fn custom_volume_track_mapping_round_trips() {
        for volume in [0.0, 0.25, 0.5, 1.0, 1.25, 1.5] {
            let round_trip = volume_from_track_position(volume_track_position(volume));
            assert!((round_trip - volume).abs() < 0.0001);
        }
    }
}
