use pw_graph_alsamidi::AlsaMidiDriver;
use pw_graph_backend::{AudioMeter, GraphDriver, MeterPolicy, PipewireDriver};

#[cfg(feature = "relay")]
use pw_graph_backend::RelayDriver;
use pw_graph_core::{Graph, GraphError, Link, LinkId, Node, NodeId, PortId, PortType};
use std::collections::BTreeSet;

#[cfg(feature = "relay")]
pub(crate) trait AppDriver: GraphDriver + RelayDriver {}

#[cfg(feature = "relay")]
impl<T> AppDriver for T where T: GraphDriver + RelayDriver {}

#[cfg(not(feature = "relay"))]
pub(crate) trait AppDriver: GraphDriver {}

#[cfg(not(feature = "relay"))]
impl<T> AppDriver for T where T: GraphDriver {}

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

    /// Rebuild the graph exposed to the app from the children’s already
    /// synchronized snapshots. Unlike [`GraphDriver::refresh`], this does not
    /// start another PipeWire round-trip, which matters after a native effect
    /// operation has already committed its filter or link rewrite.
    fn rebuild_merged_graph(&mut self) -> Result<(), GraphError> {
        let mut graph = Graph::default();
        if let Some(driver) = self.pipewire.as_ref() {
            Self::merge_graph(&mut graph, driver.graph())?;
        }
        if let Some(driver) = self.alsa.as_ref() {
            Self::merge_graph(&mut graph, driver.graph())?;
        }
        self.graph = graph;
        Ok(())
    }

    /// An effect operation has succeeded by the time this is called. Do not
    /// turn a subsequent UI-snapshot merge failure into a false operation
    /// failure — doing so would leave a live but unpersisted native effect.
    /// Backend IDs are namespaced, so a merge error would indicate a bug; the
    /// normal refresh path will retry on the next graph update.
    fn rebuild_after_effect_mutation(&mut self) {
        if let Err(error) = self.rebuild_merged_graph() {
            eprintln!("could not rebuild merged graph after effect mutation: {error}");
        }
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
        self.rebuild_merged_graph()?;
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

    fn set_node_mute(&mut self, node: NodeId, muted: bool) -> pw_graph_backend::BackendResult<()> {
        let alsa = 1_u64 << 63;
        if node.0 & alsa != 0 {
            return Err(pw_graph_backend::BackendError::Unsupported(
                "ALSA MIDI nodes do not expose audio mute".into(),
            ));
        }
        self.pipewire
            .as_mut()
            .ok_or_else(|| {
                pw_graph_backend::BackendError::Unsupported("PipeWire backend is disabled".into())
            })?
            .set_node_mute(node, muted)
    }

    fn set_node_volume(
        &mut self,
        node: NodeId,
        volume: f32,
    ) -> pw_graph_backend::BackendResult<()> {
        let alsa = 1_u64 << 63;
        if node.0 & alsa != 0 {
            return Err(pw_graph_backend::BackendError::Unsupported(
                "ALSA MIDI nodes do not expose audio volume".into(),
            ));
        }
        self.pipewire
            .as_mut()
            .ok_or_else(|| {
                pw_graph_backend::BackendError::Unsupported("PipeWire backend is disabled".into())
            })?
            .set_node_volume(node, volume)
    }

    fn graph(&self) -> &Graph {
        &self.graph
    }
    fn graph_dirty(&self) -> bool {
        self.pipewire
            .as_ref()
            .is_some_and(|driver| driver.graph_dirty())
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

    fn set_meter_policy(&mut self, policy: MeterPolicy) -> pw_graph_backend::BackendResult<()> {
        match self.pipewire.as_mut() {
            Some(driver) => driver.set_meter_policy(policy),
            None => Ok(()),
        }
    }

    fn request_meters(&mut self, nodes: &BTreeSet<NodeId>) -> pw_graph_backend::BackendResult<()> {
        match self.pipewire.as_mut() {
            Some(driver) => driver.request_meters(nodes),
            None => Ok(()),
        }
    }

    fn reset_audio_config(&mut self) -> pw_graph_backend::BackendResult<()> {
        match self.pipewire.as_mut() {
            Some(driver) => driver.reset_audio_config(),
            None => Ok(()),
        }
    }
}

impl pw_graph_backend::EffectDriver for CompositeDriver {
    fn effect_descriptors(&self) -> Vec<pw_graph_effects::EffectDescriptor> {
        self.pipewire
            .as_ref()
            .map(|driver| driver.effect_descriptors())
            .unwrap_or_default()
    }

    fn effect_instances(&self) -> Vec<pw_graph_backend::EffectInstance> {
        self.pipewire
            .as_ref()
            .map(|driver| driver.effect_instances())
            .unwrap_or_default()
    }

    fn supports_effect_nodes(&self) -> bool {
        self.pipewire
            .as_ref()
            .is_some_and(|driver| driver.supports_effect_nodes())
    }

    fn create_effect_node(
        &mut self,
        request: pw_graph_backend::EffectNodeRequest,
    ) -> pw_graph_backend::BackendResult<pw_graph_backend::EffectInstance> {
        let instance = self
            .pipewire
            .as_mut()
            .ok_or_else(|| {
                pw_graph_backend::BackendError::Unsupported("PipeWire backend is disabled".into())
            })?
            .create_effect_node(request)?;
        // `PipewireDriver` has already rebuilt its native registry snapshot;
        // mirror it into the composite without a second round-trip.
        self.rebuild_after_effect_mutation();
        Ok(instance)
    }

    fn insert_effect(
        &mut self,
        request: pw_graph_backend::EffectInsertRequest,
    ) -> pw_graph_backend::BackendResult<pw_graph_backend::EffectInstance> {
        let instance = self
            .pipewire
            .as_mut()
            .ok_or_else(|| {
                pw_graph_backend::BackendError::Unsupported("PipeWire backend is disabled".into())
            })?
            .insert_effect(request)?;
        self.rebuild_after_effect_mutation();
        Ok(instance)
    }

    fn set_effect_enabled(
        &mut self,
        instance_id: &str,
        enabled: bool,
    ) -> pw_graph_backend::BackendResult<()> {
        self.pipewire
            .as_mut()
            .ok_or_else(|| {
                pw_graph_backend::BackendError::Unsupported("PipeWire backend is disabled".into())
            })?
            .set_effect_enabled(instance_id, enabled)
    }

    fn set_effect_parameter(
        &mut self,
        instance_id: &str,
        parameter: &str,
        value: f32,
    ) -> pw_graph_backend::BackendResult<()> {
        self.pipewire
            .as_mut()
            .ok_or_else(|| {
                pw_graph_backend::BackendError::Unsupported("PipeWire backend is disabled".into())
            })?
            .set_effect_parameter(instance_id, parameter, value)
    }

    fn remove_effect(&mut self, instance_id: &str) -> pw_graph_backend::BackendResult<()> {
        self.pipewire
            .as_mut()
            .ok_or_else(|| {
                pw_graph_backend::BackendError::Unsupported("PipeWire backend is disabled".into())
            })?
            .remove_effect(instance_id)?;
        self.rebuild_after_effect_mutation();
        Ok(())
    }
}

#[cfg(feature = "relay")]
impl pw_graph_backend::RelayDriver for CompositeDriver {
    fn relay_available(&self) -> bool {
        self.pipewire
            .as_ref()
            .is_some_and(|driver| driver.relay_available())
    }

    fn relay_status(&self) -> pw_graph_backend::RelayEngineStatus {
        self.pipewire
            .as_ref()
            .map(|driver| driver.relay_status())
            .unwrap_or_default()
    }

    fn relay_devices_active(&self) -> bool {
        self.pipewire
            .as_ref()
            .is_some_and(|driver| driver.relay_devices_active())
    }

    fn relay_start_host(
        &mut self,
        request: pw_graph_backend::RelayHostRequest,
    ) -> pw_graph_backend::BackendResult<u16> {
        let port = self
            .pipewire
            .as_mut()
            .ok_or_else(|| {
                pw_graph_backend::BackendError::Unsupported("PipeWire backend is disabled".into())
            })?
            .relay_start_host(request)?;
        // Starting the host lazily creates the virtual relay devices; mirror
        // them into the composite graph the same way effect creation does.
        self.rebuild_after_effect_mutation();
        Ok(port)
    }

    fn relay_stop_host(&mut self) -> pw_graph_backend::BackendResult<()> {
        match self.pipewire.as_mut() {
            Some(driver) => driver.relay_stop_host(),
            None => Err(pw_graph_backend::BackendError::Unsupported(
                "PipeWire backend is disabled".into(),
            )),
        }
    }

    fn relay_connect(
        &mut self,
        target: std::net::SocketAddr,
        pin: &str,
        roles: pw_graph_backend::RelayRoles,
    ) -> pw_graph_backend::BackendResult<()> {
        self.pipewire
            .as_mut()
            .ok_or_else(|| {
                pw_graph_backend::BackendError::Unsupported("PipeWire backend is disabled".into())
            })?
            .relay_connect(target, pin, roles)?;
        self.rebuild_after_effect_mutation();
        Ok(())
    }

    fn relay_disconnect(
        &mut self,
        session: pw_graph_backend::RelaySessionId,
    ) -> pw_graph_backend::BackendResult<()> {
        match self.pipewire.as_mut() {
            Some(driver) => driver.relay_disconnect(session),
            None => Err(pw_graph_backend::BackendError::Unsupported(
                "PipeWire backend is disabled".into(),
            )),
        }
    }

    fn relay_events(&mut self) -> Vec<pw_graph_backend::RelayEvent> {
        match self.pipewire.as_mut() {
            Some(driver) => driver.relay_events(),
            None => Vec::new(),
        }
    }

    fn relay_discovery_start(&mut self) -> pw_graph_backend::BackendResult<()> {
        match self.pipewire.as_mut() {
            Some(driver) => driver.relay_discovery_start(),
            None => Err(pw_graph_backend::BackendError::Unsupported(
                "PipeWire backend is disabled".into(),
            )),
        }
    }

    fn relay_discovery_stop(&mut self) {
        if let Some(driver) = self.pipewire.as_mut() {
            driver.relay_discovery_stop();
        }
    }

    fn relay_peers(&self) -> Vec<pw_graph_backend::RelayPeerInfo> {
        self.pipewire
            .as_ref()
            .map(|driver| driver.relay_peers())
            .unwrap_or_default()
    }
}
