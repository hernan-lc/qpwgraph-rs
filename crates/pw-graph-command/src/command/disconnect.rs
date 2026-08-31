//! Removing links: one, a chosen set, or every mutable link.

use super::*;

pub struct DisconnectCommand {
    pub(super) link: Option<Link>,
    pub(super) link_id: LinkId,
    pub(super) keys: Option<(PortKey, PortKey)>,
    pub(super) removed: bool,
}

/// Disconnect every live link as one undoable operation.
pub struct DisconnectAllCommand {
    pub(super) links: Vec<Link>,
    pub(super) keys: Vec<(PortKey, PortKey)>,
    pub(super) removed_keys: Vec<(PortKey, PortKey)>,
}

pub struct DisconnectManyCommand {
    pub(super) link_ids: Vec<LinkId>,
    pub(super) keys: Vec<(PortKey, PortKey)>,
    pub(super) removed_keys: Vec<(PortKey, PortKey)>,
    pub(super) links: Vec<Link>,
}

impl DisconnectManyCommand {
    pub fn new(link_ids: Vec<LinkId>) -> Self {
        Self {
            link_ids,
            keys: Vec::new(),
            removed_keys: Vec::new(),
            links: Vec::new(),
        }
    }

    pub fn from_links(graph: &Graph, links: Vec<Link>) -> Self {
        let keys = links
            .iter()
            .filter_map(|link| stable_pair(graph, link.output_port, link.input_port))
            .collect();
        Self {
            link_ids: links.iter().map(|link| link.id).collect(),
            keys,
            removed_keys: Vec::new(),
            links,
        }
    }
}

impl Command for DisconnectManyCommand {
    fn name(&self) -> &'static str {
        "Disconnect group"
    }

    fn description(&self) -> String {
        format!("{} ({} links)", self.name(), self.keys.len())
    }

    fn execute(&mut self, driver: &mut dyn GraphDriver) -> Result<(), CommandError> {
        driver.refresh()?;
        if self.keys.is_empty() {
            self.keys = self
                .link_ids
                .iter()
                .filter_map(|id| driver.graph().link(*id))
                .filter_map(|link| stable_pair(driver.graph(), link.output_port, link.input_port))
                .collect();
        }
        self.links = disconnect_keys(driver, self.name(), &self.keys, &mut self.removed_keys)?;
        Ok(())
    }

    fn undo(&mut self, driver: &mut dyn GraphDriver) -> Result<(), CommandError> {
        self.links = restore_keys(driver, self.name(), &self.removed_keys)?;
        Ok(())
    }
}

impl DisconnectAllCommand {
    pub fn new() -> Self {
        Self {
            links: Vec::new(),
            keys: Vec::new(),
            removed_keys: Vec::new(),
        }
    }
}

impl Default for DisconnectAllCommand {
    fn default() -> Self {
        Self::new()
    }
}

impl Command for DisconnectAllCommand {
    fn name(&self) -> &'static str {
        "Disconnect all"
    }

    fn description(&self) -> String {
        format!("{} ({} links)", self.name(), self.keys.len())
    }

    fn execute(&mut self, driver: &mut dyn GraphDriver) -> Result<(), CommandError> {
        driver.refresh()?;
        self.keys = driver
            .graph()
            .links
            .values()
            .filter(|link| driver.is_link_mutable(link.id))
            .filter_map(|link| stable_pair(driver.graph(), link.output_port, link.input_port))
            .collect();
        self.links = disconnect_keys(driver, self.name(), &self.keys, &mut self.removed_keys)?;
        Ok(())
    }

    fn undo(&mut self, driver: &mut dyn GraphDriver) -> Result<(), CommandError> {
        self.links = restore_keys(driver, self.name(), &self.removed_keys)?;
        Ok(())
    }
}

impl DisconnectCommand {
    pub fn new(link_id: LinkId) -> Self {
        Self {
            link: None,
            link_id,
            keys: None,
            removed: false,
        }
    }

    pub fn from_link(graph: &Graph, link: Link) -> Self {
        Self {
            link_id: link.id,
            keys: stable_pair(graph, link.output_port, link.input_port),
            link: Some(link),
            removed: false,
        }
    }
}

impl Command for DisconnectCommand {
    fn name(&self) -> &'static str {
        "Disconnect"
    }

    fn description(&self) -> String {
        pair_description(self.name(), &self.keys)
    }

    fn execute(&mut self, driver: &mut dyn GraphDriver) -> Result<(), CommandError> {
        self.removed = false;
        driver.refresh()?;
        if self.keys.is_none() {
            self.keys = driver
                .graph()
                .link(self.link_id)
                .and_then(|link| stable_pair(driver.graph(), link.output_port, link.input_port));
        }
        let Some((output, input)) = self.keys.as_ref() else {
            return Ok(());
        };
        if let Some(link) = driver.graph().find_link_by_keys(output, input) {
            if !driver.is_link_mutable(link.id) {
                return Ok(());
            }
        }
        if let Some(link) = driver.disconnect_by_key_if_present(output, input)? {
            self.link_id = link.id;
            self.link = Some(link);
            self.removed = true;
        }
        Ok(())
    }

    fn undo(&mut self, driver: &mut dyn GraphDriver) -> Result<(), CommandError> {
        if !self.removed {
            return Ok(());
        }
        let (output, input) = self.keys.as_ref().ok_or(CommandError::MissingUndoLink)?;
        if let Some(restored) = driver.connect_by_key_if_missing(output, input)? {
            self.link_id = restored.id;
            self.link = Some(restored);
        } else if let Some(restored) = driver.graph().find_link_by_keys(output, input) {
            self.link_id = restored.id;
            self.link = Some(restored);
        }
        self.removed = false;
        Ok(())
    }
}
