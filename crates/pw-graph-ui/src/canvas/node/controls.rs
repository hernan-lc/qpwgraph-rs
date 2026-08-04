use crate::{
    setting_row, CanvasAction, GraphCanvas, MeterReading, NodeAppearance, SliderProps, SwitchProps,
    TextInputProps, UiDocument, Value,
};
use egui::{pos2, vec2, Color32, Key, ProgressBar, Rect, Response, RichText, Stroke, Ui};
use pw_graph_core::Node;
use pw_graph_i18n::I18n;
use std::cell::Cell;

use super::super::icons::{self, NodeIcon};
use super::helpers::{level_db, meter_fraction};
use super::{node_button, sync_document_value, AudioInfo};

const MAX_VOLUME: f32 = 1.5;
const UNITY_TRACK_POSITION: f32 = 0.9;
const EFFECT_SETTING_LABEL_WIDTH: f32 = 82.0;
const EFFECT_SETTING_SWITCH_WIDTH: f32 = 36.0;
const EFFECT_SETTING_SWITCH_HEIGHT: f32 = 20.0;
const EFFECT_SETTING_MIN_SLIDER_WIDTH: f32 = 42.0;

/// Keep the conventional 0–100% range across most of the track while
/// retaining the optional 150% boost at the end.
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

fn paint_volume_meter(ui: &Ui, response: &Response, meter: Option<MeterReading>) {
    let rect = response.rect;
    let scale = (rect.height() / 26.0).max(0.1);
    let track = Rect::from_min_max(
        pos2(rect.left() + 6.0 * scale, rect.top()),
        pos2(rect.right() - 6.0 * scale, rect.bottom()),
    );
    let meter_level = meter
        .filter(|reading| reading.available)
        .map(|reading| meter_fraction(reading.peak));
    let tick_top = rect.bottom() - 7.0 * scale;
    let tick_bottom = rect.bottom() - 3.0 * scale;
    let painter = ui.painter();
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
}

fn effect_setting_label(ui: &mut Ui, label: &str, width: f32) {
    ui.set_min_width(width);
    ui.label(RichText::new(label).small());
}

fn effect_slider_width(ui: &Ui, control_scale: f32) -> f32 {
    let row_width = ui.available_width().max(0.0);
    let label_width = EFFECT_SETTING_LABEL_WIDTH * control_scale;
    let minimum = EFFECT_SETTING_MIN_SLIDER_WIDTH * control_scale;
    let maximum = (row_width - 54.0 * control_scale).max(minimum);
    (row_width - label_width - ui.spacing().item_spacing.x).clamp(minimum, maximum)
}

fn effect_parameter_tooltip(
    parameter_name: &str,
    minimum: f32,
    maximum: f32,
    unit: &str,
) -> String {
    let range = format!("{minimum}–{maximum}");
    if unit.is_empty() {
        format!("{parameter_name}: {range}")
    } else {
        format!("{parameter_name}: {range} {unit}")
    }
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
        document: &mut UiDocument,
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
        document.record_click(format!("node.{}.info", node.id), info_response.clicked());
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
                if node_button(
                    document,
                    ui,
                    &format!("node.{}.meter.pin", node.id),
                    if monitor_pinned {
                        i18n.text("canvas.audio_meter_pinned")
                    } else {
                        i18n.text("canvas.audio_meter_pin")
                    },
                ) {
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
                            if node_button(
                                document,
                                ui,
                                &format!("node.{}.appearance.collapse", node.id),
                                i18n.text(if appearance.collapsed {
                                    "canvas.expand_node"
                                } else {
                                    "canvas.collapse_node"
                                }),
                            ) {
                                working_appearance.collapsed = !working_appearance.collapsed;
                                appearance_changed = true;
                                ui.close_menu();
                            }
                            ui.separator();
                        }

                        if node.node_type == pw_graph_core::NodeType::Effect {
                            if node_button(
                                document,
                                ui,
                                &format!("node.{}.appearance.remove-effect", node.id),
                                i18n.text("canvas.remove_effect"),
                            ) {
                                actions.push(CanvasAction::RemoveEffect { node: node.id });
                                ui.close_menu();
                            }
                            ui.separator();
                        }

                        ui.label(i18n.text("canvas.node_name"));
                        let name_id = format!("node.{}.appearance.name", node.id);
                        sync_document_value(document, &name_id, Value::String(name_draft.clone()));
                        let name_response = document
                            .text_input(ui, TextInputProps::new(&name_id, "").value(&name_draft));
                        name_draft = document.text(&name_id).unwrap_or(&name_draft).to_owned();
                        let submit_name = name_response.lost_focus()
                            && ui.input(|input| input.key_pressed(Key::Enter));
                        if node_button(
                            document,
                            ui,
                            &format!("node.{}.appearance.apply-name", node.id),
                            i18n.text("canvas.apply_name"),
                        ) || submit_name
                        {
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
                        if node_button(
                            document,
                            ui,
                            &format!("node.{}.appearance.reset-name", node.id),
                            i18n.text("canvas.reset_name"),
                        ) {
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
                        if node_button(
                            document,
                            ui,
                            &format!("node.{}.appearance.reset-color", node.id),
                            i18n.text("canvas.reset_color"),
                        ) {
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
        document: &mut UiDocument,
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
                        let volume_id = format!("node.{}.audio.volume", node.id);
                        let volume_position = volume_track_position(state.volume);
                        sync_document_value(
                            document,
                            &volume_id,
                            Value::Number(f64::from(volume_position)),
                        );
                        let volume_response = document.slider(
                            ui,
                            SliderProps::new(&volume_id, "")
                                .value(f64::from(volume_position))
                                .range(0.0, 1.0)
                                .step(0.01)
                                .show_value(false)
                                .size(slider_width, 26.0 * control_scale)
                                .tooltip(format!("{volume_label}: {:.0}%", state.volume * 100.0)),
                        );
                        paint_volume_meter(ui, &volume_response, meter);
                        let volume_position = document
                            .number(&volume_id)
                            .unwrap_or(f64::from(volume_position))
                            as f32;
                        state.volume = volume_from_track_position(volume_position);
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
                        let mute_id = format!("node.{}.audio.mute", node.id);
                        if document.record_click(&mute_id, mute_response.clicked()) {
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
        actions: &mut Vec<CanvasAction>,
        i18n: &I18n,
        document: &mut UiDocument,
    ) {
        let Some(control) = self.effect_controls.get(&node.id).cloned() else {
            return;
        };
        let controls_height = self.effect_controls_height(node);
        // Scale the native egui widgets along with the scene. Without this,
        // zooming the canvas out shrinks the card but leaves its sliders at
        // normal screen size, making an effect node look much larger than
        // every other node.
        let control_scale = self.zoom.clamp(0.35, 1.5);
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
                for font_id in ui.style_mut().text_styles.values_mut() {
                    font_id.size *= control_scale;
                }
                ui.spacing_mut().item_spacing *= control_scale;
                ui.spacing_mut().interact_size *= control_scale;
                let enabled_id = format!("node.{}.effect.enabled", node.id);
                sync_document_value(document, &enabled_id, Value::Bool(control.enabled));
                setting_row(
                    ui,
                    |ui| {
                        effect_setting_label(
                            ui,
                            &i18n.text("effects.enabled"),
                            EFFECT_SETTING_LABEL_WIDTH * control_scale,
                        );
                    },
                    |ui| {
                        document.switch(
                            ui,
                            SwitchProps::new(&enabled_id, String::new())
                                .checked(control.enabled)
                                .size(
                                    EFFECT_SETTING_SWITCH_WIDTH * control_scale,
                                    EFFECT_SETTING_SWITCH_HEIGHT * control_scale,
                                )
                                .tooltip(i18n.text("effects.enabled")),
                        )
                    },
                );
                if document.changed(&enabled_id) {
                    let enabled = document.checked(&enabled_id).unwrap_or(control.enabled);
                    actions.push(CanvasAction::SetEffectEnabled {
                        node: node.id,
                        enabled,
                    });
                }
                for parameter in control.parameters {
                    if parameter.boolean {
                        let parameter_id =
                            format!("node.{}.effect.parameter.{}", node.id, parameter.id);
                        let current = parameter.value >= 0.5;
                        sync_document_value(document, &parameter_id, Value::Bool(current));
                        let label = parameter.name.clone();
                        setting_row(
                            ui,
                            |ui| {
                                effect_setting_label(
                                    ui,
                                    &label,
                                    EFFECT_SETTING_LABEL_WIDTH * control_scale,
                                );
                            },
                            |ui| {
                                document.switch(
                                    ui,
                                    SwitchProps::new(&parameter_id, String::new())
                                        .checked(current)
                                        .size(
                                            EFFECT_SETTING_SWITCH_WIDTH * control_scale,
                                            EFFECT_SETTING_SWITCH_HEIGHT * control_scale,
                                        )
                                        .tooltip(label.clone()),
                                )
                            },
                        );
                        if document.changed(&parameter_id) {
                            let value = document.checked(&parameter_id).unwrap_or(current);
                            actions.push(CanvasAction::SetEffectParameter {
                                node: node.id,
                                parameter: parameter.id,
                                value: if value { 1.0 } else { 0.0 },
                            });
                        }
                    } else {
                        let parameter_id =
                            format!("node.{}.effect.parameter.{}", node.id, parameter.id);
                        let current = f64::from(parameter.value);
                        let label = parameter.name.clone();
                        let suffix = if parameter.unit.is_empty() {
                            String::new()
                        } else {
                            format!(" {}", parameter.unit)
                        };
                        let tooltip = effect_parameter_tooltip(
                            &parameter.name,
                            parameter.minimum,
                            parameter.maximum,
                            &parameter.unit,
                        );
                        let slider_width = effect_slider_width(ui, control_scale);
                        sync_document_value(document, &parameter_id, Value::Number(current));
                        setting_row(
                            ui,
                            |ui| {
                                effect_setting_label(
                                    ui,
                                    &label,
                                    EFFECT_SETTING_LABEL_WIDTH * control_scale,
                                );
                            },
                            |ui| {
                                document.slider(
                                    ui,
                                    SliderProps::new(&parameter_id, String::new())
                                        .value(current)
                                        .range(
                                            f64::from(parameter.minimum),
                                            f64::from(parameter.maximum),
                                        )
                                        .show_value(true)
                                        .suffix(suffix)
                                        .width(slider_width)
                                        .tooltip(tooltip),
                                )
                            },
                        );
                        if document.changed(&parameter_id) {
                            let value = document.number(&parameter_id).unwrap_or(current) as f32;
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
