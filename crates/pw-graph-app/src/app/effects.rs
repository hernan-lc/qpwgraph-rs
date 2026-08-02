use super::QpwgraphApp;
use pw_graph_backend::{EffectInsertRequest, GraphDriver};
use pw_graph_config::PersistedEffect;
use pw_graph_core::{LinkId, PortType};
use pw_graph_effects::{EffectDescriptor, NOISE_GATE_ID};
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// State for the modal effect-creation wizard. Keeping this separate from the
/// persisted configuration means cancelling a setup never changes the saved
/// graph or effect list.
#[derive(Clone, Debug)]
pub(crate) struct EffectWizardState {
    pub(crate) step: usize,
    pub(crate) effect_id: String,
    pub(crate) link_id: Option<LinkId>,
    pub(crate) enabled: bool,
    pub(crate) parameters: BTreeMap<String, f32>,
}

impl EffectWizardState {
    fn new(descriptor: &EffectDescriptor, link_id: Option<LinkId>) -> Self {
        Self {
            step: 0,
            effect_id: descriptor.id.clone(),
            link_id,
            enabled: true,
            parameters: default_parameters(descriptor),
        }
    }
}

pub(crate) fn default_parameters(descriptor: &EffectDescriptor) -> BTreeMap<String, f32> {
    descriptor
        .parameters
        .iter()
        .map(|parameter| (parameter.id.clone(), parameter.default))
        .collect()
}

pub(crate) fn audio_link_options(driver: &dyn GraphDriver) -> Vec<(LinkId, String)> {
    driver
        .graph()
        .links
        .values()
        .filter_map(|link| {
            let source = driver.graph().port_key(link.output_port)?;
            let destination = driver.graph().port_key(link.input_port)?;
            (source.port_type == PortType::Audio && destination.port_type == PortType::Audio).then(
                || {
                    (
                        link.id,
                        format!(
                            "{} / {}  →  {} / {}",
                            source.node_name,
                            source.port_name,
                            destination.node_name,
                            destination.port_name
                        ),
                    )
                },
            )
        })
        .collect()
}

impl QpwgraphApp {
    pub(crate) fn open_effect_wizard(&mut self) {
        let mut descriptors = self.driver.effect_descriptors();
        // The native PipeWire effect host is still being connected to the
        // filter-node runtime. Keep the built-in effect visible in the setup
        // screen so the action is discoverable and can report a precise
        // backend error at commit time instead of appearing inert.
        if descriptors.is_empty() {
            descriptors = pw_graph_effects::EffectHost::new().descriptors();
        }
        let Some(descriptor) = descriptors.first() else {
            self.status = self.t("effects.no_available");
            return;
        };
        let links = audio_link_options(self.driver.as_ref());
        let selected_link = self
            .canvas
            .selected_link()
            .filter(|link_id| links.iter().any(|(id, _)| id == link_id))
            .or_else(|| links.first().map(|(id, _)| *id));
        self.show_shortcuts = false;
        self.show_history = false;
        self.show_preferences = false;
        self.effect_wizard = Some(EffectWizardState::new(descriptor, selected_link));
    }

    pub(crate) fn finish_effect_wizard(&mut self, wizard: EffectWizardState) {
        let Some(link_id) = wizard.link_id else {
            self.status = self.t("effects.select_link");
            return;
        };
        let Some(link) = self.driver.graph().link(link_id).cloned() else {
            self.status = self.t("effects.link_unavailable");
            return;
        };
        let Some(source) = self.driver.graph().port_key(link.output_port) else {
            self.status = self.t("effects.link_unavailable");
            return;
        };
        let Some(destination) = self.driver.graph().port_key(link.input_port) else {
            self.status = self.t("effects.link_unavailable");
            return;
        };
        if source.port_type != PortType::Audio || destination.port_type != PortType::Audio {
            self.status = self.t("effects.audio_only");
            return;
        }

        let instance_id = unique_effect_id();
        let request = EffectInsertRequest {
            instance_id,
            effect_id: if wizard.effect_id.is_empty() {
                NOISE_GATE_ID.into()
            } else {
                wizard.effect_id
            },
            module_path: None,
            source: source.clone(),
            destination: destination.clone(),
            enabled: wizard.enabled,
            parameters: wizard.parameters,
        };
        match self.driver.insert_effect(request) {
            Ok(instance) => {
                self.config.effects.push(PersistedEffect {
                    instance: instance.config,
                    source,
                    destination,
                });
                self.status = self.t("effects.inserted");
                self.canvas.clear_selected_link();
            }
            Err(error) => {
                self.status = self.tf("effects.insert_failed", &[("error", error.to_string())]);
            }
        }
    }

    pub(crate) fn set_effect_enabled_from_ui(&mut self, instance_id: &str, enabled: bool) {
        match self.driver.set_effect_enabled(instance_id, enabled) {
            Ok(()) => {
                if let Some(saved) = self
                    .config
                    .effects
                    .iter_mut()
                    .find(|effect| effect.instance.instance_id == instance_id)
                {
                    saved.instance.enabled = enabled;
                }
            }
            Err(error) => {
                self.status = self.tf("effects.update_failed", &[("error", error.to_string())]);
            }
        }
    }

    pub(crate) fn set_effect_parameter_from_ui(
        &mut self,
        instance_id: &str,
        parameter: &str,
        value: f32,
    ) {
        match self
            .driver
            .set_effect_parameter(instance_id, parameter, value)
        {
            Ok(()) => {
                if let Some(saved) = self
                    .config
                    .effects
                    .iter_mut()
                    .find(|effect| effect.instance.instance_id == instance_id)
                {
                    saved
                        .instance
                        .parameters
                        .insert(parameter.to_owned(), value);
                }
            }
            Err(error) => {
                self.status = self.tf("effects.update_failed", &[("error", error.to_string())]);
            }
        }
    }

    pub(crate) fn remove_effect_from_ui(&mut self, instance_id: &str) {
        match self.driver.remove_effect(instance_id) {
            Ok(()) => {
                self.config
                    .effects
                    .retain(|effect| effect.instance.instance_id != instance_id);
                self.status = self.t("effects.removed");
            }
            Err(error) => {
                self.status = self.tf("effects.remove_failed", &[("error", error.to_string())]);
            }
        }
    }

    pub(crate) fn restore_effects(&mut self) {
        let saved = self.config.effects.clone();
        for effect in saved {
            let request = EffectInsertRequest {
                instance_id: effect.instance.instance_id.clone(),
                effect_id: effect.instance.effect_id.clone(),
                module_path: effect.instance.module_path.clone(),
                source: effect.source,
                destination: effect.destination,
                enabled: effect.instance.enabled,
                parameters: effect.instance.parameters,
            };
            if let Err(error) = self.driver.insert_effect(request) {
                self.status = self.tf("effects.restore_failed", &[("error", error.to_string())]);
            }
        }
    }
}

fn unique_effect_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("effect-{nanos:x}")
}
