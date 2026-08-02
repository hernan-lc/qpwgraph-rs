//! Persistent connection sets and activation policies.
//!
//! The native qpwgraph format is XML and resolves rules by node/port names.
//! JSON remains supported as a convenient machine-readable format for tooling
//! and for compatibility with the first Rust prototype.

use pw_graph_backend::{existing_connections, BackendError, GraphDriver};
use pw_graph_core::{Direction, Graph, LinkId, NodeType, PortId, PortType};
use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event};
use quick_xml::{Reader, Writer, XmlVersion};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use thiserror::Error;

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

    fn to_xml(&self) -> Result<String, PatchbayError> {
        let mut writer = Writer::new_with_indent(Vec::new(), b' ', 2);
        writer
            .write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))
            .map_err(PatchbayError::XmlWrite)?;
        writer
            .write_event(Event::DocType(BytesText::new("patchbay")))
            .map_err(PatchbayError::XmlWrite)?;
        let mut root = BytesStart::new("patchbay");
        root.push_attribute(("name", self.name.as_str()));
        root.push_attribute(("version", "0.8.3"));
        writer
            .write_event(Event::Start(root))
            .map_err(PatchbayError::XmlWrite)?;
        writer
            .write_event(Event::Start(BytesStart::new("items")))
            .map_err(PatchbayError::XmlWrite)?;
        for connection in &self.connections {
            let mut item = BytesStart::new("item");
            item.push_attribute(("node-type", node_type_text(connection.node_type)));
            item.push_attribute((
                "port-type",
                port_type_text(connection.node_type, connection.port_type),
            ));
            writer
                .write_event(Event::Start(item))
                .map_err(PatchbayError::XmlWrite)?;

            let mut output = BytesStart::new("output");
            output.push_attribute(("node", connection.output_node.as_str()));
            output.push_attribute(("port", connection.output_name.as_str()));
            writer
                .write_event(Event::Empty(output))
                .map_err(PatchbayError::XmlWrite)?;

            let mut input = BytesStart::new("input");
            input.push_attribute(("node", connection.input_node.as_str()));
            input.push_attribute(("port", connection.input_name.as_str()));
            writer
                .write_event(Event::Empty(input))
                .map_err(PatchbayError::XmlWrite)?;
            writer
                .write_event(Event::End(BytesEnd::new("item")))
                .map_err(PatchbayError::XmlWrite)?;
        }
        writer
            .write_event(Event::End(BytesEnd::new("items")))
            .map_err(PatchbayError::XmlWrite)?;
        writer
            .write_event(Event::End(BytesEnd::new("patchbay")))
            .map_err(PatchbayError::XmlWrite)?;
        Ok(String::from_utf8(writer.into_inner()).expect("XML writer emits UTF-8"))
    }

    fn from_xml(text: &str) -> Result<Self, PatchbayError> {
        let mut reader = Reader::from_str(text);
        reader.config_mut().trim_text(true);
        let mut patchbay = Patchbay::new("patchbay");
        let mut current: Option<PatchConnection> = None;
        loop {
            match reader.read_event()? {
                Event::Start(element) if element.name().as_ref() == b"patchbay" => {
                    let attributes = attributes(&reader, &element)?;
                    if let Some(name) = attributes.get("name") {
                        patchbay.name = name.clone();
                    }
                }
                Event::Start(element) if element.name().as_ref() == b"item" => {
                    let attributes = attributes(&reader, &element)?;
                    current = Some(PatchConnection {
                        node_type: node_type_from_text(attributes.get("node-type")),
                        port_type: port_type_from_text(attributes.get("port-type")),
                        ..PatchConnection::default()
                    });
                }
                Event::Empty(element) | Event::Start(element)
                    if element.name().as_ref() == b"output"
                        || element.name().as_ref() == b"input" =>
                {
                    let attributes = attributes(&reader, &element)?;
                    if let Some(connection) = current.as_mut() {
                        let node = attributes.get("node").cloned().unwrap_or_default();
                        let port = attributes.get("port").cloned().unwrap_or_default();
                        if element.name().as_ref() == b"output" {
                            connection.output_node = node;
                            connection.output_name = port;
                        } else {
                            connection.input_node = node;
                            connection.input_name = port;
                        }
                    }
                }
                Event::End(element) if element.name().as_ref() == b"item" => {
                    if let Some(connection) = current.take() {
                        if !connection.output_node.is_empty()
                            && !connection.output_name.is_empty()
                            && !connection.input_node.is_empty()
                            && !connection.input_name.is_empty()
                        {
                            patchbay.connections.push(connection);
                        }
                    }
                }
                Event::Eof => break,
                _ => {}
            }
        }
        Ok(patchbay)
    }

    /// Connect all saved edges. Name-based rules are resolved against the
    /// current registry snapshot, allowing IDs to change between sessions.
    pub fn activate(
        &self,
        driver: &mut dyn GraphDriver,
        exclusive: bool,
        auto_disconnect: bool,
    ) -> Result<ActivationReport, PatchbayError> {
        let mut report = ActivationReport::default();
        let resolved: Vec<(PortId, PortId)> = self
            .connections
            .iter()
            .filter_map(|connection| self.resolve_connection(driver.graph(), connection))
            .collect();
        let saved: BTreeSet<_> = resolved.iter().copied().collect();

        if exclusive {
            let live: Vec<_> = driver.graph().links.values().cloned().collect();
            for link in live {
                if !saved.contains(&(link.output_port, link.input_port)) {
                    driver.disconnect(link.id)?;
                    report.disconnected += 1;
                }
            }
        }

        for (output_port, input_port) in resolved {
            let current = existing_connections(driver);
            if current.contains(&(output_port, input_port)) {
                report.already_present += 1;
                continue;
            }

            if auto_disconnect {
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

            match driver.connect(output_port, input_port) {
                Ok(_) => report.connected += 1,
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
        if graph.port(connection.output_port).is_some()
            && graph.port(connection.input_port).is_some()
        {
            return Some((connection.output_port, connection.input_port));
        }
        let output_node = graph.nodes.values().find(|node| {
            node.node_type == connection.node_type && node.name == connection.output_node
        })?;
        let input_node = graph.nodes.values().find(|node| {
            node.node_type == connection.node_type && node.name == connection.input_node
        })?;
        let output = output_node.ports.iter().find_map(|id| {
            let port = graph.port(*id)?;
            (port.name == connection.output_name && port.direction == Direction::Source)
                .then_some(port.id)
        })?;
        let input = input_node.ports.iter().find_map(|id| {
            let port = graph.port(*id)?;
            (port.name == connection.input_name && port.direction == Direction::Sink)
                .then_some(port.id)
        })?;
        Some((output, input))
    }
}

fn attributes(
    reader: &Reader<&[u8]>,
    element: &BytesStart<'_>,
) -> Result<BTreeMap<String, String>, PatchbayError> {
    element
        .attributes()
        .map(|attribute| {
            let attribute = attribute.map_err(|_| PatchbayError::XmlAttributes)?;
            let key = String::from_utf8_lossy(attribute.key.as_ref()).into_owned();
            let value = attribute
                .decoded_and_normalized_value(XmlVersion::default(), reader.decoder())
                .map_err(PatchbayError::Xml)?
                .into_owned();
            Ok((key, value))
        })
        .collect()
}

fn node_type_text(node_type: NodeType) -> &'static str {
    match node_type {
        NodeType::PipeWire | NodeType::Unknown => "pipewire",
        NodeType::AlsaMidi => "alsa",
    }
}

fn port_type_text(node_type: NodeType, port_type: PortType) -> &'static str {
    match (node_type, port_type) {
        (NodeType::AlsaMidi, PortType::MidiAlsa) => "alsa-midi",
        (_, PortType::Audio) => "pipewire-audio",
        (_, PortType::MidiJack) => "pipewire-midi",
        (_, PortType::Video) => "pipewire-video",
        _ => "pipewire-other",
    }
}

fn node_type_from_text(value: Option<&String>) -> NodeType {
    match value.map(String::as_str) {
        Some("alsa") => NodeType::AlsaMidi,
        Some("pipewire") => NodeType::PipeWire,
        _ => NodeType::Unknown,
    }
}

fn port_type_from_text(value: Option<&String>) -> PortType {
    match value.map(String::as_str) {
        Some("pipewire-audio") => PortType::Audio,
        Some("pipewire-midi") => PortType::MidiJack,
        Some("pipewire-video") => PortType::Video,
        Some("alsa-midi") => PortType::MidiAlsa,
        _ => PortType::Unknown,
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
