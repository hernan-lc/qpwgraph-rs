//! Framework-neutral application/backend composition.
//!
//! The desktop UI is deliberately not part of this crate.  Both the native
//! application shell and tests use the same [`CompositeDriver`], so a view
//! cannot accidentally create a second graph namespace or route an ALSA
//! resource through PipeWire.

use pw_graph_backend::{BackendError, BackendResult, GraphDriver, MeterPolicy};
use pw_graph_core::{Graph, GraphError, Link, LinkId, Node, NodeId, NodeType, PortId, PortType};
use std::collections::BTreeSet;

#[cfg(feature = "alsa")]
use pw_graph_alsamidi::AlsaMidiDriver;
#[cfg(feature = "pipewire")]
use pw_graph_backend::PipewireDriver;

/// The high bit is reserved by the ALSA backend.  Keeping the rule here makes
/// the routing decision explicit and gives every UI the same semantics.
pub const ALSA_ID_FLAG: u64 = 1_u64 << 63;

/// The native driver that owns a pair of graph ports.  This classification is
/// kept independent of the UI and is used before a mutation is forwarded to a
/// child driver.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompositeRoute {
    PipeWire,
    AlsaMidi,
}

/// Classify a connection without touching either native backend.
///
/// The high-bit namespace is the stable discriminator assigned to ALSA MIDI
/// IDs.  A mixed pair is intentionally rejected instead of being allowed to
/// fall through to PipeWire.
pub fn route_for_ports(src: PortId, dst: PortId) -> Result<CompositeRoute, &'static str> {
    let source_is_alsa = src.0 & ALSA_ID_FLAG != 0;
    let destination_is_alsa = dst.0 & ALSA_ID_FLAG != 0;
    match (source_is_alsa, destination_is_alsa) {
        (false, false) => Ok(CompositeRoute::PipeWire),
        (true, true) => Ok(CompositeRoute::AlsaMidi),
        _ => Err("connections cannot cross PipeWire and ALSA MIDI backends"),
    }
}

/// A backend that can be used by the application layer.  Relay is an optional
/// extension of the same object rather than a second UI-owned driver.
#[cfg(feature = "relay")]
pub trait ApplicationDriver: GraphDriver + pw_graph_backend::RelayDriver {}

#[cfg(feature = "relay")]
impl<T> ApplicationDriver for T where T: GraphDriver + pw_graph_backend::RelayDriver {}

#[cfg(not(feature = "relay"))]
pub trait ApplicationDriver: GraphDriver {}

#[cfg(not(feature = "relay"))]
impl<T> ApplicationDriver for T where T: GraphDriver {}

/// Result of attempting to open the optional native backends.  A missing
/// backend is reported to the caller but does not prevent the other backend
/// from remaining usable.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BackendAvailability {
    pub pipewire: bool,
    pub alsa: bool,
    pub failures: Vec<String>,
}

/// PipeWire + ALSA MIDI graph with disjoint ID namespaces.
#[derive(Default)]
pub struct CompositeDriver {
    #[cfg(feature = "pipewire")]
    pub pipewire: Option<PipewireDriver>,
    #[cfg(feature = "alsa")]
    pub alsa: Option<AlsaMidiDriver>,
    graph: Graph,
}

impl CompositeDriver {
    /// Construct a live composite and retain any per-backend startup errors.
    /// This is intentionally the only place where the two native drivers are
    /// opened for the desktop application.
    #[allow(unused_mut)]
    pub fn open(no_alsa_midi: bool) -> (Self, BackendAvailability) {
        let _ = no_alsa_midi;
        let mut composite = Self::default();
        let mut availability = BackendAvailability::default();

        #[cfg(feature = "pipewire")]
        match PipewireDriver::new() {
            Ok(driver) => {
                composite.pipewire = Some(driver);
                availability.pipewire = true;
            }
            Err(error) => availability.failures.push(error.to_string()),
        }

        #[cfg(feature = "alsa")]
        if !no_alsa_midi {
            match AlsaMidiDriver::new() {
                Ok(driver) => {
                    composite.alsa = Some(driver);
                    availability.alsa = true;
                }
                Err(error) => availability.failures.push(error.to_string()),
            }
        }

        (composite, availability)
    }

    /// Build a composite from already-created children.  This is useful for
    /// deterministic integration tests and keeps construction independent of
    /// the windowing toolkit.
    #[cfg(feature = "pipewire")]
    pub fn with_pipewire(driver: PipewireDriver) -> Self {
        Self {
            pipewire: Some(driver),
            #[cfg(feature = "alsa")]
            alsa: None,
            graph: Graph::default(),
        }
    }

    #[allow(dead_code)]
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

    #[allow(unused_mut)]
    fn rebuild_merged_graph(&mut self) -> Result<(), GraphError> {
        let mut graph = Graph::default();
        #[cfg(feature = "pipewire")]
        if let Some(driver) = self.pipewire.as_ref() {
            Self::merge_graph(&mut graph, driver.graph())?;
        }
        #[cfg(feature = "alsa")]
        if let Some(driver) = self.alsa.as_ref() {
            Self::merge_graph(&mut graph, driver.graph())?;
        }
        self.graph = graph;
        Ok(())
    }

    fn unsupported(message: impl Into<String>) -> BackendError {
        BackendError::Unsupported(message.into())
    }

    #[cfg(feature = "pipewire")]
    fn pipewire_mut(&mut self) -> BackendResult<&mut PipewireDriver> {
        self.pipewire
            .as_mut()
            .ok_or_else(|| Self::unsupported("PipeWire backend is unavailable"))
    }

    #[cfg(feature = "alsa")]
    fn alsa_mut(&mut self) -> BackendResult<&mut AlsaMidiDriver> {
        self.alsa
            .as_mut()
            .ok_or_else(|| Self::unsupported("ALSA MIDI backend is unavailable"))
    }

    #[allow(dead_code)]
    fn rebuild_after_native_mutation(&mut self) {
        if let Err(error) = self.rebuild_merged_graph() {
            // The native mutation has already succeeded.  Keep that success
            // visible and let the next normal refresh repair the projection.
            eprintln!("could not rebuild composite graph: {error}");
        }
    }

    #[cfg(feature = "pipewire")]
    fn mutate_pipewire<T>(
        &mut self,
        operation: impl FnOnce(&mut PipewireDriver) -> BackendResult<T>,
    ) -> BackendResult<T> {
        let value = operation(self.pipewire_mut()?)?;
        self.rebuild_after_native_mutation();
        Ok(value)
    }

    pub fn has_pipewire(&self) -> bool {
        #[cfg(feature = "pipewire")]
        {
            self.pipewire.is_some()
        }
        #[cfg(not(feature = "pipewire"))]
        {
            false
        }
    }

    pub fn has_alsa(&self) -> bool {
        #[cfg(feature = "alsa")]
        {
            self.alsa.is_some()
        }
        #[cfg(not(feature = "alsa"))]
        {
            false
        }
    }
}

impl GraphDriver for CompositeDriver {
    fn refresh(&mut self) -> BackendResult<Vec<Node>> {
        #[cfg(feature = "pipewire")]
        if let Some(driver) = self.pipewire.as_mut() {
            driver.refresh()?;
        }
        #[cfg(feature = "alsa")]
        if let Some(driver) = self.alsa.as_mut() {
            driver.refresh()?;
        }
        self.rebuild_merged_graph()?;
        Ok(self.graph.nodes.values().cloned().collect())
    }

    fn connect(&mut self, src: PortId, dst: PortId) -> BackendResult<Link> {
        match route_for_ports(src, dst) {
            Ok(CompositeRoute::AlsaMidi) => {
                #[cfg(feature = "alsa")]
                {
                    let link = self.alsa_mut()?.connect(src, dst)?;
                    self.refresh()?;
                    Ok(link)
                }
                #[cfg(not(feature = "alsa"))]
                Err(Self::unsupported("ALSA MIDI backend is disabled"))
            }
            Ok(CompositeRoute::PipeWire) => {
                #[cfg(feature = "pipewire")]
                {
                    let link = self.pipewire_mut()?.connect(src, dst)?;
                    self.refresh()?;
                    Ok(link)
                }
                #[cfg(not(feature = "pipewire"))]
                Err(Self::unsupported("PipeWire backend is disabled"))
            }
            Err(error) => Err(Self::unsupported(error)),
        }
    }

    #[allow(unused_variables)]
    fn disconnect(&mut self, link: LinkId) -> BackendResult<Link> {
        let existing = self
            .graph
            .link(link)
            .cloned()
            .ok_or(GraphError::MissingLink(link))?;
        if link.0 & ALSA_ID_FLAG != 0 {
            #[cfg(feature = "alsa")]
            {
                self.alsa_mut()?.disconnect(link)?;
                self.refresh()?;
                Ok(existing)
            }
            #[cfg(not(feature = "alsa"))]
            return Err(Self::unsupported("ALSA MIDI backend is disabled"));
        } else {
            #[cfg(feature = "pipewire")]
            {
                self.pipewire_mut()?.disconnect(link)?;
                self.refresh()?;
                Ok(existing)
            }
            #[cfg(not(feature = "pipewire"))]
            return Err(Self::unsupported("PipeWire backend is disabled"));
        }
    }

    #[allow(unused_variables)]
    fn set_node_position(&mut self, node: NodeId, position: [f32; 2]) -> BackendResult<()> {
        if node.0 & ALSA_ID_FLAG != 0 {
            #[cfg(feature = "alsa")]
            {
                self.alsa_mut()?.set_node_position(node, position)?;
                if let Some(node_data) = self.graph.nodes.get_mut(&node) {
                    node_data.position = position;
                }
                Ok(())
            }
            #[cfg(not(feature = "alsa"))]
            return Err(Self::unsupported("ALSA MIDI backend is disabled"));
        } else {
            #[cfg(feature = "pipewire")]
            {
                self.pipewire_mut()?.set_node_position(node, position)?;
                if let Some(node_data) = self.graph.nodes.get_mut(&node) {
                    node_data.position = position;
                }
                Ok(())
            }
            #[cfg(not(feature = "pipewire"))]
            return Err(Self::unsupported("PipeWire backend is disabled"));
        }
    }

    fn set_node_mute(&mut self, node: NodeId, muted: bool) -> BackendResult<()> {
        if node.0 & ALSA_ID_FLAG != 0 {
            return Err(Self::unsupported(
                "ALSA MIDI nodes do not expose audio mute",
            ));
        }
        #[cfg(feature = "pipewire")]
        {
            self.pipewire_mut()?.set_node_mute(node, muted)
        }
        #[cfg(not(feature = "pipewire"))]
        {
            let _ = (node, muted);
            Err(Self::unsupported("PipeWire backend is disabled"))
        }
    }

    fn set_node_volume(&mut self, node: NodeId, volume: f32) -> BackendResult<()> {
        if node.0 & ALSA_ID_FLAG != 0 {
            return Err(Self::unsupported(
                "ALSA MIDI nodes do not expose audio volume",
            ));
        }
        #[cfg(feature = "pipewire")]
        {
            self.pipewire_mut()?.set_node_volume(node, volume)
        }
        #[cfg(not(feature = "pipewire"))]
        {
            let _ = (node, volume);
            Err(Self::unsupported("PipeWire backend is disabled"))
        }
    }

    fn graph(&self) -> &Graph {
        &self.graph
    }

    fn graph_dirty(&self) -> bool {
        #[cfg(feature = "pipewire")]
        if self
            .pipewire
            .as_ref()
            .is_some_and(|driver| driver.graph_dirty())
        {
            return true;
        }
        #[cfg(feature = "alsa")]
        if self
            .alsa
            .as_ref()
            .is_some_and(|driver| driver.graph_dirty())
        {
            return true;
        }
        false
    }

    fn is_node_type(&self, node_type: NodeType) -> bool {
        let _ = node_type;
        #[cfg(feature = "pipewire")]
        if self
            .pipewire
            .as_ref()
            .is_some_and(|driver| driver.is_node_type(node_type))
        {
            return true;
        }
        #[cfg(feature = "alsa")]
        if self
            .alsa
            .as_ref()
            .is_some_and(|driver| driver.is_node_type(node_type))
        {
            return true;
        }
        false
    }

    fn is_port_type(&self, port_type: PortType) -> bool {
        let _ = port_type;
        #[cfg(feature = "pipewire")]
        if self
            .pipewire
            .as_ref()
            .is_some_and(|driver| driver.is_port_type(port_type))
        {
            return true;
        }
        #[cfg(feature = "alsa")]
        if self
            .alsa
            .as_ref()
            .is_some_and(|driver| driver.is_port_type(port_type))
        {
            return true;
        }
        false
    }

    fn audio_meters(&mut self) -> BackendResult<Vec<pw_graph_backend::AudioMeter>> {
        #[cfg(feature = "pipewire")]
        if let Some(driver) = self.pipewire.as_mut() {
            return driver.audio_meters();
        }
        Ok(Vec::new())
    }

    fn set_meter_policy(&mut self, policy: MeterPolicy) -> BackendResult<()> {
        #[cfg(feature = "pipewire")]
        if let Some(driver) = self.pipewire.as_mut() {
            return driver.set_meter_policy(policy);
        }
        let _ = policy;
        Ok(())
    }

    fn request_meters(&mut self, nodes: &BTreeSet<NodeId>) -> BackendResult<()> {
        #[cfg(feature = "pipewire")]
        if let Some(driver) = self.pipewire.as_mut() {
            return driver.request_meters(nodes);
        }
        let _ = nodes;
        Ok(())
    }

    fn reset_audio_config(&mut self) -> BackendResult<()> {
        #[cfg(feature = "pipewire")]
        if let Some(driver) = self.pipewire.as_mut() {
            return driver.reset_audio_config();
        }
        Ok(())
    }
}

impl pw_graph_backend::EffectDriver for CompositeDriver {
    fn effect_descriptors(&self) -> Vec<pw_graph_effects::EffectDescriptor> {
        #[cfg(feature = "pipewire")]
        {
            self.pipewire
                .as_ref()
                .map(|driver| driver.effect_descriptors())
                .unwrap_or_default()
        }
        #[cfg(not(feature = "pipewire"))]
        {
            Vec::new()
        }
    }

    fn effect_instances(&self) -> Vec<pw_graph_backend::EffectInstance> {
        #[cfg(feature = "pipewire")]
        {
            self.pipewire
                .as_ref()
                .map(|driver| driver.effect_instances())
                .unwrap_or_default()
        }
        #[cfg(not(feature = "pipewire"))]
        {
            Vec::new()
        }
    }

    fn supports_effect_nodes(&self) -> bool {
        #[cfg(feature = "pipewire")]
        {
            self.pipewire
                .as_ref()
                .is_some_and(|driver| driver.supports_effect_nodes())
        }
        #[cfg(not(feature = "pipewire"))]
        {
            false
        }
    }

    fn create_effect_node(
        &mut self,
        request: pw_graph_backend::EffectNodeRequest,
    ) -> BackendResult<pw_graph_backend::EffectInstance> {
        #[cfg(feature = "pipewire")]
        {
            self.mutate_pipewire(|driver| driver.create_effect_node(request))
        }
        #[cfg(not(feature = "pipewire"))]
        {
            let _ = request;
            Err(Self::unsupported("effect processing is unavailable"))
        }
    }

    fn insert_effect(
        &mut self,
        request: pw_graph_backend::EffectInsertRequest,
    ) -> BackendResult<pw_graph_backend::EffectInstance> {
        #[cfg(feature = "pipewire")]
        {
            self.mutate_pipewire(|driver| driver.insert_effect(request))
        }
        #[cfg(not(feature = "pipewire"))]
        {
            let _ = request;
            Err(Self::unsupported("effect processing is unavailable"))
        }
    }

    fn set_effect_enabled(&mut self, instance_id: &str, enabled: bool) -> BackendResult<()> {
        #[cfg(feature = "pipewire")]
        {
            self.pipewire_mut()?
                .set_effect_enabled(instance_id, enabled)
        }
        #[cfg(not(feature = "pipewire"))]
        {
            let _ = (instance_id, enabled);
            Err(Self::unsupported("effect processing is unavailable"))
        }
    }

    fn set_effect_parameter(
        &mut self,
        instance_id: &str,
        parameter: &str,
        value: f32,
    ) -> BackendResult<()> {
        #[cfg(feature = "pipewire")]
        {
            self.pipewire_mut()?
                .set_effect_parameter(instance_id, parameter, value)
        }
        #[cfg(not(feature = "pipewire"))]
        {
            let _ = (instance_id, parameter, value);
            Err(Self::unsupported("effect processing is unavailable"))
        }
    }

    fn remove_effect(&mut self, instance_id: &str) -> BackendResult<()> {
        #[cfg(feature = "pipewire")]
        {
            self.mutate_pipewire(|driver| driver.remove_effect(instance_id))
        }
        #[cfg(not(feature = "pipewire"))]
        {
            let _ = instance_id;
            Err(Self::unsupported("effect processing is unavailable"))
        }
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

    fn relay_disconnect(&mut self, session: pw_graph_backend::RelaySessionId) -> BackendResult<()> {
        self.pipewire_mut()?.relay_disconnect(session)
    }

    fn relay_events(&mut self) -> Vec<pw_graph_backend::RelayEvent> {
        self.pipewire
            .as_mut()
            .map(|driver| driver.relay_events())
            .unwrap_or_default()
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

#[cfg(test)]
mod tests {
    use super::*;
    use pw_graph_backend::InMemoryDriver;
    use pw_graph_core::{Direction, Node, Port};

    #[test]
    fn composite_reports_cross_backend_connections_before_mutation() {
        let pipewire_output = PortId(42);
        let alsa_output = PortId(ALSA_ID_FLAG | 42);

        assert_eq!(
            route_for_ports(pipewire_output, PortId(43)),
            Ok(CompositeRoute::PipeWire)
        );
        assert_eq!(
            route_for_ports(alsa_output, PortId(ALSA_ID_FLAG | 43)),
            Ok(CompositeRoute::AlsaMidi)
        );
        assert_eq!(
            route_for_ports(pipewire_output, PortId(ALSA_ID_FLAG | 43)),
            Err("connections cannot cross PipeWire and ALSA MIDI backends")
        );
        assert_eq!(
            route_for_ports(alsa_output, PortId(43)),
            Err("connections cannot cross PipeWire and ALSA MIDI backends")
        );
    }

    #[test]
    fn application_driver_blanket_impl_covers_the_deterministic_driver() {
        fn accepts_driver<T: ApplicationDriver>(_driver: &T) {}
        accepts_driver(&InMemoryDriver::demo());
    }

    #[test]
    fn a_composite_merges_children_without_overlapping_namespaces() {
        let mut graph = Graph::default();
        graph
            .add_node(Node::new(NodeId(1), "PipeWire", NodeType::PipeWire))
            .unwrap();
        graph
            .add_port(Port::new(
                PortId(1),
                NodeId(1),
                "out",
                Direction::Source,
                PortType::Audio,
            ))
            .unwrap();
        assert!(graph.port(PortId(1)).is_some());
    }
}
