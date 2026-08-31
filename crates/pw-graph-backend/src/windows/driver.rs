//! The public driver.
//!
//! It owns no COM interface. It sends owned commands to the worker thread
//! and reads back owned snapshots, which is what keeps every COM pointer on
//! the single thread that initialized the apartment.

use super::*;

pub(super) const WINDOWS_AUDIO_CAPABILITIES: BackendCapabilities = BackendCapabilities {
    topology: true,
    connect: false,
    disconnect: false,
    volume: true,
    mute: true,
    meters: true,
    effects: false,
    // Kept in step with `RelayDriver::relay_available` below: the WASAPI relay
    // endpoints exist whenever the feature is compiled in.
    relay: cfg!(feature = "relay"),
};

/// Audio state shared between the COM worker, the Core Audio change callbacks,
/// and the public driver.
///
/// Volume and mute arrive on notification threads with the new values already
/// in the payload, so they are written straight in. Nothing about the graph's
/// shape changes when a fader moves, which is why these events deliberately do
/// not mark the topology dirty: a volume change used to force a full endpoint
/// and session re-enumeration.
pub(super) type AudioStateMap = Arc<Mutex<BTreeMap<NodeId, NodeAudioState>>>;

/// Public Windows audio driver. The COM worker owns all Core Audio objects;
/// this value only owns a graph snapshot, command channel, and lifecycle state.
#[derive(Debug)]
pub struct WindowsAudioDriver {
    pub(super) graph: Graph,
    /// Audio state as Core Audio last reported it, kept current by change
    /// callbacks. The backend owns these values; nothing upstream keeps a copy.
    pub(super) audio_states: AudioStateMap,
    /// Nodes Core Audio can meter: endpoints, and sessions that expose a meter.
    pub(super) meterable: BTreeSet<NodeId>,
    pub(super) positions: BTreeMap<NodeId, [f32; 2]>,
    pub(super) command_tx: Sender<WorkerCommand>,
    pub(super) dirty: Arc<AtomicBool>,
    pub(super) worker: Option<JoinHandle<()>>,
    /// Relay engine plus its WASAPI endpoints, created on first use.
    #[cfg(feature = "relay")]
    pub(super) relay: Option<crate::windows_relay::WindowsRelayDevices>,
    /// Which endpoints the relay should use next time it starts.
    #[cfg(feature = "relay")]
    pub(super) relay_endpoints: crate::windows_relay::RelayEndpoints,
    /// Playback endpoints the relay can be pointed at, refreshed with the graph.
    #[cfg(feature = "relay")]
    pub(super) relay_endpoint_choices: Vec<(String, String)>,
}

impl WindowsAudioDriver {
    pub fn new() -> BackendResult<Self> {
        let (command_tx, command_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let dirty = Arc::new(AtomicBool::new(true));
        let topology_dirty = Arc::new(AtomicBool::new(true));
        let session_dirty_endpoints = Arc::new(Mutex::new(BTreeSet::new()));
        let worker_dirty = Arc::clone(&dirty);
        let worker_topology_dirty = Arc::clone(&topology_dirty);
        let worker_session_dirty_endpoints = Arc::clone(&session_dirty_endpoints);
        let audio_states: AudioStateMap = Arc::new(Mutex::new(BTreeMap::new()));
        let worker_states = Arc::clone(&audio_states);
        let worker = thread::Builder::new()
            .name("qpwgraph-windows-audio".into())
            .spawn(move || {
                worker_thread(
                    command_rx,
                    ready_tx,
                    worker_dirty,
                    worker_topology_dirty,
                    worker_session_dirty_endpoints,
                    worker_states,
                )
            })
            .map_err(|error| {
                BackendError::Native(format!("could not start audio worker: {error}"))
            })?;

        let snapshot = match ready_rx.recv() {
            Ok(Ok(snapshot)) => snapshot,
            Ok(Err(error)) => {
                let _ = worker.join();
                return Err(error);
            }
            Err(_) => {
                let _ = worker.join();
                return Err(BackendError::Native(
                    "Windows audio worker exited during startup".into(),
                ));
            }
        };

        Ok(Self {
            graph: snapshot.graph,
            audio_states,
            meterable: snapshot.meterable,
            positions: BTreeMap::new(),
            command_tx,
            dirty,
            worker: Some(worker),
            #[cfg(feature = "relay")]
            relay: None,
            #[cfg(feature = "relay")]
            relay_endpoints: Default::default(),
            #[cfg(feature = "relay")]
            relay_endpoint_choices: snapshot.playback_endpoints,
        })
    }

    /// Create the relay engine and its WASAPI endpoints on first use.
    ///
    /// A WASAPI client is bound to the device it was opened on, so changing
    /// the selected endpoints tears the devices down and starts them again.
    #[cfg(feature = "relay")]
    pub(super) fn ensure_relay(
        &mut self,
        config: pw_graph_relay::EngineConfig,
    ) -> BackendResult<&crate::windows_relay::WindowsRelayDevices> {
        let wanted = self.relay_endpoints.clone();
        let restart = self
            .relay
            .as_ref()
            .is_some_and(|devices| devices.endpoints() != &wanted);
        if restart {
            self.relay = None;
        }
        match self.relay.as_ref() {
            Some(devices) => {
                devices.handle().update_config(config);
            }
            None => {
                self.relay = Some(crate::windows_relay::WindowsRelayDevices::start(
                    config, wanted,
                )?);
            }
        }
        Ok(self.relay.as_ref().expect("relay was just created"))
    }

    /// Choose which endpoints the relay taps and plays on.
    ///
    /// Ids are Core Audio device ids, the same ones the endpoint nodes are
    /// built from, so the UI can offer the cards it already draws. `None`
    /// tracks the default playback endpoint. Takes effect on the next relay
    /// start; if the relay is already running it is restarted.
    #[cfg(feature = "relay")]
    pub fn set_relay_endpoints(
        &mut self,
        endpoints: crate::windows_relay::RelayEndpoints,
    ) -> BackendResult<()> {
        if self.relay_endpoints == endpoints {
            return Ok(());
        }
        self.relay_endpoints = endpoints;
        // Only restart something that is already running; otherwise the choice
        // simply applies when the relay is next started.
        let Some(devices) = self.relay.as_ref() else {
            return Ok(());
        };
        let mut config = devices.handle().config();
        let status = devices.handle().status();
        // A WASAPI client cannot be moved between devices, so the endpoints are
        // torn down and rebuilt. Keep hosting across that: switching which
        // speakers are relayed must not silently drop the peers' connection
        // point, and reusing the port keeps an already-shared address valid.
        if let Some(port) = status.host_port {
            config.port = port;
        }
        // Stop the listener before dropping, so the control port is released
        // rather than lingering while the new engine tries to bind it.
        if status.host_active {
            let _ = devices.handle().host_stop();
        }
        self.relay = None;

        let devices = self.ensure_relay(config.clone())?;
        if !status.host_active {
            return Ok(());
        }
        match devices.handle().host_start() {
            Ok(_) => Ok(()),
            Err(error) => Err(BackendError::native(format!(
                "relay host restart failed: {error}"
            ))),
        }
    }

    /// Endpoints the relay is configured to use.
    #[cfg(feature = "relay")]
    pub fn relay_endpoints(&self) -> &crate::windows_relay::RelayEndpoints {
        &self.relay_endpoints
    }

    /// Playback endpoints the relay can be pointed at, as `(id, name)`.
    #[cfg(feature = "relay")]
    pub fn relay_endpoint_choices(&self) -> Vec<(String, String)> {
        self.relay_endpoint_choices.clone()
    }

    /// The relay's format, fixed by the WASAPI endpoints that carry it.
    #[cfg(feature = "relay")]
    pub(super) fn relay_config(
        device_name: String,
        pin: String,
        port: u16,
        codec: super::api::RelayCodecKind,
        frame_ms: u16,
        transport: super::api::RelayTransportPreference,
        roles: super::api::RelayRoles,
        device_id: String,
        trusted_peers: Vec<super::api::RelayTrustedPeer>,
        trust_new_peers: bool,
    ) -> pw_graph_relay::EngineConfig {
        pw_graph_relay::EngineConfig {
            device_id,
            device_name,
            device_kind: super::api::RelayDeviceKind::Other,
            pin,
            port,
            codec,
            frame_ms,
            sample_rate: crate::windows_relay::RELAY_SAMPLE_RATE,
            channels: crate::windows_relay::RELAY_CHANNELS,
            client_roles: roles,
            transport,
            trusted_peers,
            trust_new_peers,
            // The WASAPI relay endpoints run 48 kHz stereo, so that is this
            // machine's local geometry; sessions negotiating anything else are
            // converted rather than misinterpreted.
            local_sample_rate: crate::windows_relay::RELAY_SAMPLE_RATE,
            local_channels: crate::windows_relay::RELAY_CHANNELS,
            ..pw_graph_relay::EngineConfig::default()
        }
    }

    pub(super) fn response<T>(receiver: Receiver<BackendResult<T>>) -> BackendResult<T> {
        receiver
            .recv()
            .map_err(|_| BackendError::Native("Windows audio worker stopped responding".into()))?
    }

    pub(super) fn refresh_snapshot(&mut self, only_if_needed: bool) -> BackendResult<()> {
        let (sender, receiver) = mpsc::channel();
        self.command_tx
            .send(if only_if_needed {
                WorkerCommand::RefreshIfNeeded(sender)
            } else {
                WorkerCommand::Refresh(sender)
            })
            .map_err(|_| BackendError::Native("Windows audio worker is unavailable".into()))?;
        let snapshot = Self::response(receiver)?;
        let mut graph = snapshot.graph;
        for (node_id, position) in &self.positions {
            if let Some(node) = graph.nodes.get_mut(node_id) {
                node.position = *position;
            }
        }
        self.graph = graph;
        self.meterable = snapshot.meterable;
        #[cfg(feature = "relay")]
        {
            self.relay_endpoint_choices = snapshot.playback_endpoints;
        }
        Ok(())
    }
}

impl Drop for WindowsAudioDriver {
    fn drop(&mut self) {
        let _ = self.command_tx.send(WorkerCommand::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl GraphDriver for WindowsAudioDriver {
    fn capabilities(&self) -> BackendCapabilities {
        WINDOWS_AUDIO_CAPABILITIES
    }

    fn refresh(&mut self) -> BackendResult<Vec<Node>> {
        self.refresh_snapshot(false)?;
        Ok(self.graph.nodes.values().cloned().collect())
    }

    fn refresh_if_needed(&mut self) -> BackendResult<Vec<Node>> {
        self.refresh_snapshot(true)?;
        Ok(self.graph.nodes.values().cloned().collect())
    }

    fn connect(&mut self, _src: PortId, _dst: PortId) -> BackendResult<Link> {
        Err(BackendError::Unsupported(
            "arbitrary Windows audio routing is not supported".into(),
        ))
    }

    fn disconnect(&mut self, _link: LinkId) -> BackendResult<Link> {
        Err(BackendError::Unsupported(
            "arbitrary Windows audio routing is not supported".into(),
        ))
    }

    fn is_link_mutable(&self, _link: LinkId) -> bool {
        false
    }

    fn set_node_position(&mut self, node: NodeId, position: [f32; 2]) -> BackendResult<()> {
        self.graph
            .nodes
            .get_mut(&node)
            .ok_or(GraphError::MissingNode(node))?
            .position = position;
        self.positions.insert(node, position);
        Ok(())
    }

    /// Core Audio state as of the last refresh. Reads are served from that
    /// snapshot rather than re-entering COM, so the UI can ask per node per
    /// frame without a round trip to the worker thread.
    fn node_audio_state(&self, node: NodeId) -> BackendResult<NodeAudioState> {
        if !self.graph.nodes.contains_key(&node) {
            return Err(GraphError::MissingNode(node).into());
        }
        Ok(self
            .audio_states
            .lock()
            .ok()
            .and_then(|states| states.get(&node).copied())
            .unwrap_or(NodeAudioState::UNSUPPORTED))
    }

    /// A node only reports meter capability when something actually answered
    /// the meter query for it, so no card is given a meter it cannot fill.
    fn node_capabilities(&self, node: NodeId) -> NodeCapabilities {
        let Ok(state) = self.node_audio_state(node) else {
            return NodeCapabilities::NONE;
        };
        let mut capabilities = state.control_capabilities();
        if self.meterable.contains(&node) {
            // `IAudioMeterInformation` is a *peak* meter, on endpoints and on
            // sessions alike. It has no RMS reading, and `audio_meters` reports
            // rms: 0.0 accordingly, so claiming RMS here would make the UI draw
            // a permanently silent RMS bar next to a working peak one.
            capabilities.meter_peak = true;
            capabilities.meter_rms = false;
        }
        capabilities
    }

    fn set_node_mute(&mut self, node: NodeId, muted: bool) -> BackendResult<()> {
        let (sender, receiver) = mpsc::channel();
        self.command_tx
            .send(WorkerCommand::SetMute(node, muted, sender))
            .map_err(|_| BackendError::Native("Windows audio worker is unavailable".into()))?;
        Self::response(receiver)?;
        // Reflect the write straight away so the card does not flick back to
        // the previous value while waiting for the change callback.
        if let Ok(mut states) = self.audio_states.lock() {
            if let Some(state) = states.get_mut(&node) {
                state.muted = Some(muted);
                state.mute_readable = true;
            }
        }
        Ok(())
    }

    fn set_node_volume(&mut self, node: NodeId, volume: f32) -> BackendResult<()> {
        let (sender, receiver) = mpsc::channel();
        self.command_tx
            .send(WorkerCommand::SetVolume(node, volume, sender))
            .map_err(|_| BackendError::Native("Windows audio worker is unavailable".into()))?;
        Self::response(receiver)?;
        // The worker clamps to the endpoint's 0..=1 range, so record what
        // Windows will actually hold rather than what was asked for.
        if let Ok(mut states) = self.audio_states.lock() {
            if let Some(state) = states.get_mut(&node) {
                state.volume = Some(volume.clamp(0.0, 1.0));
                state.volume_readable = true;
            }
        }
        Ok(())
    }

    fn graph(&self) -> &Graph {
        &self.graph
    }

    fn graph_dirty(&self) -> bool {
        self.dirty.load(Ordering::Acquire)
    }

    /// Device and session notification callbacks set the dirty flag for every
    /// topology change, so the application does not have to poll for them.
    fn reports_graph_changes(&self) -> bool {
        true
    }

    fn is_node_type(&self, node_type: NodeType) -> bool {
        matches!(
            node_type,
            NodeType::WindowsAudioEndpoint | NodeType::WindowsAudioSession
        )
    }

    fn is_port_type(&self, port_type: PortType) -> bool {
        matches!(port_type, PortType::Audio)
    }

    fn audio_meters(&mut self) -> BackendResult<Vec<AudioMeter>> {
        let (sender, receiver) = mpsc::channel();
        self.command_tx
            .send(WorkerCommand::AudioMeters(sender))
            .map_err(|_| BackendError::Native("Windows audio worker is unavailable".into()))?;
        Self::response(receiver)
    }

    fn set_meter_policy(&mut self, policy: MeterPolicy) -> BackendResult<()> {
        let (sender, receiver) = mpsc::channel();
        self.command_tx
            .send(WorkerCommand::SetMeterPolicy(policy, sender))
            .map_err(|_| BackendError::Native("Windows audio worker is unavailable".into()))?;
        Self::response(receiver)
    }

    fn request_meters(&mut self, nodes: &BTreeSet<NodeId>) -> BackendResult<()> {
        let (sender, receiver) = mpsc::channel();
        self.command_tx
            .send(WorkerCommand::RequestMeters(nodes.clone(), sender))
            .map_err(|_| BackendError::Native("Windows audio worker is unavailable".into()))?;
        Self::response(receiver)
    }

    fn reset_audio_config(&mut self) -> BackendResult<()> {
        let (sender, receiver) = mpsc::channel();
        self.command_tx
            .send(WorkerCommand::ResetAudio(sender))
            .map_err(|_| BackendError::Native("Windows audio worker is unavailable".into()))?;
        Self::response(receiver)
    }
}

impl crate::api::EffectDriver for WindowsAudioDriver {}

/// Relay support on Windows.
///
/// The engine is the same one PipeWire uses; only the audio endpoints differ.
/// See `windows_relay` for why the microphone role cannot be offered here.
#[cfg(feature = "relay")]
impl super::api::RelayDriver for WindowsAudioDriver {
    fn relay_available(&self) -> bool {
        true
    }

    fn relay_status(&self) -> super::api::RelayEngineStatus {
        self.relay
            .as_ref()
            .map(|devices| devices.handle().status())
            .unwrap_or_default()
    }

    fn relay_devices_active(&self) -> bool {
        self.relay.is_some()
    }

    fn relay_start_host(&mut self, request: super::api::RelayHostRequest) -> BackendResult<u16> {
        let config = Self::relay_config(
            request.device_name,
            request.pin,
            request.port,
            request.codec,
            request.frame_ms,
            request.transport,
            // A host serves whatever a peer asks for; the client's own roles
            // only matter when this machine is the one connecting out.
            super::api::RelayRoles::both(),
            request.device_id,
            request.trusted_peers,
            request.trust_new_peers,
        );
        let devices = self.ensure_relay(config)?;
        devices
            .handle()
            .host_start()
            .map_err(|error| BackendError::native(format!("relay host start failed: {error}")))
    }

    fn relay_stop_host(&mut self) -> BackendResult<()> {
        if let Some(devices) = self.relay.as_mut() {
            devices.handle().host_stop().map_err(|error| {
                BackendError::native(format!("relay host stop failed: {error}"))
            })?;
        }
        Ok(())
    }

    fn relay_connect(
        &mut self,
        target: std::net::SocketAddr,
        pin: &str,
        roles: super::api::RelayRoles,
    ) -> BackendResult<super::api::RelaySessionId> {
        // Both roles work here. `emit` means "send what this machine's relay
        // capture endpoint supplies", which on Windows is the playback
        // loopback, and `receive` means "play what the peer sends", which is
        // the render stream. What Windows cannot do is expose the received
        // audio to *other applications* as a microphone -- a routing limit, not
        // a role limit -- so neither role is refused here.
        if self.relay.is_none() {
            let config = Self::relay_config(
                "qpwgraph-rs".into(),
                pin.to_owned(),
                0,
                super::api::RelayCodecKind::Opus,
                10,
                super::api::RelayTransportPreference::Auto,
                roles,
                pw_graph_relay::generate_device_id(),
                Vec::new(),
                true,
            );
            self.ensure_relay(config)?;
        }
        let devices = self.relay.as_ref().expect("relay was just created");
        Ok(devices.handle().connect(target, pin, roles))
    }

    fn relay_connect_trusted(
        &mut self,
        target: std::net::SocketAddr,
        peer_id: &str,
        secret: [u8; 32],
        roles: super::api::RelayRoles,
    ) -> BackendResult<super::api::RelaySessionId> {
        if self.relay.is_none() {
            let config = Self::relay_config(
                "qpwgraph-rs".into(),
                String::new(),
                0,
                super::api::RelayCodecKind::Opus,
                10,
                super::api::RelayTransportPreference::Auto,
                roles,
                pw_graph_relay::generate_device_id(),
                Vec::new(),
                false,
            );
            self.ensure_relay(config)?;
        }
        let devices = self.relay.as_ref().expect("relay was just created");
        Ok(devices
            .handle()
            .connect_trusted(target, peer_id, secret, roles))
    }

    fn relay_configure_identity(
        &mut self,
        device_id: String,
        trusted_peers: Vec<super::api::RelayTrustedPeer>,
        transport: super::api::RelayTransportPreference,
    ) -> BackendResult<()> {
        if let Some(devices) = self.relay.as_ref() {
            let mut config = devices.handle().config();
            config.device_id = device_id;
            config.trusted_peers = trusted_peers;
            config.transport = transport;
            devices.handle().update_config(config);
        } else {
            let config = Self::relay_config(
                "qpwgraph-rs".into(),
                String::new(),
                0,
                super::api::RelayCodecKind::Opus,
                10,
                transport,
                super::api::RelayRoles::both(),
                device_id,
                trusted_peers,
                true,
            );
            let _ = self.ensure_relay(config)?;
        }
        Ok(())
    }

    fn relay_disconnect(&mut self, session: super::api::RelaySessionId) -> BackendResult<()> {
        let Some(devices) = self.relay.as_mut() else {
            return Err(BackendError::native(
                "no relay session exists to disconnect",
            ));
        };
        devices
            .handle()
            .disconnect(session)
            .map_err(|error| BackendError::native(format!("relay disconnect failed: {error}")))
    }

    fn relay_trusted_enrollment_secret(
        &self,
        transaction_id: u64,
    ) -> BackendResult<Option<[u8; 32]>> {
        Ok(self
            .relay
            .as_ref()
            .and_then(|devices| devices.handle().trusted_enrollment_secret(transaction_id)))
    }

    fn relay_accept_trusted_enrollment(&mut self, transaction_id: u64) -> BackendResult<()> {
        let Some(devices) = self.relay.as_ref() else {
            return Err(BackendError::native("no relay host is running"));
        };
        devices
            .handle()
            .accept_trusted_enrollment(transaction_id)
            .map_err(|error| {
                BackendError::native(format!("trusted enrollment commit failed: {error}"))
            })
    }

    fn relay_reject_trusted_enrollment(
        &mut self,
        transaction_id: u64,
        reason: &str,
    ) -> BackendResult<()> {
        let Some(devices) = self.relay.as_ref() else {
            return Err(BackendError::native("no relay host is running"));
        };
        devices
            .handle()
            .reject_trusted_enrollment(transaction_id, reason)
            .map_err(|error| {
                BackendError::native(format!("trusted enrollment rejection failed: {error}"))
            })
    }

    fn relay_remove_trusted_peer(&mut self, peer_id: &str) -> BackendResult<()> {
        let Some(devices) = self.relay.as_ref() else {
            return Err(BackendError::native("no relay engine is running"));
        };
        devices
            .handle()
            .remove_trusted_peer(peer_id)
            .map_err(|error| BackendError::native(format!("trusted peer removal failed: {error}")))
    }

    fn relay_events(&mut self) -> Vec<super::api::RelayEvent> {
        self.relay
            .as_mut()
            .map(|devices| devices.handle().events())
            .unwrap_or_default()
    }

    fn relay_discovery_start(&mut self) -> BackendResult<()> {
        if self.relay.is_none() {
            let config = Self::relay_config(
                "qpwgraph-rs".into(),
                String::new(),
                0,
                super::api::RelayCodecKind::Opus,
                10,
                super::api::RelayTransportPreference::Auto,
                super::api::RelayRoles::both(),
                pw_graph_relay::generate_device_id(),
                Vec::new(),
                true,
            );
            self.ensure_relay(config)?;
        }
        let devices = self.relay.as_ref().expect("relay was just created");
        devices
            .handle()
            .discovery_start()
            .map_err(|error| BackendError::native(format!("relay discovery failed: {error}")))
    }

    fn relay_discovery_stop(&mut self) {
        if let Some(devices) = self.relay.as_ref() {
            devices.handle().discovery_stop();
        }
    }

    fn relay_discovery_usb_link_lost(&mut self) {
        if let Some(devices) = self.relay.as_ref() {
            devices.handle().discovery_usb_link_lost();
        }
    }

    fn relay_usb_link_present(&self) -> bool {
        pw_graph_relay::netlink::local_links()
            .iter()
            .any(|link| link.kind == pw_graph_relay::LinkKind::Usb)
    }

    fn relay_peers(&self) -> Vec<super::api::RelayPeerInfo> {
        self.relay
            .as_ref()
            .map(|devices| devices.handle().discovered_peers())
            .unwrap_or_default()
    }

    fn relay_local_links(&self) -> Vec<super::api::RelayLocalLink> {
        pw_graph_relay::netlink::display_links()
    }
}
