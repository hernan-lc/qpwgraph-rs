//! TOML configuration compatible with the state surface described by qpwgraph.

use serde::{Deserialize, Serialize};
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

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct NodeViewConfig {
    pub collapsed: bool,
    pub custom_name: Option<String>,
    pub color: Option<[u8; 4]>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
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
    pub node_view_by_name: std::collections::BTreeMap<String, NodeViewConfig>,
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
        let directory =
            std::env::temp_dir().join(format!("pw-graph-config-{}", std::process::id()));
        let path = directory.join("config.toml");
        let expected = AppConfig::default();
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
            NodeViewConfig {
                collapsed: true,
                custom_name: Some("Microphone".into()),
                color: Some([82, 207, 133, 255]),
            },
        );
        expected.save_to(&path).unwrap();
        assert_eq!(AppConfig::load_from(&path).unwrap(), expected);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
