use super::QpwgraphApp;
use pw_graph_backend::{EffectInsertRequest, GraphDriver};
use pw_graph_config::PersistedEffect;
use pw_graph_core::{NodeId, PortType};
use pw_graph_effects::EffectDescriptor;
use pw_graph_ui::{EffectNodeControl, EffectNodeParameter};
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) fn default_parameters(descriptor: &EffectDescriptor) -> BTreeMap<String, f32> {
    descriptor
        .parameters
        .iter()
        .map(|parameter| (parameter.id.clone(), parameter.default))
        .collect()
}

pub(crate) fn available_descriptors(driver: &dyn GraphDriver) -> Vec<EffectDescriptor> {
    let descriptors = driver.effect_descriptors();
    if descriptors.is_empty() {
        pw_graph_effects::EffectHost::new().descriptors()
    } else {
        descriptors
    }
}

impl QpwgraphApp {
    /// Copy backend effect metadata into the canvas model so effect nodes can
    /// render their controls without opening a second editor window.
    pub(crate) fn sync_effect_controls(&mut self) {
        let descriptors = available_descriptors(self.driver.as_ref());
        let controls = self
            .driver
            .effect_instances()
            .into_iter()
            .filter_map(|instance| {
                let descriptor = descriptors
                    .iter()
                    .find(|descriptor| descriptor.id == instance.config.effect_id)?;
                let parameters = descriptor
                    .parameters
                    .iter()
                    .map(|parameter| EffectNodeParameter {
                        id: parameter.id.clone(),
                        name: parameter.name.clone(),
                        minimum: parameter.minimum,
                        maximum: parameter.maximum,
                        value: instance
                            .config
                            .parameters
                            .get(&parameter.id)
                            .copied()
                            .unwrap_or(parameter.default),
                        unit: parameter.unit.clone(),
                        boolean: parameter.unit == "boolean",
                    })
                    .collect();
                Some((
                    instance.node_id,
                    EffectNodeControl {
                        enabled: instance.config.enabled,
                        parameters,
                    },
                ))
            })
            .collect();
        self.canvas.set_effect_controls(controls);
    }

    pub(crate) fn add_selected_effect(&mut self) {
        let Some(link_id) = self.canvas.selected_link() else {
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
        let descriptors = available_descriptors(self.driver.as_ref());
        let Some(descriptor) = descriptors
            .iter()
            .find(|descriptor| descriptor.id == self.effect_to_add)
            .or_else(|| descriptors.first())
        else {
            self.status = self.t("effects.no_available");
            return;
        };
        let request = EffectInsertRequest {
            instance_id: unique_effect_id(),
            effect_id: descriptor.id.clone(),
            module_path: None,
            source,
            destination,
            enabled: true,
            parameters: default_parameters(descriptor),
        };
        match self.driver.insert_effect(request) {
            Ok(instance) => {
                self.config.effects.push(PersistedEffect {
                    instance: instance.config,
                    source: instance.source,
                    destination: instance.destination,
                });
                self.status = self.t("effects.inserted");
                self.canvas.clear_selected_link();
            }
            Err(error) => {
                self.status = self.tf("effects.insert_failed", &[("error", error.to_string())]);
            }
        }
    }

    pub(crate) fn set_effect_enabled_for_node(&mut self, node_id: NodeId, enabled: bool) {
        let Some(instance_id) = self.effect_instance_id(node_id) else {
            self.status = self.t("effects.not_found");
            return;
        };
        match self.driver.set_effect_enabled(&instance_id, enabled) {
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

    pub(crate) fn set_effect_parameter_for_node(
        &mut self,
        node_id: NodeId,
        parameter: &str,
        value: f32,
    ) {
        let Some(instance_id) = self.effect_instance_id(node_id) else {
            self.status = self.t("effects.not_found");
            return;
        };
        match self
            .driver
            .set_effect_parameter(&instance_id, parameter, value)
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

    pub(crate) fn remove_effect_node(&mut self, node_id: NodeId) {
        let Some(instance_id) = self.effect_instance_id(node_id) else {
            self.status = self.t("effects.not_found");
            return;
        };
        match self.driver.remove_effect(&instance_id) {
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

    fn effect_instance_id(&self, node_id: NodeId) -> Option<String> {
        self.driver
            .effect_instances()
            .into_iter()
            .find(|instance| instance.node_id == node_id)
            .map(|instance| instance.config.instance_id)
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
