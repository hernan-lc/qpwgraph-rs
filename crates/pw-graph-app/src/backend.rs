use pw_graph_alsamidi::AlsaMidiDriver;
use pw_graph_backend::{AudioMeter, GraphDriver, PipewireDriver};
use pw_graph_core::{Graph, GraphError, Link, LinkId, Node, NodeId, PortId, PortType};

#[derive(Default)]
pub(crate) struct CompositeDriver {
    pub(crate) pipewire: Option<PipewireDriver>,
    pub(crate) alsa: Option<AlsaMidiDriver>,
    graph: Graph,
}

impl CompositeDriver {
    fn merge_graph(destination: &mut Graph, source: &Graph) -> Result<(), GraphError> {
        for node in source.nodes.values().cloned() {
            destination.add_node(node)?;
        }
        for port in source.ports.values().cloned() {
            destination.add_port(port)?;
        }
        for link in source.links.values().cloned() {
            destination.insert_existing_link(link)?;
        }
        Ok(())
    }
}

impl GraphDriver for CompositeDriver {
    fn refresh(&mut self) -> pw_graph_backend::BackendResult<Vec<Node>> {
        if let Some(driver) = self.pipewire.as_mut() {
            driver.refresh()?;
        }
        if let Some(driver) = self.alsa.as_mut() {
            driver.refresh()?;
        }
        let mut graph = Graph::default();
        if let Some(driver) = self.pipewire.as_ref() {
            Self::merge_graph(&mut graph, driver.graph())?;
        }
        if let Some(driver) = self.alsa.as_ref() {
            Self::merge_graph(&mut graph, driver.graph())?;
        }
        self.graph = graph;
        Ok(self.graph.nodes.values().cloned().collect())
    }

    fn connect(&mut self, src: PortId, dst: PortId) -> pw_graph_backend::BackendResult<Link> {
        let alsa = 1_u64 << 63;
        let link = if src.0 & alsa != 0 && dst.0 & alsa != 0 {
            self.alsa
                .as_mut()
                .ok_or_else(|| {
                    pw_graph_backend::BackendError::Unsupported("ALSA backend is disabled".into())
                })?
                .connect(src, dst)?
        } else if src.0 & alsa == 0 && dst.0 & alsa == 0 {
            self.pipewire
                .as_mut()
                .ok_or_else(|| {
                    pw_graph_backend::BackendError::Unsupported(
                        "PipeWire backend is disabled".into(),
                    )
                })?
                .connect(src, dst)?
        } else {
            return Err(pw_graph_backend::BackendError::Unsupported(
                "connections cannot cross PipeWire and ALSA MIDI backends".into(),
            ));
        };
        self.refresh()?;
        Ok(link)
    }

    fn disconnect(&mut self, link: LinkId) -> pw_graph_backend::BackendResult<Link> {
        let alsa = 1_u64 << 63;
        let existing = self
            .graph
            .link(link)
            .cloned()
            .ok_or(GraphError::MissingLink(link))?;
        if link.0 & alsa != 0 {
            self.alsa
                .as_mut()
                .ok_or_else(|| {
                    pw_graph_backend::BackendError::Unsupported("ALSA backend is disabled".into())
                })?
                .disconnect(link)?;
        } else {
            self.pipewire
                .as_mut()
                .ok_or_else(|| {
                    pw_graph_backend::BackendError::Unsupported(
                        "PipeWire backend is disabled".into(),
                    )
                })?
                .disconnect(link)?;
        }
        self.refresh()?;
        Ok(existing)
    }

    fn rename_node(&mut self, node: NodeId, name: String) -> pw_graph_backend::BackendResult<()> {
        if node.0 & (1_u64 << 63) != 0 {
            self.alsa
                .as_mut()
                .ok_or_else(|| {
                    pw_graph_backend::BackendError::Unsupported("ALSA backend is disabled".into())
                })?
                .rename_node(node, name)
        } else {
            self.pipewire
                .as_mut()
                .ok_or_else(|| {
                    pw_graph_backend::BackendError::Unsupported(
                        "PipeWire backend is disabled".into(),
                    )
                })?
                .rename_node(node, name)
        }
    }

    fn set_node_position(
        &mut self,
        node: NodeId,
        position: [f32; 2],
    ) -> pw_graph_backend::BackendResult<()> {
        if node.0 & (1_u64 << 63) != 0 {
            self.alsa
                .as_mut()
                .ok_or_else(|| {
                    pw_graph_backend::BackendError::Unsupported("ALSA backend is disabled".into())
                })?
                .set_node_position(node, position)?;
        } else {
            self.pipewire
                .as_mut()
                .ok_or_else(|| {
                    pw_graph_backend::BackendError::Unsupported(
                        "PipeWire backend is disabled".into(),
                    )
                })?
                .set_node_position(node, position)?;
        }
        if let Some(node_data) = self.graph.nodes.get_mut(&node) {
            node_data.position = position;
        }
        Ok(())
    }

    fn graph(&self) -> &Graph {
        &self.graph
    }
    fn is_node_type(&self, node_type: pw_graph_core::NodeType) -> bool {
        self.pipewire
            .as_ref()
            .is_some_and(|driver| driver.is_node_type(node_type))
            || self
                .alsa
                .as_ref()
                .is_some_and(|driver| driver.is_node_type(node_type))
    }
    fn is_port_type(&self, port_type: PortType) -> bool {
        self.pipewire
            .as_ref()
            .is_some_and(|driver| driver.is_port_type(port_type))
            || self
                .alsa
                .as_ref()
                .is_some_and(|driver| driver.is_port_type(port_type))
    }

    fn audio_meters(&mut self) -> pw_graph_backend::BackendResult<Vec<AudioMeter>> {
        if let Some(driver) = self.pipewire.as_mut() {
            return driver.audio_meters();
        }
        Ok(Vec::new())
    }
}
