use pw_graph_alsamidi::AlsaMidiDriver;
use pw_graph_backend::{AudioMeter, BackendError, BackendResult, GraphDriver, MeterPolicy, PipewireDriver};

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

/// The high bit of backend IDs marks ALSA MIDI resources, keeping the merged
/// graph namespaces apart so a PipeWire and an ALSA ID can never collide.
const ALSA_FLAG: u64 = 1_u64 << 63;

fn unsupported(message: &str) -> BackendError {
    BackendError::Unsupported(message.into())
}

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

    fn pipewire_mut(&mut self) -> BackendResult<&mut PipewireDriver> {
        self.pipewire
            .as_mut()
            .ok_or_else(|| unsupported("PipeWire backend is disabled"))
    }

    fn alsa_mut(&mut self) -> BackendResult<&mut AlsaMidiDriver> {
        self.alsa
            .as_mut()
            .ok_or_else(|| unsupported("ALSA backend is disabled"))
    }

    /// Runs a native mutation and mirrors the driver's new registry snapshot
    /// into the composite graph without a second round-trip.
    fn mutate_pipewire<T>(
        &mut self,
        f: impl FnOnce(&mut PipewireDriver) -> BackendResult<T>,
    ) -> BackendResult<T> {
        let value = f(self.pipewire_mut()?)?;
        self.rebuild_after_effect_mutation();
        Ok(value)
    }
}

impl GraphDriver for CompositeDriver {
    fn refresh(&mut self) -> BackendResult<Vec<Node>> {
        if let Some(driver) = self.pipewire.as_mut() {
            driver.refresh()?;
        }
        if let Some(driver) = self.alsa.as_mut() {
            driver.refresh()?;
        }
        self.rebuild_merged_graph()?;
        Ok(self.graph.nodes.values().cloned().collect())
    }

    fn connect(&mut self, src: PortId, dst: PortId) -> BackendResult<Link> {
        let link = if src.0 & ALSA_FLAG != 0 && dst.0 & ALSA_FLAG != 0 {
            self.alsa_mut()?.connect(src, dst)?
        } else if src.0 & ALSA_FLAG == 0 && dst.0 & ALSA_FLAG == 0 {
            self.pipewire_mut()?.connect(src, dst)?
        } else {
            return Err(unsupported(
                "connections cannot cross PipeWire and ALSA MIDI backends",
            ));
        };
        self.refresh()?;
        Ok(link)
    }

    fn disconnect(&mut self, link: LinkId) -> BackendResult<Link> {
        let existing = self
            .graph
            .link(link)
            .cloned()
            .ok_or(GraphError::MissingLink(link))?;
        if link.0 & ALSA_FLAG != 0 {
            self.alsa_mut()?.disconnect(link)?;
        } else {
            self.pipewire_mut()?.disconnect(link)?;
        }
        self.refresh()?;
        Ok(existing)
    }

    fn set_node_position(
        &mut self,
        node: NodeId,
        position: [f32; 2],
    ) -> BackendResult<()> {
        if node.0 & ALSA_FLAG != 0 {
            self.alsa_mut()?.set_node_position(node, position)?;
        } else {
            self.pipewire_mut()?.set_node_position(node, position)?;
        }
        if let Some(node_data) = self.graph.nodes.get_mut(&node) {
            node_data.position = position;
        }
        Ok(())
    }

    fn set_node_mute(&mut self, node: NodeId, muted: bool) -> BackendResult<()> {
        if node.0 & ALSA_FLAG != 0 {
            return Err(unsupported("ALSA MIDI nodes do not expose audio mute"));
        }
        self.pipewire_mut()?.set_node_mute(node, muted)
    }

    fn set_node_volume(
        &mut self,
        node: NodeId,
        volume: f32,
    ) -> BackendResult<()> {
        if node.0 & ALSA_FLAG != 0 {
            return Err(unsupported("ALSA MIDI nodes do not expose audio volume"));
        }
        self.pipewire_mut()?.set_node_volume(node, volume)
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

    fn audio_meters(&mut self) -> BackendResult<Vec<AudioMeter>> {
        if let Some(driver) = self.pipewire.as_mut() {
            return driver.audio_meters();
        }
        Ok(Vec::new())
    }

    fn set_meter_policy(&mut self, policy: MeterPolicy) -> BackendResult<()> {
        match self.pipewire.as_mut() {
            Some(driver) => driver.set_meter_policy(policy),
            None => Ok(()),
        }
    }

    fn request_meters(&mut self, nodes: &BTreeSet<NodeId>) -> BackendResult<()> {
        match self.pipewire.as_mut() {
            Some(driver) => driver.request_meters(nodes),
            None => Ok(()),
        }
    }

    fn reset_audio_config(&mut self) -> BackendResult<()> {
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
    ) -> BackendResult<pw_graph_backend::EffectInstance> {
        self.mutate_pipewire(|driver| driver.create_effect_node(request))
    }

    fn insert_effect(
        &mut self,
        request: pw_graph_backend::EffectInsertRequest,
    ) -> BackendResult<pw_graph_backend::EffectInstance> {
        self.mutate_pipewire(|driver| driver.insert_effect(request))
    }

    fn set_effect_enabled(
        &mut self,
        instance_id: &str,
        enabled: bool,
    ) -> BackendResult<()> {
        self.pipewire_mut()?.set_effect_enabled(instance_id, enabled)
    }

    fn set_effect_parameter(
        &mut self,
        instance_id: &str,
        parameter: &str,
        value: f32,
    ) -> BackendResult<()> {
        self.pipewire_mut()?
            .set_effect_parameter(instance_id, parameter, value)
    }

    fn remove_effect(&mut self, instance_id: &str) -> BackendResult<()> {
        self.mutate_pipewire(|driver| driver.remove_effect(instance_id))
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
    ) -> BackendResult<u16> {
        self.mutate_pipewire(|driver| driver.relay_start_host(request))
    }

    fn relay_stop_host(&mut self) -> BackendResult<()> {
        self.pipewire_mut()?.relay_stop_host()
    }

    fn relay_connect(
        &mut self,
        target: std::net::SocketAddr,
        pin: &str,
        roles: pw_graph_backend::RelayRoles,
    ) -> BackendResult<()> {
        self.mutate_pipewire(|driver| driver.relay_connect(target, pin, roles))
    }

    fn relay_disconnect(
        &mut self,
        session: pw_graph_backend::RelaySessionId,
    ) -> BackendResult<()> {
        self.pipewire_mut()?.relay_disconnect(session)
    }

    fn relay_events(&mut self) -> Vec<pw_graph_backend::RelayEvent> {
        match self.pipewire.as_mut() {
            Some(driver) => driver.relay_events(),
            None => Vec::new(),
        }
    }

    fn relay_discovery_start(&mut self) -> BackendResult<()> {
        self.pipewire_mut()?.relay_discovery_start()
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

    fn relay_local_links(&self) -> Vec<pw_graph_backend::RelayLocalLink> {
        self.pipewire
            .as_ref()
            .map(|driver| driver.relay_local_links())
            .unwrap_or_default()
    }
}
