use crate::canvas::{self, CanvasGeometry, LinkGeometry, NodeGeometry, PinGeometry};
use crate::model::{
    node_type_color, port_type_color, ConnectMode, GraphSnapshot, LinkView, MeterState, NodeView,
};
use pw_graph_config::{config_path, AppConfig};
use pw_graph_core::Direction;
use pw_graph_i18n::I18n;
use pw_graph_patchbay::Patchbay;
#[cfg(not(feature = "relay"))]
use slint::Image;
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use super::app::{toast_visible, PreviewApp, PreviewAudioControl};
use super::effects::{effect_options, effect_rows};
use super::meters::meter_fallback;
#[cfg(feature = "relay")]
use super::relay::{qr_image, relay_host_endpoint};
use super::relay::{
    relay_codec_index, relay_frame_index, relay_nodes_visible, relay_qr_payload, relay_role_index,
    relay_rows, relay_transport_index,
};
use super::utils::{
    color, language_index, localized_meter_label, localized_node_type, meter_fraction,
    meter_policy_index,
};
use super::{LinkRow, MainWindow, MinimapNode, NodeRow, PortRow, RuleRow, ShortcutRow, UiI18n};

pub(crate) fn sync_models(
    window: &MainWindow,
    preview: &mut PreviewApp,
    nodes: &Rc<VecModel<NodeRow>>,
    links: &Rc<VecModel<LinkRow>>,
    minimap_nodes: &Rc<VecModel<MinimapNode>>,
    geometry: &Rc<RefCell<CanvasGeometry>>,
    geometry_version: &Rc<Cell<i32>>,
) {
    preview.view.relay_nodes_visible = relay_nodes_visible(preview);
    let snapshot = preview.view.snapshot_with_meters(
        preview.source.graph(),
        &preview.config,
        &preview.meters,
        meter_fallback(&preview.source),
    );
    let node_rows = snapshot
        .nodes
        .iter()
        .map(|node| {
            node_row(
                node,
                preview
                    .audio_controls
                    .get(&node.node_id)
                    .copied()
                    .unwrap_or_default(),
                &preview.i18n,
            )
        })
        .collect::<Vec<_>>();
    let rows_applied = sync_node_rows(window, nodes, node_rows);
    links.set_vec(snapshot.links.iter().map(link_row).collect::<Vec<_>>());
    minimap_nodes.set_vec(
        snapshot
            .nodes
            .iter()
            .map(|node| MinimapNode {
                id: node.id,
                x: node.position[0],
                y: node.position[1],
                width: node.width,
                height: node.height,
                color: color(
                    node.appearance
                        .color
                        .unwrap_or_else(|| node_type_color(node.node_type)),
                ),
            })
            .collect::<Vec<_>>(),
    );
    // Only refresh the cache when the rendered rows were refreshed with it,
    // so a gesture in flight can never hit-test against geometry the user
    // cannot see.
    if rows_applied || geometry.borrow().is_empty() {
        rebuild_geometry(geometry, &snapshot, preview.view.connect_mode);
        geometry_version.set(geometry_version.get().wrapping_add(1));
        window.set_geometry_version(geometry_version.get());
        let bounds = graph_bounds(&snapshot);
        window.set_graph_min_x(bounds[0]);
        window.set_graph_min_y(bounds[1]);
        window.set_graph_max_x(bounds[2]);
        window.set_graph_max_y(bounds[3]);
    }
    let (node_count, port_count, link_count) = preview.view.visible_counts(&snapshot);
    window.set_status(SharedString::from(preview.status.clone()));
    window.set_toast_message(SharedString::from(preview.toast_message.clone()));
    window.set_toast_visible(toast_visible(preview));
    window.set_toast_error(preview.toast_error);
    window.set_backend(SharedString::from(preview.source.backend_name()));
    window.set_graph_counts(SharedString::from(format!(
        "{node_count} nodes · {port_count} ports · {link_count} links"
    )));
    window.set_show_minimap(preview.view.minimap_visible);
    window.set_media_filter(SharedString::from(preview.view.media_filter.as_str()));
    window.set_connect_mode(SharedString::from(preview.view.connect_mode.as_str()));
    window.set_thumbnail_view(preview.view.thumbnail_mode);
    window.set_show_common_actions(preview.config.toolbar);
    window.set_show_patchbay_toolbar(preview.config.patchbay_toolbar);
    window.set_repel_overlaps(preview.config.repel_overlapping_nodes);
    window.set_connect_through(preview.config.connect_through_nodes);
    window.set_language_index(language_index(&preview.config.language));
    window
        .global::<UiI18n>()
        .set_version(language_index(&preview.config.language));
    window.set_meter_policy_index(meter_policy_index(preview.source.meter_policy()));
    window.set_ui_text_scale(preview.config.ui_text_scale);
    window.set_panel_text_scale(preview.config.panel_text_scale);
    window.set_node_text_scale(preview.view.node_text_scale);
    window.set_patchbay_exclusive(preview.config.patchbay_exclusive);
    window.set_patchbay_auto_disconnect(preview.config.patchbay_auto_disconnect);
    window.set_patchbay_auto_pin(preview.config.patchbay_auto_pin);
    window.set_patchbay_activated(preview.config.patchbay_activated);
    window.set_zoom(preview.view.zoom);
    window.set_pan_x(preview.view.pan[0]);
    window.set_pan_y(preview.view.pan[1]);
    window.set_relay_device_name(SharedString::from(preview.config.relay_device_name.clone()));
    window.set_relay_host_pin(SharedString::from(preview.config.relay_host_pin.clone()));
    window.set_relay_host_port_text(SharedString::from(
        preview.config.relay_host_port.to_string(),
    ));
    window.set_relay_client_target(SharedString::from(
        preview.config.relay_client_target.clone(),
    ));
    window.set_relay_client_pin(SharedString::from(preview.config.relay_client_pin.clone()));
    window.set_relay_role_index(relay_role_index(&preview.config.relay_role));
    window.set_relay_codec_index(relay_codec_index(&preview.config.relay_codec));
    window.set_relay_frame_index(relay_frame_index(preview.config.relay_frame_ms));
    window.set_relay_transport_index(relay_transport_index(&preview.config.relay_transport));
    window.set_effects(ModelRc::from(Rc::new(VecModel::from(effect_rows(
        &preview.source,
    )))));
    window.set_effect_options(ModelRc::from(Rc::new(VecModel::from(effect_options(
        &preview.source,
    )))));
    window.set_effects_available(preview.source.supports_effect_nodes());
    window.set_relay_rows(ModelRc::from(Rc::new(VecModel::from(relay_rows(preview)))));
    #[cfg(feature = "relay")]
    {
        let relay_status = preview.source.relay_status();
        window.set_relay_available(preview.source.relay_available());
        window.set_relay_host_active(relay_status.host_active);
        window.set_relay_host_endpoint(SharedString::from(relay_host_endpoint(
            preview,
            relay_status.host_port,
        )));
        let payload = relay_qr_payload(preview).unwrap_or_default();
        window.set_relay_qr_payload(SharedString::from(payload.clone()));
        window.set_relay_qr_image(qr_image(&payload));
    }
    #[cfg(not(feature = "relay"))]
    {
        window.set_relay_available(false);
        window.set_relay_host_active(false);
        window.set_relay_host_endpoint(SharedString::new());
        window.set_relay_qr_payload(SharedString::new());
        window.set_relay_qr_image(Image::default());
    }
    preview.snapshot = snapshot;
}

/// Replacing a Slint model invalidates its repeated component instances. Update
/// stable rows in place so the 50ms refresh timer cannot cancel pointer capture
/// between mouse-down and release. Defer structural changes during a drag.
/// Push fresh rows into the model, returning whether they were applied. A
/// gesture in flight keeps the current rows so the pointer cannot lose the
/// component it is dragging.
fn sync_node_rows(window: &MainWindow, nodes: &VecModel<NodeRow>, rows: Vec<NodeRow>) -> bool {
    let stable_shape = nodes.row_count() == rows.len()
        && rows.iter().enumerate().all(|(index, row)| {
            nodes
                .row_data(index)
                .is_some_and(|current| current.id == row.id)
        });
    if stable_shape {
        for (index, row) in rows.into_iter().enumerate() {
            nodes.set_row_data(index, row);
        }
        true
    } else if !window.get_graph_node_dragging() {
        nodes.set_vec(rows);
        true
    } else {
        false
    }
}

fn node_row(node: &NodeView, audio: PreviewAudioControl, i18n: &I18n) -> NodeRow {
    NodeRow {
        id: node.id,
        node_title: SharedString::from(node.title.clone()),
        node_subtitle: SharedString::from(localized_node_type(i18n, node.node_type)),
        x: node.position[0],
        y: node.position[1],
        width: node.width,
        height: node.height,
        selected: node.selected,
        collapsed: node.collapsed,
        thumbnail: node.thumbnail,
        font_scale: node.font_scale,
        accent: color(
            node.appearance
                .color
                .or_else(|| {
                    node.ports
                        .first()
                        .map(|port| port_type_color(port.port_type))
                })
                .unwrap_or_else(|| node_type_color(node.node_type)),
        ),
        has_audio_controls: node.has_audio_controls,
        meter_rms: node.meter.rms,
        meter_peak: node.meter.peak,
        meter_peak_position: meter_fraction(node.meter.peak),
        meter_available: matches!(node.meter.state, MeterState::Live | MeterState::Demo),
        meter_label: SharedString::from(localized_meter_label(i18n, node.meter.state)),
        audio_volume_position: audio.volume_position,
        audio_muted: audio.muted,
        ports: ModelRc::from(Rc::new(VecModel::from(
            node.ports
                .iter()
                .enumerate()
                .map(|(index, port)| {
                    let is_output = port.direction != pw_graph_core::Direction::Sink;
                    let (pin_x, pin_y) =
                        canvas::pin_offset(node.width, index, node.has_audio_controls, is_output);
                    PortRow {
                        id: port.pin_id,
                        label: SharedString::from(port.label.clone()),
                        direction: if is_output { 1 } else { 0 },
                        color: color(port_type_color(port.port_type)),
                        row_y: canvas::port_row_top(index, node.has_audio_controls),
                        pin_x,
                        pin_y,
                    }
                })
                .collect::<Vec<_>>(),
        ))),
    }
}

/// Rebuild the world-space cache the canvas hit-tests and draws against.
fn rebuild_geometry(
    geometry: &Rc<RefCell<CanvasGeometry>>,
    snapshot: &GraphSnapshot,
    connect_mode: ConnectMode,
) {
    let mut node_geometry = Vec::with_capacity(snapshot.nodes.len());
    let mut pin_geometry = Vec::new();
    for node in &snapshot.nodes {
        let pins_visible = !node.collapsed && !node.thumbnail;
        node_geometry.push(NodeGeometry {
            id: node.id,
            x: node.position[0],
            y: node.position[1],
            width: node.width,
            height: node.height,
            selected: node.selected,
            pins_visible,
        });
        for (index, port) in node.ports.iter().enumerate() {
            let is_output = port.direction != Direction::Sink;
            let (offset_x, offset_y) =
                canvas::pin_offset(node.width, index, node.has_audio_controls, is_output);
            pin_geometry.push(PinGeometry {
                pin_id: port.pin_id,
                node_id: node.id,
                is_output,
                x: node.position[0] + offset_x,
                y: node.position[1] + offset_y,
                visible: pins_visible,
                node_selected: node.selected,
            });
        }
    }
    let link_geometry = snapshot
        .links
        .iter()
        .map(|link| LinkGeometry {
            id: link.id,
            start_pin: link.start_pin_id,
            end_pin: link.end_pin_id,
        })
        .collect();
    geometry.borrow_mut().replace(
        node_geometry,
        pin_geometry,
        link_geometry,
        connect_mode == ConnectMode::Easy,
    );
}

/// Bounding box of every card, used to frame the minimap.
fn graph_bounds(snapshot: &GraphSnapshot) -> [f32; 4] {
    let mut bounds = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
    for node in &snapshot.nodes {
        bounds[0] = bounds[0].min(node.position[0]);
        bounds[1] = bounds[1].min(node.position[1]);
        bounds[2] = bounds[2].max(node.position[0] + node.width);
        bounds[3] = bounds[3].max(node.position[1] + node.height);
    }
    if snapshot.nodes.is_empty() {
        return [0.0, 0.0, 1600.0, 1200.0];
    }
    bounds
}

pub(crate) fn shortcut_rows(i18n: &I18n, query: &str) -> Vec<ShortcutRow> {
    const ENTRIES: [(&str, &str); 22] = [
        ("F1", "shortcuts.help"),
        ("Esc", "shortcuts.close_cancel"),
        ("Delete / Backspace", "shortcuts.delete_link"),
        ("Ctrl/Cmd+Z", "shortcuts.undo"),
        ("Ctrl/Cmd+Shift+Z", "shortcuts.redo"),
        ("Ctrl/Cmd+Y", "shortcuts.redo"),
        ("Ctrl/Cmd+S", "shortcuts.save_config"),
        ("Ctrl/Cmd+Shift+S", "shortcuts.save_patchbay"),
        ("Ctrl/Cmd+O", "shortcuts.load_patchbay"),
        ("Ctrl/Cmd+F", "shortcuts.search_hint"),
        ("R", "shortcuts.refresh"),
        ("A", "shortcuts.arrange"),
        ("T", "shortcuts.thumbnail"),
        ("Arrow keys", "shortcuts.pan_keyboard"),
        ("0", "shortcuts.filter_all"),
        ("1", "shortcuts.filter_audio"),
        ("2", "shortcuts.filter_video"),
        ("3", "shortcuts.filter_midi"),
        ("+ / -", "shortcuts.zoom"),
        ("Scroll", "shortcuts.scroll_pan"),
        ("Shift+Scroll", "shortcuts.scroll_pan_horizontal"),
        ("Ctrl/Cmd+Scroll", "shortcuts.scroll_zoom"),
    ];
    let query = query.trim().to_ascii_lowercase();
    ENTRIES
        .into_iter()
        .filter_map(|(keys, key)| {
            let description = i18n.text(key);
            (query.is_empty()
                || keys.to_ascii_lowercase().contains(&query)
                || description.to_ascii_lowercase().contains(&query))
            .then(|| ShortcutRow {
                keys: SharedString::from(keys),
                description: SharedString::from(description),
            })
        })
        .collect()
}

fn link_row(link: &LinkView) -> LinkRow {
    LinkRow {
        id: link.id,
        color: color(link.color),
        selected: link.selected,
    }
}

pub(crate) fn rule_rows(config: &AppConfig) -> Vec<RuleRow> {
    let path = selected_patchbay_path(config);
    Patchbay::load_from(path)
        .map(|patchbay| {
            patchbay
                .connections
                .into_iter()
                .map(|rule| RuleRow {
                    output: SharedString::from(format!(
                        "{} · {}",
                        rule.output_node, rule.output_name
                    )),
                    input: SharedString::from(format!("{} · {}", rule.input_node, rule.input_name)),
                    pinned: rule.pinned,
                })
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn selected_patchbay_path(config: &AppConfig) -> std::path::PathBuf {
    let default_file = config_path("qpwgraph-rs").with_file_name("default.qpwgraph");
    config
        .patchbay_profiles
        .get(&config.active_patchbay_profile)
        .cloned()
        .or_else(|| config.patchbay_path.clone())
        .unwrap_or(default_file)
}
