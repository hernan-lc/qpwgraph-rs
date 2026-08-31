//! Framework-neutral state projected into Slint models.
//!
//! | Module | Owns |
//! | --- | --- |
//! | [`state`] | selection, viewport, collapse, and the projection itself |
//! | [`view`] | the projected shapes: node, port group, link, snapshot |
//! | [`projection`] | geometry and colour, plus the stored positions it starts from |
//! | [`id_map`] | stable `i32` identities for Slint models |
//! | [`filter`] | the media filter and the connect mode |
//! | [`layout`] | where a dragged node may land, and position restore |
//! | [`pairing`] | which ports easy-connect matches to which |
//!
//! Only the meter types and the node geometry constants stay here, because
//! every one of those modules needs them.

use pw_graph_backend::{GraphDriver, NodeAudioState, NodeCapabilities};
use pw_graph_config::AppConfig;
use pw_graph_core::{
    Direction, Graph, LinkId, Node, NodeAppearance, NodeId, NodeType, Port, PortId, PortType,
};
use std::collections::{BTreeMap, BTreeSet, HashMap};

mod filter;
mod id_map;
mod layout;
mod pairing;
mod projection;
mod state;
mod view;

#[cfg(test)]
mod tests;

// These were one 2,100-line file. `pub(super)` in a submodule means what a
// bare item meant there: private to `model`.
pub(crate) use self::filter::*;
pub(crate) use self::id_map::*;
pub(crate) use self::layout::*;
use self::pairing::*;
pub(crate) use self::projection::*;
pub(crate) use self::state::*;
pub(crate) use self::view::*;

const NODE_WIDTH: f32 = 244.0;
const NODE_HEADER_HEIGHT: f32 = 42.0;
const COLLAPSED_NODE_HEIGHT: f32 = 50.0;
const PORT_ROW_HEIGHT: f32 = 25.0;
const AUDIO_CONTROLS_HEIGHT: f32 = 42.0;
pub(crate) use pw_graph_core::{
    RELAY_SINK_NODE_NAME as RELAY_SINK_NAME, RELAY_SOURCE_NODE_NAME as RELAY_SOURCE_NAME,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum MeterState {
    #[default]
    Unavailable,
    Disabled,
    Waiting,
    Live,
    Demo,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct MeterReading {
    pub(crate) rms: f32,
    pub(crate) peak: f32,
    pub(crate) state: MeterState,
}
