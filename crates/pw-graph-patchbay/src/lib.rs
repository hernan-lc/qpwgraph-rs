//! Persistent connection sets and activation policies.
//!
//! The native qpwgraph format is XML and resolves rules by node/port names.
//! JSON remains supported as a convenient machine-readable format for tooling
//! and for compatibility with the first Rust prototype.

use pw_graph_backend::{BackendError, GraphDriver};
use pw_graph_core::{Direction, Graph, LinkId, NodeType, PortId, PortKey, PortType};
use serde::{Deserialize, Serialize};
use std::path::Path;
use thiserror::Error;

mod xml;

#[derive(Debug, Error)]
pub enum PatchbayError {
    #[error("could not read patchbay file: {0}")]
    Read(#[source] std::io::Error),
    #[error("could not write patchbay file: {0}")]
    Write(#[source] std::io::Error),
    #[error("invalid patchbay JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid patchbay XML: {0}")]
    Xml(#[from] quick_xml::Error),
    #[error("could not serialize patchbay XML: {0}")]
    XmlWrite(#[source] std::io::Error),
    #[error("patchbay XML contains invalid attributes")]
    XmlAttributes,
    #[error(transparent)]
    Backend(#[from] BackendError),
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct Patchbay {
    pub version: u32,
    pub name: String,
    pub connections: Vec<PatchConnection>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct PatchConnection {
    #[serde(default)]
    pub output_port: PortId,
    #[serde(default)]
    pub input_port: PortId,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub node_type: NodeType,
    #[serde(default)]
    pub port_type: PortType,
    #[serde(default)]
    pub output_node: String,
    #[serde(default)]
    pub output_name: String,
    #[serde(default)]
    pub input_node: String,
    #[serde(default)]
    pub input_name: String,
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
            ..PatchConnection::default()
        });
    }

    pub fn add_graph_connection(
        &mut self,
        graph: &Graph,
        output_port: PortId,
        input_port: PortId,
        pinned: bool,
    ) {
        let Some(output) = graph.port(output_port) else {
            self.add_connection(output_port, input_port, pinned);
            return;
        };
        let Some(input) = graph.port(input_port) else {
            self.add_connection(output_port, input_port, pinned);
            return;
        };
        let Some(output_node) = graph.node(output.node_id) else {
            self.add_connection(output_port, input_port, pinned);
            return;
        };
        let Some(input_node) = graph.node(input.node_id) else {
            self.add_connection(output_port, input_port, pinned);
            return;
        };
        if let Some(connection) = self.connections.iter_mut().find(|connection| {
            (connection.output_port == output_port && connection.input_port == input_port)
                || (connection.output_node == output_node.name
                    && connection.output_name == output.name
                    && connection.input_node == input_node.name
                    && connection.input_name == input.name)
        }) {
            connection.pinned |= pinned;
            connection.output_port = output_port;
            connection.input_port = input_port;
            return;
        }
        self.connections.push(PatchConnection {
            output_port,
            input_port,
            pinned,
            node_type: output_node.node_type,
            port_type: output.port_type,
            output_node: output_node.name.clone(),
            output_name: output.name.clone(),
            input_node: input_node.name.clone(),
            input_name: input.name.clone(),
        });
    }

    pub fn remove_connection(&mut self, output_port: PortId, input_port: PortId) -> bool {
        let original_len = self.connections.len();
        self.connections.retain(|connection| {
            connection.output_port != output_port || connection.input_port != input_port
        });
        original_len != self.connections.len()
    }

    pub fn snapshot_graph(&mut self, graph: &Graph, pinned: bool) {
        let links: Vec<_> = graph.links.values().cloned().collect();
        self.connections.clear();
        for link in links {
            self.add_graph_connection(graph, link.output_port, link.input_port, pinned);
        }
    }

    pub fn save_to(&self, path: impl AsRef<Path>) -> Result<(), PatchbayError> {
        let path = path.as_ref();
        let is_xml = path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| matches!(extension.to_ascii_lowercase().as_str(), "qpwgraph" | "xml"))
            .unwrap_or(false);
        if is_xml {
            let xml = self.to_xml()?;
            std::fs::write(path, xml).map_err(PatchbayError::Write)
        } else {
            let json = serde_json::to_string_pretty(self)?;
            std::fs::write(path, json).map_err(PatchbayError::Write)
        }
    }

    pub fn load_from(path: impl AsRef<Path>) -> Result<Self, PatchbayError> {
        let text = std::fs::read_to_string(path).map_err(PatchbayError::Read)?;
        if text.trim_start().starts_with('<') {
            Self::from_xml(&text)
        } else {
            Ok(serde_json::from_str(&text)?)
        }
    }

    /// Connect all saved edges. Name-based rules are resolved against the
    /// current registry snapshot, allowing IDs to change between sessions.
    pub fn activate(
        &self,
        driver: &mut dyn GraphDriver,
        exclusive: bool,
        auto_disconnect: bool,
    ) -> Result<ActivationReport, PatchbayError> {
        driver.refresh()?;
        let mut report = ActivationReport::default();
        let resolved: Vec<(PortKey, PortKey)> = self
            .connections
            .iter()
            .filter_map(|connection| {
                let (output, input) = self.resolve_connection(driver.graph(), connection)?;
                Some((
                    driver.graph().port_key(output)?,
                    driver.graph().port_key(input)?,
                ))
            })
            .collect();

        if exclusive {
            let live: Vec<_> = driver.graph().links.values().cloned().collect();
            for link in live {
                let saved = resolved.iter().any(|(output, input)| {
                    driver
                        .graph()
                        .find_link_by_keys(output, input)
                        .is_some_and(|saved_link| {
                            saved_link.output_port == link.output_port
                                && saved_link.input_port == link.input_port
                        })
                });
                if !saved {
                    driver.disconnect(link.id)?;
                    report.disconnected += 1;
                }
            }
        }

        for (output, input) in resolved {
            if driver.graph().find_link_by_keys(&output, &input).is_some() {
                report.already_present += 1;
                continue;
            }

            if auto_disconnect {
                let Some(input_port) = driver.graph().resolve_port_key(&input) else {
                    continue;
                };
                let stale: Vec<LinkId> = driver
                    .graph()
                    .links_for_port(input_port)
                    .filter(|link| link.input_port == input_port)
                    .map(|link| link.id)
                    .collect();
                for link_id in stale {
                    driver.disconnect(link_id)?;
                    report.disconnected += 1;
                }
            }

            match driver.connect_by_key_if_missing(&output, &input) {
                Ok(Some(_)) => report.connected += 1,
                Ok(None) => report.already_present += 1,
                Err(error) => report.failed.push(error.to_string()),
            }
        }
        Ok(report)
    }

    fn resolve_connection(
        &self,
        graph: &Graph,
        connection: &PatchConnection,
    ) -> Option<(PortId, PortId)> {
        let has_names = !connection.output_node.is_empty()
            && !connection.output_name.is_empty()
            && !connection.input_node.is_empty()
            && !connection.input_name.is_empty();
        if has_names {
            let output_node = graph.nodes.values().find(|node| {
                node.node_type == connection.node_type && node.name == connection.output_node
            })?;
            let input_node = graph.nodes.values().find(|node| {
                node.node_type == connection.node_type && node.name == connection.input_node
            })?;
            let output = output_node.ports.iter().find_map(|id| {
                let port = graph.port(*id)?;
                (port.name == connection.output_name
                    && port.direction == Direction::Source
                    && (connection.port_type == PortType::Unknown
                        || port.port_type == connection.port_type))
                    .then_some(port.id)
            })?;
            let input = input_node.ports.iter().find_map(|id| {
                let port = graph.port(*id)?;
                (port.name == connection.input_name
                    && port.direction == Direction::Sink
                    && (connection.port_type == PortType::Unknown
                        || port.port_type == connection.port_type))
                    .then_some(port.id)
            })?;
            return Some((output, input));
        }

        // Legacy files may contain only numeric IDs. Keep that fallback, but
        // never let an old numeric ID override a complete name-based rule.
        let output = graph.port(connection.output_port)?;
        let input = graph.port(connection.input_port)?;
        (output.direction == Direction::Source && input.direction == Direction::Sink)
            .then_some((output.id, input.id))
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

    #[test]
    fn qpwgraph_xml_round_trip_uses_names() {
        let mut patchbay = Patchbay::new("demo");
        let driver = InMemoryDriver::demo();
        patchbay.add_graph_connection(driver.graph(), PortId(1), PortId(3), true);
        let path =
            std::env::temp_dir().join(format!("qpwgraph-rs-{}.qpwgraph", std::process::id()));
        patchbay.save_to(&path).unwrap();
        let loaded = Patchbay::load_from(&path).unwrap();
        assert_eq!(loaded.connections.len(), 1);
        assert_eq!(loaded.connections[0].output_node, "Audio Capture");
        assert_eq!(loaded.connections[0].output_name, "capture_FL");
        std::fs::remove_file(path).unwrap();
    }
}
