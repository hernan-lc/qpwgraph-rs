use super::QpwgraphApp;
use pw_graph_backend::{EffectInsertRequest, GraphDriver};
use pw_graph_config::PersistedEffect;
use pw_graph_core::{LinkId, PortType};
use pw_graph_effects::NOISE_GATE_ID;
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

impl QpwgraphApp {
    pub(crate) fn insert_selected_noise_gate(&mut self) {
        let Some(link_id) = self.canvas.selected_link() else {
            self.status = self.t("effects.select_link");
            return;
        };
        let Some(link) = self.driver.graph().link(link_id).cloned() else {
            self.status = self.t("effects.link_unavailable");
            return;
        };
        let Some(output) = self.driver.graph().port(link.output_port) else {
            self.status = self.t("effects.link_unavailable");
            return;
        };
        let Some(input) = self.driver.graph().port(link.input_port) else {
            self.status = self.t("effects.link_unavailable");
            return;
        };
        if output.port_type != PortType::Audio || input.port_type != PortType::Audio {
            self.status = self.t("effects.audio_only");
            return;
        }
        let Some(source) = self.driver.graph().port_key(link.output_port) else {
            self.status = self.t("effects.link_unavailable");
            return;
        };
        let Some(destination) = self.driver.graph().port_key(link.input_port) else {
            self.status = self.t("effects.link_unavailable");
            return;
        };
        let instance_id = unique_effect_id();
        let request = EffectInsertRequest {
            instance_id: instance_id.clone(),
            effect_id: NOISE_GATE_ID.into(),
            module_path: None,
            source: source.clone(),
            destination: destination.clone(),
            enabled: true,
            parameters: BTreeMap::new(),
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

#[allow(dead_code)]
fn _link_type_is_audio(driver: &dyn GraphDriver, link_id: LinkId) -> bool {
    let Some(link) = driver.graph().link(link_id) else {
        return false;
    };
    driver
        .graph()
        .port(link.output_port)
        .is_some_and(|port| port.port_type == PortType::Audio)
}
