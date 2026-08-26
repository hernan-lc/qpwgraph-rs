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
use std::sync::Arc;
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
    relay: false,
};

#[derive(Debug)]
enum WorkerCommand {
    Refresh(Sender<BackendResult<Graph>>),
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
    positions: BTreeMap<NodeId, [f32; 2]>,
    command_tx: Sender<WorkerCommand>,
    dirty: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl WindowsAudioDriver {
    pub fn new() -> BackendResult<Self> {
        let (command_tx, command_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let dirty = Arc::new(AtomicBool::new(true));
        let worker_dirty = Arc::clone(&dirty);
        let worker = thread::Builder::new()
            .name("qpwgraph-windows-audio".into())
            .spawn(move || worker_thread(command_rx, ready_tx, worker_dirty))
            .map_err(|error| {
                BackendError::Native(format!("could not start audio worker: {error}"))
            })?;

        let graph = match ready_rx.recv() {
            Ok(Ok(graph)) => graph,
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
            graph,
            positions: BTreeMap::new(),
            command_tx,
            dirty,
            worker: Some(worker),
        })
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
        let mut graph = Self::response(receiver)?;
        for (node_id, position) in &self.positions {
            if let Some(node) = graph.nodes.get_mut(node_id) {
                node.position = *position;
            }
        }
        self.graph = graph;
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

    fn set_node_mute(&mut self, node: NodeId, muted: bool) -> BackendResult<()> {
        let (sender, receiver) = mpsc::channel();
        self.command_tx
            .send(WorkerCommand::SetMute(node, muted, sender))
            .map_err(|_| BackendError::Native("Windows audio worker is unavailable".into()))?;
        Self::response(receiver)
    }

    fn set_node_volume(&mut self, node: NodeId, volume: f32) -> BackendResult<()> {
        let (sender, receiver) = mpsc::channel();
        self.command_tx
            .send(WorkerCommand::SetVolume(node, volume, sender))
            .map_err(|_| BackendError::Native("Windows audio worker is unavailable".into()))?;
        Self::response(receiver)
    }

    fn graph(&self) -> &Graph {
        &self.graph
    }

    fn graph_dirty(&self) -> bool {
        self.dirty.load(Ordering::Acquire)
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

fn worker_thread(
    command_rx: Receiver<WorkerCommand>,
    ready_tx: Sender<BackendResult<Graph>>,
    dirty: Arc<AtomicBool>,
) {
    let initialized = unsafe { Com::CoInitializeEx(None, COINIT_MULTITHREADED) };
    if initialized.is_err() {
        let _ = ready_tx.send(Err(BackendError::Native(format!(
            "could not initialize Windows COM: {initialized:?}"
        ))));
        return;
    }

    let worker = CoreAudioWorker::new(Arc::clone(&dirty));
    let mut worker = match worker {
        Ok(worker) => worker,
        Err(error) => {
            let _ = ready_tx.send(Err(error));
            unsafe { Com::CoUninitialize() };
            return;
        }
    };

    match worker.refresh_graph() {
        Ok(graph) => {
            let _ = ready_tx.send(Ok(graph));
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
}

impl CoreAudioWorker {
    fn new(dirty: Arc<AtomicBool>) -> BackendResult<Self> {
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
            endpoints: Vec::new(),
            sessions: Vec::new(),
            meter_policy: MeterPolicy::OnDemand,
            requested_meters: BTreeSet::new(),
        })
    }

    fn refresh_graph(&mut self) -> BackendResult<Graph> {
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
        Ok(graph)
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

            let events: Audio::IAudioSessionEvents = SessionEventsClient {
                dirty: Arc::clone(&self.dirty),
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

    fn audio_meters(&self) -> BackendResult<Vec<AudioMeter>> {
        if self.meter_policy == MeterPolicy::Disabled {
            return Ok(Vec::new());
        }
        let mut result = Vec::new();
        for endpoint in &self.endpoints {
            if self.meter_policy == MeterPolicy::OnDemand
                && !self.requested_meters.contains(&endpoint.node_id)
            {
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

#[windows::core::implement(Audio::IAudioSessionEvents)]
struct SessionEventsClient {
    dirty: Arc<AtomicBool>,
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

    fn OnSimpleVolumeChanged(
        &self,
        _new_volume: f32,
        _new_mute: BOOL,
        _event_context: *const GUID,
    ) -> windows::core::Result<()> {
        self.dirty.store(true, Ordering::Release);
        Ok(())
    }

    fn OnChannelVolumeChanged(
        &self,
        _channel_count: u32,
        _new_channel_volume_array: *const f32,
        _changed_channel: u32,
        _event_context: *const GUID,
    ) -> windows::core::Result<()> {
        self.dirty.store(true, Ordering::Release);
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
