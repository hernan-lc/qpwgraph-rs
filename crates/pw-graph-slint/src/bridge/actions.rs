use crate::model::{ConnectMode, MediaFilter};
use std::time::Instant;

use super::app::PreviewApp;
use super::config::save_config;
use super::connections::delete_selected_connections;
use super::effects::{
    create_effect, inspect_effect, remove_effect, set_effect_parameter, toggle_effect,
};
use super::relay::{
    connect_relay, disconnect_relay, relay_host_active, relay_qr_payload, start_relay_discovery,
    start_relay_host, stop_relay_discovery, stop_relay_host,
};
use super::MainWindow;

pub(crate) fn handle_action(window: &MainWindow, preview: &mut PreviewApp, action: &str) {
    match action {
        "refresh" => match preview.source.refresh() {
            Ok(()) => {
                preview.last_refresh = Instant::now();
                preview.status = preview
                    .i18n
                    .text("status.refreshed")
                    .replace("{count}", &preview.source.graph().nodes.len().to_string());
            }
            Err(error) => preview.status = format!("Could not refresh graph: {error}"),
        },
        "zoom-in" => preview.view.zoom = (preview.view.zoom * 1.1).clamp(0.35, 2.5),
        "zoom-out" => preview.view.zoom = (preview.view.zoom / 1.1).clamp(0.35, 2.5),
        "toggle-thumbnail" => {
            preview.view.thumbnail_mode = !preview.view.thumbnail_mode;
            preview.status = "Thumbnail view changed locally".into();
        }
        "toggle-minimap" => preview.view.minimap_visible = !preview.view.minimap_visible,
        "toggle-connect-mode" => {
            preview.view.connect_mode = match preview.view.connect_mode {
                ConnectMode::Advanced => ConnectMode::Easy,
                ConnectMode::Easy => ConnectMode::Advanced,
            };
            preview.status = format!(
                "{} connection mode is active locally",
                if preview.view.connect_mode == ConnectMode::Easy {
                    "Easy"
                } else {
                    "Advanced"
                }
            );
        }
        "filter-all" => preview.view.media_filter = MediaFilter::All,
        "filter-audio" => preview.view.media_filter = MediaFilter::Audio,
        "filter-video" => preview.view.media_filter = MediaFilter::Video,
        "filter-midi" => preview.view.media_filter = MediaFilter::Midi,
        "cycle-filter" => {
            preview.view.media_filter = match preview.view.media_filter {
                MediaFilter::All => MediaFilter::Audio,
                MediaFilter::Audio => MediaFilter::Video,
                MediaFilter::Video => MediaFilter::Midi,
                MediaFilter::Midi => MediaFilter::All,
            }
        }
        "arrange" => {
            let positions = preview.source.graph().default_node_positions();
            for (node_id, position) in positions {
                if let Some(ui_id) = preview.view.ids.node(node_id) {
                    preview
                        .view
                        .set_local_position(ui_id, position[0], position[1]);
                }
            }
            preview.status = "Nodes arranged; configuration will be saved".into();
        }
        "preferences" => toggle_overlay(window, Overlay::Preferences),
        "history" => toggle_overlay(window, Overlay::History),
        "shortcuts" => toggle_overlay(window, Overlay::Shortcuts),
        "effects" => toggle_overlay(window, Overlay::Effects),
        "relay" => {
            let show = !window.get_show_relay();
            window.set_show_relay(show);
            close_modals(window);
            if show {
                start_relay_discovery(preview);
            } else {
                stop_relay_discovery(preview);
            }
        }
        "relay-show-qr" => {
            if relay_qr_payload(preview).is_some() {
                window.set_show_qr(true);
            } else {
                preview.status = "Start the relay host before showing its QR code".into();
            }
        }
        "close-qr" => window.set_show_qr(false),
        "relay-connect-configured" => connect_relay(preview, None),
        "relay-connect" => connect_relay(preview, None),
        "relay-host-toggle" => {
            if relay_host_active(preview) {
                stop_relay_host(preview);
            } else {
                start_relay_host(preview);
            }
        }
        "relay-host-start" => start_relay_host(preview),
        "relay-host-stop" => stop_relay_host(preview),
        _ if action == "effect-create" || action == "create-effect" => {
            create_effect(window, preview);
        }
        _ if action == "effect-inspect" || action == "inspect-effect" => {
            inspect_effect(preview, None);
        }
        "toggle-statusbar" => window.set_show_statusbar(!window.get_show_statusbar()),
        "reset-audio" => {
            preview.source.reset_meters();
            preview.meters.clear();
            preview.status = "Audio monitoring helpers were reset".into();
        }
        "escape" => {
            close_modals(window);
            window.set_show_relay(false);
            window.set_show_qr(false);
            stop_relay_discovery(preview);
        }
        "save-config" => save_config(preview, true),
        "delete-selection" => delete_selected_connections(preview),
        "undo"
        | "redo"
        | "save-patchbay"
        | "load-patchbay"
        | "activate-patchbay"
        | "save-profile"
        | "choose-patchbay-directory"
        | "add-rule" => {
            preview.status = format!(
                "Read-only preview: {} is not available",
                action.replace('-', " ")
            );
        }
        _ if action.strip_prefix("effect-toggle:").is_some() => {
            let instance_id = action.strip_prefix("effect-toggle:").unwrap_or_default();
            toggle_effect(preview, instance_id);
        }
        _ if action.strip_prefix("effect-parameter:").is_some() => {
            let details = action.strip_prefix("effect-parameter:").unwrap_or_default();
            set_effect_parameter(preview, details);
        }
        _ if action.strip_prefix("effect-remove:").is_some() => {
            let instance_id = action.strip_prefix("effect-remove:").unwrap_or_default();
            remove_effect(preview, instance_id);
        }
        _ if action.strip_prefix("effect-inspect:").is_some() => {
            let instance_id = action.strip_prefix("effect-inspect:").unwrap_or_default();
            inspect_effect(preview, Some(instance_id));
        }
        _ if action.strip_prefix("relay-connect:").is_some() => {
            let target = action.strip_prefix("relay-connect:").unwrap_or_default();
            connect_relay(preview, Some(target));
        }
        _ if action.strip_prefix("relay-disconnect:").is_some() => {
            let session = action
                .strip_prefix("relay-disconnect:")
                .and_then(|value| value.parse::<u64>().ok());
            disconnect_relay(preview, session);
        }
        _ => {
            preview.status = format!("Read-only preview: {action} is not available");
        }
    }
    if preview.debug {
        eprintln!("[qpwgraph-slint] {}", preview.status);
    }
}

#[derive(Clone, Copy)]
enum Overlay {
    Preferences,
    History,
    Shortcuts,
    Effects,
}

fn toggle_overlay(window: &MainWindow, overlay: Overlay) {
    let currently_open = match overlay {
        Overlay::Preferences => window.get_show_preferences(),
        Overlay::History => window.get_show_history(),
        Overlay::Shortcuts => window.get_show_shortcuts(),
        Overlay::Effects => window.get_show_effects(),
    };
    close_modals(window);
    match overlay {
        Overlay::Preferences => window.set_show_preferences(!currently_open),
        Overlay::History => window.set_show_history(!currently_open),
        Overlay::Shortcuts => window.set_show_shortcuts(!currently_open),
        Overlay::Effects => window.set_show_effects(!currently_open),
    }
}

fn close_modals(window: &MainWindow) {
    window.set_show_preferences(false);
    window.set_show_history(false);
    window.set_show_shortcuts(false);
    window.set_show_effects(false);
}
