use super::*;

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
