use super::*;

/// Mirror the daemon's registry into [`RegistryState`].
///
/// PipeWire announces every node, port and link as a "global" and withdraws
/// it by id. The callbacks run on the thread loop, so they only record what
/// changed and set the dirty flag; the driver rebuilds its graph from the
/// state on its next pass rather than doing that work on the loop thread.
///
/// The returned listener owns the subscription: dropping it unsubscribes,
/// which is why the driver keeps it alive in a field.
pub(super) fn install_registry_listener(
    registry: &pw::registry::Registry,
    state: &Arc<Mutex<RegistryState>>,
    registry_dirty: &Arc<AtomicBool>,
) -> pw::registry::Listener {
    let state_for_globals = state.clone();
    let state_for_removals = state.clone();
    let dirty_for_globals = registry_dirty.clone();
    let dirty_for_removals = registry_dirty.clone();
    registry
        .add_listener_local()
        .global(move |global| {
            let Some(props) = global.props else {
                return;
            };
            let mut state = state_for_globals.lock().unwrap();
            match &global.type_ {
                pw::types::ObjectType::Node => {
                    let name = props
                        .get(NODE_NAME)
                        .or_else(|| props.get(NODE_DESCRIPTION))
                        .unwrap_or("PipeWire node")
                        .to_owned();
                    let media_class = props.get(MEDIA_CLASS).unwrap_or_default().to_owned();
                    let serial = props
                        .get(OBJECT_SERIAL)
                        .and_then(|value| value.parse().ok());
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
                            channel: props.get(AUDIO_CHANNEL).map(str::to_owned),
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
            dirty_for_globals.store(true, Ordering::Relaxed);
        })
        .global_remove(move |id| {
            let mut state = state_for_removals.lock().unwrap();
            state.nodes.remove(&id);
            state.ports.remove(&id);
            state.links.remove(&id);
            dirty_for_removals.store(true, Ordering::Relaxed);
        })
        .register()
}

#[derive(Clone, Debug, Default)]
pub(super) struct NodeRecord {
    pub(super) name: String,
    pub(super) media_class: String,
    /// `object.serial` is unique for the lifetime of the daemon, while node
    /// names are not. Targeting by serial keeps a meter pinned to the node the
    /// user actually asked about when several share a name.
    pub(super) serial: Option<u64>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct PortRecord {
    pub(super) node_id: u32,
    pub(super) name: String,
    pub(super) channel: Option<String>,
    pub(super) direction: Direction,
    pub(super) media_type: String,
}

#[derive(Clone, Debug, Default)]
pub(super) struct LinkRecord {
    pub(super) output_port: u32,
    pub(super) input_port: u32,
}

#[derive(Clone, Debug, Default)]
pub(super) struct RegistryState {
    pub(super) nodes: BTreeMap<u32, NodeRecord>,
    pub(super) ports: BTreeMap<u32, PortRecord>,
    pub(super) links: BTreeMap<u32, LinkRecord>,
}

pub(super) fn classify_port_type(media_type: &str, node_media_class: Option<&str>) -> PortType {
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
