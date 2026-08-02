use super::QpwgraphApp;
use pw_graph_command::{
    ConnectCommand, ConnectManyCommand, DisconnectAllCommand, DisconnectCommand,
    DisconnectManyCommand, MoveNodesCommand,
};
use pw_graph_core::{LinkId, NodeId};
use pw_graph_ui::CanvasAction;

impl QpwgraphApp {
    pub(super) fn handle_canvas_actions(&mut self, actions: Vec<CanvasAction>) {
        for action in actions {
            match action {
                CanvasAction::Connect { output, input } => {
                    let stable_pair = self
                        .driver
                        .graph()
                        .port_key(output)
                        .zip(self.driver.graph().port_key(input));
                    let command = stable_pair
                        .clone()
                        .map(|(output, input)| Box::new(ConnectCommand::from_keys(output, input)))
                        .unwrap_or_else(|| Box::new(ConnectCommand::new(output, input)));
                    match self.commands.execute(command, self.driver.as_mut()) {
                        Ok(()) => {
                            if let Some((output_key, input_key)) = stable_pair {
                                if let (Some(output), Some(input)) = (
                                    self.driver.graph().resolve_port_key(&output_key),
                                    self.driver.graph().resolve_port_key(&input_key),
                                ) {
                                    self.patchbay.add_graph_connection(
                                        self.driver.graph(),
                                        output,
                                        input,
                                        self.config.patchbay_auto_pin,
                                    );
                                }
                            }
                            self.status = self.tf(
                                "status.connected",
                                &[("output", output.to_string()), ("input", input.to_string())],
                            );
                        }
                        Err(error) => {
                            self.status =
                                self.tf("status.connect_failed", &[("error", error.to_string())])
                        }
                    }
                }
                CanvasAction::ConnectMany { pairs } => self.connect_many(pairs),
                CanvasAction::Disconnect { link } => self.disconnect(link),
                CanvasAction::DisconnectMany { links } => self.disconnect_many(links),
                CanvasAction::DisconnectNode { node } => self.disconnect_node(node),
                CanvasAction::ArrangeNodes { nodes } => self.arrange_selected_nodes(nodes),
                CanvasAction::MoveNode { node, position } => {
                    let _ = self.driver.set_node_position(node, position);
                }
                CanvasAction::CommitNodeMove { before, after } => {
                    let _ = self.commands.execute(
                        Box::new(MoveNodesCommand::new(before, after)),
                        self.driver.as_mut(),
                    );
                }
            }
        }
    }

    fn connect_many(&mut self, pairs: Vec<(pw_graph_core::PortId, pw_graph_core::PortId)>) {
        if pairs.is_empty() {
            return;
        }
        let count = pairs.len();
        let stable_pairs: Vec<_> = pairs
            .iter()
            .filter_map(|(output, input)| {
                self.driver
                    .graph()
                    .port_key(*output)
                    .zip(self.driver.graph().port_key(*input))
            })
            .collect();
        match self.commands.execute(
            Box::new(ConnectManyCommand::with_keys(
                pairs.clone(),
                stable_pairs.clone(),
            )),
            self.driver.as_mut(),
        ) {
            Ok(()) => {
                for (output_key, input_key) in stable_pairs {
                    if let (Some(output), Some(input)) = (
                        self.driver.graph().resolve_port_key(&output_key),
                        self.driver.graph().resolve_port_key(&input_key),
                    ) {
                        self.patchbay.add_graph_connection(
                            self.driver.graph(),
                            output,
                            input,
                            self.config.patchbay_auto_pin,
                        );
                    }
                }
                self.status = self.tf("status.connected_many", &[("count", count.to_string())]);
            }
            Err(error) => {
                self.status = self.tf("status.connect_failed", &[("error", error.to_string())]);
            }
        }
    }

    fn disconnect_node(&mut self, node: NodeId) {
        let links: Vec<_> = self
            .driver
            .graph()
            .links
            .values()
            .filter(|link| {
                self.driver
                    .graph()
                    .port(link.output_port)
                    .is_some_and(|port| port.node_id == node)
                    || self
                        .driver
                        .graph()
                        .port(link.input_port)
                        .is_some_and(|port| port.node_id == node)
            })
            .cloned()
            .collect();
        let ids: Vec<_> = links.iter().map(|link| link.id).collect();
        if ids.is_empty() {
            return;
        }
        let stable_pairs: Vec<_> = links
            .iter()
            .filter_map(|link| {
                self.driver
                    .graph()
                    .port_key(link.output_port)
                    .zip(self.driver.graph().port_key(link.input_port))
            })
            .collect();
        let count = ids.len();
        match self.commands.execute(
            Box::new(DisconnectManyCommand::from_links(
                self.driver.graph(),
                links.clone(),
            )),
            self.driver.as_mut(),
        ) {
            Ok(()) => {
                for (output, input) in stable_pairs {
                    self.patchbay.remove_stable_connection(&output, &input);
                }
                self.status = self.tf("status.disconnected_all", &[("count", count.to_string())]);
            }
            Err(error) => {
                self.status = self.tf("status.disconnect_failed", &[("error", error.to_string())]);
            }
        }
    }

    fn arrange_selected_nodes(&mut self, nodes: Vec<NodeId>) {
        let defaults = self.driver.graph().default_node_positions();
        let before: Vec<_> = nodes
            .iter()
            .filter_map(|node| {
                self.driver
                    .graph()
                    .node(*node)
                    .map(|item| (*node, item.position))
            })
            .collect();
        let after: Vec<_> = before
            .iter()
            .map(|(node, current)| (*node, defaults.get(node).copied().unwrap_or(*current)))
            .collect();
        if before == after {
            return;
        }
        if self
            .commands
            .execute(
                Box::new(MoveNodesCommand::new(before, after)),
                self.driver.as_mut(),
            )
            .is_ok()
        {
            self.status = self.tf("status.arranged", &[("count", nodes.len().to_string())]);
        }
    }

    pub(crate) fn disconnect(&mut self, link: LinkId) {
        let Some(existing) = self.driver.graph().link(link).cloned() else {
            return;
        };
        let stable_pair = self
            .driver
            .graph()
            .port_key(existing.output_port)
            .zip(self.driver.graph().port_key(existing.input_port));
        match self.commands.execute(
            Box::new(DisconnectCommand::from_link(
                self.driver.graph(),
                existing.clone(),
            )),
            self.driver.as_mut(),
        ) {
            Ok(()) => {
                if let Some((output, input)) = stable_pair {
                    self.patchbay.remove_stable_connection(&output, &input);
                } else {
                    self.patchbay
                        .remove_connection(existing.output_port, existing.input_port);
                }
                self.status = self.tf("status.disconnected", &[("link", link.to_string())]);
            }
            Err(error) => {
                self.status = self.tf("status.disconnect_failed", &[("error", error.to_string())])
            }
        }
    }

    pub(crate) fn disconnect_many(&mut self, link_ids: Vec<LinkId>) {
        if link_ids.is_empty() {
            return;
        }
        let links: Vec<_> = link_ids
            .iter()
            .filter_map(|link_id| self.driver.graph().link(*link_id).cloned())
            .collect();
        if links.is_empty() {
            return;
        }
        let stable_pairs: Vec<_> = links
            .iter()
            .filter_map(|link| {
                self.driver
                    .graph()
                    .port_key(link.output_port)
                    .zip(self.driver.graph().port_key(link.input_port))
            })
            .collect();
        let count = links.len();
        match self.commands.execute(
            Box::new(DisconnectManyCommand::from_links(
                self.driver.graph(),
                links.clone(),
            )),
            self.driver.as_mut(),
        ) {
            Ok(()) => {
                for (output, input) in stable_pairs {
                    self.patchbay.remove_stable_connection(&output, &input);
                }
                self.canvas.clear_selected_link();
                self.status = self.tf("status.disconnected_all", &[("count", count.to_string())]);
            }
            Err(error) => {
                self.status = self.tf("status.disconnect_failed", &[("error", error.to_string())]);
            }
        }
    }

    pub(crate) fn disconnect_all(&mut self) {
        let count = self.driver.graph().links.len();
        if count == 0 {
            self.status = self.t("status.no_links");
            return;
        }

        match self
            .commands
            .execute(Box::new(DisconnectAllCommand::new()), self.driver.as_mut())
        {
            Ok(()) => {
                self.patchbay.connections.clear();
                self.canvas.clear_selected_link();
                self.status = self.tf("status.disconnected_all", &[("count", count.to_string())]);
            }
            Err(error) => {
                self.status = self.tf("status.disconnect_failed", &[("error", error.to_string())])
            }
        }
    }

    pub(crate) fn undo(&mut self) {
        match self.commands.undo(self.driver.as_mut()) {
            Ok(true) => self.status = self.t("status.undo_complete"),
            Ok(false) => self.status = self.t("status.nothing_to_undo"),
            Err(error) => {
                self.status = self.tf("status.undo_failed", &[("error", error.to_string())])
            }
        }
    }

    pub(crate) fn redo(&mut self) {
        match self.commands.redo(self.driver.as_mut()) {
            Ok(true) => self.status = self.t("status.redo_complete"),
            Ok(false) => self.status = self.t("status.nothing_to_redo"),
            Err(error) => {
                self.status = self.tf("status.redo_failed", &[("error", error.to_string())])
            }
        }
    }

    pub(crate) fn arrange_nodes(&mut self) {
        let defaults = self.driver.graph().default_node_positions();
        let before: Vec<_> = self
            .driver
            .graph()
            .nodes
            .values()
            .map(|node| (node.id, node.position))
            .collect();
        let after: Vec<_> = before
            .iter()
            .map(|(node, position)| (*node, defaults.get(node).copied().unwrap_or(*position)))
            .collect();
        if before != after
            && self
                .commands
                .execute(
                    Box::new(MoveNodesCommand::new(before, after)),
                    self.driver.as_mut(),
                )
                .is_ok()
        {
            self.status = self.tf(
                "status.arranged",
                &[("count", self.driver.graph().nodes.len().to_string())],
            );
        }
    }
}
