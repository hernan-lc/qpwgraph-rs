//! Persistent connection sets and activation policies.

use pw_graph_backend::{existing_connections, BackendError, GraphDriver};
use pw_graph_core::{LinkId, PortId};
use serde::{Deserialize, Serialize};
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PatchbayError {
    #[error("could not read patchbay file: {0}")]
    Read(#[source] std::io::Error),
    #[error("could not write patchbay file: {0}")]
    Write(#[source] std::io::Error),
    #[error("invalid patchbay JSON: {0}")]
    Format(#[from] serde_json::Error),
    #[error(transparent)]
    Backend(#[from] BackendError),
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct Patchbay {
    pub version: u32,
    pub name: String,
    pub connections: Vec<PatchConnection>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PatchConnection {
    pub output_port: PortId,
    pub input_port: PortId,
    #[serde(default)]
    pub pinned: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ActivationReport {
    pub connected: usize,
    pub already_present: usize,
    pub disconnected: usize,
    pub failed: Vec<String>,
}

impl Patchbay {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            version: 1,
            name: name.into(),
            connections: Vec::new(),
        }
    }

    pub fn add_connection(&mut self, output_port: PortId, input_port: PortId, pinned: bool) {
        if let Some(connection) = self.connections.iter_mut().find(|connection| {
            connection.output_port == output_port && connection.input_port == input_port
        }) {
            connection.pinned |= pinned;
            return;
        }
        self.connections.push(PatchConnection {
            output_port,
            input_port,
            pinned,
        });
    }

    pub fn remove_connection(&mut self, output_port: PortId, input_port: PortId) -> bool {
        let original_len = self.connections.len();
        self.connections.retain(|connection| {
            connection.output_port != output_port || connection.input_port != input_port
        });
        original_len != self.connections.len()
    }

    pub fn save_to(&self, path: impl AsRef<Path>) -> Result<(), PatchbayError> {
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json).map_err(PatchbayError::Write)
    }

    pub fn load_from(path: impl AsRef<Path>) -> Result<Self, PatchbayError> {
        let json = std::fs::read_to_string(path).map_err(PatchbayError::Read)?;
        Ok(serde_json::from_str(&json)?)
    }

    /// Connect all saved edges. With `exclusive`, live edges not represented by
    /// the patchbay are removed first. `auto_disconnect` also removes stale
    /// edges touching a destination before reconnecting the saved edge.
    pub fn activate(
        &self,
        driver: &mut dyn GraphDriver,
        exclusive: bool,
        auto_disconnect: bool,
    ) -> Result<ActivationReport, PatchbayError> {
        let mut report = ActivationReport::default();
        let saved: std::collections::BTreeSet<_> = self
            .connections
            .iter()
            .map(|connection| (connection.output_port, connection.input_port))
            .collect();

        if exclusive {
            let live: Vec<_> = driver.graph().links.values().cloned().collect();
            for link in live {
                if !saved.contains(&(link.output_port, link.input_port)) {
                    driver.disconnect(link.id)?;
                    report.disconnected += 1;
                }
            }
        }

        for connection in &self.connections {
            let current = existing_connections(driver);
            if current.contains(&(connection.output_port, connection.input_port)) {
                report.already_present += 1;
                continue;
            }

            if auto_disconnect {
                let stale: Vec<LinkId> = driver
                    .graph()
                    .links_for_port(connection.input_port)
                    .filter(|link| link.input_port == connection.input_port)
                    .map(|link| link.id)
                    .collect();
                for link_id in stale {
                    driver.disconnect(link_id)?;
                    report.disconnected += 1;
                }
            }

            match driver.connect(connection.output_port, connection.input_port) {
                Ok(_) => report.connected += 1,
                Err(error) => report.failed.push(error.to_string()),
            }
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pw_graph_backend::InMemoryDriver;

    #[test]
    fn activates_and_is_idempotent() {
        let mut patchbay = Patchbay::new("demo");
        patchbay.add_connection(PortId(1), PortId(3), true);
        let mut driver = InMemoryDriver::demo();
        assert_eq!(
            patchbay
                .activate(&mut driver, false, false)
                .unwrap()
                .connected,
            1
        );
        assert_eq!(
            patchbay
                .activate(&mut driver, false, false)
                .unwrap()
                .already_present,
            1
        );
    }
}
