use super::*;

impl PipewireDriver {
    pub(super) fn connect_locked(&mut self, src: PortId, dst: PortId) -> BackendResult<Link> {
        let output = self
            .graph
            .port(src)
            .cloned()
            .ok_or(GraphError::MissingPort(src))?;
        let input = self
            .graph
            .port(dst)
            .cloned()
            .ok_or(GraphError::MissingPort(dst))?;
        if let (Some(output_key), Some(input_key)) =
            (self.graph.port_key(src), self.graph.port_key(dst))
        {
            self.allow_blocked_connection(&output_key, &input_key);
        }
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
        if self
            .graph
            .links
            .values()
            .any(|link| link.output_port == src && link.input_port == dst)
        {
            return Err(GraphError::DuplicateConnection(src, dst).into());
        }

        let properties = pw::properties::properties! {
            "link.output.node" => native_node_id(output.node_id).to_string(),
            "link.output.port" => native_port_id(src).to_string(),
            "link.input.node" => native_node_id(input.node_id).to_string(),
            "link.input.port" => native_port_id(dst).to_string(),
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
            .find(|(_, link)| {
                link.output_port == native_port_id(src) && link.input_port == native_port_id(dst)
            })
            .map(|(id, _)| *id)
            .unwrap_or(proxy_id);
        self.rebuild_graph_locked()?;
        Ok(self
            .graph
            .link(LinkId(graph_id(link_id as u64)))
            .cloned()
            .unwrap_or(Link {
                id: LinkId(graph_id(link_id as u64)),
                output_port: src,
                input_port: dst,
            }))
    }

    pub(super) fn disconnect_locked(&mut self, link: LinkId) -> BackendResult<Link> {
        let existing = self
            .graph
            .link(link)
            .cloned()
            .ok_or(GraphError::MissingLink(link))?;
        self.block_connection(&existing);
        self.registry()?
            .destroy_global(native_link_id(link))
            .into_result()
            .map_err(|error| native_error("PipeWire link destruction", error))?;
        self.roundtrip_locked()?;
        self.rebuild_graph_locked()?;
        Ok(existing)
    }
}
