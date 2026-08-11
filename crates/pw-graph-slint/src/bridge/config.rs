use pw_graph_i18n::I18n;
use std::time::{Duration, Instant};

use super::app::PreviewApp;
use super::relay::{
    relay_codec_from_index, relay_frame_from_index, relay_role_from_index,
    relay_transport_from_index,
};
use super::utils::{language_code, meter_policy_from_index};
use super::MainWindow;

pub(crate) fn read_window_state(window: &MainWindow, preview: &mut PreviewApp) {
    preview.view.zoom = window.get_zoom().clamp(0.35, 2.5);
    preview.view.pan = [window.get_pan_x(), window.get_pan_y()];
    preview.view.search_query = window.get_search_text().to_string();
    preview.view.thumbnail_mode = window.get_thumbnail_view();
    preview.view.node_text_scale = window.get_node_text_scale().clamp(0.8, 2.0);
    preview.config.statusbar = window.get_show_statusbar();
    preview.config.toolbar = window.get_show_common_actions();
    preview.config.patchbay_toolbar = window.get_show_patchbay_toolbar();
    preview.config.repel_overlapping_nodes = window.get_repel_overlaps();
    preview.config.connect_through_nodes = window.get_connect_through();
    preview.config.thumbnail_view = preview.view.thumbnail_mode;
    preview.config.ui_text_scale = window.get_ui_text_scale().clamp(0.8, 2.0);
    preview.config.panel_text_scale = window.get_panel_text_scale().clamp(0.8, 2.0);
    preview.config.node_text_scale = preview.view.node_text_scale;
    preview.config.patchbay_exclusive = window.get_patchbay_exclusive();
    preview.config.patchbay_auto_disconnect = window.get_patchbay_auto_disconnect();
    preview.config.patchbay_auto_pin = window.get_patchbay_auto_pin();
    preview.config.patchbay_activated = window.get_patchbay_activated();
    let language = language_code(window.get_language_index());
    if preview.config.language != language {
        preview.config.language = language.into();
        preview.i18n = I18n::from_language(language);
        preview.status = preview.i18n.text("status.language_changed");
    }
    preview.config.window_width = window.get_width_().max(760.0);
    preview.config.window_height = window.get_height_().max(520.0);
    preview.config.relay_device_name = window.get_relay_device_name().to_string();
    preview.config.relay_host_pin = window.get_relay_host_pin().to_string();
    preview.config.relay_host_port = window
        .get_relay_host_port_text()
        .trim()
        .parse::<u16>()
        .unwrap_or(preview.config.relay_host_port);
    preview.config.relay_client_target = window.get_relay_client_target().to_string();
    preview.config.relay_client_pin = window.get_relay_client_pin().to_string();
    preview.config.relay_role = relay_role_from_index(window.get_relay_role_index()).into();
    preview.config.relay_codec = relay_codec_from_index(window.get_relay_codec_index()).into();
    preview.config.relay_frame_ms = relay_frame_from_index(window.get_relay_frame_index());
    preview.config.relay_transport =
        relay_transport_from_index(window.get_relay_transport_index()).into();

    let meter_policy = meter_policy_from_index(window.get_meter_policy_index());
    if meter_policy != preview.source.meter_policy() {
        preview.config.audio_meters = meter_policy.as_str().into();
        if let Err(error) = preview.source.set_meter_policy(meter_policy) {
            preview.status = format!("Could not change audio metering policy: {error}");
        } else {
            preview.meters.clear();
            preview.status = format!(
                "Audio metering is {} for this preview",
                meter_policy.as_str()
            );
        }
    }
}

fn sync_config(preview: &mut PreviewApp) {
    preview.config.zoom = preview.view.zoom;
    preview.config.thumbnail_view = preview.view.thumbnail_mode;
    preview.config.minimap_visible = preview.view.minimap_visible;
    preview.config.connect_mode = preview.view.connect_mode.as_str().into();
    preview.config.media_filter = preview.view.media_filter.as_str().into();
    preview.config.graph_search = preview.view.search_query.clone();
    preview.config.node_text_scale = preview.view.node_text_scale;
    preview
        .view
        .write_to_config(preview.source.graph(), &mut preview.config);
}

pub(crate) fn autosave_config(preview: &mut PreviewApp) {
    sync_config(preview);
    if preview.config == preview.config_saved_snapshot {
        preview.config_dirty_since = None;
        return;
    }
    let dirty_since = preview.config_dirty_since.get_or_insert_with(Instant::now);
    if dirty_since.elapsed() >= Duration::from_millis(500) {
        save_config(preview, false);
    }
}

pub(crate) fn save_config(preview: &mut PreviewApp, report_success: bool) {
    sync_config(preview);
    match preview.config.save_to(&preview.config_file) {
        Ok(()) => {
            preview.config_saved_snapshot = preview.config.clone();
            preview.config_dirty_since = None;
            if report_success {
                preview.status =
                    format!("Configuration saved to {}", preview.config_file.display());
            }
        }
        Err(error) => {
            preview.status = format!("Could not save configuration: {error}");
            preview.config_dirty_since = Some(Instant::now());
        }
    }
}
