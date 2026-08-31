//! What the canvas is showing and how a drag connects: the two user
//! choices that change how the same graph is projected.

use super::*;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum MediaFilter {
    #[default]
    All,
    Audio,
    Video,
    Midi,
}

impl MediaFilter {
    pub(crate) fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "audio" => Self::Audio,
            "video" => Self::Video,
            "midi" => Self::Midi,
            _ => Self::All,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Audio => "audio",
            Self::Video => "video",
            Self::Midi => "midi",
        }
    }

    pub(crate) fn matches_port_type(self, port_type: PortType) -> bool {
        match self {
            Self::All => true,
            Self::Audio => port_type == PortType::Audio,
            Self::Video => port_type == PortType::Video,
            Self::Midi => matches!(port_type, PortType::MidiJack | PortType::MidiAlsa),
        }
    }

    pub(super) fn matches_node(self, graph: &Graph, node: &Node) -> bool {
        self == Self::All
            || node.ports.iter().any(|port_id| {
                graph
                    .port(*port_id)
                    .is_some_and(|port| self.matches_port_type(port.port_type))
            })
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum ConnectMode {
    #[default]
    Advanced,
    Easy,
}

impl ConnectMode {
    pub(crate) fn parse(value: &str) -> Self {
        if value.eq_ignore_ascii_case("easy") {
            Self::Easy
        } else {
            Self::Advanced
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Advanced => "advanced",
            Self::Easy => "easy",
        }
    }
}
