use crate::model::MeterState;
use pw_graph_backend::MeterPolicy;
use pw_graph_core::NodeType;
use pw_graph_i18n::I18n;
use slint::Color;

pub(crate) fn localized_node_type(i18n: &I18n, node_type: NodeType) -> String {
    let key = match node_type {
        NodeType::PipeWire => "canvas.node_type_pipewire",
        NodeType::Effect => "canvas.node_type_effect",
        NodeType::AlsaMidi => "canvas.node_type_alsa_midi",
        NodeType::Unknown => "canvas.node_type_unknown",
    };
    i18n.text(key)
}

pub(crate) fn localized_meter_label(i18n: &I18n, state: MeterState) -> String {
    let key = match state {
        MeterState::Unavailable => "canvas.unknown",
        MeterState::Disabled => "meters.off",
        MeterState::Waiting => "canvas.audio_meter_starting",
        MeterState::Live => "canvas.audio_meter_live",
        MeterState::Demo => "canvas.audio_meter_demo",
    };
    i18n.text(key)
}

pub(crate) fn meter_policy_index(policy: MeterPolicy) -> i32 {
    match policy {
        MeterPolicy::Disabled => 0,
        MeterPolicy::OnDemand => 1,
        MeterPolicy::Always => 2,
    }
}

pub(crate) fn meter_policy_from_index(index: i32) -> MeterPolicy {
    match index {
        0 => MeterPolicy::Disabled,
        2 => MeterPolicy::Always,
        _ => MeterPolicy::OnDemand,
    }
}

pub(crate) fn localized_meter_policy(i18n: &I18n, policy: MeterPolicy) -> String {
    let key = match policy {
        MeterPolicy::Disabled => "meters.off",
        MeterPolicy::OnDemand => "meters.on_demand",
        MeterPolicy::Always => "meters.always",
    };
    i18n.text(key)
}

pub(crate) fn language_index(language: &str) -> i32 {
    match language.trim().to_ascii_lowercase().as_str() {
        "es" | "es-es" => 1,
        "fr" | "fr-fr" => 2,
        _ => 0,
    }
}

pub(crate) fn language_code(index: i32) -> &'static str {
    match index {
        1 => "es",
        2 => "fr",
        _ => "en",
    }
}

pub(crate) fn volume_from_track_position(position: f32) -> f32 {
    const UNITY_TRACK_POSITION: f32 = 0.9;
    const MAX_VOLUME: f32 = 1.5;
    let position = position.clamp(0.0, 1.0);
    if position <= UNITY_TRACK_POSITION {
        position / UNITY_TRACK_POSITION
    } else {
        1.0 + (position - UNITY_TRACK_POSITION) / (1.0 - UNITY_TRACK_POSITION) * (MAX_VOLUME - 1.0)
    }
}

/// Match the canvas meter scale: audio levels are shown over a -60 dBFS to
/// 0 dBFS range, not as a linear 0.0–1.0 amplitude fraction.
pub(crate) fn meter_fraction(value: f32) -> f32 {
    let value = value.clamp(0.000001, 1.0);
    ((20.0 * value.log10() + 60.0) / 60.0).clamp(0.0, 1.0)
}

pub(crate) fn color(rgba: [u8; 4]) -> Color {
    Color::from_argb_u8(rgba[3], rgba[0], rgba[1], rgba[2])
}
