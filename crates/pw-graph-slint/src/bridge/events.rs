use slint::VecModel;
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use super::actions::handle_action;
use super::app::{set_connection_feedback, PreviewApp, UiEvent};
use super::config::{autosave_config, read_window_state};
use super::connections::{easy_connect_from_pin, easy_connect_nodes, handle_link_requested};
use super::meters::refresh_meters;
use super::models::{shortcut_rows, sync_models};
use super::persist::{autosave_slint_state, restore_missing_audio_controls};
use super::relay::poll_relay_events;
use super::utils::volume_from_track_position;
use super::{CanvasGeometry, LinkRow, MainWindow, MinimapNode, NodeRow, ShortcutRow};

#[allow(clippy::too_many_arguments)]
pub(crate) fn pump(
    window: &MainWindow,
    app: &Rc<RefCell<PreviewApp>>,
    nodes: &Rc<VecModel<NodeRow>>,
    links: &Rc<VecModel<LinkRow>>,
    minimap_nodes: &Rc<VecModel<MinimapNode>>,
    shortcuts: &Rc<VecModel<ShortcutRow>>,
    events: &Rc<RefCell<Vec<UiEvent>>>,
    geometry: &Rc<RefCell<CanvasGeometry>>,
    geometry_version: &Rc<Cell<i32>>,
) {
    let pending = coalesce_audio_volume_events(std::mem::take(&mut *events.borrow_mut()));
    let mut preview = app.borrow_mut();
    read_window_state(window, &mut preview);
    for event in pending {
        process_event(window, &mut preview, event);
    }
    poll_relay_events(&mut preview);
    if preview.source.graph_dirty() || preview.last_refresh.elapsed() >= Duration::from_millis(500)
    {
        if let Err(error) = preview.source.refresh() {
            preview.status = format!("Could not refresh graph: {error}");
        } else {
            preview.last_refresh = Instant::now();
        }
    }
    restore_missing_audio_controls(&mut preview);
    refresh_meters(window, &mut preview);
    autosave_config(&mut preview);
    autosave_slint_state(&mut preview);
    shortcuts.set_vec(shortcut_rows(
        &preview.i18n,
        window.get_shortcut_search().as_str(),
    ));
    sync_models(
        window,
        &mut preview,
        nodes,
        links,
        minimap_nodes,
        geometry,
        geometry_version,
    );
}

pub(crate) fn coalesce_audio_volume_events(pending: Vec<UiEvent>) -> Vec<UiEvent> {
    let mut compacted = Vec::with_capacity(pending.len());
    let mut volume_indices = BTreeMap::<i32, usize>::new();
    for event in pending {
        match event {
            UiEvent::SetAudioVolume(id, position) => {
                if let Some(index) = volume_indices.get(&id).copied() {
                    compacted[index] = UiEvent::SetAudioVolume(id, position);
                } else {
                    volume_indices.insert(id, compacted.len());
                    compacted.push(UiEvent::SetAudioVolume(id, position));
                }
            }
            event => compacted.push(event),
        }
    }
    compacted
}

pub(crate) fn process_event(window: &MainWindow, preview: &mut PreviewApp, event: UiEvent) {
    match event {
        UiEvent::Action(action) => handle_action(window, preview, &action),
        UiEvent::SelectNode(id, shift) => preview.view.select_node(id, shift),
        UiEvent::SelectLink(id, shift) => preview.view.select_link(id, shift),
        UiEvent::ClearSelection => preview.view.clear_selection(),
        UiEvent::SelectBox(x, y, width, height, shift) => {
            preview
                .view
                .select_box(&preview.snapshot, x, y, width, height, shift)
        }
        UiEvent::LinkRequested(start, end) => handle_link_requested(preview, start, end),
        UiEvent::LinkCancelled => {
            preview.pending_connection_pin = None;
            set_connection_feedback(preview, "Connection preview cancelled", false);
        }
        UiEvent::NodeConnectDropped(source, x, y, target_pin) => {
            preview.pending_connection_pin = None;
            easy_connect_nodes(preview, source, x, y, target_pin)
        }
        UiEvent::LinkDropped(source_pin, x, y) => {
            preview.pending_connection_pin = None;
            easy_connect_from_pin(preview, source_pin, x, y)
        }
        UiEvent::ToggleCollapse(id) => {
            preview.view.toggle_local_collapse(id, &preview.snapshot);
            preview.status = "Node expansion changed; configuration will be saved".into();
        }
        UiEvent::DragCommitted(id, dx, dy) => {
            preview.view.move_selected(id, dx, dy, &preview.snapshot);
            preview.status = "Node arrangement changed; configuration will be saved".into();
        }
        UiEvent::SetAudioVolume(id, position) => {
            if let Some(node_id) = preview.view.ids.node_id(id) {
                let position = position.clamp(0.0, 1.0);
                let volume = volume_from_track_position(position);
                match preview.source.set_node_volume(node_id, volume) {
                    Ok(()) => {
                        preview
                            .audio_controls
                            .entry(node_id)
                            .or_default()
                            .volume_position = position;
                        preview.status = format!("Node volume: {:.0}%", volume * 100.0);
                    }
                    Err(error) => preview.status = format!("Could not change node volume: {error}"),
                }
            }
        }
        UiEvent::ToggleAudioMute(id) => {
            if let Some(node_id) = preview.view.ids.node_id(id) {
                let muted = !preview
                    .audio_controls
                    .get(&node_id)
                    .copied()
                    .unwrap_or_default()
                    .muted;
                match preview.source.set_node_mute(node_id, muted) {
                    Ok(()) => {
                        preview.audio_controls.entry(node_id).or_default().muted = muted;
                        preview.status = if muted {
                            "Node muted".into()
                        } else {
                            "Node unmuted".into()
                        };
                    }
                    Err(error) => preview.status = format!("Could not change node mute: {error}"),
                }
            }
        }
    }
}
