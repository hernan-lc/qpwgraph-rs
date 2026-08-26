use super::*;

impl PipewireDriver {
    pub(super) fn set_node_props_locked(
        &self,
        node: NodeId,
        properties: Vec<pw::spa::pod::Property>,
    ) -> BackendResult<()> {
        let object = pw::registry::GlobalObject {
            id: native_node_id(node),
            permissions: pw::permissions::PermissionFlags::empty(),
            type_: pw::types::ObjectType::Node,
            // pipewire-rs keeps ObjectType::client_version private; version 3
            // is the public node interface version used by pipewire 0.8.
            version: NODE_INTERFACE_VERSION,
            props: None::<pw::properties::Properties>,
        };
        let proxy = self
            .registry()?
            .bind::<pw::node::Node, _>(&object)
            .map_err(|error| native_error("PipeWire node binding", error))?;
        let value = pw::spa::pod::Value::Object(pw::spa::pod::Object {
            type_: pw::spa::utils::SpaTypes::ObjectParamProps.as_raw(),
            id: ParamType::Props.as_raw(),
            properties,
        });
        let pod_bytes = PodSerializer::serialize(Cursor::new(Vec::new()), &value)
            .map_err(|error| native_error("PipeWire node properties serialization", error))?
            .0
            .into_inner();
        let pod = Pod::from_bytes(&pod_bytes).ok_or_else(|| {
            BackendError::Native("could not serialize PipeWire node properties".into())
        })?;
        proxy.set_param(ParamType::Props, 0, pod);
        drop(proxy);
        self.roundtrip_locked()
    }

    pub(super) fn set_node_mute_locked(&mut self, node: NodeId, muted: bool) -> BackendResult<()> {
        self.set_node_props_locked(
            node,
            vec![pw::spa::pod::Property::new(
                pw::spa::sys::SPA_PROP_mute,
                pw::spa::pod::Value::Bool(muted),
            )],
        )?;
        self.audio_controls.entry(node).or_default().muted = muted;
        Ok(())
    }

    pub(super) fn set_node_volume_locked(
        &mut self,
        node: NodeId,
        volume: f32,
    ) -> BackendResult<()> {
        let volume = volume.clamp(0.0, 1.5);
        let spa_volume = ui_volume_to_spa_volume(volume);
        self.set_node_props_locked(
            node,
            vec![pw::spa::pod::Property::new(
                pw::spa::sys::SPA_PROP_volume,
                pw::spa::pod::Value::Float(spa_volume),
            )],
        )?;
        self.audio_controls.entry(node).or_default().volume = volume;
        Ok(())
    }
}
