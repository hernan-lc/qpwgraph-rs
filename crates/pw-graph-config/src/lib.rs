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

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct AppConfig {
    pub language: String,
    pub window_width: f32,
    pub window_height: f32,
    pub zoom: f32,
    pub sort_type: String,
    pub sort_order: String,
    pub repel_overlapping_nodes: bool,
    pub connect_through_nodes: bool,
    pub statusbar: bool,
    pub toolbar: bool,
    pub patchbay_toolbar: bool,
    pub menubar: bool,
    pub patchbay_auto_pin: bool,
    pub patchbay_auto_disconnect: bool,
    pub patchbay_exclusive: bool,
    pub patchbay_activated: bool,
    pub patchbay_path: Option<PathBuf>,
    pub patchbay_dir: Option<PathBuf>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            language: "en".into(),
            window_width: 1100.0,
            window_height: 760.0,
            zoom: 1.0,
            sort_type: "name".into(),
            sort_order: "ascending".into(),
            repel_overlapping_nodes: true,
            connect_through_nodes: false,
            statusbar: true,
            toolbar: true,
            patchbay_toolbar: true,
            menubar: true,
            patchbay_auto_pin: false,
            patchbay_auto_disconnect: false,
            patchbay_exclusive: false,
            patchbay_activated: false,
            patchbay_path: None,
            patchbay_dir: None,
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
}
