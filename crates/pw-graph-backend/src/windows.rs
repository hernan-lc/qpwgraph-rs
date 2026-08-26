//! Windows Core Audio backend.
//!
//! Core Audio exposes endpoint and application-session state, but it does not
//! expose PipeWire's arbitrary patchbay graph. This driver therefore presents
//! the relationships Windows reports as an observed graph and deliberately
//! rejects topology mutations. All COM interfaces stay on the worker thread;
//! the public driver communicates with that thread through owned commands and
//! snapshots.

use super::api::{
    AudioMeter, BackendCapabilities, BackendError, BackendResult, GraphDriver, MeterPolicy,
    NodeAudioState, NodeCapabilities,
};
use pw_graph_core::{
    encode_backend_id, BackendNamespace, Direction, Graph, GraphError, Link, LinkId, Node, NodeId,
    NodeType, Port, PortId, PortType, LOCAL_ID_MASK,
};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use windows::core::{Interface, GUID, PCWSTR, PWSTR};
use windows::Win32::Devices::Properties;
use windows::Win32::Foundation::{CloseHandle, PROPERTYKEY};
use windows::Win32::Media::Audio;
use windows::Win32::Media::Audio::Endpoints::{IAudioEndpointVolume, IAudioMeterInformation};
use windows::Win32::System::Com::{
    self, StructuredStorage, CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ,
};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::System::Variant::VT_LPWSTR;
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
use windows_core::BOOL;

const WINDOWS_AUDIO_CAPABILITIES: BackendCapabilities = BackendCapabilities {
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
type AudioStateMap = Arc<Mutex<BTreeMap<NodeId, NodeAudioState>>>;

/// One refresh answer. Audio state is not carried here -- it lives in the
/// shared map, which the refresh fills and the callbacks keep current.
struct WorkerSnapshot {
    graph: Graph,
    /// Nodes that can report a level: endpoints, and sessions whose control
    /// answered the meter query.
    meterable: BTreeSet<NodeId>,
    /// Render endpoints as `(device id, display name)`, for relay selection.
    playback_endpoints: Vec<(String, String)>,
}

#[derive(Debug)]
enum WorkerCommand {
    Refresh(Sender<BackendResult<WorkerSnapshot>>),
    SetVolume(NodeId, f32, Sender<BackendResult<()>>),
    SetMute(NodeId, bool, Sender<BackendResult<()>>),
    SetMeterPolicy(MeterPolicy, Sender<BackendResult<()>>),
    RequestMeters(BTreeSet<NodeId>, Sender<BackendResult<()>>),
    AudioMeters(Sender<BackendResult<Vec<AudioMeter>>>),
    ResetAudio(Sender<BackendResult<()>>),
    Shutdown,
}

/// Public Windows audio driver. The COM worker owns all Core Audio objects;
/// this value only owns a graph snapshot, command channel, and lifecycle state.
#[derive(Debug)]
pub struct WindowsAudioDriver {
    graph: Graph,
    /// Audio state as Core Audio last reported it, kept current by change
    /// callbacks. The backend owns these values; nothing upstream keeps a copy.
    audio_states: AudioStateMap,
    /// Nodes Core Audio can meter: endpoints, and sessions that expose a meter.
    meterable: BTreeSet<NodeId>,
    positions: BTreeMap<NodeId, [f32; 2]>,
    command_tx: Sender<WorkerCommand>,
    dirty: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    /// Relay engine plus its WASAPI endpoints, created on first use.
    #[cfg(feature = "relay")]
    relay: Option<crate::windows_relay::WindowsRelayDevices>,
    /// Which endpoints the relay should use next time it starts.
    #[cfg(feature = "relay")]
    relay_endpoints: crate::windows_relay::RelayEndpoints,
    /// Playback endpoints the relay can be pointed at, refreshed with the graph.
    #[cfg(feature = "relay")]
    relay_endpoint_choices: Vec<(String, String)>,
}

impl WindowsAudioDriver {
    pub fn new() -> BackendResult<Self> {
        let (command_tx, command_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let dirty = Arc::new(AtomicBool::new(true));
        let worker_dirty = Arc::clone(&dirty);
        let audio_states: AudioStateMap = Arc::new(Mutex::new(BTreeMap::new()));
        let worker_states = Arc::clone(&audio_states);
        let worker = thread::Builder::new()
            .name("qpwgraph-windows-audio".into())
            .spawn(move || worker_thread(command_rx, ready_tx, worker_dirty, worker_states))
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
    fn ensure_relay(
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
            Err(_) if config.port != 0 => {
                // The old socket has not been released yet. Keeping the host
                // running matters more than keeping its port, so fall back to
                // a fresh ephemeral one; callers read the port from status.
                config.port = 0;
                devices.handle().update_config(config);
                devices.handle().host_start().map(|_| ()).map_err(|error| {
                    BackendError::native(format!("relay host restart failed: {error}"))
                })
            }
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
    fn relay_config(
        device_name: String,
        pin: String,
        port: u16,
        codec: super::api::RelayCodecKind,
        frame_ms: u16,
        transport: super::api::RelayTransportPreference,
        roles: super::api::RelayRoles,
    ) -> pw_graph_relay::EngineConfig {
        pw_graph_relay::EngineConfig {
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
        }
    }

    fn response<T>(receiver: Receiver<BackendResult<T>>) -> BackendResult<T> {
        receiver
            .recv()
            .map_err(|_| BackendError::Native("Windows audio worker stopped responding".into()))?
    }

    fn refresh_snapshot(&mut self) -> BackendResult<()> {
        let (sender, receiver) = mpsc::channel();
        self.command_tx
            .send(WorkerCommand::Refresh(sender))
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
        self.refresh_snapshot()?;
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

impl super::api::EffectDriver for WindowsAudioDriver {}

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
    ) -> BackendResult<()> {
        // Both roles work here. `emit` means "send what this machine's relay
        // capture endpoint supplies", which on Windows is the playback
        // loopback, and `receive` means "play what the peer sends", which is
        // the render stream. What Windows cannot do is expose the received
        // audio to *other applications* as a microphone -- a routing limit, not
        // a role limit -- so neither role is refused here.
        let device_name = self
            .relay
            .as_ref()
            .map(|devices| devices.handle().config().device_name)
            .unwrap_or_else(|| "qpwgraph-rs".into());
        let config = Self::relay_config(
            device_name,
            pin.to_owned(),
            0,
            super::api::RelayCodecKind::Opus,
            10,
            super::api::RelayTransportPreference::Auto,
            roles,
        );
        let devices = self.ensure_relay(config)?;
        devices.handle().connect(target, pin, roles);
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

    fn relay_events(&mut self) -> Vec<super::api::RelayEvent> {
        self.relay
            .as_mut()
            .map(|devices| devices.handle().events())
            .unwrap_or_default()
    }

    fn relay_discovery_start(&mut self) -> BackendResult<()> {
        let config = Self::relay_config(
            "qpwgraph-rs".into(),
            String::new(),
            0,
            super::api::RelayCodecKind::Opus,
            10,
            super::api::RelayTransportPreference::Auto,
            super::api::RelayRoles::both(),
        );
        let devices = self.ensure_relay(config)?;
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

fn worker_thread(
    command_rx: Receiver<WorkerCommand>,
    ready_tx: Sender<BackendResult<WorkerSnapshot>>,
    dirty: Arc<AtomicBool>,
    audio_states: AudioStateMap,
) {
    let initialized = unsafe { Com::CoInitializeEx(None, COINIT_MULTITHREADED) };
    if initialized.is_err() {
        let _ = ready_tx.send(Err(BackendError::Native(format!(
            "could not initialize Windows COM: {initialized:?}"
        ))));
        return;
    }

    let worker = CoreAudioWorker::new(Arc::clone(&dirty), audio_states);
    let mut worker = match worker {
        Ok(worker) => worker,
        Err(error) => {
            let _ = ready_tx.send(Err(error));
            unsafe { Com::CoUninitialize() };
            return;
        }
    };

    match worker.refresh_graph() {
        Ok(snapshot) => {
            let _ = ready_tx.send(Ok(snapshot));
        }
        Err(error) => {
            let _ = ready_tx.send(Err(error));
            unsafe { Com::CoUninitialize() };
            return;
        }
    }

    while let Ok(command) = command_rx.recv() {
        match command {
            WorkerCommand::Refresh(sender) => {
                let _ = sender.send(worker.refresh_graph());
            }
            WorkerCommand::SetVolume(node, volume, sender) => {
                let _ = sender.send(worker.set_volume(node, volume));
            }
            WorkerCommand::SetMute(node, muted, sender) => {
                let _ = sender.send(worker.set_mute(node, muted));
            }
            WorkerCommand::SetMeterPolicy(policy, sender) => {
                worker.meter_policy = policy;
                let _ = sender.send(Ok(()));
            }
            WorkerCommand::RequestMeters(nodes, sender) => {
                worker.requested_meters = nodes;
                let _ = sender.send(Ok(()));
            }
            WorkerCommand::AudioMeters(sender) => {
                let _ = sender.send(worker.audio_meters());
            }
            WorkerCommand::ResetAudio(sender) => {
                worker.requested_meters.clear();
                let _ = sender.send(Ok(()));
            }
            WorkerCommand::Shutdown => break,
        }
    }

    drop(worker);
    unsafe { Com::CoUninitialize() };
}

struct EndpointRecord {
    id: String,
    flow: Audio::EDataFlow,
    device: Audio::IMMDevice,
    node_id: NodeId,
    port_id: PortId,
}

struct SessionRecord {
    endpoint_id: String,
    session_id: String,
    flow: Audio::EDataFlow,
    node_id: NodeId,
    /// The session's own peak meter, kept so a level can be read without
    /// re-enumerating the endpoint every frame.
    ///
    /// `IAudioMeterInformation` is documented as an endpoint facility, but a
    /// session control implements it too, and that is the only per-application
    /// level Windows offers short of process loopback capture -- which needs
    /// build 20348. Verified against a played tone: a 0.4 amplitude sine reads
    /// back as 0.39999998 on the owning session and 0.0 on every other.
    meter: Option<IAudioMeterInformation>,
}

struct CoreAudioWorker {
    enumerator: Audio::IMMDeviceEnumerator,
    endpoint_notification: Audio::IMMNotificationClient,
    dirty: Arc<AtomicBool>,
    session_notifications: Vec<(
        Audio::IAudioSessionManager2,
        Audio::IAudioSessionNotification,
    )>,
    session_events: Vec<(Audio::IAudioSessionControl, Audio::IAudioSessionEvents)>,
    endpoints: Vec<EndpointRecord>,
    sessions: Vec<SessionRecord>,
    meter_policy: MeterPolicy,
    requested_meters: BTreeSet<NodeId>,
    /// Shared with the public driver and every change callback.
    audio_states: AudioStateMap,
    /// Endpoint volume callbacks, kept registered for the endpoint's lifetime.
    endpoint_volume_events: Vec<(
        IAudioEndpointVolume,
        Audio::Endpoints::IAudioEndpointVolumeCallback,
    )>,
}

impl CoreAudioWorker {
    fn new(dirty: Arc<AtomicBool>, audio_states: AudioStateMap) -> BackendResult<Self> {
        let enumerator: Audio::IMMDeviceEnumerator =
            unsafe { Com::CoCreateInstance(&Audio::MMDeviceEnumerator, None, CLSCTX_ALL) }
                .map_err(|error| native_error("create MMDeviceEnumerator", error))?;
        let endpoint_notification: Audio::IMMNotificationClient = EndpointNotificationClient {
            dirty: Arc::clone(&dirty),
        }
        .into();
        unsafe {
            enumerator
                .RegisterEndpointNotificationCallback(&endpoint_notification)
                .map_err(|error| native_error("register endpoint notifications", error))?;
        }
        Ok(Self {
            enumerator,
            endpoint_notification,
            dirty,
            session_notifications: Vec::new(),
            session_events: Vec::new(),
            audio_states,
            endpoint_volume_events: Vec::new(),
            endpoints: Vec::new(),
            sessions: Vec::new(),
            meter_policy: MeterPolicy::OnDemand,
            requested_meters: BTreeSet::new(),
        })
    }

    fn refresh_graph(&mut self) -> BackendResult<WorkerSnapshot> {
        self.clear_session_callbacks();
        let mut endpoint_specs = Vec::new();
        for flow in [Audio::eRender, Audio::eCapture] {
            endpoint_specs.extend(self.enumerate_endpoints(flow)?);
        }
        endpoint_specs.sort_by(|left, right| left.0.cmp(&right.0));

        let mut graph = Graph::default();
        let mut endpoints = Vec::with_capacity(endpoint_specs.len());
        let mut sessions = Vec::new();

        for (endpoint_id, flow, device) in endpoint_specs {
            let node_id = NodeId(graph_id(endpoint_node_local_id(&endpoint_id)));
            let port_id = PortId(graph_id(endpoint_port_local_id(&endpoint_id)));
            let direction = endpoint_direction(flow);
            let name = endpoint_name(&device).unwrap_or_else(|| {
                format!(
                    "Windows {} endpoint",
                    if flow == Audio::eRender {
                        "playback"
                    } else {
                        "capture"
                    }
                )
            });
            graph.add_node(
                Node::new(node_id, name, NodeType::WindowsAudioEndpoint)
                    .with_serial(stable_local_id(&format!("endpoint:{endpoint_id}"))),
            )?;
            graph.add_port(Port::new(
                port_id,
                node_id,
                "audio",
                direction,
                PortType::Audio,
            ))?;

            let endpoint = EndpointRecord {
                id: endpoint_id,
                flow,
                device,
                node_id,
                port_id,
            };
            sessions.extend(self.add_sessions(&endpoint, &mut graph)?);
            endpoints.push(endpoint);
        }

        for (node_id, position) in graph.default_node_positions() {
            if let Some(node) = graph.nodes.get_mut(&node_id) {
                node.position = position;
            }
        }
        self.endpoints = endpoints;
        self.sessions = sessions;
        self.dirty.store(false, Ordering::Release);
        let states = self.read_audio_states();
        if let Ok(mut shared) = self.audio_states.lock() {
            *shared = states;
        }
        self.register_endpoint_volume_callbacks();
        let meterable = self.meterable_nodes();
        let playback_endpoints = self
            .endpoints
            .iter()
            .filter(|endpoint| endpoint.flow == Audio::eRender)
            .map(|endpoint| {
                let name = graph
                    .nodes
                    .get(&endpoint.node_id)
                    .map(|node| node.name.clone())
                    .unwrap_or_else(|| endpoint.id.clone());
                (endpoint.id.clone(), name)
            })
            .collect();
        Ok(WorkerSnapshot {
            graph,
            meterable,
            playback_endpoints,
        })
    }

    /// Subscribe to endpoint volume/mute changes so the hardware keys and the
    /// system mixer are reflected without polling or a topology rebuild.
    fn register_endpoint_volume_callbacks(&mut self) {
        self.clear_endpoint_volume_callbacks();
        for endpoint in &self.endpoints {
            let Ok(control) = (unsafe {
                endpoint
                    .device
                    .Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None)
            }) else {
                continue;
            };
            let callback: Audio::Endpoints::IAudioEndpointVolumeCallback = EndpointVolumeCallback {
                node_id: endpoint.node_id,
                states: Arc::clone(&self.audio_states),
            }
            .into();
            if unsafe { control.RegisterControlChangeNotify(&callback) }.is_ok() {
                self.endpoint_volume_events.push((control, callback));
            }
        }
    }

    fn clear_endpoint_volume_callbacks(&mut self) {
        for (control, callback) in self.endpoint_volume_events.drain(..) {
            let _ = unsafe { control.UnregisterControlChangeNotify(&callback) };
        }
    }

    /// Read volume and mute for every endpoint and session Core Audio knows
    /// about. A node whose control cannot be activated right now is reported as
    /// unreadable rather than dropped, so the card still renders and simply
    /// shows no value.
    fn read_audio_states(&self) -> BTreeMap<NodeId, NodeAudioState> {
        let mut states = BTreeMap::new();
        for endpoint in &self.endpoints {
            states.insert(endpoint.node_id, self.endpoint_audio_state(endpoint));
        }
        for session in &self.sessions {
            states.insert(session.node_id, self.session_audio_state(session));
        }
        states
    }

    fn endpoint_audio_state(&self, endpoint: &EndpointRecord) -> NodeAudioState {
        let Ok(control) = (unsafe {
            endpoint
                .device
                .Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None)
        }) else {
            return NodeAudioState::UNSUPPORTED;
        };
        // Endpoint volume is a 0..=1 scalar; Windows has no boost range here,
        // which is why volume never exceeds unity on this backend.
        let volume = unsafe { control.GetMasterVolumeLevelScalar() }.ok();
        let muted = unsafe { control.GetMute() }
            .ok()
            .map(|muted| muted.as_bool());
        NodeAudioState {
            volume,
            muted,
            volume_readable: volume.is_some(),
            volume_writable: true,
            mute_readable: muted.is_some(),
            mute_writable: true,
        }
    }

    fn session_audio_state(&self, session: &SessionRecord) -> NodeAudioState {
        let Some(endpoint) = self
            .endpoints
            .iter()
            .find(|endpoint| endpoint.id == session.endpoint_id && endpoint.flow == session.flow)
        else {
            return NodeAudioState::UNSUPPORTED;
        };
        let Ok(control) = self.find_session_control(endpoint, &session.session_id) else {
            return NodeAudioState::UNSUPPORTED;
        };
        let Ok(volume_control) = control.cast::<Audio::ISimpleAudioVolume>() else {
            return NodeAudioState::UNSUPPORTED;
        };
        let volume = unsafe { volume_control.GetMasterVolume() }.ok();
        let muted = unsafe { volume_control.GetMute() }
            .ok()
            .map(|muted| muted.as_bool());
        NodeAudioState {
            volume,
            muted,
            volume_readable: volume.is_some(),
            volume_writable: true,
            mute_readable: muted.is_some(),
            mute_writable: true,
        }
    }

    fn enumerate_endpoints(
        &self,
        flow: Audio::EDataFlow,
    ) -> BackendResult<Vec<(String, Audio::EDataFlow, Audio::IMMDevice)>> {
        let collection = unsafe {
            self.enumerator
                .EnumAudioEndpoints(flow, Audio::DEVICE_STATE_ACTIVE)
        }
        .map_err(|error| native_error("enumerate audio endpoints", error))?;
        let count = unsafe { collection.GetCount() }
            .map_err(|error| native_error("read audio endpoint count", error))?;
        let mut result = Vec::with_capacity(count as usize);
        for index in 0..count {
            let device = unsafe { collection.Item(index) }
                .map_err(|error| native_error("read audio endpoint", error))?;
            let id = unsafe { device.GetId() }
                .map(take_pwstr)
                .map_err(|error| native_error("read audio endpoint ID", error))?;
            result.push((id, flow, device));
        }
        Ok(result)
    }

    fn add_sessions(
        &mut self,
        endpoint: &EndpointRecord,
        graph: &mut Graph,
    ) -> BackendResult<Vec<SessionRecord>> {
        let manager: Audio::IAudioSessionManager2 =
            match unsafe { endpoint.device.Activate(CLSCTX_ALL, None) } {
                Ok(manager) => manager,
                Err(_) => return Ok(Vec::new()),
            };

        let notification: Audio::IAudioSessionNotification = SessionNotificationClient {
            dirty: Arc::clone(&self.dirty),
        }
        .into();
        if unsafe { manager.RegisterSessionNotification(&notification) }.is_ok() {
            self.session_notifications
                .push((manager.clone(), notification));
        }

        let enumerator = match unsafe { manager.GetSessionEnumerator() } {
            Ok(enumerator) => enumerator,
            Err(_) => return Ok(Vec::new()),
        };
        let count = unsafe { enumerator.GetCount() }.unwrap_or(0);
        let mut result = Vec::new();
        for index in 0..count {
            let control = match unsafe { enumerator.GetSession(index) } {
                Ok(control) => control,
                Err(_) => continue,
            };
            let control2: Audio::IAudioSessionControl2 = match control.cast() {
                Ok(control) => control,
                Err(_) => continue,
            };
            let state = match unsafe { control.GetState() } {
                Ok(state) => state,
                Err(_) => continue,
            };
            if state != Audio::AudioSessionStateActive {
                continue;
            }
            let session_id = match unsafe { control2.GetSessionInstanceIdentifier() } {
                Ok(value) => take_pwstr(value),
                Err(_) => continue,
            };
            let process_id = unsafe { control2.GetProcessId() }.unwrap_or(0);
            let display_name = unsafe { control2.GetDisplayName() }
                .map(take_pwstr)
                .unwrap_or_default();
            let name = if display_name.trim().is_empty() {
                process_name(process_id).unwrap_or_else(|| format!("Audio session ({process_id})"))
            } else {
                display_name
            };
            let node_id = NodeId(graph_id(session_node_local_id(&endpoint.id, &session_id)));
            let port_id = PortId(graph_id(session_port_local_id(&endpoint.id, &session_id)));
            graph.add_node(
                Node::new(node_id, name, NodeType::WindowsAudioSession).with_serial(
                    stable_local_id(&format!("session:{}:{session_id}", endpoint.id)),
                ),
            )?;
            let session_direction = session_direction(endpoint.flow);
            graph.add_port(Port::new(
                port_id,
                node_id,
                "audio",
                session_direction,
                PortType::Audio,
            ))?;
            let (output, input) = session_link_ports(endpoint.flow, port_id, endpoint.port_id);
            let link_id = LinkId(graph_id(session_link_local_id(&endpoint.id, &session_id)));
            graph.insert_existing_link(Link {
                id: link_id,
                output_port: output,
                input_port: input,
            })?;

            // Query the meter before the control is handed to the notification
            // registration, which consumes it.
            let meter = control.cast::<IAudioMeterInformation>().ok();
            let events: Audio::IAudioSessionEvents = SessionEventsClient {
                dirty: Arc::clone(&self.dirty),
                node_id,
                states: Arc::clone(&self.audio_states),
            }
            .into();
            if unsafe { control.RegisterAudioSessionNotification(&events) }.is_ok() {
                self.session_events.push((control, events));
            }
            result.push(SessionRecord {
                endpoint_id: endpoint.id.clone(),
                session_id,
                flow: endpoint.flow,
                node_id,
                meter,
            });
        }
        Ok(result)
    }

    fn clear_session_callbacks(&mut self) {
        for (control, events) in self.session_events.drain(..) {
            let _ = unsafe { control.UnregisterAudioSessionNotification(&events) };
        }
        for (manager, notification) in self.session_notifications.drain(..) {
            let _ = unsafe { manager.UnregisterSessionNotification(&notification) };
        }
    }

    fn set_volume(&self, node: NodeId, volume: f32) -> BackendResult<()> {
        let volume = volume.clamp(0.0, 1.0);
        if let Some(endpoint) = self
            .endpoints
            .iter()
            .find(|endpoint| endpoint.node_id == node)
        {
            let control: IAudioEndpointVolume =
                unsafe { endpoint.device.Activate(CLSCTX_ALL, None) }
                    .map_err(|error| native_error("activate endpoint volume", error))?;
            return unsafe { control.SetMasterVolumeLevelScalar(volume, std::ptr::null()) }
                .map_err(|error| native_error("set endpoint volume", error));
        }
        let session = self
            .sessions
            .iter()
            .find(|session| session.node_id == node)
            .ok_or_else(|| BackendError::Unsupported("Windows audio node is unavailable".into()))?;
        let endpoint = self
            .endpoints
            .iter()
            .find(|endpoint| endpoint.id == session.endpoint_id && endpoint.flow == session.flow)
            .ok_or_else(|| {
                BackendError::Unsupported("Windows audio endpoint is unavailable".into())
            })?;
        let control = self.find_session_control(endpoint, &session.session_id)?;
        let volume_control: Audio::ISimpleAudioVolume = control
            .cast()
            .map_err(|error| native_error("activate session volume", error))?;
        unsafe { volume_control.SetMasterVolume(volume, std::ptr::null()) }
            .map_err(|error| native_error("set session volume", error))
    }

    fn set_mute(&self, node: NodeId, muted: bool) -> BackendResult<()> {
        if let Some(endpoint) = self
            .endpoints
            .iter()
            .find(|endpoint| endpoint.node_id == node)
        {
            let control: IAudioEndpointVolume =
                unsafe { endpoint.device.Activate(CLSCTX_ALL, None) }
                    .map_err(|error| native_error("activate endpoint volume", error))?;
            return unsafe { control.SetMute(muted, std::ptr::null()) }
                .map_err(|error| native_error("set endpoint mute", error));
        }
        let session = self
            .sessions
            .iter()
            .find(|session| session.node_id == node)
            .ok_or_else(|| BackendError::Unsupported("Windows audio node is unavailable".into()))?;
        let endpoint = self
            .endpoints
            .iter()
            .find(|endpoint| endpoint.id == session.endpoint_id && endpoint.flow == session.flow)
            .ok_or_else(|| {
                BackendError::Unsupported("Windows audio endpoint is unavailable".into())
            })?;
        let control = self.find_session_control(endpoint, &session.session_id)?;
        let volume_control: Audio::ISimpleAudioVolume = control
            .cast()
            .map_err(|error| native_error("activate session volume", error))?;
        unsafe { volume_control.SetMute(muted, std::ptr::null()) }
            .map_err(|error| native_error("set session mute", error))
    }

    fn find_session_control(
        &self,
        endpoint: &EndpointRecord,
        expected_id: &str,
    ) -> BackendResult<Audio::IAudioSessionControl> {
        let manager: Audio::IAudioSessionManager2 =
            unsafe { endpoint.device.Activate(CLSCTX_ALL, None) }
                .map_err(|error| native_error("activate session manager", error))?;
        let sessions = unsafe { manager.GetSessionEnumerator() }
            .map_err(|error| native_error("enumerate audio sessions", error))?;
        let count = unsafe { sessions.GetCount() }
            .map_err(|error| native_error("read audio session count", error))?;
        for index in 0..count {
            let control = unsafe { sessions.GetSession(index) }
                .map_err(|error| native_error("read audio session", error))?;
            let control2: Audio::IAudioSessionControl2 = control
                .cast()
                .map_err(|error| native_error("read audio session identity", error))?;
            let session_id = unsafe { control2.GetSessionInstanceIdentifier() }
                .map(take_pwstr)
                .map_err(|error| native_error("read audio session identity", error))?;
            if session_id == expected_id {
                return Ok(control);
            }
        }
        Err(BackendError::Unsupported(
            "Windows audio session is no longer available".into(),
        ))
    }

    /// Whether a node should currently own a meter, under the active policy.
    fn meter_wanted(&self, node: NodeId) -> bool {
        match self.meter_policy {
            MeterPolicy::Disabled => false,
            MeterPolicy::Always => true,
            MeterPolicy::OnDemand => self.requested_meters.contains(&node),
        }
    }

    /// Nodes that can report a level: every endpoint, plus every session whose
    /// control answered the meter query.
    fn meterable_nodes(&self) -> BTreeSet<NodeId> {
        self.endpoints
            .iter()
            .map(|endpoint| endpoint.node_id)
            .chain(
                self.sessions
                    .iter()
                    .filter(|session| session.meter.is_some())
                    .map(|session| session.node_id),
            )
            .collect()
    }

    fn audio_meters(&self) -> BackendResult<Vec<AudioMeter>> {
        if self.meter_policy == MeterPolicy::Disabled {
            return Ok(Vec::new());
        }
        let mut result = Vec::new();
        // Per-application levels, straight off each session's own meter.
        for session in &self.sessions {
            let Some(meter) = session.meter.as_ref() else {
                continue;
            };
            if !self.meter_wanted(session.node_id) {
                continue;
            }
            let Ok(peak) = (unsafe { meter.GetPeakValue() }) else {
                continue;
            };
            result.push(AudioMeter {
                node_id: session.node_id,
                port_id: None,
                // Like the endpoint meter, this is peak only.
                rms: 0.0,
                peak: peak.clamp(0.0, 1.0),
                age_ms: 0,
                available: true,
            });
        }
        for endpoint in &self.endpoints {
            if !self.meter_wanted(endpoint.node_id) {
                continue;
            }
            let meter: IAudioMeterInformation =
                match unsafe { endpoint.device.Activate(CLSCTX_ALL, None) } {
                    Ok(meter) => meter,
                    Err(_) => continue,
                };
            let peak = match unsafe { meter.GetPeakValue() } {
                Ok(peak) => peak.clamp(0.0, 1.0),
                Err(_) => continue,
            };
            // Core Audio's endpoint meter exposes peak level, not RMS. The
            // legacy application contract still has an f32 RMS field, so it
            // is left at zero rather than presenting peak as fabricated RMS.
            result.push(AudioMeter {
                node_id: endpoint.node_id,
                port_id: Some(endpoint.port_id),
                rms: 0.0,
                peak,
                age_ms: 0,
                available: true,
            });
        }
        Ok(result)
    }
}

impl Drop for CoreAudioWorker {
    fn drop(&mut self) {
        self.clear_session_callbacks();
        let _ = unsafe {
            self.enumerator
                .UnregisterEndpointNotificationCallback(&self.endpoint_notification)
        };
    }
}

#[windows::core::implement(Audio::IMMNotificationClient)]
struct EndpointNotificationClient {
    dirty: Arc<AtomicBool>,
}

impl Audio::IMMNotificationClient_Impl for EndpointNotificationClient_Impl {
    fn OnDeviceStateChanged(
        &self,
        _device_id: &PCWSTR,
        _new_state: Audio::DEVICE_STATE,
    ) -> windows::core::Result<()> {
        self.dirty.store(true, Ordering::Release);
        Ok(())
    }

    fn OnDeviceAdded(&self, _device_id: &PCWSTR) -> windows::core::Result<()> {
        self.dirty.store(true, Ordering::Release);
        Ok(())
    }

    fn OnDeviceRemoved(&self, _device_id: &PCWSTR) -> windows::core::Result<()> {
        self.dirty.store(true, Ordering::Release);
        Ok(())
    }

    fn OnDefaultDeviceChanged(
        &self,
        _flow: Audio::EDataFlow,
        _role: Audio::ERole,
        _device_id: &PCWSTR,
    ) -> windows::core::Result<()> {
        self.dirty.store(true, Ordering::Release);
        Ok(())
    }

    fn OnPropertyValueChanged(
        &self,
        _device_id: &PCWSTR,
        _key: &PROPERTYKEY,
    ) -> windows::core::Result<()> {
        self.dirty.store(true, Ordering::Release);
        Ok(())
    }
}

#[windows::core::implement(Audio::IAudioSessionNotification)]
struct SessionNotificationClient {
    dirty: Arc<AtomicBool>,
}

impl Audio::IAudioSessionNotification_Impl for SessionNotificationClient_Impl {
    fn OnSessionCreated(
        &self,
        _new_session: windows::core::Ref<Audio::IAudioSessionControl>,
    ) -> windows::core::Result<()> {
        self.dirty.store(true, Ordering::Release);
        Ok(())
    }
}

/// Applies a volume/mute change to the shared state map.
///
/// Both callbacks receive the new values in the payload, so nothing has to be
/// read back over COM and the graph never needs rebuilding for a fader move.
fn apply_state_change(states: &AudioStateMap, node_id: NodeId, volume: f32, muted: bool) {
    let Ok(mut states) = states.lock() else {
        return;
    };
    if let Some(state) = states.get_mut(&node_id) {
        if state.volume_readable {
            state.volume = Some(volume);
        }
        if state.mute_readable {
            state.muted = Some(muted);
        }
    }
}

#[windows::core::implement(Audio::Endpoints::IAudioEndpointVolumeCallback)]
struct EndpointVolumeCallback {
    node_id: NodeId,
    states: AudioStateMap,
}

impl Audio::Endpoints::IAudioEndpointVolumeCallback_Impl for EndpointVolumeCallback_Impl {
    fn OnNotify(
        &self,
        notify: *mut Audio::AUDIO_VOLUME_NOTIFICATION_DATA,
    ) -> windows::core::Result<()> {
        if notify.is_null() {
            return Ok(());
        }
        // Fields are read through the raw pointer: the struct is variable
        // length (a trailing channel-volume array) so it is never referenced
        // as a whole value.
        let (volume, muted) = unsafe {
            (
                std::ptr::addr_of!((*notify).fMasterVolume).read_unaligned(),
                std::ptr::addr_of!((*notify).bMuted).read_unaligned(),
            )
        };
        apply_state_change(&self.states, self.node_id, volume, muted.as_bool());
        Ok(())
    }
}

#[windows::core::implement(Audio::IAudioSessionEvents)]
struct SessionEventsClient {
    dirty: Arc<AtomicBool>,
    node_id: NodeId,
    states: AudioStateMap,
}

impl Audio::IAudioSessionEvents_Impl for SessionEventsClient_Impl {
    fn OnDisplayNameChanged(
        &self,
        _new_display_name: &PCWSTR,
        _event_context: *const GUID,
    ) -> windows::core::Result<()> {
        self.dirty.store(true, Ordering::Release);
        Ok(())
    }

    fn OnIconPathChanged(
        &self,
        _new_icon_path: &PCWSTR,
        _event_context: *const GUID,
    ) -> windows::core::Result<()> {
        Ok(())
    }

    /// The payload already carries the new values, so this updates the shared
    /// state directly. It deliberately does not mark the topology dirty: a
    /// session''s volume changing does not change the graph, and forcing a full
    /// endpoint and session re-enumeration for every fader tick was the bulk of
    /// the refresh churn.
    fn OnSimpleVolumeChanged(
        &self,
        new_volume: f32,
        new_mute: BOOL,
        _event_context: *const GUID,
    ) -> windows::core::Result<()> {
        apply_state_change(&self.states, self.node_id, new_volume, new_mute.as_bool());
        Ok(())
    }

    /// Per-channel volume does not change the master scalar this driver
    /// reports, and it never changes the graph, so it is ignored rather than
    /// triggering a rebuild.
    fn OnChannelVolumeChanged(
        &self,
        _channel_count: u32,
        _new_channel_volume_array: *const f32,
        _changed_channel: u32,
        _event_context: *const GUID,
    ) -> windows::core::Result<()> {
        Ok(())
    }

    fn OnGroupingParamChanged(
        &self,
        _new_grouping_param: *const GUID,
        _event_context: *const GUID,
    ) -> windows::core::Result<()> {
        Ok(())
    }

    fn OnStateChanged(&self, _new_state: Audio::AudioSessionState) -> windows::core::Result<()> {
        self.dirty.store(true, Ordering::Release);
        Ok(())
    }

    fn OnSessionDisconnected(
        &self,
        _disconnect_reason: Audio::AudioSessionDisconnectReason,
    ) -> windows::core::Result<()> {
        self.dirty.store(true, Ordering::Release);
        Ok(())
    }
}

fn native_error(operation: &str, error: impl std::fmt::Display) -> BackendError {
    BackendError::Native(format!("{operation} failed: {error}"))
}

fn graph_id(local_id: u64) -> u64 {
    encode_backend_id(BackendNamespace::WindowsAudio, local_id)
}

fn endpoint_direction(flow: Audio::EDataFlow) -> Direction {
    if flow == Audio::eRender {
        Direction::Sink
    } else {
        Direction::Source
    }
}

fn session_direction(flow: Audio::EDataFlow) -> Direction {
    if flow == Audio::eRender {
        Direction::Source
    } else {
        Direction::Sink
    }
}

fn session_link_ports(
    flow: Audio::EDataFlow,
    session_port: PortId,
    endpoint_port: PortId,
) -> (PortId, PortId) {
    if flow == Audio::eRender {
        (session_port, endpoint_port)
    } else {
        (endpoint_port, session_port)
    }
}

fn stable_local_id(value: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    let local = hash & LOCAL_ID_MASK;
    if local == 0 {
        1
    } else {
        local
    }
}

fn endpoint_node_local_id(endpoint_id: &str) -> u64 {
    stable_local_id(&format!("endpoint-node:{endpoint_id}"))
}

fn endpoint_port_local_id(endpoint_id: &str) -> u64 {
    stable_local_id(&format!("endpoint-port:{endpoint_id}"))
}

fn session_node_local_id(endpoint_id: &str, session_id: &str) -> u64 {
    stable_local_id(&format!("session-node:{endpoint_id}:{session_id}"))
}

fn session_port_local_id(endpoint_id: &str, session_id: &str) -> u64 {
    stable_local_id(&format!("session-port:{endpoint_id}:{session_id}"))
}

fn session_link_local_id(endpoint_id: &str, session_id: &str) -> u64 {
    stable_local_id(&format!("session-link:{endpoint_id}:{session_id}"))
}

fn take_pwstr(value: PWSTR) -> String {
    let text = unsafe { value.to_string() }.unwrap_or_default();
    unsafe { Com::CoTaskMemFree(Some(value.0 as *mut _)) };
    text
}

fn endpoint_name(device: &Audio::IMMDevice) -> Option<String> {
    unsafe {
        property_string(
            device,
            &Properties::DEVPKEY_Device_FriendlyName as *const _ as *const _,
        )
    }
}

unsafe fn property_string(device: &Audio::IMMDevice, key: *const PROPERTYKEY) -> Option<String> {
    let store: IPropertyStore = device.OpenPropertyStore(STGM_READ).ok()?;
    let mut value = store.GetValue(key).ok()?;
    let prop_variant = &value.Anonymous.Anonymous;
    if prop_variant.vt != VT_LPWSTR {
        let _ = StructuredStorage::PropVariantClear(&mut value);
        return None;
    }
    let ptr = *(&prop_variant.Anonymous as *const _ as *const *const u16);
    if ptr.is_null() {
        let _ = StructuredStorage::PropVariantClear(&mut value);
        return None;
    }
    let mut length = 0usize;
    while length < 32_768 && *ptr.add(length) != 0 {
        length += 1;
    }
    let text = if length == 32_768 {
        None
    } else {
        Some(
            OsString::from_wide(std::slice::from_raw_parts(ptr, length))
                .to_string_lossy()
                .into_owned(),
        )
    };
    let _ = StructuredStorage::PropVariantClear(&mut value);
    text
}

fn process_name(process_id: u32) -> Option<String> {
    if process_id == 0 {
        return None;
    }
    let process =
        unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id) }.ok()?;
    let mut buffer = [0u16; 512];
    let mut length = buffer.len() as u32;
    let result = unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut length,
        )
    };
    let _ = unsafe { CloseHandle(process) };
    result.ok()?;
    let path = OsString::from_wide(&buffer[..length as usize]);
    let name = std::path::Path::new(&path).file_stem()?.to_string_lossy();
    (!name.is_empty()).then(|| name.into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pw_graph_core::{backend_for_node, backend_for_port, BackendKind};
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    #[test]
    fn stable_ids_are_deterministic_and_namespaced() {
        let endpoint = graph_id(endpoint_node_local_id("speaker-id"));
        let port = graph_id(endpoint_port_local_id("speaker-id"));
        assert_eq!(endpoint, graph_id(endpoint_node_local_id("speaker-id")));
        assert_ne!(endpoint, port);
        assert_eq!(
            backend_for_node(NodeId(endpoint)),
            Some(BackendKind::WindowsAudio)
        );
        assert_eq!(
            backend_for_port(PortId(port)),
            Some(BackendKind::WindowsAudio)
        );
    }

    #[test]
    fn session_link_identity_depends_on_both_native_identifiers() {
        assert_ne!(
            session_link_local_id("endpoint-a", "session-a"),
            session_link_local_id("endpoint-b", "session-a")
        );
        assert_ne!(
            session_link_local_id("endpoint-a", "session-a"),
            session_link_local_id("endpoint-a", "session-b")
        );
    }

    #[test]
    fn endpoint_and_session_direction_mapping_matches_core_audio_flow() {
        assert_eq!(endpoint_direction(Audio::eRender), Direction::Sink);
        assert_eq!(endpoint_direction(Audio::eCapture), Direction::Source);
        assert_eq!(session_direction(Audio::eRender), Direction::Source);
        assert_eq!(session_direction(Audio::eCapture), Direction::Sink);

        let session_port = PortId(10);
        let endpoint_port = PortId(20);
        assert_eq!(
            session_link_ports(Audio::eRender, session_port, endpoint_port),
            (session_port, endpoint_port)
        );
        assert_eq!(
            session_link_ports(Audio::eCapture, session_port, endpoint_port),
            (endpoint_port, session_port)
        );
    }

    #[test]
    fn endpoint_notifications_mark_the_graph_dirty() {
        let dirty = Arc::new(AtomicBool::new(false));
        let callback: Audio::IMMNotificationClient = EndpointNotificationClient {
            dirty: Arc::clone(&dirty),
        }
        .into();

        unsafe {
            callback
                .OnDeviceAdded(PCWSTR(std::ptr::null()))
                .expect("notification callback should accept a device event");
        }
        assert!(dirty.load(Ordering::Acquire));
    }

    #[test]
    fn live_backend_startup_is_optional_for_headless_windows_ci() {
        let Ok(mut driver) = WindowsAudioDriver::new() else {
            // Windows CI runners may not expose an audio service or endpoint.
            return;
        };
        let nodes = driver
            .refresh()
            .expect("Core Audio refresh should succeed after startup");
        assert!(nodes.iter().all(|node| {
            matches!(
                node.node_type,
                NodeType::WindowsAudioEndpoint | NodeType::WindowsAudioSession
            )
        }));
        assert!(driver
            .graph()
            .ports
            .values()
            .all(|port| port.port_type == PortType::Audio));
        assert!(driver.graph().links.values().all(|link| {
            driver.graph().port(link.output_port).is_some()
                && driver.graph().port(link.input_port).is_some()
        }));
    }
}
