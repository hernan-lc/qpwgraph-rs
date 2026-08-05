use super::super::components::{
    document_button, document_setting_slider, document_setting_switch_plain,
    modal_step_heading,
};
use super::super::shared::{fresh_scroll_area, show_centered_dialog};
use crate::app::effects::{available_descriptors, EffectGalleryPhase, EffectGalleryState};
use crate::app::QpwgraphApp;
use eframe::egui::{self, Color32, RichText, Sense, Stroke, Ui};
use pw_graph_effects::{EffectDescriptor, EffectParameter};
use pw_graph_ui::UiDocument;

fn effect_gallery_card(
    document: &mut UiDocument,
    ui: &mut Ui,
    descriptor: &EffectDescriptor,
    summary: String,
    selected: bool,
) -> bool {
    let width = ui.available_width().max(1.0);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 96.0), Sense::click());
    let visuals = ui.style().interact_selectable(&response, selected);
    let fill = if selected {
        Color32::from_rgb(35, 86, 119)
    } else if response.hovered() {
        Color32::from_rgb(45, 54, 68)
    } else {
        Color32::from_rgb(34, 40, 50)
    };
    let stroke = if selected {
        Stroke::new(1.5_f32, Color32::from_rgb(96, 190, 250))
    } else {
        visuals.bg_stroke
    };
    ui.painter().rect(rect, 7.0, fill, stroke);
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(rect.shrink(10.0))
            .id_salt(("effect-gallery-card", &descriptor.id)),
        |ui| {
            ui.label(RichText::new(&descriptor.name).strong().color(Color32::from_rgb(240, 244, 250)));
            ui.label(
                RichText::new(format!("{} · {}", descriptor.vendor, descriptor.version))
                    .small()
                    .color(Color32::from_rgb(180, 195, 215)),
            );
            ui.add_space(8.0);
            ui.label(
                RichText::new(summary)
                    .small()
                    .color(Color32::from_rgb(205, 222, 240)),
            );
        },
    );
    document.record_click(
        format!("modals.effects.card.{}", descriptor.id),
        response.clicked(),
    )
}

fn effect_parameter_hint(parameter: &EffectParameter) -> String {
    let range = format!("{}–{}", parameter.minimum, parameter.maximum);
    if parameter.unit.is_empty() {
        format!("Range {range}")
    } else {
        format!("{} · {range}", parameter.unit)
    }
}

fn show_effect_initial_settings(
    document: &mut UiDocument,
    ui: &mut Ui,
    descriptor: &EffectDescriptor,
    gallery: &mut EffectGalleryState,
    initial_settings_label: String,
    setup_hint: String,
    enabled_label: String,
) {
    egui::Frame::group(ui.style())
        .inner_margin(10.0)
        .show(ui, |ui| {
            ui.label(RichText::new(initial_settings_label).strong().color(Color32::from_rgb(240, 244, 250)));
            ui.label(RichText::new(setup_hint).small().color(Color32::from_rgb(180, 195, 215)));
            ui.add_space(6.0);
            gallery.enabled = document_setting_switch_plain(
                document,
                ui,
                "modals.effects.enabled",
                gallery.enabled,
                enabled_label,
                String::new(),
            );
            for parameter in &descriptor.parameters {
                if parameter.unit == "boolean" {
                    let value = gallery
                        .parameters
                        .get(&parameter.id)
                        .copied()
                        .unwrap_or(parameter.default)
                        >= 0.5;
                    let value = document_setting_switch_plain(
                        document,
                        ui,
                        &format!("modals.effects.parameters.{}.boolean", parameter.id),
                        value,
                        parameter.name.clone(),
                        String::new(),
                    );
                    gallery
                        .parameters
                        .insert(parameter.id.clone(), if value { 1.0 } else { 0.0 });
                } else {
                    let value = gallery
                        .parameters
                        .get(&parameter.id)
                        .copied()
                        .unwrap_or(parameter.default);
                    let value = document_setting_slider(
                        document,
                        ui,
                        &format!("modals.effects.parameters.{}", parameter.id),
                        value,
                        parameter.minimum,
                        parameter.maximum,
                        0.0,
                        parameter.name.clone(),
                        effect_parameter_hint(parameter),
                        210.0,
                    );
                    gallery.parameters.insert(parameter.id.clone(), value);
                }
            }
        });
}

impl QpwgraphApp {
    pub(crate) fn show_effect_gallery_modal(&mut self, ctx: &egui::Context) {
        if self.effect_gallery.is_none() {
            return;
        }
        let descriptors = available_descriptors(self.driver.as_ref());
        let supports_effect_nodes = self.driver.supports_effect_nodes();
        let Some(mut gallery) = self.effect_gallery.take() else {
            return;
        };
        if gallery.effect_id.is_empty()
            || !descriptors
                .iter()
                .any(|descriptor| descriptor.id == gallery.effect_id)
        {
            if let Some(descriptor) = descriptors.first() {
                gallery.select_effect(descriptor);
            }
        }
        let mut cancel = false;
        let mut create = false;
        let mut next = false;
        let mut back = false;
        let initial_settings_label = self.i18n.text("effects.initial_settings");
        let setup_hint = self.i18n.text("effects.setup_hint");
        let enabled_label = self.i18n.text("effects.enabled");
        let mut document = std::mem::take(&mut self.ui_document);
        let dialog_response = show_centered_dialog(
            &mut document,
            ctx,
            "effects-gallery",
            self.i18n.text("effects.gallery_title"),
            720.0,
            |ui, document| {
                ui.horizontal(|ui| {
                    modal_step_heading(
                        ui,
                        0,
                        gallery.phase.index(),
                        self.i18n.text("effects.step_effect"),
                    );
                    ui.separator();
                    modal_step_heading(
                        ui,
                        1,
                        gallery.phase.index(),
                        self.i18n.text("effects.step_setup"),
                    );
                });
                ui.add_space(6.0);
                ui.label(
                    RichText::new(match gallery.phase {
                        EffectGalleryPhase::Choose => self.i18n.text("effects.choose_effect_hint"),
                        EffectGalleryPhase::Configure => setup_hint.clone(),
                    })
                    .color(Color32::from_rgb(180, 195, 215)),
                );
                if !supports_effect_nodes {
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(self.i18n.text("effects.backend_unavailable"))
                            .color(Color32::from_rgb(239, 169, 82)),
                    );
                }
                ui.add_space(8.0);

                fresh_scroll_area(("effects-gallery-scroll", gallery.scroll_epoch), 400.0).show(
                    ui,
                    |ui| match gallery.phase {
                        EffectGalleryPhase::Choose => {
                            ui.label(
                                RichText::new(self.i18n.text("effects.choose_effect"))
                                    .strong()
                                    .color(Color32::from_rgb(240, 244, 250)),
                            );
                            ui.add_space(6.0);
                            if descriptors.is_empty() {
                                ui.label(
                                    RichText::new(self.i18n.text("effects.no_available"))
                                        .color(Color32::from_rgb(180, 195, 215)),
                                );
                            } else if ui.available_width() < 440.0 {
                                for descriptor in &descriptors {
                                    let selected = gallery.effect_id == descriptor.id;
                                    let summary = format!(
                                        "{} · {}",
                                        self.tf(
                                            "effects.parameter_count",
                                            &[("count", descriptor.parameters.len().to_string())],
                                        ),
                                        self.i18n.text("effects.port_flow"),
                                    );
                                    if effect_gallery_card(
                                        document, ui, descriptor, summary, selected,
                                    ) {
                                        gallery.select_effect(descriptor);
                                    }
                                    ui.add_space(8.0);
                                }
                            } else {
                                ui.columns(2, |columns| {
                                    for (index, descriptor) in descriptors.iter().enumerate() {
                                        let column = &mut columns[index % 2];
                                        let selected = gallery.effect_id == descriptor.id;
                                        let summary = format!(
                                            "{} · {}",
                                            self.tf(
                                                "effects.parameter_count",
                                                &[(
                                                    "count",
                                                    descriptor.parameters.len().to_string(),
                                                )],
                                            ),
                                            self.i18n.text("effects.port_flow"),
                                        );
                                        if effect_gallery_card(
                                            document, column, descriptor, summary, selected,
                                        ) {
                                            gallery.select_effect(descriptor);
                                        }
                                        column.add_space(8.0);
                                    }
                                });
                            }
                        }
                        EffectGalleryPhase::Configure => {
                            let selected_descriptor = descriptors
                                .iter()
                                .find(|descriptor| descriptor.id == gallery.effect_id);
                            if let Some(descriptor) = selected_descriptor {
                                show_effect_initial_settings(
                                    document,
                                    ui,
                                    descriptor,
                                    &mut gallery,
                                    initial_settings_label.clone(),
                                    setup_hint.clone(),
                                    enabled_label.clone(),
                                );
                            } else {
                                ui.label(
                                    RichText::new(self.i18n.text("effects.no_available"))
                                        .color(Color32::from_rgb(180, 195, 215)),
                                );
                            }
                        }
                    },
                );

                ui.add_space(10.0);
                ui.separator();
                ui.horizontal(|ui| {
                    if document_button(
                        document,
                        ui,
                        "modals.effects.cancel",
                        self.i18n.text("effects.cancel"),
                        true,
                    ) {
                        cancel = true;
                    }
                    if gallery.phase == EffectGalleryPhase::Configure
                        && document_button(
                            document,
                            ui,
                            "modals.effects.back",
                            self.i18n.text("effects.back"),
                            true,
                        )
                    {
                        back = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        match gallery.phase {
                            EffectGalleryPhase::Choose => {
                                if document_button(
                                    document,
                                    ui,
                                    "modals.effects.next",
                                    self.i18n.text("effects.next"),
                                    supports_effect_nodes && !gallery.effect_id.is_empty(),
                                ) {
                                    next = true;
                                }
                            }
                            EffectGalleryPhase::Configure => {
                                if document_button(
                                    document,
                                    ui,
                                    "modals.effects.create_node",
                                    self.i18n.text("effects.create_node"),
                                    supports_effect_nodes && !gallery.effect_id.is_empty(),
                                ) {
                                    create = true;
                                }
                            }
                        }
                    });
                });
            },
        );
        self.ui_document = document;

        if dialog_response.backdrop_clicked {
            self.effect_gallery = None;
            return;
        }

        if back {
            gallery.previous_phase();
        } else if next {
            gallery.next_phase();
        }
        if create && self.create_effect_from_gallery(&gallery) {
            return;
        }
        if !cancel {
            self.effect_gallery = Some(gallery);
        }
    }
}
