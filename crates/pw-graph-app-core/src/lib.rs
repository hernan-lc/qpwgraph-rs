//! Framework-neutral application/backend composition.
//!
//! The desktop UI is deliberately not part of this crate.  Both the native
//! application shell and tests use the same [`CompositeDriver`], so a view
//! cannot accidentally create a second graph namespace or route an ALSA
//! resource through PipeWire.

use pw_graph_backend::{
    BackendCapabilities, BackendError, BackendResult, GraphDriver, MeterPolicy, NodeAudioState,
    NodeCapabilities,
};
use pw_graph_core::{
    backend_for_link, backend_for_node, backend_for_port, BackendKind, Graph, GraphError, Link,
    LinkId, Node, NodeId, NodeType, PortId, PortType,
};
use std::collections::BTreeSet;

/// Legacy public compatibility constant. New routing code uses the shared
/// backend namespace helpers in `pw-graph-core`; the high bit remains
/// recognized only for IDs written by older ALSA builds.
pub const ALSA_ID_FLAG: u64 = 1_u64 << 63;

#[cfg(all(target_os = "linux", feature = "alsa"))]
use pw_graph_alsamidi::AlsaMidiDriver;
#[cfg(all(target_os = "linux", feature = "pipewire"))]
use pw_graph_backend::PipewireDriver;
#[cfg(target_os = "windows")]
use pw_graph_backend::WindowsAudioDriver;

/// The native driver that owns a pair of graph ports.  This classification is
/// kept independent of the UI and is used before a mutation is forwarded to a
/// child driver.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompositeRoute {
    PipeWire,
    AlsaMidi,
    WindowsAudio,
    WindowsMidi,
    Demo,
}

impl CompositeRoute {
    fn from_backend(backend: BackendKind) -> Self {
        match backend {
            BackendKind::PipeWire => Self::PipeWire,
            BackendKind::AlsaMidi => Self::AlsaMidi,
            BackendKind::WindowsAudio => Self::WindowsAudio,
            BackendKind::WindowsMidi => Self::WindowsMidi,
            BackendKind::Demo => Self::Demo,
        }
    }
}

/// Classify a connection without touching either native backend.
///
/// Backend ownership is read from the shared ID namespace. A mixed pair is
/// intentionally rejected instead of being allowed to fall through to a
/// default native backend.
pub fn route_for_ports(src: PortId, dst: PortId) -> Result<CompositeRoute, &'static str> {
    let source = backend_for_port(src).ok_or("source port has an unknown backend namespace")?;
    let destination =
        backend_for_port(dst).ok_or("destination port has an unknown backend namespace")?;
    if source != destination {
        return Err(match (source, destination) {
            (BackendKind::PipeWire, BackendKind::AlsaMidi)
            | (BackendKind::AlsaMidi, BackendKind::PipeWire) => {
                "connections cannot cross PipeWire and ALSA MIDI backends"
            }
            _ => "connections cannot cross native backends",
        });
    }
    Ok(CompositeRoute::from_backend(source))
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
    pub windows_audio: bool,
    pub failures: Vec<String>,
}

/// PipeWire + ALSA MIDI graph with disjoint ID namespaces.
#[derive(Default)]
pub struct CompositeDriver {
    #[cfg(all(target_os = "linux", feature = "pipewire"))]
    pub pipewire: Option<PipewireDriver>,
    #[cfg(all(target_os = "linux", feature = "alsa"))]
    pub alsa: Option<AlsaMidiDriver>,
    #[cfg(target_os = "windows")]
    pub windows_audio: Option<WindowsAudioDriver>,
    graph: Graph,
}

impl CompositeDriver {
    /// Construct a live composite and retain any per-backend startup errors.
    /// This is intentionally the only place where the two native drivers are
    /// opened for the desktop application.
    #[allow(unused_mut)]
    pub fn open(no_midi: bool) -> (Self, BackendAvailability) {
        let _ = no_midi;
        let mut composite = Self::default();
        let mut availability = BackendAvailability::default();

        #[cfg(all(target_os = "linux", feature = "pipewire"))]
        match PipewireDriver::new() {
            Ok(driver) => {
                composite.pipewire = Some(driver);
                availability.pipewire = true;
            }
            Err(error) => availability.failures.push(error.to_string()),
        }

        #[cfg(all(target_os = "linux", feature = "alsa"))]
        if !no_midi {
            match AlsaMidiDriver::new() {
                Ok(driver) => {
                    composite.alsa = Some(driver);
                    availability.alsa = true;
                }
                Err(error) => availability.failures.push(error.to_string()),
            }
        }

        #[cfg(target_os = "windows")]
        match WindowsAudioDriver::new() {
            Ok(driver) => {
                composite.windows_audio = Some(driver);
                availability.windows_audio = true;
            }
            Err(error) => availability.failures.push(error.to_string()),
        }

        (composite, availability)
    }

    /// Build a composite from already-created children.  This is useful for
    /// deterministic integration tests and keeps construction independent of
    /// the windowing toolkit.
    #[cfg(all(target_os = "linux", feature = "pipewire"))]
    pub fn with_pipewire(driver: PipewireDriver) -> Self {
        Self {
            pipewire: Some(driver),
            #[cfg(all(target_os = "linux", feature = "alsa"))]
            alsa: None,
            #[cfg(target_os = "windows")]
            windows_audio: None,
            graph: Graph::default(),
        }
    }

    #[cfg(target_os = "windows")]
    pub fn with_windows_audio(driver: WindowsAudioDriver) -> Self {
        Self {
            windows_audio: Some(driver),
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
        #[cfg(all(target_os = "linux", feature = "pipewire"))]
        if let Some(driver) = self.pipewire.as_ref() {
            Self::merge_graph(&mut graph, driver.graph())?;
        }
        #[cfg(all(target_os = "linux", feature = "alsa"))]
        if let Some(driver) = self.alsa.as_ref() {
            Self::merge_graph(&mut graph, driver.graph())?;
        }
        #[cfg(target_os = "windows")]
        if let Some(driver) = self.windows_audio.as_ref() {
            Self::merge_graph(&mut graph, driver.graph())?;
        }
        self.graph = graph;
        Ok(())
    }

    fn unsupported(message: impl Into<String>) -> BackendError {
        BackendError::Unsupported(message.into())
    }

    #[cfg(all(target_os = "linux", feature = "pipewire"))]
    fn pipewire_mut(&mut self) -> BackendResult<&mut PipewireDriver> {
        self.pipewire
            .as_mut()
            .ok_or_else(|| Self::unsupported("PipeWire backend is unavailable"))
    }

    #[cfg(all(target_os = "linux", feature = "alsa"))]
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

    #[cfg(all(target_os = "linux", feature = "pipewire"))]
    fn mutate_pipewire<T>(
        &mut self,
        operation: impl FnOnce(&mut PipewireDriver) -> BackendResult<T>,
    ) -> BackendResult<T> {
        let value = operation(self.pipewire_mut()?)?;
        self.rebuild_after_native_mutation();
        Ok(value)
    }

    pub fn has_pipewire(&self) -> bool {
        #[cfg(all(target_os = "linux", feature = "pipewire"))]
        {
            self.pipewire.is_some()
        }
        #[cfg(not(all(target_os = "linux", feature = "pipewire")))]
        {
            false
        }
    }

    pub fn has_alsa(&self) -> bool {
        #[cfg(all(target_os = "linux", feature = "alsa"))]
        {
            self.alsa.is_some()
        }
        #[cfg(not(all(target_os = "linux", feature = "alsa")))]
        {
            false
        }
    }

    pub fn has_windows_audio(&self) -> bool {
        #[cfg(target_os = "windows")]
        {
            self.windows_audio.is_some()
        }
        #[cfg(not(target_os = "windows"))]
        {
            false
        }
    }

    /// Capabilities for one backend namespace in this composite.
    pub fn capabilities_for_backend(&self, backend: BackendKind) -> BackendCapabilities {
        match backend {
            BackendKind::PipeWire => {
                #[cfg(all(target_os = "linux", feature = "pipewire"))]
                {
                    return self
                        .pipewire
                        .as_ref()
                        .map(GraphDriver::capabilities)
                        .unwrap_or_default();
                }
                #[cfg(not(all(target_os = "linux", feature = "pipewire")))]
                {
                    BackendCapabilities::default()
                }
            }
            BackendKind::AlsaMidi => {
                #[cfg(all(target_os = "linux", feature = "alsa"))]
                {
                    return self
                        .alsa
                        .as_ref()
                        .map(GraphDriver::capabilities)
                        .unwrap_or_default();
                }
                #[cfg(not(all(target_os = "linux", feature = "alsa")))]
                {
                    BackendCapabilities::default()
                }
            }
            BackendKind::WindowsAudio | BackendKind::WindowsMidi | BackendKind::Demo => {
                #[cfg(target_os = "windows")]
                if backend == BackendKind::WindowsAudio {
                    return self
                        .windows_audio
                        .as_ref()
                        .map(GraphDriver::capabilities)
                        .unwrap_or_default();
                }
                BackendCapabilities::default()
            }
        }
    }

    /// Capabilities for the backend that owns a graph node.
    pub fn capabilities_for_node(&self, node: NodeId) -> BackendCapabilities {
        backend_for_node(node)
            .map(|backend| self.capabilities_for_backend(backend))
            .unwrap_or_default()
    }

    /// Return whether a projected link can be persisted and mutated by the
    /// native child backend.
    pub fn is_link_mutable(&self, link: LinkId) -> bool {
        GraphDriver::is_link_mutable(self, link)
    }
}

impl GraphDriver for CompositeDriver {
    #[allow(unused_mut)]
    fn capabilities(&self) -> BackendCapabilities {
        let mut capabilities = BackendCapabilities::default();
        #[cfg(all(target_os = "linux", feature = "pipewire"))]
        if let Some(driver) = self.pipewire.as_ref() {
            capabilities = capabilities.union(driver.capabilities());
        }
        #[cfg(all(target_os = "linux", feature = "alsa"))]
        if let Some(driver) = self.alsa.as_ref() {
            capabilities = capabilities.union(driver.capabilities());
        }
        #[cfg(target_os = "windows")]
        if let Some(driver) = self.windows_audio.as_ref() {
            capabilities = capabilities.union(driver.capabilities());
        }
        capabilities
    }

    fn refresh(&mut self) -> BackendResult<Vec<Node>> {
        #[cfg(all(target_os = "linux", feature = "pipewire"))]
        if let Some(driver) = self.pipewire.as_mut() {
            driver.refresh()?;
        }
        #[cfg(all(target_os = "linux", feature = "alsa"))]
        if let Some(driver) = self.alsa.as_mut() {
            driver.refresh()?;
        }
        #[cfg(target_os = "windows")]
        if let Some(driver) = self.windows_audio.as_mut() {
            driver.refresh()?;
        }
        self.rebuild_merged_graph()?;
        Ok(self.graph.nodes.values().cloned().collect())
    }

    fn connect(&mut self, src: PortId, dst: PortId) -> BackendResult<Link> {
        match route_for_ports(src, dst) {
            Ok(CompositeRoute::AlsaMidi) => {
                #[cfg(all(target_os = "linux", feature = "alsa"))]
                {
                    let link = self.alsa_mut()?.connect(src, dst)?;
                    self.refresh()?;
                    Ok(link)
                }
                #[cfg(not(all(target_os = "linux", feature = "alsa")))]
                Err(Self::unsupported("ALSA MIDI backend is disabled"))
            }
            Ok(CompositeRoute::PipeWire) => {
                #[cfg(all(target_os = "linux", feature = "pipewire"))]
                {
                    let link = self.pipewire_mut()?.connect(src, dst)?;
                    self.refresh()?;
                    Ok(link)
                }
                #[cfg(not(all(target_os = "linux", feature = "pipewire")))]
                Err(Self::unsupported("PipeWire backend is disabled"))
            }
            Ok(CompositeRoute::WindowsAudio) => {
                Err(Self::unsupported("Windows audio routing is not supported"))
            }
            Ok(CompositeRoute::WindowsMidi) => {
                Err(Self::unsupported("Windows MIDI routing is not supported"))
            }
            Ok(CompositeRoute::Demo) => Err(Self::unsupported(
                "demo resources are not part of the live composite",
            )),
            Err(error) => Err(Self::unsupported(error)),
        }
    }

    fn disconnect(&mut self, link: LinkId) -> BackendResult<Link> {
        let existing = self
            .graph
            .link(link)
            .cloned()
            .ok_or(GraphError::MissingLink(link))?;
        let route = backend_for_link(link)
            .map(CompositeRoute::from_backend)
            .or_else(|| route_for_ports(existing.output_port, existing.input_port).ok())
            .ok_or_else(|| Self::unsupported("link has an unknown backend namespace"))?;
        match route {
            CompositeRoute::AlsaMidi => {
                #[cfg(all(target_os = "linux", feature = "alsa"))]
                {
                    self.alsa_mut()?.disconnect(link)?;
                    self.refresh()?;
                    Ok(existing)
                }
                #[cfg(not(all(target_os = "linux", feature = "alsa")))]
                Err(Self::unsupported("ALSA MIDI backend is disabled"))
            }
            CompositeRoute::PipeWire => {
                #[cfg(all(target_os = "linux", feature = "pipewire"))]
                {
                    self.pipewire_mut()?.disconnect(link)?;
                    self.refresh()?;
                    Ok(existing)
                }
                #[cfg(not(all(target_os = "linux", feature = "pipewire")))]
                Err(Self::unsupported("PipeWire backend is disabled"))
            }
            CompositeRoute::WindowsAudio => {
                Err(Self::unsupported("Windows audio routing is not supported"))
            }
            CompositeRoute::WindowsMidi => {
                Err(Self::unsupported("Windows MIDI routing is not supported"))
            }
            CompositeRoute::Demo => Err(Self::unsupported(
                "demo resources are not part of the live composite",
            )),
        }
    }

    fn is_link_mutable(&self, link: LinkId) -> bool {
        let Some(existing) = self.graph.link(link) else {
            return false;
        };
        let route = backend_for_link(link)
            .map(CompositeRoute::from_backend)
            .or_else(|| route_for_ports(existing.output_port, existing.input_port).ok());
        match route {
            Some(CompositeRoute::PipeWire) => {
                #[cfg(all(target_os = "linux", feature = "pipewire"))]
                {
                    return self
                        .pipewire
                        .as_ref()
                        .is_some_and(|driver| driver.is_link_mutable(link));
                }
            }
            Some(CompositeRoute::AlsaMidi) => {
                #[cfg(all(target_os = "linux", feature = "alsa"))]
                {
                    return self
                        .alsa
                        .as_ref()
                        .is_some_and(|driver| driver.is_link_mutable(link));
                }
            }
            Some(CompositeRoute::WindowsAudio) => {
                #[cfg(target_os = "windows")]
                {
                    return self
                        .windows_audio
                        .as_ref()
                        .is_some_and(|driver| driver.is_link_mutable(link));
                }
            }
            Some(CompositeRoute::WindowsMidi | CompositeRoute::Demo) | None => {}
        }
        false
    }

    fn set_node_position(&mut self, node: NodeId, position: [f32; 2]) -> BackendResult<()> {
        let _ = position;
        match backend_for_node(node) {
            Some(BackendKind::AlsaMidi) => {
                #[cfg(all(target_os = "linux", feature = "alsa"))]
                {
                    self.alsa_mut()?.set_node_position(node, position)?;
                    if let Some(node_data) = self.graph.nodes.get_mut(&node) {
                        node_data.position = position;
                    }
                    Ok(())
                }
                #[cfg(not(all(target_os = "linux", feature = "alsa")))]
                Err(Self::unsupported("ALSA MIDI backend is disabled"))
            }
            Some(BackendKind::PipeWire) => {
                #[cfg(all(target_os = "linux", feature = "pipewire"))]
                {
                    self.pipewire_mut()?.set_node_position(node, position)?;
                    if let Some(node_data) = self.graph.nodes.get_mut(&node) {
                        node_data.position = position;
                    }
                    Ok(())
                }
                #[cfg(not(all(target_os = "linux", feature = "pipewire")))]
                Err(Self::unsupported("PipeWire backend is disabled"))
            }
            Some(BackendKind::WindowsAudio) => {
                #[cfg(target_os = "windows")]
                {
                    self.windows_audio
                        .as_mut()
                        .ok_or_else(|| Self::unsupported("Windows audio backend is unavailable"))?
                        .set_node_position(node, position)?;
                    if let Some(node_data) = self.graph.nodes.get_mut(&node) {
                        node_data.position = position;
                    }
                    Ok(())
                }
                #[cfg(not(target_os = "windows"))]
                {
                    Err(Self::unsupported("Windows audio backend is unavailable"))
                }
            }
            Some(BackendKind::WindowsMidi) => Err(Self::unsupported(
                "Windows MIDI layout is managed by the application",
            )),
            Some(BackendKind::Demo) => Err(Self::unsupported(
                "demo resources are not part of the live composite",
            )),
            None => Err(Self::unsupported("node has an unknown backend namespace")),
        }
    }

    /// Forwarded to the backend that owns the node.
    ///
    /// This must stay in sync with `set_node_volume`/`set_node_mute`: the trait
    /// default answers `UNSUPPORTED`, so a composite that forgets to delegate
    /// silently strips every audio control off every live card instead of
    /// failing loudly. Covered by `composite_forwards_audio_state_to_the_owning_backend`.
    fn node_audio_state(&self, node: NodeId) -> BackendResult<NodeAudioState> {
        match backend_for_node(node) {
            // Real nodes with nothing to control, not errors.
            Some(BackendKind::AlsaMidi) | Some(BackendKind::WindowsMidi) => {
                Ok(NodeAudioState::UNSUPPORTED)
            }
            Some(BackendKind::PipeWire) => {
                #[cfg(all(target_os = "linux", feature = "pipewire"))]
                {
                    match self.pipewire.as_ref() {
                        Some(driver) => driver.node_audio_state(node),
                        None => Ok(NodeAudioState::UNSUPPORTED),
                    }
                }
                #[cfg(not(all(target_os = "linux", feature = "pipewire")))]
                {
                    let _ = node;
                    Ok(NodeAudioState::UNSUPPORTED)
                }
            }
            Some(BackendKind::WindowsAudio) => {
                #[cfg(target_os = "windows")]
                {
                    match self.windows_audio.as_ref() {
                        Some(driver) => driver.node_audio_state(node),
                        None => Ok(NodeAudioState::UNSUPPORTED),
                    }
                }
                #[cfg(not(target_os = "windows"))]
                {
                    let _ = node;
                    Ok(NodeAudioState::UNSUPPORTED)
                }
            }
            Some(BackendKind::Demo) => Err(Self::unsupported(
                "demo resources are not part of the live composite",
            )),
            None => Err(Self::unsupported("node has an unknown backend namespace")),
        }
    }

    fn node_capabilities(&self, node: NodeId) -> NodeCapabilities {
        match backend_for_node(node) {
            Some(BackendKind::AlsaMidi) | Some(BackendKind::WindowsMidi) => NodeCapabilities::NONE,
            Some(BackendKind::PipeWire) => {
                #[cfg(all(target_os = "linux", feature = "pipewire"))]
                {
                    match self.pipewire.as_ref() {
                        Some(driver) => driver.node_capabilities(node),
                        None => NodeCapabilities::NONE,
                    }
                }
                #[cfg(not(all(target_os = "linux", feature = "pipewire")))]
                {
                    let _ = node;
                    NodeCapabilities::NONE
                }
            }
            Some(BackendKind::WindowsAudio) => {
                #[cfg(target_os = "windows")]
                {
                    match self.windows_audio.as_ref() {
                        Some(driver) => driver.node_capabilities(node),
                        None => NodeCapabilities::NONE,
                    }
                }
                #[cfg(not(target_os = "windows"))]
                {
                    let _ = node;
                    NodeCapabilities::NONE
                }
            }
            Some(BackendKind::Demo) | None => NodeCapabilities::NONE,
        }
    }

    fn set_node_mute(&mut self, node: NodeId, muted: bool) -> BackendResult<()> {
        match backend_for_node(node) {
            Some(BackendKind::AlsaMidi) => Err(Self::unsupported(
                "ALSA MIDI nodes do not expose audio mute",
            )),
            Some(BackendKind::PipeWire) => {
                #[cfg(all(target_os = "linux", feature = "pipewire"))]
                {
                    self.pipewire_mut()?.set_node_mute(node, muted)
                }
                #[cfg(not(all(target_os = "linux", feature = "pipewire")))]
                {
                    let _ = (node, muted);
                    Err(Self::unsupported("PipeWire backend is disabled"))
                }
            }
            Some(BackendKind::WindowsAudio) => {
                #[cfg(target_os = "windows")]
                {
                    self.windows_audio
                        .as_mut()
                        .ok_or_else(|| Self::unsupported("Windows audio backend is unavailable"))?
                        .set_node_mute(node, muted)
                }
                #[cfg(not(target_os = "windows"))]
                {
                    let _ = (node, muted);
                    Err(Self::unsupported("Windows audio backend is unavailable"))
                }
            }
            Some(BackendKind::WindowsMidi) => Err(Self::unsupported(
                "Windows MIDI nodes do not expose audio mute",
            )),
            Some(BackendKind::Demo) => Err(Self::unsupported(
                "demo resources are not part of the live composite",
            )),
            None => Err(Self::unsupported("node has an unknown backend namespace")),
        }
    }

    fn set_node_volume(&mut self, node: NodeId, volume: f32) -> BackendResult<()> {
        match backend_for_node(node) {
            Some(BackendKind::AlsaMidi) => Err(Self::unsupported(
                "ALSA MIDI nodes do not expose audio volume",
            )),
            Some(BackendKind::PipeWire) => {
                #[cfg(all(target_os = "linux", feature = "pipewire"))]
                {
                    self.pipewire_mut()?.set_node_volume(node, volume)
                }
                #[cfg(not(all(target_os = "linux", feature = "pipewire")))]
                {
                    let _ = (node, volume);
                    Err(Self::unsupported("PipeWire backend is disabled"))
                }
            }
            Some(BackendKind::WindowsAudio) => {
                #[cfg(target_os = "windows")]
                {
                    self.windows_audio
                        .as_mut()
                        .ok_or_else(|| Self::unsupported("Windows audio backend is unavailable"))?
                        .set_node_volume(node, volume)
                }
                #[cfg(not(target_os = "windows"))]
                {
                    let _ = (node, volume);
                    Err(Self::unsupported("Windows audio backend is unavailable"))
                }
            }
            Some(BackendKind::WindowsMidi) => Err(Self::unsupported(
                "Windows MIDI nodes do not expose audio volume",
            )),
            Some(BackendKind::Demo) => Err(Self::unsupported(
                "demo resources are not part of the live composite",
            )),
            None => Err(Self::unsupported("node has an unknown backend namespace")),
        }
    }

    /// Only trustworthy when every live child reports its own changes: one
    /// child that must be polled means the composite must be polled.
    fn reports_graph_changes(&self) -> bool {
        let mut children = 0;
        let mut reporting = 0;
        #[cfg(all(target_os = "linux", feature = "pipewire"))]
        if let Some(driver) = self.pipewire.as_ref() {
            children += 1;
            reporting += usize::from(driver.reports_graph_changes());
        }
        #[cfg(all(target_os = "linux", feature = "alsa"))]
        if let Some(driver) = self.alsa.as_ref() {
            children += 1;
            reporting += usize::from(driver.reports_graph_changes());
        }
        #[cfg(target_os = "windows")]
        if let Some(driver) = self.windows_audio.as_ref() {
            children += 1;
            reporting += usize::from(driver.reports_graph_changes());
        }
        children > 0 && children == reporting
    }

    fn graph(&self) -> &Graph {
        &self.graph
    }

    fn graph_dirty(&self) -> bool {
        #[cfg(all(target_os = "linux", feature = "pipewire"))]
        if self
            .pipewire
            .as_ref()
            .is_some_and(|driver| driver.graph_dirty())
        {
            return true;
        }
        #[cfg(all(target_os = "linux", feature = "alsa"))]
        if self
            .alsa
            .as_ref()
            .is_some_and(|driver| driver.graph_dirty())
        {
            return true;
        }
        #[cfg(target_os = "windows")]
        if self
            .windows_audio
            .as_ref()
            .is_some_and(|driver| driver.graph_dirty())
        {
            return true;
        }
        false
    }

    fn is_node_type(&self, node_type: NodeType) -> bool {
        let _ = node_type;
        #[cfg(all(target_os = "linux", feature = "pipewire"))]
        if self
            .pipewire
            .as_ref()
            .is_some_and(|driver| driver.is_node_type(node_type))
        {
            return true;
        }
        #[cfg(all(target_os = "linux", feature = "alsa"))]
        if self
            .alsa
            .as_ref()
            .is_some_and(|driver| driver.is_node_type(node_type))
        {
            return true;
        }
        #[cfg(target_os = "windows")]
        if self
            .windows_audio
            .as_ref()
            .is_some_and(|driver| driver.is_node_type(node_type))
        {
            return true;
        }
        false
    }

    fn is_port_type(&self, port_type: PortType) -> bool {
        let _ = port_type;
        #[cfg(all(target_os = "linux", feature = "pipewire"))]
        if self
            .pipewire
            .as_ref()
            .is_some_and(|driver| driver.is_port_type(port_type))
        {
            return true;
        }
        #[cfg(all(target_os = "linux", feature = "alsa"))]
        if self
            .alsa
            .as_ref()
            .is_some_and(|driver| driver.is_port_type(port_type))
        {
            return true;
        }
        #[cfg(target_os = "windows")]
        if self
            .windows_audio
            .as_ref()
            .is_some_and(|driver| driver.is_port_type(port_type))
        {
            return true;
        }
        false
    }

    fn audio_meters(&mut self) -> BackendResult<Vec<pw_graph_backend::AudioMeter>> {
        #[cfg(all(target_os = "linux", feature = "pipewire"))]
        if let Some(driver) = self.pipewire.as_mut() {
            return driver.audio_meters();
        }
        #[cfg(target_os = "windows")]
        if let Some(driver) = self.windows_audio.as_mut() {
            return driver.audio_meters();
        }
        Ok(Vec::new())
    }

    fn set_meter_policy(&mut self, policy: MeterPolicy) -> BackendResult<()> {
        #[cfg(all(target_os = "linux", feature = "pipewire"))]
        if let Some(driver) = self.pipewire.as_mut() {
            return driver.set_meter_policy(policy);
        }
        #[cfg(target_os = "windows")]
        if let Some(driver) = self.windows_audio.as_mut() {
            return driver.set_meter_policy(policy);
        }
        let _ = policy;
        Ok(())
    }

    fn request_meters(&mut self, nodes: &BTreeSet<NodeId>) -> BackendResult<()> {
        #[cfg(all(target_os = "linux", feature = "pipewire"))]
        if let Some(driver) = self.pipewire.as_mut() {
            return driver.request_meters(nodes);
        }
        #[cfg(target_os = "windows")]
        if let Some(driver) = self.windows_audio.as_mut() {
            return driver.request_meters(nodes);
        }
        let _ = nodes;
        Ok(())
    }

    fn reset_audio_config(&mut self) -> BackendResult<()> {
        #[cfg(all(target_os = "linux", feature = "pipewire"))]
        if let Some(driver) = self.pipewire.as_mut() {
            return driver.reset_audio_config();
        }
        #[cfg(target_os = "windows")]
        if let Some(driver) = self.windows_audio.as_mut() {
            return driver.reset_audio_config();
        }
        Ok(())
    }
}

impl pw_graph_backend::EffectDriver for CompositeDriver {
    fn effect_descriptors(&self) -> Vec<pw_graph_effects::EffectDescriptor> {
        #[cfg(all(target_os = "linux", feature = "pipewire"))]
        {
            self.pipewire
                .as_ref()
                .map(|driver| driver.effect_descriptors())
                .unwrap_or_default()
        }
        #[cfg(not(all(target_os = "linux", feature = "pipewire")))]
        {
            Vec::new()
        }
    }

    fn effect_instances(&self) -> Vec<pw_graph_backend::EffectInstance> {
        #[cfg(all(target_os = "linux", feature = "pipewire"))]
        {
            self.pipewire
                .as_ref()
                .map(|driver| driver.effect_instances())
                .unwrap_or_default()
        }
        #[cfg(not(all(target_os = "linux", feature = "pipewire")))]
        {
            Vec::new()
        }
    }

    fn supports_effect_nodes(&self) -> bool {
        #[cfg(all(target_os = "linux", feature = "pipewire"))]
        {
            self.pipewire
                .as_ref()
                .is_some_and(|driver| driver.supports_effect_nodes())
        }
        #[cfg(not(all(target_os = "linux", feature = "pipewire")))]
        {
            false
        }
    }

    fn create_effect_node(
        &mut self,
        request: pw_graph_backend::EffectNodeRequest,
    ) -> BackendResult<pw_graph_backend::EffectInstance> {
        #[cfg(all(target_os = "linux", feature = "pipewire"))]
        {
            self.mutate_pipewire(|driver| driver.create_effect_node(request))
        }
        #[cfg(not(all(target_os = "linux", feature = "pipewire")))]
        {
            let _ = request;
            Err(Self::unsupported("effect processing is unavailable"))
        }
    }

    fn insert_effect(
        &mut self,
        request: pw_graph_backend::EffectInsertRequest,
    ) -> BackendResult<pw_graph_backend::EffectInstance> {
        #[cfg(all(target_os = "linux", feature = "pipewire"))]
        {
            self.mutate_pipewire(|driver| driver.insert_effect(request))
        }
        #[cfg(not(all(target_os = "linux", feature = "pipewire")))]
        {
            let _ = request;
            Err(Self::unsupported("effect processing is unavailable"))
        }
    }

    fn set_effect_enabled(&mut self, instance_id: &str, enabled: bool) -> BackendResult<()> {
        #[cfg(all(target_os = "linux", feature = "pipewire"))]
        {
            self.pipewire_mut()?
                .set_effect_enabled(instance_id, enabled)
        }
        #[cfg(not(all(target_os = "linux", feature = "pipewire")))]
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
        #[cfg(all(target_os = "linux", feature = "pipewire"))]
        {
            self.pipewire_mut()?
                .set_effect_parameter(instance_id, parameter, value)
        }
        #[cfg(not(all(target_os = "linux", feature = "pipewire")))]
        {
            let _ = (instance_id, parameter, value);
            Err(Self::unsupported("effect processing is unavailable"))
        }
    }

    fn remove_effect(&mut self, instance_id: &str) -> BackendResult<()> {
        #[cfg(all(target_os = "linux", feature = "pipewire"))]
        {
            self.mutate_pipewire(|driver| driver.remove_effect(instance_id))
        }
        #[cfg(not(all(target_os = "linux", feature = "pipewire")))]
        {
            let _ = instance_id;
            Err(Self::unsupported("effect processing is unavailable"))
        }
    }
}

/// Which child driver hosts the relay on this platform.
///
/// PipeWire carries it on Linux through virtual devices; Windows carries it
/// through WASAPI endpoints. Resolving the concrete type once here lets the
/// delegation below be written a single time, so the two platforms cannot
/// drift apart method by method.
#[cfg(feature = "relay")]
impl CompositeDriver {
    #[cfg(all(target_os = "linux", feature = "pipewire"))]
    fn relay_backend(&self) -> Option<&PipewireDriver> {
        self.pipewire.as_ref()
    }

    #[cfg(all(target_os = "linux", feature = "pipewire"))]
    fn relay_backend_mut(&mut self) -> Option<&mut PipewireDriver> {
        self.pipewire.as_mut()
    }

    #[cfg(target_os = "windows")]
    fn relay_backend(&self) -> Option<&WindowsAudioDriver> {
        self.windows_audio.as_ref()
    }

    #[cfg(target_os = "windows")]
    fn relay_backend_mut(&mut self) -> Option<&mut WindowsAudioDriver> {
        self.windows_audio.as_mut()
    }

    // No relay-capable backend on this target; `DemoDriver` only supplies a
    // concrete type that implements the trait so the signatures line up.
    #[cfg(not(any(all(target_os = "linux", feature = "pipewire"), target_os = "windows")))]
    fn relay_backend(&self) -> Option<&pw_graph_backend::DemoDriver> {
        None
    }

    #[cfg(not(any(all(target_os = "linux", feature = "pipewire"), target_os = "windows")))]
    fn relay_backend_mut(&mut self) -> Option<&mut pw_graph_backend::DemoDriver> {
        None
    }

    fn relay_unavailable() -> BackendError {
        Self::unsupported("audio relay is not available for this backend")
    }
}

#[cfg(feature = "relay")]
impl pw_graph_backend::RelayDriver for CompositeDriver {
    fn relay_available(&self) -> bool {
        self.relay_backend()
            .is_some_and(|driver| driver.relay_available())
    }

    fn relay_status(&self) -> pw_graph_backend::RelayEngineStatus {
        self.relay_backend()
            .map(|driver| driver.relay_status())
            .unwrap_or_default()
    }

    fn relay_devices_active(&self) -> bool {
        self.relay_backend()
            .is_some_and(|driver| driver.relay_devices_active())
    }

    fn relay_start_host(
        &mut self,
        request: pw_graph_backend::RelayHostRequest,
    ) -> BackendResult<u16> {
        let port = self
            .relay_backend_mut()
            .ok_or_else(Self::relay_unavailable)?
            .relay_start_host(request)?;
        // Starting the host can add virtual nodes to the child graph, so the
        // merged view has to catch up. A no-op where it cannot.
        self.rebuild_after_native_mutation();
        Ok(port)
    }

    fn relay_stop_host(&mut self) -> BackendResult<()> {
        self.relay_backend_mut()
            .ok_or_else(Self::relay_unavailable)?
            .relay_stop_host()
    }

    fn relay_connect(
        &mut self,
        target: std::net::SocketAddr,
        pin: &str,
        roles: pw_graph_backend::RelayRoles,
    ) -> BackendResult<()> {
        self.relay_backend_mut()
            .ok_or_else(Self::relay_unavailable)?
            .relay_connect(target, pin, roles)?;
        self.rebuild_after_native_mutation();
        Ok(())
    }

    fn relay_disconnect(&mut self, session: pw_graph_backend::RelaySessionId) -> BackendResult<()> {
        self.relay_backend_mut()
            .ok_or_else(Self::relay_unavailable)?
            .relay_disconnect(session)
    }

    fn relay_events(&mut self) -> Vec<pw_graph_backend::RelayEvent> {
        self.relay_backend_mut()
            .map(|driver| driver.relay_events())
            .unwrap_or_default()
    }

    fn relay_discovery_start(&mut self) -> BackendResult<()> {
        self.relay_backend_mut()
            .ok_or_else(Self::relay_unavailable)?
            .relay_discovery_start()
    }

    fn relay_discovery_stop(&mut self) {
        if let Some(driver) = self.relay_backend_mut() {
            driver.relay_discovery_stop();
        }
    }

    fn relay_peers(&self) -> Vec<pw_graph_backend::RelayPeerInfo> {
        self.relay_backend()
            .map(|driver| driver.relay_peers())
            .unwrap_or_default()
    }

    fn relay_local_links(&self) -> Vec<pw_graph_backend::RelayLocalLink> {
        self.relay_backend()
            .map(|driver| driver.relay_local_links())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pw_graph_backend::InMemoryDriver;
    use pw_graph_core::{encode_backend_id, BackendNamespace, Direction, Node, Port};

    /// Regression: the composite implemented `set_node_volume`/`set_node_mute`
    /// but not `node_audio_state`/`node_capabilities`, so the trait default
    /// answered `UNSUPPORTED` for every live node. The UI drives its controls
    /// off that, so every card on a real backend lost its volume and mute
    /// controls even though the driver underneath reported both.
    #[cfg(target_os = "windows")]
    #[test]
    fn composite_forwards_audio_state_to_the_owning_backend() {
        let Ok(mut driver) = pw_graph_backend::WindowsAudioDriver::new() else {
            // No Core Audio in this environment.
            return;
        };
        if driver.refresh().is_err() || driver.graph().nodes.is_empty() {
            return;
        }
        let expected: Vec<_> = driver
            .graph()
            .nodes
            .keys()
            .map(|node_id| {
                (
                    *node_id,
                    driver.node_audio_state(*node_id).ok(),
                    driver.node_capabilities(*node_id),
                )
            })
            .collect();
        assert!(
            expected
                .iter()
                .any(|(_, _, capabilities)| capabilities.has_any_control()),
            "an endpoint should report controls, or this proves nothing"
        );

        let mut composite = CompositeDriver::with_windows_audio(driver);
        composite.refresh().expect("composite refresh");

        for (node_id, state, capabilities) in expected {
            assert_eq!(
                composite.node_audio_state(node_id).ok(),
                state,
                "composite must not swallow the backend's reading"
            );
            assert_eq!(composite.node_capabilities(node_id), capabilities);
        }
    }

    /// The other half of the same failure: a merged graph must list each port
    /// on its node exactly once, or the card grows a second row and a phantom
    /// pin that captures the link belonging to the real one.
    #[test]
    fn merging_a_graph_lists_every_port_once() {
        let mut source = Graph::default();
        let node_id = NodeId(encode_backend_id(BackendNamespace::PipeWire, 7));
        let port_id = PortId(encode_backend_id(BackendNamespace::PipeWire, 8));
        source
            .add_node(Node::new(node_id, "Speakers", NodeType::PipeWire))
            .unwrap();
        source
            .add_port(Port::new(
                port_id,
                node_id,
                "audio",
                Direction::Sink,
                PortType::Audio,
            ))
            .unwrap();

        let mut merged = Graph::default();
        CompositeDriver::merge_graph(&mut merged, &source).expect("merge succeeds");

        assert_eq!(merged.nodes[&node_id].ports, vec![port_id]);
    }

    #[test]
    fn composite_reports_cross_backend_connections_before_mutation() {
        let pipewire_output = PortId(encode_backend_id(BackendNamespace::PipeWire, 42));
        let alsa_output = PortId(encode_backend_id(BackendNamespace::AlsaMidi, 42));
        let windows_output = PortId(encode_backend_id(BackendNamespace::WindowsAudio, 42));

        assert_eq!(
            route_for_ports(
                pipewire_output,
                PortId(encode_backend_id(BackendNamespace::PipeWire, 43))
            ),
            Ok(CompositeRoute::PipeWire)
        );
        assert_eq!(
            route_for_ports(
                alsa_output,
                PortId(encode_backend_id(BackendNamespace::AlsaMidi, 43))
            ),
            Ok(CompositeRoute::AlsaMidi)
        );
        assert_eq!(
            route_for_ports(
                pipewire_output,
                PortId(encode_backend_id(BackendNamespace::AlsaMidi, 43))
            ),
            Err("connections cannot cross PipeWire and ALSA MIDI backends")
        );
        assert_eq!(
            route_for_ports(
                alsa_output,
                PortId(encode_backend_id(BackendNamespace::PipeWire, 43))
            ),
            Err("connections cannot cross PipeWire and ALSA MIDI backends")
        );
        assert_eq!(
            route_for_ports(
                windows_output,
                PortId(encode_backend_id(BackendNamespace::WindowsAudio, 43))
            ),
            Ok(CompositeRoute::WindowsAudio)
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
