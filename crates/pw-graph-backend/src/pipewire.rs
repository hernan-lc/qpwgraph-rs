//! Native PipeWire graph driver implemented with the official Rust bindings.
//!
//! PipeWire objects are deliberately kept on a dedicated `ThreadLoop`. The
//! public driver remains synchronous for the rest of the application, while
//! every registry, link, and stream operation is protected by the loop lock.

use super::*;
use ::pipewire as pw;
use pw::proxy::ProxyT;
use pw::spa::param::audio::{AudioFormat, AudioInfoRaw};
use pw::spa::param::ParamType;
use pw::spa::pod::serialize::PodSerializer;
use pw::spa::pod::{Pod, Value};
use pw::spa::utils::Direction as SpaDirection;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Cursor;
use std::rc::Rc;
use std::time::{Duration, Instant};

const NODE_NAME: &str = "node.name";
const NODE_DESCRIPTION: &str = "node.description";
const MEDIA_CLASS: &str = "media.class";
const MEDIA_TYPE: &str = "media.type";
const FORMAT_DSP: &str = "format.dsp";
const NODE_ID: &str = "node.id";
const OBJECT_SERIAL: &str = "object.serial";
const PORT_NAME: &str = "port.name";
const PORT_DIRECTION: &str = "port.direction";
const LINK_OUTPUT_PORT: &str = "link.output.port";
const LINK_INPUT_PORT: &str = "link.input.port";

/// Node name given to our own metering streams. They are helper objects, so
/// they are filtered back out of the graph the UI renders.
const METER_NODE_PREFIX: &str = "qpwgraph-rs meter";

/// How long a metering stream outlives the last request for it. Hovering a port
/// asks for a meter every frame; without a grace period, moving the pointer
/// between two ports would tear down and rebuild streams continuously.
const METER_LINGER: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, Default)]
struct NodeRecord {
    name: String,
    media_class: String,
    /// `object.serial` is unique for the lifetime of the daemon, while node
    /// names are not. Targeting by serial keeps a meter pinned to the node the
    /// user actually asked about when several share a name.
    serial: Option<u64>,
}

#[derive(Clone, Debug, Default)]
struct PortRecord {
    node_id: u32,
    name: String,
    direction: Direction,
    media_type: String,
}

#[derive(Clone, Debug, Default)]
struct LinkRecord {
    output_port: u32,
    input_port: u32,
}

#[derive(Clone, Debug, Default)]
struct RegistryState {
    nodes: BTreeMap<u32, NodeRecord>,
    ports: BTreeMap<u32, PortRecord>,
    links: BTreeMap<u32, LinkRecord>,
}

#[derive(Debug)]
struct MeterReadingState {
    rms: f32,
    peak: f32,
    connected: bool,
    updated_at: Option<Instant>,
}

impl Default for MeterReadingState {
    fn default() -> Self {
        Self {
            rms: 0.0,
            peak: 0.0,
            connected: false,
            updated_at: None,
        }
    }
}

struct MeterCallbackState {
    shared: Rc<RefCell<MeterReadingState>>,
    format: AudioInfoRaw,
}

struct MeterHandle {
    _stream: pw::stream::Stream,
    _listener: pw::stream::StreamListener<MeterCallbackState>,
    shared: Rc<RefCell<MeterReadingState>>,
}

pub struct PipewireDriver {
    thread_loop: pw::thread_loop::ThreadLoop,
    context: Option<pw::context::Context>,
    core: Option<pw::core::Core>,
    registry: Option<pw::registry::Registry>,
    registry_listener: Option<pw::registry::Listener>,
    state: Rc<RefCell<RegistryState>>,
    meters: BTreeMap<NodeId, MeterHandle>,
    meter_policy: MeterPolicy,
    /// Nodes the UI asked to measure, with the time of the last request so a
    /// stream can linger briefly instead of dying the moment a tooltip closes.
    meter_requests: BTreeMap<NodeId, Instant>,
    graph: Graph,
    positions: BTreeMap<NodeId, [f32; 2]>,
}

impl PipewireDriver {
    pub fn new() -> BackendResult<Self> {
        pw::init();

        let thread_loop = unsafe { pw::thread_loop::ThreadLoop::new(Some("qpwgraph-rs"), None) }
            .map_err(|error| native_error("PipeWire thread loop creation", error))?;
        let context = pw::context::Context::new(&thread_loop)
            .map_err(|error| native_error("PipeWire context creation", error))?;
        let core = context
            .connect(None)
            .map_err(|error| native_error("PipeWire core connection", error))?;
        let registry = core
            .get_registry()
            .map_err(|error| native_error("PipeWire registry creation", error))?;
        let state = Rc::new(RefCell::new(RegistryState::default()));

        let state_for_globals = state.clone();
        let state_for_removals = state.clone();
        let registry_listener = registry
            .add_listener_local()
            .global(move |global| {
                let Some(props) = global.props else {
                    return;
                };
                let mut state = state_for_globals.borrow_mut();
                match &global.type_ {
                    pw::types::ObjectType::Node => {
                        let name = props
                            .get(NODE_NAME)
                            .or_else(|| props.get(NODE_DESCRIPTION))
                            .unwrap_or("PipeWire node")
                            .to_owned();
                        let media_class = props.get(MEDIA_CLASS).unwrap_or_default().to_owned();
                        let serial = props.get(OBJECT_SERIAL).and_then(|value| value.parse().ok());
                        state.nodes.insert(
                            global.id,
                            NodeRecord {
                                name,
                                media_class,
                                serial,
                            },
                        );
                    }
                    pw::types::ObjectType::Port => {
                        let media_type = props
                            .get(MEDIA_TYPE)
                            .or_else(|| props.get(FORMAT_DSP))
                            .unwrap_or_default()
                            .to_owned();
                        let direction = if props.get(PORT_DIRECTION) == Some("out") {
                            Direction::Source
                        } else {
                            Direction::Sink
                        };
                        let node_id = props
                            .get(NODE_ID)
                            .and_then(|value| value.parse().ok())
                            .unwrap_or_default();
                        state.ports.insert(
                            global.id,
                            PortRecord {
                                node_id,
                                name: props.get(PORT_NAME).unwrap_or("PipeWire port").to_owned(),
                                direction,
                                media_type,
                            },
                        );
                    }
                    pw::types::ObjectType::Link => {
                        let output_port = props
                            .get(LINK_OUTPUT_PORT)
                            .and_then(|value| value.parse().ok())
                            .unwrap_or_default();
                        let input_port = props
                            .get(LINK_INPUT_PORT)
                            .and_then(|value| value.parse().ok())
                            .unwrap_or_default();
                        state.links.insert(
                            global.id,
                            LinkRecord {
                                output_port,
                                input_port,
                            },
                        );
                    }
                    _ => {}
                }
            })
            .global_remove(move |id| {
                let mut state = state_for_removals.borrow_mut();
                state.nodes.remove(&id);
                state.ports.remove(&id);
                state.links.remove(&id);
            })
            .register();

        thread_loop.start();

        let driver = Self {
            thread_loop,
            context: Some(context),
            core: Some(core),
            registry: Some(registry),
            registry_listener: Some(registry_listener),
            state,
            meters: BTreeMap::new(),
            meter_policy: MeterPolicy::default(),
            meter_requests: BTreeMap::new(),
            graph: Graph::default(),
            positions: BTreeMap::new(),
        };

        let loop_for_initial_sync = driver.thread_loop.clone();
        let _guard = loop_for_initial_sync.lock();
        driver.roundtrip_locked()?;
        Ok(driver)
    }

    fn core(&self) -> BackendResult<&pw::core::Core> {
        self.core
            .as_ref()
            .ok_or_else(|| BackendError::Native("PipeWire core is closed".into()))
    }

    fn registry(&self) -> BackendResult<&pw::registry::Registry> {
        self.registry
            .as_ref()
            .ok_or_else(|| BackendError::Native("PipeWire registry is closed".into()))
    }

    /// Wait for all registry events queued before the sync request.
    /// The caller must hold the thread-loop lock.
    fn roundtrip_locked(&self) -> BackendResult<()> {
        let core = self.core()?.clone();
        let pending = core
            .sync(0)
            .map_err(|error| native_error("PipeWire registry synchronization", error))?;
        let done = Rc::new(Cell::new(false));
        let failure = Rc::new(RefCell::new(None::<String>));
        let done_for_callback = done.clone();
        let loop_for_done_callback = self.thread_loop.clone();
        let loop_for_error_callback = self.thread_loop.clone();
        let failure_for_callback = failure.clone();
        let listener = core
            .add_listener_local()
            .done(move |id, sequence| {
                if id == pw::core::PW_ID_CORE && sequence == pending {
                    done_for_callback.set(true);
                    loop_for_done_callback.signal(false);
                }
            })
            .error(move |_id, _sequence, result, message| {
                *failure_for_callback.borrow_mut() = Some(format!("{message} ({result})"));
                loop_for_error_callback.signal(false);
            })
            .register();

        while !done.get() && failure.borrow().is_none() {
            self.thread_loop.wait();
        }
        drop(listener);

        if let Some(error) = failure.borrow_mut().take() {
            return Err(BackendError::Native(format!(
                "PipeWire registry synchronization failed: {error}"
            )));
        }
        if !done.get() {
            return Err(BackendError::Native(
                "PipeWire registry synchronization ended unexpectedly".into(),
            ));
        }
        Ok(())
    }

    fn rebuild_graph_locked(&mut self) -> BackendResult<()> {
        let state = self.state.borrow().clone();
        let mut graph = Graph::default();
        let mut node_media_classes = HashMap::new();

        for (index, (id, record)) in state.nodes.iter().enumerate() {
            let node_id = NodeId(*id as u64);
            if record.name.starts_with(METER_NODE_PREFIX) {
                continue;
            }
            node_media_classes.insert(node_id, record.media_class.to_ascii_lowercase());
            let mut node = Node::new(node_id, &record.name, NodeType::PipeWire);
            node.position = self.positions.get(&node_id).copied().unwrap_or_else(|| {
                let column = (index % 4) as f32;
                let row = (index / 4) as f32;
                [40.0 + column * 280.0, 40.0 + row * 180.0]
            });
            self.positions.insert(node_id, node.position);
            graph.add_node(node)?;
        }

        for (id, record) in state.ports {
            let node_id = NodeId(record.node_id as u64);
            if graph.node(node_id).is_none() {
                continue;
            }
            let port_type = classify_port_type(
                &record.media_type,
                node_media_classes.get(&node_id).map(String::as_str),
            );
            graph.add_port(Port::new(
                PortId(id as u64),
                node_id,
                record.name,
                record.direction,
                port_type,
            ))?;
        }

        for (id, record) in state.links {
            let _ = graph.insert_existing_link(Link {
                id: LinkId(id as u64),
                output_port: PortId(record.output_port as u64),
                input_port: PortId(record.input_port as u64),
            });
        }

        self.graph = graph;
        self.ensure_meters_locked();
        Ok(())
    }

    /// Nodes that can be measured: they expose at least one audio source port.
    fn measurable_nodes(&self) -> BTreeSet<NodeId> {
        self.graph
            .nodes
            .values()
            .filter(|node| {
                node.ports.iter().any(|port_id| {
                    self.graph.port(*port_id).is_some_and(|port| {
                        port.direction.is_source() && port.port_type == PortType::Audio
                    })
                })
            })
            .map(|node| node.id)
            .collect()
    }

    /// Nodes that should currently own a metering stream.
    ///
    /// Under [`MeterPolicy::OnDemand`] this is driven purely by what the UI
    /// asked for, so an idle window holds no streams and never nudges the
    /// daemon into resuming or renegotiating a device.
    fn wanted_meter_nodes(&self) -> BTreeSet<NodeId> {
        let measurable = self.measurable_nodes();
        match self.meter_policy {
            MeterPolicy::Disabled => BTreeSet::new(),
            MeterPolicy::Always => measurable,
            MeterPolicy::OnDemand => self
                .meter_requests
                .keys()
                .copied()
                .filter(|node_id| measurable.contains(node_id))
                .collect(),
        }
    }

    /// Drop request entries that have outlived [`METER_LINGER`].
    fn expire_meter_requests(&mut self) {
        let now = Instant::now();
        self.meter_requests
            .retain(|_, requested_at| now.saturating_duration_since(*requested_at) < METER_LINGER);
    }

    fn ensure_meters_locked(&mut self) {
        self.expire_meter_requests();
        let wanted = self.wanted_meter_nodes();
        self.meters.retain(|node_id, _| wanted.contains(node_id));

        let missing: Vec<(NodeId, NodeRecord)> = {
            let state = self.state.borrow();
            wanted
                .into_iter()
                .filter(|node_id| !self.meters.contains_key(node_id))
                .filter_map(|node_id| {
                    state
                        .nodes
                        .get(&(node_id.0 as u32))
                        .map(|record| (node_id, record.clone()))
                })
                .collect()
        };
        for (node_id, record) in missing {
            if let Ok(handle) = self.create_meter_locked(node_id, &record) {
                self.meters.insert(node_id, handle);
            }
        }
    }

    fn create_meter_locked(
        &self,
        node_id: NodeId,
        record: &NodeRecord,
    ) -> BackendResult<MeterHandle> {
        let core = self.core()?.clone();
        // Node names are not unique; the daemon-assigned serial is. Falling back
        // to the name only matters for objects that predate `object.serial`.
        let target = record
            .serial
            .map(|serial| serial.to_string())
            .unwrap_or_else(|| record.name.clone());
        let stream_name = format!("{METER_NODE_PREFIX} {}", node_id.0);
        let description = format!("Level meter: {}", record.name);
        let mut properties = pw::properties::properties! {
            "node.name" => stream_name.as_str(),
            "node.description" => description.as_str(),
            "media.type" => "Audio",
            "media.category" => "Capture",
            "media.role" => "DSP",
            "media.class" => "Stream/Input/Audio",
            // Tells the session manager this client only observes. Monitoring
            // streams are excluded from routing decisions such as switching the
            // default device or counting active streams on a node.
            "stream.monitor" => "true",
            // Passive links never make our stream a driver and never keep the
            // target awake, so devices can still suspend while we are attached.
            "node.passive" => "true",
            // Never let the session manager move us to another node; without
            // this a meter can silently follow the default device instead.
            "node.dont-reconnect" => "true",
            "stream.dont-remix" => "true",
            "target.object" => target.as_str(),
        };
        // A capture stream aimed at a sink must be told to read that sink's
        // monitor ports. Otherwise the session manager treats it as an ordinary
        // recording client and routes it to the default *source* -- which would
        // open the user's microphone instead of metering the sink.
        if record.media_class.to_ascii_lowercase().contains("sink") {
            properties.insert("stream.capture.sink", "true");
        }
        let stream = pw::stream::Stream::new(&core, &stream_name, properties)
            .map_err(|error| native_error("PipeWire audio meter stream creation", error))?;
        let shared = Rc::new(RefCell::new(MeterReadingState::default()));
        let callback_shared = shared.clone();
        let mut callback_format = AudioInfoRaw::new();
        callback_format.set_format(AudioFormat::F32LE);
        let listener = stream
            .add_local_listener_with_user_data(MeterCallbackState {
                shared: callback_shared,
                format: callback_format,
            })
            .state_changed(|_, data, _old, new| {
                data.shared.borrow_mut().connected =
                    matches!(new, pw::stream::StreamState::Streaming);
            })
            .param_changed(|_stream, data, id, param| {
                if id != ParamType::Format.as_raw() {
                    return;
                }
                let Some(param) = param else {
                    return;
                };
                let mut format = AudioInfoRaw::new();
                if format.parse(param).is_ok() {
                    data.format = format;
                }
            })
            .process(|stream, data| process_meter_buffer(stream, data))
            .register()
            .map_err(|error| native_error("PipeWire audio meter listener", error))?;

        let pod_bytes = audio_format_pod()?;
        let pod = Pod::from_bytes(&pod_bytes).ok_or_else(|| {
            BackendError::Native("could not serialize PipeWire audio meter format".into())
        })?;
        let mut params = [pod];
        stream
            .connect(
                SpaDirection::Input,
                None,
                pw::stream::StreamFlags::AUTOCONNECT
                    | pw::stream::StreamFlags::MAP_BUFFERS
                    | pw::stream::StreamFlags::RT_PROCESS
                    | pw::stream::StreamFlags::DONT_RECONNECT,
                &mut params,
            )
            .map_err(|error| native_error("PipeWire audio meter stream connection", error))?;

        Ok(MeterHandle {
            _stream: stream,
            _listener: listener,
            shared,
        })
    }

    fn connect_locked(&mut self, src: PortId, dst: PortId) -> BackendResult<Link> {
        let output = self.graph.port(src).ok_or(GraphError::MissingPort(src))?;
        let input = self.graph.port(dst).ok_or(GraphError::MissingPort(dst))?;
        if !output.direction.is_source() {
            return Err(GraphError::NotSource(src).into());
        }
        if !input.direction.is_sink() {
            return Err(GraphError::NotSink(dst).into());
        }
        if output.port_type != input.port_type
            && output.port_type != PortType::Unknown
            && input.port_type != PortType::Unknown
        {
            return Err(GraphError::IncompatiblePorts(src, dst).into());
        }

        let properties = pw::properties::properties! {
            "link.output.node" => output.node_id.0.to_string(),
            "link.output.port" => src.0.to_string(),
            "link.input.node" => input.node_id.0.to_string(),
            "link.input.port" => dst.0.to_string(),
            "object.linger" => "1",
        };
        let proxy = self
            .core()?
            .create_object::<pw::link::Link>("link-factory", &properties)
            .map_err(|error| native_error("PipeWire link creation", error))?;
        let proxy_id = proxy.upcast_ref().id();
        drop(proxy);
        self.roundtrip_locked()?;

        let link_id = self
            .state
            .borrow()
            .links
            .iter()
            .find(|(_, link)| link.output_port == src.0 as u32 && link.input_port == dst.0 as u32)
            .map(|(id, _)| *id)
            .unwrap_or(proxy_id);
        self.rebuild_graph_locked()?;
        Ok(self
            .graph
            .link(LinkId(link_id as u64))
            .cloned()
            .unwrap_or(Link {
                id: LinkId(link_id as u64),
                output_port: src,
                input_port: dst,
            }))
    }

    fn disconnect_locked(&mut self, link: LinkId) -> BackendResult<Link> {
        let existing = self
            .graph
            .link(link)
            .cloned()
            .ok_or(GraphError::MissingLink(link))?;
        self.registry()?
            .destroy_global(link.0 as u32)
            .into_result()
            .map_err(|error| native_error("PipeWire link destruction", error))?;
        self.roundtrip_locked()?;
        self.rebuild_graph_locked()?;
        Ok(existing)
    }
}

impl Drop for PipewireDriver {
    fn drop(&mut self) {
        let guard = self.thread_loop.lock();
        self.meters.clear();
        self.registry_listener.take();
        self.registry.take();
        self.core.take();
        self.context.take();
        drop(guard);
        self.thread_loop.stop();
    }
}

impl GraphDriver for PipewireDriver {
    fn refresh(&mut self) -> BackendResult<Vec<Node>> {
        let loop_for_refresh = self.thread_loop.clone();
        let _guard = loop_for_refresh.lock();
        self.roundtrip_locked()?;
        self.rebuild_graph_locked()?;
        Ok(self.graph.nodes.values().cloned().collect())
    }

    fn connect(&mut self, src: PortId, dst: PortId) -> BackendResult<Link> {
        let loop_for_connect = self.thread_loop.clone();
        let _guard = loop_for_connect.lock();
        self.connect_locked(src, dst)
    }

    fn disconnect(&mut self, link: LinkId) -> BackendResult<Link> {
        let loop_for_disconnect = self.thread_loop.clone();
        let _guard = loop_for_disconnect.lock();
        self.disconnect_locked(link)
    }

    fn rename_node(&mut self, _node: NodeId, _name: String) -> BackendResult<()> {
        Err(BackendError::Unsupported(
            "PipeWire node names are owned by the producing client".into(),
        ))
    }

    fn set_node_position(&mut self, node: NodeId, position: [f32; 2]) -> BackendResult<()> {
        self.positions.insert(node, position);
        self.graph
            .nodes
            .get_mut(&node)
            .ok_or(GraphError::MissingNode(node))?
            .position = position;
        Ok(())
    }

    fn graph(&self) -> &Graph {
        &self.graph
    }

    fn is_node_type(&self, node_type: NodeType) -> bool {
        matches!(node_type, NodeType::PipeWire)
    }

    fn is_port_type(&self, port_type: PortType) -> bool {
        matches!(
            port_type,
            PortType::Audio | PortType::Video | PortType::MidiJack | PortType::Unknown
        )
    }

    fn audio_meters(&mut self) -> BackendResult<Vec<AudioMeter>> {
        // Readings are written by the stream's process callback, so holding the
        // loop lock is enough. A registry round-trip here would cost a full
        // core sync on every UI frame for data the sync does not affect.
        let loop_for_meters = self.thread_loop.clone();
        let _guard = loop_for_meters.lock();
        let now = Instant::now();
        Ok(self
            .meters
            .iter()
            .filter_map(|(node_id, meter)| {
                let reading = meter.shared.borrow();
                if !reading.connected {
                    return None;
                }
                let age_ms = reading
                    .updated_at
                    .map(|updated| now.saturating_duration_since(updated).as_millis())
                    .unwrap_or(u128::from(u32::MAX))
                    .min(u128::from(u32::MAX)) as u32;
                Some(AudioMeter {
                    node_id: *node_id,
                    rms: reading.rms.clamp(0.0, 1.0),
                    peak: reading.peak.clamp(0.0, 1.0),
                    age_ms,
                    available: true,
                })
            })
            .collect())
    }

    fn set_meter_policy(&mut self, policy: MeterPolicy) -> BackendResult<()> {
        if self.meter_policy == policy {
            return Ok(());
        }
        self.meter_policy = policy;
        if policy != MeterPolicy::OnDemand {
            self.meter_requests.clear();
        }
        let loop_for_policy = self.thread_loop.clone();
        let _guard = loop_for_policy.lock();
        self.ensure_meters_locked();
        Ok(())
    }

    fn request_meters(&mut self, nodes: &BTreeSet<NodeId>) -> BackendResult<()> {
        if self.meter_policy != MeterPolicy::OnDemand {
            return Ok(());
        }
        let now = Instant::now();
        for node_id in nodes {
            self.meter_requests.insert(*node_id, now);
        }
        self.expire_meter_requests();
        // The UI repeats this every frame. Only take the loop lock when the set
        // of live streams actually has to change.
        let wanted = self.wanted_meter_nodes();
        if wanted.iter().eq(self.meters.keys()) {
            return Ok(());
        }
        let loop_for_request = self.thread_loop.clone();
        let _guard = loop_for_request.lock();
        self.ensure_meters_locked();
        Ok(())
    }

    fn reset_audio_config(&mut self) -> BackendResult<()> {
        self.meter_requests.clear();
        let loop_for_reset = self.thread_loop.clone();
        let _guard = loop_for_reset.lock();
        self.meters.clear();
        Ok(())
    }
}

fn classify_port_type(media_type: &str, node_media_class: Option<&str>) -> PortType {
    let media_type = media_type.to_ascii_lowercase();
    let node_media_class = node_media_class.unwrap_or_default().to_ascii_lowercase();
    if media_type.contains("midi") {
        PortType::MidiJack
    } else if media_type.contains("video") {
        PortType::Video
    } else if media_type.contains("audio") {
        PortType::Audio
    } else if node_media_class.contains("midi") {
        PortType::MidiJack
    } else if node_media_class.contains("video") {
        PortType::Video
    } else if node_media_class.contains("audio") {
        PortType::Audio
    } else {
        PortType::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::classify_port_type;
    use pw_graph_core::PortType;

    #[test]
    fn classifies_media_types_without_case_sensitive_metadata() {
        assert_eq!(classify_port_type("Audio", None), PortType::Audio);
        assert_eq!(classify_port_type("video/raw", None), PortType::Video);
        assert_eq!(
            classify_port_type("", Some("Midi/Source")),
            PortType::MidiJack
        );
    }

    #[test]
    fn prefers_explicit_port_media_metadata() {
        assert_eq!(
            classify_port_type("audio/raw", Some("Video/Source")),
            PortType::Audio
        );
        assert_eq!(
            classify_port_type("midi/raw", Some("Audio/Source")),
            PortType::MidiJack
        );
        assert_eq!(
            classify_port_type("", Some("Stream/Output")),
            PortType::Unknown
        );
    }
}

fn native_error(operation: &str, error: impl std::fmt::Display) -> BackendError {
    BackendError::Native(format!("{operation} failed: {error}"))
}

fn audio_format_pod() -> BackendResult<Vec<u8>> {
    // Rate and channel count are deliberately left unset: a zeroed field is
    // omitted from the pod, so the daemon negotiates the node's own values
    // instead of asking it to resample or reconfigure for the meter.
    let mut audio_info = AudioInfoRaw::new();
    audio_info.set_format(AudioFormat::F32LE);
    let object = pw::spa::pod::Object {
        type_: pw::spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: ParamType::EnumFormat.as_raw(),
        properties: audio_info.into(),
    };
    PodSerializer::serialize(Cursor::new(Vec::new()), &Value::Object(object))
        .map(|success| success.0.into_inner())
        .map_err(|error| native_error("PipeWire audio format serialization", error))
}

fn process_meter_buffer(stream: &pw::stream::StreamRef, data: &mut MeterCallbackState) {
    let Some(mut buffer) = stream.dequeue_buffer() else {
        return;
    };
    let format = data.format.format();
    if !matches!(
        format,
        AudioFormat::F32LE | AudioFormat::F32BE | AudioFormat::F32P
    ) {
        return;
    }

    let mut peak = 0.0_f32;
    let mut sum = 0.0_f64;
    let mut samples = 0_u64;
    for block in buffer.datas_mut() {
        let offset = block.chunk().offset() as usize;
        let size = block.chunk().size() as usize;
        let Some(bytes) = block.data() else {
            continue;
        };
        let end = offset.saturating_add(size).min(bytes.len());
        if offset >= end {
            continue;
        }
        for chunk in bytes[offset..end].chunks_exact(std::mem::size_of::<f32>()) {
            let mut raw = [0_u8; 4];
            raw.copy_from_slice(chunk);
            let value = if format == AudioFormat::F32BE {
                f32::from_be_bytes(raw)
            } else {
                f32::from_le_bytes(raw)
            };
            if !value.is_finite() {
                continue;
            }
            let absolute = value.abs();
            peak = peak.max(absolute);
            sum += f64::from(absolute) * f64::from(absolute);
            samples += 1;
        }
    }

    if samples > 0 {
        let mut reading = data.shared.borrow_mut();
        reading.rms = (sum / samples as f64).sqrt().min(1.0) as f32;
        reading.peak = peak.min(1.0);
        reading.updated_at = Some(Instant::now());
    }
}
