//! TOML configuration compatible with the state surface described by qpwgraph.

pub use pw_graph_core::NodeAppearance;
use pw_graph_core::PortKey;
use pw_graph_effects::EffectInstanceConfig;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("could not read config: {0}")]
    Read(#[source] std::io::Error),
    #[error("could not write config: {0}")]
    Write(#[source] std::io::Error),
    #[error("invalid config TOML: {0}")]
    Format(#[from] toml::de::Error),
    #[error("could not serialize config TOML: {0}")]
    Serialize(#[from] toml::ser::Error),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default = "AppConfig::default")]
pub struct AppConfig {
    pub language: String,
    /// TOML table keys are strings, so node IDs are stored as decimal strings.
    pub node_positions: std::collections::BTreeMap<String, [f32; 2]>,
    /// Fallback layout keyed by backend-independent node type and name. This
    /// lets a saved layout survive PipeWire global node IDs changing between
    /// sessions. Ambiguous duplicate names are omitted by the app.
    pub node_positions_by_name: std::collections::BTreeMap<String, [f32; 2]>,
    /// Visual node overrides keyed by the same stable backend-independent key
    /// as the saved layout.
    pub node_view_by_name: std::collections::BTreeMap<String, NodeAppearance>,
    pub thumbnail_view: bool,
    pub minimap_visible: bool,
    pub window_width: f32,
    pub window_height: f32,
    pub zoom: f32,
    /// Multiplier for application chrome such as the toolbar and status bar.
    pub ui_text_scale: f32,
    /// Multiplier for navigation and Preferences panel text.
    pub panel_text_scale: f32,
    /// Multiplier for node titles, port labels, and node counters.
    pub node_text_scale: f32,
    pub media_filter: String,
    /// Case-insensitive text used to hide non-matching nodes and ports.
    pub graph_search: String,
    pub sort_type: String,
    pub sort_order: String,
    /// When helper streams may be attached to measure audio levels:
    /// `off`, `on-demand`, or `always`. See `pw_graph_backend::MeterPolicy`.
    pub audio_meters: String,
    pub repel_overlapping_nodes: bool,
    pub connect_through_nodes: bool,
    /// Node connect drag mode: `easy` (whole-node, matches all compatible
    /// ports) or `advanced` (precise, one port at a time).
    pub connect_mode: String,
    pub statusbar: bool,
    pub toolbar: bool,
    pub patchbay_toolbar: bool,
    pub patchbay_auto_pin: bool,
    pub patchbay_auto_disconnect: bool,
    pub patchbay_exclusive: bool,
    pub patchbay_activated: bool,
    pub patchbay_path: Option<PathBuf>,
    pub patchbay_dir: Option<PathBuf>,
    /// Most recently used patchbay files, newest first.
    pub recent_patchbay_paths: Vec<PathBuf>,
    /// Optional named patchbay profiles and their files.
    pub patchbay_profiles: std::collections::BTreeMap<String, PathBuf>,
    pub active_patchbay_profile: String,
    /// Effect definitions are kept in application config rather than the
    /// qpwgraph XML format, which has no portable representation for DSP
    /// modules.
    pub effects: Vec<PersistedEffect>,
    pub relay_device_name: String,
    pub relay_host_pin: String,
    pub relay_host_port: u16,
    pub relay_client_target: String,
    pub relay_client_pin: String,
    pub relay_role: String,
    pub relay_codec: String,
    pub relay_frame_ms: u16,
    pub relay_transport: String,
    /// Preserve fields written by a newer version so opening and saving a
    /// config with this version does not silently erase forward-compatible
    /// settings.
    #[serde(flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PersistedEffect {
    pub instance: EffectInstanceConfig,
    /// The original endpoints for an effect inserted into a link. A detached
    /// effect node deliberately has no endpoints until the user patches it.
    #[serde(default)]
    pub source: Option<PortKey>,
    #[serde(default)]
    pub destination: Option<PortKey>,
    /// Stored independently of graph node IDs because PipeWire assigns fresh
    /// IDs whenever an effect node is recreated on startup.
    #[serde(default = "default_effect_position")]
    pub position: [f32; 2],
}

fn default_effect_position() -> [f32; 2] {
    [260.0, 180.0]
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            language: "en".into(),
            node_positions: std::collections::BTreeMap::new(),
            node_positions_by_name: std::collections::BTreeMap::new(),
            node_view_by_name: std::collections::BTreeMap::new(),
            thumbnail_view: false,
            minimap_visible: false,
            window_width: 1100.0,
            window_height: 760.0,
            zoom: 1.0,
            ui_text_scale: 1.10,
            panel_text_scale: 1.20,
            node_text_scale: 1.15,
            media_filter: "all".into(),
            graph_search: String::new(),
            sort_type: "name".into(),
            sort_order: "ascending".into(),
            audio_meters: "on-demand".into(),
            repel_overlapping_nodes: true,
            connect_through_nodes: false,
            connect_mode: "advanced".into(),
            statusbar: true,
            toolbar: true,
            patchbay_toolbar: true,
            patchbay_auto_pin: false,
            patchbay_auto_disconnect: false,
            patchbay_exclusive: false,
            patchbay_activated: false,
            patchbay_path: None,
            patchbay_dir: None,
            recent_patchbay_paths: Vec::new(),
            patchbay_profiles: std::collections::BTreeMap::new(),
            active_patchbay_profile: "default".into(),
            effects: Vec::new(),
            relay_device_name: "qpwgraph-rs".into(),
            relay_host_pin: "123456".into(),
            relay_host_port: 0,
            relay_client_target: String::new(),
            relay_client_pin: "123456".into(),
            relay_role: "both".into(),
            relay_codec: "opus".into(),
            // Ten milliseconds halves the codec-side latency floor of the
            // previous 20 ms default at the cost of doubling the packet rate
            // to 100/s, which local Wi-Fi and USB tether links carry
            // comfortably. The relay panel's advanced settings still expose
            // 5–60 ms for links that prefer fewer, larger packets.
            relay_frame_ms: 10,
            relay_transport: "auto".into(),
            extra: BTreeMap::new(),
        }
    }
}

impl AppConfig {
    pub fn load_from(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(ConfigError::Read)?;
        Ok(toml::from_str(&text)?)
    }

    pub fn save_to(&self, path: impl AsRef<Path>) -> Result<(), ConfigError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(ConfigError::Write)?;
        }
        let text = toml::to_string_pretty(self)?;
        std::fs::write(path, text).map_err(ConfigError::Write)
    }
}

pub fn config_dir(app_name: &str) -> PathBuf {
    if let Some(path) = std::env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(path).join(app_name);
    }
    if let Some(path) = std::env::var_os("HOME") {
        return PathBuf::from(path).join(".config").join(app_name);
    }
    PathBuf::from(".").join(format!(".{app_name}"))
}

pub fn config_path(app_name: &str) -> PathBuf {
    config_dir(app_name).join("config.toml")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_round_trip() {
        assert_eq!(AppConfig::default().relay_host_pin, "123456");
        assert_eq!(AppConfig::default().relay_client_pin, "123456");
        let directory =
            std::env::temp_dir().join(format!("pw-graph-config-{}", std::process::id()));
        let path = directory.join("config.toml");
        let expected = AppConfig {
            relay_device_name: "studio-pc".into(),
            relay_host_pin: "123456".into(),
            relay_client_target: "192.168.1.20:48123".into(),
            ..AppConfig::default()
        };
        expected.save_to(&path).unwrap();
        assert_eq!(AppConfig::load_from(&path).unwrap(), expected);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn node_positions_round_trip() {
        let directory =
            std::env::temp_dir().join(format!("pw-graph-config-positions-{}", std::process::id()));
        let path = directory.join("config.toml");
        let mut expected = AppConfig::default();
        expected.node_positions.insert("42".into(), [120.5, -18.0]);
        expected
            .node_positions
            .insert("9001".into(), [640.0, 240.25]);
        expected
            .node_positions_by_name
            .insert("PipeWire:Capture".into(), [120.5, -18.0]);
        expected.node_view_by_name.insert(
            "PipeWire:Capture".into(),
            NodeAppearance {
                collapsed: true,
                custom_name: Some("Microphone".into()),
                color: Some([82, 207, 133, 255]),
            },
        );
        expected.save_to(&path).unwrap();
        assert_eq!(AppConfig::load_from(&path).unwrap(), expected);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn effect_configuration_round_trips() {
        let directory =
            std::env::temp_dir().join(format!("pw-graph-config-effects-{}", std::process::id()));
        let path = directory.join("config.toml");
        let mut expected = AppConfig::default();
        expected.effects.push(PersistedEffect {
            instance: EffectInstanceConfig {
                instance_id: "effect-1".into(),
                effect_id: "builtin.noise-gate".into(),
                module_path: None,
                enabled: true,
                parameters: [("threshold-db".into(), -42.0)].into_iter().collect(),
            },
            source: Some(PortKey {
                node_name: "Capture".into(),
                node_serial: None,
                node_type: pw_graph_core::NodeType::PipeWire,
                port_name: "out_FL".into(),
                channel: Some("FL".into()),
                direction: pw_graph_core::Direction::Source,
                port_type: pw_graph_core::PortType::Audio,
            }),
            destination: Some(PortKey {
                node_name: "Playback".into(),
                node_serial: None,
                node_type: pw_graph_core::NodeType::PipeWire,
                port_name: "in_FL".into(),
                channel: Some("FL".into()),
                direction: pw_graph_core::Direction::Sink,
                port_type: pw_graph_core::PortType::Audio,
            }),
            position: [260.0, 180.0],
        });
        expected.save_to(&path).unwrap();
        assert_eq!(AppConfig::load_from(&path).unwrap(), expected);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn legacy_effect_without_routing_or_position_loads_as_a_standalone_node() {
        let config: AppConfig = toml::from_str(
            r#"
effects = [{ instance = { instance_id = "legacy-effect", effect_id = "builtin.noise-gate" } }]
"#,
        )
        .unwrap();

        let effect = config.effects.first().unwrap();
        assert_eq!(effect.instance.instance_id, "legacy-effect");
        assert_eq!(effect.source, None);
        assert_eq!(effect.destination, None);
        assert_eq!(effect.position, [260.0, 180.0]);
    }

    #[test]
    fn unknown_fields_survive_a_config_round_trip() {
        let directory =
            std::env::temp_dir().join(format!("pw-graph-config-extra-{}", std::process::id()));
        let path = directory.join("config.toml");
        let original = r#"
language = "es"
future_setting = "keep me"
[future_table]
enabled = true
"#;
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(&path, original).unwrap();

        let config = AppConfig::load_from(&path).unwrap();
        assert_eq!(
            config.extra.get("future_setting"),
            Some(&toml::Value::String("keep me".into()))
        );
        config.save_to(&path).unwrap();
        let restored = AppConfig::load_from(&path).unwrap();
        assert_eq!(restored.extra, config.extra);
        assert_eq!(
            restored.extra.get("future_table"),
            config.extra.get("future_table")
        );
        std::fs::remove_dir_all(directory).unwrap();
    }
}
