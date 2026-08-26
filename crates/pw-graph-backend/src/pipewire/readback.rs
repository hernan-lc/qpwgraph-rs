//! Reading node volume and mute back from PipeWire.
//!
//! Writing a control is a fire-and-forget `set_param`, but reading one means
//! binding the node proxy, asking for its `Props`, and waiting for the reply.
//! Until this existed the driver only knew the values it had written itself,
//! so a level set anywhere else -- pavucontrol, a media key, another app --
//! was reported as unknown.
//!
//! The read happens once per graph rebuild rather than per query: every node
//! is bound, every request is queued, and a single roundtrip collects all the
//! replies. Rebuilds are event-driven, so this is not a poll.

use super::*;
use crate::api::spa_volume_to_ui_volume;
use pw::spa::pod::deserialize::PodDeserializer;

/// One node's controls as PipeWire reported them.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct PropsReading {
    pub(super) volume: Option<f32>,
    pub(super) muted: Option<bool>,
}

impl PropsReading {
    fn is_empty(&self) -> bool {
        self.volume.is_none() && self.muted.is_none()
    }
}

/// Pull `volume`, `channelVolumes` and `mute` out of a `Props` object pod.
///
/// `channelVolumes` wins when present: a device with per-channel gain leaves
/// the single `volume` property at unity, so trusting it would report full
/// volume for a node the user has turned down.
pub(super) fn parse_props(bytes: &[u8]) -> PropsReading {
    let mut reading = PropsReading::default();
    let Ok((_, Value::Object(object))) = PodDeserializer::deserialize_any_from(bytes) else {
        return reading;
    };
    let mut channel_volume = None;
    for property in object.properties {
        match property.key {
            pw::spa::sys::SPA_PROP_volume => {
                if let Value::Float(value) = property.value {
                    reading.volume = Some(spa_volume_to_ui_volume(value));
                }
            }
            pw::spa::sys::SPA_PROP_mute => {
                if let Value::Bool(value) = property.value {
                    reading.muted = Some(value);
                }
            }
            pw::spa::sys::SPA_PROP_channelVolumes => {
                if let Value::ValueArray(pw::spa::pod::ValueArray::Float(values)) = property.value {
                    // Channels are normally uniform; the loudest is the one a
                    // single fader should show.
                    channel_volume = values
                        .iter()
                        .copied()
                        .fold(None::<f32>, |best, value| {
                            Some(best.map_or(value, |best| best.max(value)))
                        })
                        .map(spa_volume_to_ui_volume);
                }
            }
            _ => {}
        }
    }
    if let Some(value) = channel_volume {
        reading.volume = Some(value);
    }
    reading
}

impl PipewireDriver {
    /// Refresh the cached controls for every node in the current graph.
    ///
    /// Called from the graph rebuild, which already holds the loop lock. A
    /// node that does not answer keeps whatever was known before, so a
    /// transient failure does not blank a card that was reading fine.
    pub(super) fn read_node_controls_locked(&mut self) {
        let Ok(registry) = self.registry() else {
            return;
        };
        let nodes: Vec<NodeId> = self.graph.nodes.keys().copied().collect();
        if nodes.is_empty() {
            return;
        }

        // Proxies and listeners have to outlive the roundtrip, so they are
        // held here until every reply has landed.
        let mut proxies = Vec::with_capacity(nodes.len());
        let mut listeners = Vec::with_capacity(nodes.len());
        let readings: Rc<RefCell<BTreeMap<NodeId, PropsReading>>> =
            Rc::new(RefCell::new(BTreeMap::new()));

        for node_id in nodes {
            let object = pw::registry::GlobalObject {
                id: native_node_id(node_id),
                permissions: pw::permissions::PermissionFlags::empty(),
                type_: pw::types::ObjectType::Node,
                version: NODE_INTERFACE_VERSION,
                props: None::<pw::properties::Properties>,
            };
            let Ok(proxy) = registry.bind::<pw::node::Node, _>(&object) else {
                continue;
            };
            let sink = readings.clone();
            let listener = proxy
                .add_listener_local()
                .param(move |_seq, id, _index, _next, param| {
                    if id != ParamType::Props {
                        return;
                    }
                    let Some(param) = param else {
                        return;
                    };
                    let reading = parse_props(param.as_bytes());
                    if !reading.is_empty() {
                        sink.borrow_mut().insert(node_id, reading);
                    }
                })
                .register();
            proxy.enum_params(0, Some(ParamType::Props), 0, u32::MAX);
            proxies.push(proxy);
            listeners.push(listener);
        }

        // One roundtrip for the whole graph rather than one per node.
        let _ = self.roundtrip_locked();
        drop(listeners);
        drop(proxies);

        for (node_id, reading) in readings.borrow().iter() {
            let control = self.audio_controls.entry(*node_id).or_default();
            if let Some(volume) = reading.volume {
                control.volume = volume.clamp(0.0, PIPEWIRE_MAX_VOLUME);
            }
            if let Some(muted) = reading.muted {
                control.muted = muted;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pod_that_is_not_a_props_object_reads_as_nothing() {
        assert!(parse_props(&[]).is_empty());
        assert!(parse_props(&[0xff, 0x00, 0x11]).is_empty());
    }
}
