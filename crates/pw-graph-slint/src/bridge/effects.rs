use crate::source::ReadOnlyGraphSource;
use pw_graph_backend::{EffectInsertRequest, EffectInstance, EffectNodeRequest};
use pw_graph_config::{AppConfig, PersistedEffect};
use slint::SharedString;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use super::app::PreviewApp;
use super::{EffectRow, MainWindow};

pub(crate) fn restore_configured_effects(
    source: &mut ReadOnlyGraphSource,
    config: &AppConfig,
    status: &mut String,
) {
    if config.effects.is_empty() {
        return;
    }
    if !source.supports_effect_nodes() {
        *status = format!(
            "{status} · {} saved effect(s) could not be restored: effect processing is unavailable",
            config.effects.len()
        );
        return;
    }

    for saved in config.effects.iter().cloned() {
        let result = match (&saved.source, &saved.destination) {
            (Some(source_port), Some(destination_port)) => source
                .connect_by_key_if_missing(source_port, destination_port)
                .map(|_| ())
                .and_then(|_| {
                    source.insert_effect(EffectInsertRequest {
                        instance_id: saved.instance.instance_id.clone(),
                        effect_id: saved.instance.effect_id.clone(),
                        module_path: saved.instance.module_path.clone(),
                        source: source_port.clone(),
                        destination: destination_port.clone(),
                        enabled: saved.instance.enabled,
                        parameters: saved.instance.parameters.clone(),
                        position: saved.position,
                    })
                }),
            (None, None) => source.create_effect_node(EffectNodeRequest {
                instance_id: saved.instance.instance_id.clone(),
                effect_id: saved.instance.effect_id.clone(),
                module_path: saved.instance.module_path.clone(),
                enabled: saved.instance.enabled,
                parameters: saved.instance.parameters.clone(),
                position: saved.position,
            }),
            _ => Err("effect routing is incomplete".into()),
        };
        if let Err(error) = result {
            *status = format!("{status} · Could not restore effect: {error}");
        }
    }
}

pub(crate) fn create_effect(window: &MainWindow, preview: &mut PreviewApp) {
    if !preview.source.supports_effect_nodes() {
        preview.status = "Effect processing is not available for this backend".into();
        return;
    }
    let descriptors = preview.source.effect_descriptors();
    let Some(descriptor) = descriptors
        .get(window.get_effect_selection_index().max(0) as usize)
        .or_else(|| descriptors.first())
    else {
        preview.status = "No effects are available".into();
        return;
    };

    let instance_id = unique_effect_id(preview);
    let parameters = descriptor
        .parameters
        .iter()
        .map(|parameter| (parameter.id.clone(), parameter.default))
        .collect();
    let request = EffectNodeRequest {
        instance_id,
        effect_id: descriptor.id.clone(),
        module_path: None,
        enabled: true,
        parameters,
        position: preferred_effect_position(preview),
    };
    match preview.source.create_effect_node(request) {
        Ok(instance) => {
            let name = descriptor.name.clone();
            persist_effect(preview, instance);
            match preview.source.refresh() {
                Ok(()) => preview.last_refresh = Instant::now(),
                Err(error) => {
                    preview.status = format!("Effect created, but graph refresh failed: {error}");
                    return;
                }
            }
            preview.status = format!("Effect created: {name}");
        }
        Err(error) => preview.status = format!("Could not create effect: {error}"),
    }
}

pub(crate) fn toggle_effect(preview: &mut PreviewApp, instance_id: &str) {
    let Some(instance) = preview
        .source
        .effect_instances()
        .into_iter()
        .find(|instance| instance.config.instance_id == instance_id)
    else {
        preview.status = format!("Effect instance not found: {instance_id}");
        return;
    };
    let enabled = !instance.config.enabled;
    match preview.source.set_effect_enabled(instance_id, enabled) {
        Ok(()) => {
            if let Some(saved) = preview
                .config
                .effects
                .iter_mut()
                .find(|effect| effect.instance.instance_id == instance_id)
            {
                saved.instance.enabled = enabled;
            }
            preview.status = format!(
                "Effect {}: {}",
                instance_id,
                if enabled { "enabled" } else { "bypassed" }
            );
        }
        Err(error) => preview.status = format!("Could not change effect state: {error}"),
    }
}

pub(crate) fn set_effect_parameter(preview: &mut PreviewApp, details: &str) {
    let Some((details, value)) = details.rsplit_once(':') else {
        preview.status = "Invalid effect parameter action".into();
        return;
    };
    let Some((instance_id, parameter)) = details.rsplit_once(':') else {
        preview.status = "Invalid effect parameter action".into();
        return;
    };
    let Ok(value) = value.parse::<f32>() else {
        preview.status = "Invalid effect parameter value".into();
        return;
    };
    match preview
        .source
        .set_effect_parameter(instance_id, parameter, value)
    {
        Ok(()) => {
            if let Some(saved) = preview
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
            preview.status = format!("{instance_id} · {parameter} = {value:.2}");
        }
        Err(error) => preview.status = format!("Could not change effect parameter: {error}"),
    }
}

pub(crate) fn remove_effect(preview: &mut PreviewApp, instance_id: &str) {
    match preview.source.remove_effect(instance_id) {
        Ok(()) => {
            preview
                .config
                .effects
                .retain(|effect| effect.instance.instance_id != instance_id);
            if let Err(error) = preview.source.refresh() {
                preview.status = format!("Effect removed, but graph refresh failed: {error}");
            } else {
                preview.last_refresh = Instant::now();
                preview.status = format!("Effect removed: {instance_id}");
            }
        }
        Err(error) => preview.status = format!("Could not remove effect: {error}"),
    }
}

pub(crate) fn inspect_effect(preview: &mut PreviewApp, instance_id: Option<&str>) {
    let instance = match instance_id {
        Some(instance_id) => preview
            .source
            .effect_instances()
            .into_iter()
            .find(|instance| instance.config.instance_id == instance_id),
        None => preview.source.effect_instances().into_iter().next(),
    };
    let Some(instance) = instance else {
        preview.status = "No effect instance is available".into();
        return;
    };
    let descriptor = preview
        .source
        .effect_descriptors()
        .into_iter()
        .find(|descriptor| descriptor.id == instance.config.effect_id);
    let name = descriptor
        .as_ref()
        .map(|descriptor| descriptor.name.as_str())
        .unwrap_or(instance.config.effect_id.as_str());
    let parameters = instance
        .config
        .parameters
        .iter()
        .map(|(id, value)| format!("{id}={value:.2}"))
        .collect::<Vec<_>>()
        .join(", ");
    preview.status = if parameters.is_empty() {
        format!("{name} · {}", instance.config.instance_id)
    } else {
        format!("{name} · {} · {parameters}", instance.config.instance_id)
    };
}

fn persist_effect(preview: &mut PreviewApp, instance: EffectInstance) {
    let position = preview
        .source
        .graph()
        .node(instance.node_id)
        .map(|node| node.position)
        .unwrap_or([260.0, 180.0]);
    preview
        .config
        .effects
        .retain(|effect| effect.instance.instance_id != instance.config.instance_id);
    preview.config.effects.push(PersistedEffect {
        instance: instance.config,
        source: instance.source,
        destination: instance.destination,
        position,
    });
}

fn preferred_effect_position(preview: &PreviewApp) -> [f32; 2] {
    let rightmost = preview
        .source
        .graph()
        .nodes
        .values()
        .map(|node| node.position[0])
        .fold(0.0_f32, f32::max);
    [rightmost + 290.0, 180.0]
}

fn unique_effect_id(preview: &PreviewApp) -> String {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    loop {
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let id = format!("slint-effect-{sequence}");
        if !preview
            .source
            .effect_instances()
            .iter()
            .any(|effect| effect.config.instance_id == id)
            && !preview
                .config
                .effects
                .iter()
                .any(|effect| effect.instance.instance_id == id)
        {
            return id;
        }
    }
}

pub(crate) fn effect_rows(source: &ReadOnlyGraphSource) -> Vec<EffectRow> {
    let descriptors = source.effect_descriptors();
    let mut instances = source.effect_instances();
    instances.sort_by(|a, b| a.config.instance_id.cmp(&b.config.instance_id));
    instances
        .into_iter()
        .map(|instance| {
            let descriptor = descriptors
                .iter()
                .find(|descriptor| descriptor.id == instance.config.effect_id);
            let name = descriptor
                .map(|descriptor| descriptor.name.clone())
                .unwrap_or_else(|| instance.config.effect_id.clone());
            let vendor = descriptor
                .map(|descriptor| descriptor.vendor.clone())
                .unwrap_or_else(|| "Unknown effect provider".into());
            let description = instance.config.instance_id.clone();
            let vendor = match instance.error {
                Some(error) => format!("{vendor} · error: {error}"),
                None => vendor,
            };
            let parameter = descriptor.and_then(|descriptor| descriptor.parameters.first());
            EffectRow {
                name: SharedString::from(name),
                vendor: SharedString::from(vendor),
                description: SharedString::from(description),
                enabled: instance.config.enabled,
                has_parameter: parameter.is_some(),
                parameter_id: SharedString::from(
                    parameter
                        .map(|parameter| parameter.id.clone())
                        .unwrap_or_default(),
                ),
                parameter_label: SharedString::from(
                    parameter
                        .map(|parameter| parameter.name.clone())
                        .unwrap_or_default(),
                ),
                parameter_minimum: parameter.map(|parameter| parameter.minimum).unwrap_or(0.0),
                parameter_maximum: parameter.map(|parameter| parameter.maximum).unwrap_or(1.0),
                parameter_value: parameter
                    .map(|parameter| {
                        instance
                            .config
                            .parameters
                            .get(&parameter.id)
                            .copied()
                            .unwrap_or(parameter.default)
                    })
                    .unwrap_or_default(),
            }
        })
        .collect()
}

pub(crate) fn effect_options(source: &ReadOnlyGraphSource) -> Vec<SharedString> {
    source
        .effect_descriptors()
        .into_iter()
        .map(|descriptor| SharedString::from(descriptor.name))
        .collect()
}
