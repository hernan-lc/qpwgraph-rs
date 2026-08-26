use pw_graph_i18n::I18n;
use std::time::{Duration, Instant};

use super::app::Application;
use super::patchbay::activate_patchbay;
use super::relay::{
    relay_codec_from_index, relay_frame_from_index, relay_role_from_index,
    relay_transport_from_index,
};
use super::utils::{language_code, localized_meter_policy, meter_policy_from_index};
use super::MainWindow;

pub(crate) fn read_window_state(window: &MainWindow, application: &mut Application) {
    let patchbay_was_activated = application.config.patchbay_activated;
    application.view.zoom = window.get_zoom().clamp(0.35, 2.5);
    application.view.pan = [window.get_pan_x(), window.get_pan_y()];
    application.view.search_query = window.get_search_text().to_string();
    application.view.thumbnail_mode = window.get_thumbnail_view();
    application.view.node_text_scale = window.get_node_text_scale().clamp(0.8, 2.0);
    application.config.statusbar = window.get_show_statusbar();
    application.config.toolbar = window.get_show_common_actions();
    application.config.patchbay_toolbar = window.get_show_patchbay_toolbar();
    application.config.repel_overlapping_nodes = window.get_repel_overlaps();
    application.config.connect_through_nodes = window.get_connect_through();
    application.config.thumbnail_view = application.view.thumbnail_mode;
    application.config.ui_text_scale = window.get_ui_text_scale().clamp(0.8, 2.0);
    application.config.panel_text_scale = window.get_panel_text_scale().clamp(0.8, 2.0);
    application.config.node_text_scale = application.view.node_text_scale;
    application.config.patchbay_exclusive = window.get_patchbay_exclusive();
    application.config.patchbay_auto_disconnect = window.get_patchbay_auto_disconnect();
    application.config.patchbay_auto_pin = window.get_patchbay_auto_pin();
    application.config.patchbay_activated = window.get_patchbay_activated();
    application.config.active_patchbay_profile = window.get_profile_name().to_string();
    let language = language_code(window.get_language_index());
    if application.config.language != language {
        application.config.language = language.into();
        application.i18n = I18n::from_language(language);
        application.status = application.i18n.text("status.language_changed");
    }
    application.config.window_width = window.get_width_().max(760.0);
    application.config.window_height = window.get_height_().max(520.0);
    application.config.relay_device_name = window.get_relay_device_name().to_string();
    application.config.relay_host_pin = window.get_relay_host_pin().to_string();
    application.config.relay_host_port = window
        .get_relay_host_port_text()
        .trim()
        .parse::<u16>()
        .unwrap_or(application.config.relay_host_port);
    application.config.relay_client_target = window.get_relay_client_target().to_string();
    application.config.relay_client_pin = window.get_relay_client_pin().to_string();
    application.config.relay_role = relay_role_from_index(window.get_relay_role_index()).into();
    application.config.relay_codec = relay_codec_from_index(window.get_relay_codec_index()).into();
    application.config.relay_frame_ms = relay_frame_from_index(window.get_relay_frame_index());
    application.config.relay_transport =
        relay_transport_from_index(window.get_relay_transport_index()).into();

    let meter_policy = meter_policy_from_index(window.get_meter_policy_index());
    if meter_policy != application.source.meter_policy() {
        application.config.audio_meters = meter_policy.as_str().into();
        if let Err(error) = application.source.set_meter_policy(meter_policy) {
            application.status = application.tf("status.meter_policy_failed", &[("error", error)]);
        } else {
            application.meters.clear();
            application.status = application.tf(
                "status.meter_policy_changed",
                &[(
                    "policy",
                    localized_meter_policy(&application.i18n, meter_policy),
                )],
            );
        }
    }

    if !patchbay_was_activated && application.config.patchbay_activated {
        activate_patchbay(application);
    }
}

fn sync_config(application: &mut Application) {
    application.config.zoom = application.view.zoom;
    application.config.thumbnail_view = application.view.thumbnail_mode;
    application.config.minimap_visible = application.view.minimap_visible;
    application.config.connect_mode = application.view.connect_mode.as_str().into();
    application.config.media_filter = application.view.media_filter.as_str().into();
    application.config.graph_search = application.view.search_query.clone();
    application.config.node_text_scale = application.view.node_text_scale;
    application.config.sort_type = if application.view.sort_ports_by_name {
        "name"
    } else {
        "id"
    }
    .into();
    application.config.sort_order = if application.view.sort_ports_descending {
        "descending"
    } else {
        "ascending"
    }
    .into();
    application
        .view
        .write_to_config(application.source.graph(), &mut application.config);
}

pub(crate) fn autosave_config(application: &mut Application) {
    sync_config(application);
    if application.config == application.config_saved_snapshot {
        application.config_dirty_since = None;
        return;
    }
    let dirty_since = application
        .config_dirty_since
        .get_or_insert_with(Instant::now);
    if dirty_since.elapsed() >= Duration::from_millis(500) {
        save_config(application, false);
    }
}

pub(crate) fn save_config(application: &mut Application, report_success: bool) {
    sync_config(application);
    match application.config.save_to(&application.config_file) {
        Ok(()) => {
            application.config_saved_snapshot = application.config.clone();
            application.config_dirty_since = None;
            if report_success {
                application.status = application.tf(
                    "status.config_saved_to",
                    &[("path", application.config_file.display().to_string())],
                );
            }
        }
        Err(error) => {
            application.status =
                application.tf("status.config_save_failed", &[("error", error.to_string())]);
            application.config_dirty_since = Some(Instant::now());
        }
    }
}
