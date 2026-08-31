//! The UI's own state over the graph -- selection, viewport, filtering,
//! collapse and appearance -- and the projection that turns a backend graph
//! plus this state into a [`GraphSnapshot`].

use super::*;

pub(crate) struct UiGraphState {
    pub(crate) zoom: f32,
    pub(crate) pan: [f32; 2],
    pub(crate) node_text_scale: f32,
    pub(crate) sort_ports_by_name: bool,
    pub(crate) sort_ports_descending: bool,
    pub(crate) thumbnail_mode: bool,
    pub(crate) minimap_visible: bool,
    pub(crate) connect_mode: ConnectMode,
    pub(crate) media_filter: MediaFilter,
    pub(crate) search_query: String,
    pub(crate) relay_nodes_visible: bool,
    pub(crate) selected_nodes: BTreeSet<NodeId>,
    pub(crate) selected_links: BTreeSet<LinkId>,
    pub(crate) ids: SlintIdMap,
    pub(super) local_positions: BTreeMap<NodeId, [f32; 2]>,
    pub(super) local_appearances: BTreeMap<NodeId, NodeAppearance>,
}

impl UiGraphState {
    pub(crate) fn from_config(config: &AppConfig) -> Self {
        Self {
            zoom: config.zoom.clamp(0.35, 2.5),
            pan: [24.0, 24.0],
            node_text_scale: config.node_text_scale.clamp(0.8, 2.0),
            sort_ports_by_name: config.sort_type != "id",
            sort_ports_descending: config.sort_order == "descending",
            thumbnail_mode: config.thumbnail_view,
            minimap_visible: config.minimap_visible,
            connect_mode: ConnectMode::parse(&config.connect_mode),
            media_filter: MediaFilter::parse(&config.media_filter),
            search_query: config.graph_search.clone(),
            relay_nodes_visible: false,
            selected_nodes: BTreeSet::new(),
            selected_links: BTreeSet::new(),
            ids: SlintIdMap::default(),
            local_positions: BTreeMap::new(),
            local_appearances: BTreeMap::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn snapshot(&mut self, graph: &Graph, config: &AppConfig) -> GraphSnapshot {
        self.snapshot_with_meters(
            graph,
            config,
            &BTreeMap::new(),
            MeterState::Unavailable,
            &fully_capable_backend_profiles(graph),
        )
    }

    pub(crate) fn snapshot_with_meters(
        &mut self,
        graph: &Graph,
        config: &AppConfig,
        meters: &BTreeMap<NodeId, MeterReading>,
        meter_fallback: MeterState,
        backend_profiles: &BTreeMap<NodeId, NodeBackendProfile>,
    ) -> GraphSnapshot {
        self.ids.rebuild(graph);
        self.local_positions
            .retain(|id, _| graph.nodes.contains_key(id));
        self.local_appearances
            .retain(|id, _| graph.nodes.contains_key(id));

        let visible: BTreeSet<_> = graph
            .nodes
            .values()
            .filter(|node| self.relay_nodes_visible || !is_relay_node(node))
            .filter(|node| self.media_filter.matches_node(graph, node))
            .filter(|node| self.search_matches(graph, node))
            .map(|node| node.id)
            .collect();
        self.selected_nodes.retain(|id| visible.contains(id));
        self.selected_links
            .retain(|id| graph.links.contains_key(id));

        let appearances = configured_appearances(graph, config);
        let positions = self.effective_positions(graph, config);
        let mut pin_groups = HashMap::<PortId, i32>::new();
        let mut nodes = Vec::new();

        for node in graph
            .nodes
            .values()
            .filter(|node| visible.contains(&node.id))
        {
            let appearance = self
                .local_appearances
                .get(&node.id)
                .cloned()
                .unwrap_or_else(|| appearances.get(&node.id).cloned().unwrap_or_default());
            let ports = self.project_ports(graph, node);
            for port in &ports {
                for id in &port.ports {
                    pin_groups.insert(*id, port.pin_id);
                }
            }
            // Which controls exist is the backend's call, not a guess from the
            // node type: a Windows application session and a Windows endpoint
            // are both "audio nodes" but expose different controls.
            let audio = backend_profiles.get(&node.id).copied().unwrap_or_default();
            let has_audio = node.ports.iter().any(|id| {
                graph
                    .port(*id)
                    .is_some_and(|port| port.port_type == PortType::Audio)
            });
            let has_audio_controls = audio.capabilities.has_any_control() && has_audio;
            let has_meter = audio.capabilities.has_any_meter() && has_audio;
            let has_audio_panel = has_audio_controls || has_meter;
            let collapsed = appearance.collapsed;
            let thumbnail = self.thumbnail_mode;
            let height = node_height(thumbnail, collapsed, has_audio_panel, ports.len());
            nodes.push(NodeView {
                id: self.ids.node(node.id).unwrap_or_default(),
                node_id: node.id,
                title: appearance
                    .custom_name
                    .clone()
                    .unwrap_or_else(|| node.name.clone()),
                node_type: node.node_type,
                position: positions.get(&node.id).copied().unwrap_or(node.position),
                width: NODE_WIDTH,
                height,
                selected: self.selected_nodes.contains(&node.id),
                collapsed,
                thumbnail,
                font_scale: self.node_text_scale,
                appearance,
                has_audio_controls,
                has_audio_panel,
                connectable: audio.connectable,
                audio,
                meter: meters
                    .get(&node.id)
                    .copied()
                    .unwrap_or_else(|| MeterReading {
                        state: if has_meter {
                            meter_fallback
                        } else {
                            MeterState::Unavailable
                        },
                        ..MeterReading::default()
                    }),
                ports,
            });
        }

        let links = if !self.thumbnail_mode {
            graph
                .links
                .values()
                .filter_map(|link| {
                    let output = graph.port(link.output_port)?;
                    let input = graph.port(link.input_port)?;
                    (visible.contains(&output.node_id) && visible.contains(&input.node_id))
                        .then_some(LinkView {
                            id: self.ids.link(link.id)?,
                            link_id: link.id,
                            start_pin_id: *pin_groups.get(&link.output_port)?,
                            end_pin_id: *pin_groups.get(&link.input_port)?,
                            color: link_color(output.port_type, output.direction, &output.name),
                            selected: self.selected_links.contains(&link.id),
                        })
                })
                .collect()
        } else {
            Vec::new()
        };

        GraphSnapshot { nodes, links }
    }

    pub(crate) fn select_node(&mut self, node_id: i32, shift: bool) {
        let Some(node_id) = self.ids.node_id(node_id) else {
            return;
        };
        if !shift {
            self.selected_nodes.clear();
            self.selected_links.clear();
        }
        if shift && !self.selected_nodes.insert(node_id) {
            self.selected_nodes.remove(&node_id);
        } else {
            self.selected_nodes.insert(node_id);
        }
    }

    pub(crate) fn select_link(&mut self, link_id: i32, shift: bool) {
        let Some(link_id) = self.ids.link_id(link_id) else {
            return;
        };
        if !shift {
            self.selected_nodes.clear();
            self.selected_links.clear();
        }
        if shift && !self.selected_links.insert(link_id) {
            self.selected_links.remove(&link_id);
        } else {
            self.selected_links.insert(link_id);
        }
    }

    pub(crate) fn clear_selection(&mut self) {
        self.selected_nodes.clear();
        self.selected_links.clear();
    }

    pub(crate) fn select_box(
        &mut self,
        snapshot: &GraphSnapshot,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        shift: bool,
    ) {
        if !shift {
            self.clear_selection();
        }
        for node in &snapshot.nodes {
            if intersects(node.position, [node.width, node.height], x, y, w, h) {
                self.selected_nodes.insert(node.node_id);
            }
        }
        for link in &snapshot.links {
            let endpoint_in_box = |pin_id: i32| {
                snapshot
                    .nodes
                    .iter()
                    .find_map(|node| {
                        node.ports
                            .iter()
                            .enumerate()
                            .find(|(_, port)| port.pin_id == pin_id)
                            .map(|(index, port)| {
                                if node.collapsed {
                                    (
                                        if port.direction == Direction::Source {
                                            node.position[0] + node.width
                                        } else {
                                            node.position[0]
                                        },
                                        node.position[1] + NODE_HEADER_HEIGHT / 2.0,
                                    )
                                } else {
                                    let (offset_x, offset_y) = crate::canvas::pin_offset(
                                        node.width,
                                        index,
                                        node.has_audio_panel,
                                        port.direction != Direction::Sink,
                                    );
                                    (node.position[0] + offset_x, node.position[1] + offset_y)
                                }
                            })
                    })
                    .is_some_and(|point| point_in_box(point, x, y, w, h))
            };
            if endpoint_in_box(link.start_pin_id) || endpoint_in_box(link.end_pin_id) {
                self.selected_links.insert(link.link_id);
            }
        }
    }

    pub(crate) fn move_selected(
        &mut self,
        node_id: i32,
        delta_x: f32,
        delta_y: f32,
        snapshot: &GraphSnapshot,
    ) {
        let Some(dragged) = self.ids.node_id(node_id) else {
            return;
        };
        let selected: BTreeSet<_> = if self.selected_nodes.contains(&dragged) {
            self.selected_nodes.clone()
        } else {
            BTreeSet::from([dragged])
        };
        for node in snapshot
            .nodes
            .iter()
            .filter(|node| selected.contains(&node.node_id))
        {
            self.local_positions.insert(
                node.node_id,
                [node.position[0] + delta_x, node.position[1] + delta_y],
            );
        }
    }

    #[allow(dead_code)]
    pub(crate) fn set_local_position(&mut self, node_id: i32, x: f32, y: f32) {
        if let Some(node_id) = self.ids.node_id(node_id) {
            self.local_positions.insert(node_id, [x, y]);
        }
    }

    /// Adopt positions committed to the backend after an undoable command.
    /// The normal projection starts from persisted positions, but a successful
    /// move or arrange command makes the backend the new source of truth.
    pub(crate) fn adopt_backend_positions(&mut self, graph: &Graph) {
        self.local_positions = graph
            .nodes
            .values()
            .map(|node| (node.id, node.position))
            .collect();
    }

    pub(crate) fn toggle_local_collapse(&mut self, node_id: i32, snapshot: &GraphSnapshot) {
        let Some(node_id) = self.ids.node_id(node_id) else {
            return;
        };
        let Some(node) = snapshot.nodes.iter().find(|node| node.node_id == node_id) else {
            return;
        };
        let mut appearance = node.appearance.clone();
        appearance.collapsed = !appearance.collapsed;
        self.local_appearances.insert(node_id, appearance);
    }

    pub(crate) fn local_appearance(
        &self,
        node_id: NodeId,
        snapshot: &GraphSnapshot,
    ) -> Option<NodeAppearance> {
        snapshot
            .nodes
            .iter()
            .find(|node| node.node_id == node_id)
            .map(|node| {
                self.local_appearances
                    .get(&node_id)
                    .cloned()
                    .unwrap_or_else(|| node.appearance.clone())
            })
    }

    pub(crate) fn set_local_appearance(&mut self, node_id: NodeId, appearance: NodeAppearance) {
        self.local_appearances.insert(node_id, appearance);
    }

    /// Write the effective Slint layout and node appearance into the shared
    /// application configuration using the same stable keys as the desktop UI.
    pub(crate) fn write_to_config(&self, graph: &Graph, config: &mut AppConfig) {
        let configured_appearances = configured_appearances(graph, config);
        let effective_positions = self.effective_positions(graph, config);
        let mut key_counts = BTreeMap::<String, usize>::new();
        for node in graph.nodes.values() {
            *key_counts.entry(node_layout_key(node)).or_default() += 1;
        }

        config.node_positions = graph
            .nodes
            .values()
            .map(|node| {
                let position = effective_positions
                    .get(&node.id)
                    .copied()
                    .unwrap_or(node.position);
                (node.id.0.to_string(), position)
            })
            .collect();
        config.node_positions_by_name = graph
            .nodes
            .values()
            .filter_map(|node| {
                let key = node_layout_key(node);
                (key_counts.get(&key) == Some(&1)).then(|| {
                    let position = effective_positions
                        .get(&node.id)
                        .copied()
                        .unwrap_or(node.position);
                    (key, position)
                })
            })
            .collect();
        config.node_view_by_name = graph
            .nodes
            .values()
            .filter_map(|node| {
                let key = node_layout_key(node);
                if key_counts.get(&key) != Some(&1) {
                    return None;
                }
                let appearance = self
                    .local_appearances
                    .get(&node.id)
                    .cloned()
                    .or_else(|| configured_appearances.get(&node.id).cloned())
                    .unwrap_or_default();
                (appearance != NodeAppearance::default()).then_some((key, appearance))
            })
            .collect();
    }

    pub(super) fn effective_positions(
        &self,
        graph: &Graph,
        config: &AppConfig,
    ) -> BTreeMap<NodeId, [f32; 2]> {
        let mut positions = configured_positions(graph, config);
        positions.extend(
            self.local_positions
                .iter()
                .map(|(id, position)| (*id, *position)),
        );
        positions
    }

    pub(crate) fn visible_counts(&self, snapshot: &GraphSnapshot) -> (usize, usize, usize) {
        (
            snapshot.nodes.len(),
            snapshot
                .nodes
                .iter()
                .flat_map(|node| &node.ports)
                .map(|port| port.ports.len())
                .sum(),
            snapshot.links.len(),
        )
    }

    /// Return the compatible output-to-input pairs for an Easy-mode drag.
    /// Try the visual drag direction first, then reverse it so effects and
    /// relay endpoints work without requiring users to know which side owns
    /// the source ports.
    pub(crate) fn matching_port_pairs(
        &self,
        graph: &Graph,
        source: NodeId,
        target: NodeId,
    ) -> Vec<(PortId, PortId)> {
        let Some(source) = graph.node(source) else {
            return Vec::new();
        };
        let Some(target) = graph.node(target) else {
            return Vec::new();
        };
        let source_ports = self.ordered_ports(graph, source);
        let target_ports = self.ordered_ports(graph, target);
        let source_outputs = source_ports
            .iter()
            .copied()
            .filter(|port| port.direction == Direction::Source)
            .collect::<Vec<_>>();
        let target_inputs = target_ports
            .iter()
            .copied()
            .filter(|port| port.direction == Direction::Sink)
            .collect::<Vec<_>>();
        let forward = pair_ports(&source_outputs, &target_inputs);
        if !forward.is_empty() {
            return forward;
        }
        let target_outputs = target_ports
            .iter()
            .copied()
            .filter(|port| port.direction == Direction::Source)
            .collect::<Vec<_>>();
        let source_inputs = source_ports
            .iter()
            .copied()
            .filter(|port| port.direction == Direction::Sink)
            .collect::<Vec<_>>();
        pair_ports(&target_outputs, &source_inputs)
    }

    /// The rendered pin a pin id belongs to. In Easy mode one pin stands for
    /// a whole channel group (`capture_FL` + `capture_FR`), so a gesture that
    /// starts on it means "connect the group", not "connect one port".
    pub(crate) fn port_group(&self, graph: &Graph, pin_id: i32) -> Option<PortGroupView> {
        let port = self.ids.port_id(pin_id)?;
        let node = graph.node(graph.port(port)?.node_id)?;
        self.project_ports(graph, node)
            .into_iter()
            .find(|group| group.pin_id == pin_id)
    }

    /// The output-to-input pairs for a drag between two rendered pins. The
    /// channels are matched inside the two groups, so left stays left and
    /// right stays right whichever way the drag was made.
    pub(crate) fn matching_pin_pairs(
        &self,
        graph: &Graph,
        source_pin: i32,
        target_pin: i32,
    ) -> Vec<(PortId, PortId)> {
        let Some(source) = self.port_group(graph, source_pin) else {
            return Vec::new();
        };
        let Some(target) = self.port_group(graph, target_pin) else {
            return Vec::new();
        };
        let (outputs, inputs) = match (source.direction, target.direction) {
            (Direction::Source, Direction::Sink) => (&source.ports, &target.ports),
            (Direction::Sink, Direction::Source) => (&target.ports, &source.ports),
            _ => return Vec::new(),
        };
        let resolve = |ports: &Vec<PortId>| {
            ports
                .iter()
                .filter_map(|port| graph.port(*port))
                .collect::<Vec<_>>()
        };
        pair_ports(&resolve(outputs), &resolve(inputs))
    }

    /// The pairs for a drag that started on a pin and was released over a
    /// card. Only the dragged group's channels are connected, matched against
    /// whichever ports of that card face the other way.
    pub(crate) fn matching_group_to_node_pairs(
        &self,
        graph: &Graph,
        source_pin: i32,
        target: NodeId,
    ) -> Vec<(PortId, PortId)> {
        let Some(group) = self.port_group(graph, source_pin) else {
            return Vec::new();
        };
        let Some(target) = graph.node(target) else {
            return Vec::new();
        };
        let group_ports = group
            .ports
            .iter()
            .filter_map(|port| graph.port(*port))
            .collect::<Vec<_>>();
        let facing = |direction: Direction| {
            self.ordered_ports(graph, target)
                .into_iter()
                .filter(|port| port.direction == direction)
                .collect::<Vec<_>>()
        };
        match group.direction {
            Direction::Source => pair_ports(&group_ports, &facing(Direction::Sink)),
            Direction::Sink => pair_ports(&facing(Direction::Source), &group_ports),
        }
    }

    pub(crate) fn node_at(
        &self,
        snapshot: &GraphSnapshot,
        x: f32,
        y: f32,
        exclude: NodeId,
    ) -> Option<NodeId> {
        self.node_at_with_margin(snapshot, x, y, exclude, 0.0)
    }

    pub(crate) fn node_at_with_margin(
        &self,
        snapshot: &GraphSnapshot,
        x: f32,
        y: f32,
        exclude: NodeId,
        margin: f32,
    ) -> Option<NodeId> {
        snapshot
            .nodes
            .iter()
            .find(|node| {
                node.node_id != exclude
                    && x >= node.position[0] - margin
                    && x <= node.position[0] + node.width + margin
                    && y >= node.position[1] - margin
                    && y <= node.position[1] + node.height + margin
            })
            .map(|node| node.node_id)
    }

    pub(super) fn project_ports(&self, graph: &Graph, node: &Node) -> Vec<PortGroupView> {
        let ports = self.ordered_ports(graph, node);

        let mut groups: Vec<PortGroupView> = Vec::new();
        let mut group_index = HashMap::<(Direction, PortType, String), usize>::new();
        for port in ports {
            let key = (self.connect_mode == ConnectMode::Easy && port.port_type == PortType::Audio)
                .then(|| channel_base_name(port).map(|base| (port.direction, port.port_type, base)))
                .flatten();
            if let Some(index) = key.as_ref().and_then(|key| group_index.get(key).copied()) {
                groups[index].ports.push(port.id);
                if groups[index].ports.len() == 2 {
                    groups[index].label = key.as_ref().map(|key| key.2.clone()).unwrap_or_default();
                }
                continue;
            }
            let pin_id = self.ids.port(port.id).unwrap_or_default();
            let index = groups.len();
            groups.push(PortGroupView {
                pin_id,
                ports: vec![port.id],
                label: port.name.clone(),
                direction: port.direction,
                port_type: port.port_type,
                color: port_color(port.port_type, port.direction, &port.name),
            });
            if let Some(key) = key {
                group_index.insert(key, index);
            }
        }
        groups
    }

    pub(super) fn ordered_ports<'a>(&self, graph: &'a Graph, node: &Node) -> Vec<&'a Port> {
        let mut ports: Vec<_> = node
            .ports
            .iter()
            .filter_map(|id| graph.port(*id))
            .filter(|port| self.media_filter.matches_port_type(port.port_type))
            .filter(|port| self.search_matches_port(node, port))
            .collect();
        if self.sort_ports_by_name {
            ports.sort_by_key(|port| port.name.to_ascii_lowercase());
        } else {
            ports.sort_by_key(|port| port.id);
        }
        if self.sort_ports_descending {
            ports.reverse();
        }
        ports
    }

    pub(super) fn search_matches(&self, graph: &Graph, node: &Node) -> bool {
        let query = self.search_query.trim().to_ascii_lowercase();
        query.is_empty()
            || node.name.to_ascii_lowercase().contains(&query)
            || node.ports.iter().any(|port_id| {
                graph
                    .port(*port_id)
                    .is_some_and(|port| port.name.to_ascii_lowercase().contains(&query))
            })
    }

    pub(super) fn search_matches_port(&self, node: &Node, port: &Port) -> bool {
        let query = self.search_query.trim().to_ascii_lowercase();
        query.is_empty()
            || node.name.to_ascii_lowercase().contains(&query)
            || port.name.to_ascii_lowercase().contains(&query)
    }
}
