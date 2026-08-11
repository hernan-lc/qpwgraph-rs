use crate::model::node_layout_key;
use crate::source::ReadOnlyGraphSource;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use super::app::{PersistedSlintState, PreviewApp, PreviewAudioControl};
use super::utils::volume_from_track_position;

fn current_slint_state(preview: &PreviewApp) -> PersistedSlintState {
    let mut audio_controls = preview.state_saved_snapshot.audio_controls.clone();
    audio_controls.extend(
        preview
            .audio_controls
            .iter()
            .filter_map(|(node_id, control)| {
                preview
                    .source
                    .graph()
                    .node(*node_id)
                    .map(|node| (node_layout_key(node), *control))
            }),
    );
    PersistedSlintState { audio_controls }
}

pub(crate) fn load_slint_state(path: &std::path::Path) -> PersistedSlintState {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|contents| toml::from_str(&contents).ok())
        .unwrap_or_default()
}

pub(crate) fn restore_audio_controls(
    source: &mut ReadOnlyGraphSource,
    state: &PersistedSlintState,
) -> BTreeMap<pw_graph_core::NodeId, PreviewAudioControl> {
    let controls: Vec<_> = source
        .graph()
        .nodes
        .values()
        .filter_map(|node| {
            state
                .audio_controls
                .get(&node_layout_key(node))
                .copied()
                .map(|control| (node.id, control))
        })
        .collect();
    let mut restored = BTreeMap::new();
    for (node_id, control) in controls {
        let volume = volume_from_track_position(control.volume_position);
        if source.set_node_volume(node_id, volume).is_ok()
            && source.set_node_mute(node_id, control.muted).is_ok()
        {
            restored.insert(node_id, control);
        }
    }
    restored
}

pub(crate) fn restore_missing_audio_controls(preview: &mut PreviewApp) {
    let missing_keys: BTreeSet<_> = preview
        .source
        .graph()
        .nodes
        .values()
        .filter(|node| !preview.audio_controls.contains_key(&node.id))
        .map(node_layout_key)
        .collect();
    let missing: PersistedSlintState = PersistedSlintState {
        audio_controls: preview
            .state_saved_snapshot
            .audio_controls
            .iter()
            .filter(|(key, _)| missing_keys.contains(*key))
            .map(|(key, control)| (key.clone(), *control))
            .collect(),
    };
    let restored = restore_audio_controls(&mut preview.source, &missing);
    for (node_id, control) in restored {
        preview.audio_controls.entry(node_id).or_insert(control);
    }
}

pub(crate) fn autosave_slint_state(preview: &mut PreviewApp) {
    let state = current_slint_state(preview);
    if state == preview.state_saved_snapshot {
        preview.state_dirty_since = None;
        return;
    }
    let dirty_since = preview.state_dirty_since.get_or_insert_with(Instant::now);
    if dirty_since.elapsed() >= Duration::from_millis(500) {
        save_slint_state(preview, false);
    }
}

pub(crate) fn save_slint_state(preview: &mut PreviewApp, report_success: bool) {
    let state = current_slint_state(preview);
    let result = toml::to_string_pretty(&state)
        .map_err(|error| error.to_string())
        .and_then(|contents| {
            if let Some(parent) = preview.state_file.parent() {
                std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            std::fs::write(&preview.state_file, contents).map_err(|error| error.to_string())
        });
    match result {
        Ok(()) => {
            preview.state_saved_snapshot = state;
            preview.state_dirty_since = None;
            if report_success {
                preview.status = format!("Slint state saved to {}", preview.state_file.display());
            }
        }
        Err(error) => {
            preview.status = format!("Could not save Slint state: {error}");
            preview.state_dirty_since = Some(Instant::now());
        }
    }
}
