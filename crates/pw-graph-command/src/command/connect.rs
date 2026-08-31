//! Creating links, one pair or a whole group at a time.

use super::*;

pub struct ConnectCommand {
    pub(super) src: PortId,
    pub(super) dst: PortId,
    pub(super) link: Option<Link>,
    pub(super) keys: Option<(PortKey, PortKey)>,
}

/// Connects a group of compatible ports as one undoable action.
pub struct ConnectManyCommand {
    pub(super) pairs: Vec<(PortId, PortId)>,
    pub(super) keys: Vec<(PortKey, PortKey)>,
    pub(super) links: Vec<Link>,
    pub(super) created_keys: Vec<(PortKey, PortKey)>,
}

impl ConnectManyCommand {
    pub fn new(pairs: Vec<(PortId, PortId)>) -> Self {
        Self {
            pairs,
            keys: Vec::new(),
            links: Vec::new(),
            created_keys: Vec::new(),
        }
    }

    pub fn with_keys(pairs: Vec<(PortId, PortId)>, keys: Vec<(PortKey, PortKey)>) -> Self {
        let keys = dedup_pairs(keys);
        Self {
            pairs,
            keys,
            links: Vec::new(),
            created_keys: Vec::new(),
        }
    }
}

impl Command for ConnectManyCommand {
    fn name(&self) -> &'static str {
        "Connect group"
    }

    fn description(&self) -> String {
        format!(
            "{} ({} pairs)",
            self.name(),
            self.keys.len().max(self.pairs.len())
        )
    }

    fn execute(&mut self, driver: &mut dyn GraphDriver) -> Result<(), CommandError> {
        driver.refresh()?;
        if self.keys.is_empty() {
            self.keys = dedup_pairs(
                self.pairs
                    .iter()
                    .filter_map(|(src, dst)| stable_pair(driver.graph(), *src, *dst)),
            );
        }
        self.links.clear();
        self.created_keys.clear();
        for (output, input) in &self.keys {
            match driver.connect_by_key_if_missing(output, input) {
                Ok(Some(link)) => {
                    self.links.push(link);
                    self.created_keys.push((output.clone(), input.clone()));
                }
                Ok(None) => {}
                Err(error) => {
                    // Rolling back is not best-effort housekeeping: the
                    // command never completes, so `CommandStack::execute`
                    // will not record it and nothing else will offer to
                    // repair the graph. Discarding the rollback's own errors
                    // — as this used to — could leave links this command
                    // created behind while reporting a plain backend failure,
                    // as if nothing had been mutated.
                    let stranded = rollback_connects(driver, &self.created_keys);
                    self.links.clear();
                    self.created_keys.clear();
                    return Err(if stranded == 0 {
                        error.into()
                    } else {
                        CommandError::PartiallyApplied {
                            operation: "Connect group",
                            cause: error.to_string(),
                            stranded,
                        }
                    });
                }
            }
        }
        Ok(())
    }

    fn undo(&mut self, driver: &mut dyn GraphDriver) -> Result<(), CommandError> {
        let mut disconnected = Vec::new();
        for (output, input) in self.created_keys.iter().rev() {
            match driver.disconnect_by_key_if_present_without_suppression(output, input) {
                Ok(Some(_)) => {
                    driver.allow_connection(output, input);
                    disconnected.push((output.clone(), input.clone()));
                }
                Ok(None) => {}
                Err(error) => {
                    let mut rollback_errors = Vec::new();
                    for (output, input) in disconnected.iter().rev() {
                        if let Err(restore) = driver.connect_by_key_if_missing(output, input) {
                            rollback_errors.push(restore.to_string());
                        }
                    }
                    if rollback_errors.is_empty() {
                        return Err(error.into());
                    }
                    return Err(CommandError::PartiallyApplied {
                        operation: "Connect group undo",
                        cause: format!(
                            "{error}; reconnecting undone links failed: {}",
                            rollback_errors.join("; ")
                        ),
                        stranded: rollback_errors.len(),
                    });
                }
            }
        }
        self.links.clear();
        self.created_keys.clear();
        Ok(())
    }
}

impl ConnectCommand {
    pub fn new(src: PortId, dst: PortId) -> Self {
        Self {
            src,
            dst,
            link: None,
            keys: None,
        }
    }

    pub fn from_keys(output: PortKey, input: PortKey) -> Self {
        Self {
            src: PortId::default(),
            dst: PortId::default(),
            link: None,
            keys: Some((output, input)),
        }
    }
}

impl Command for ConnectCommand {
    fn name(&self) -> &'static str {
        "Connect"
    }

    fn description(&self) -> String {
        pair_description(self.name(), &self.keys)
    }

    fn execute(&mut self, driver: &mut dyn GraphDriver) -> Result<(), CommandError> {
        driver.refresh()?;
        let keys = self
            .keys
            .clone()
            .or_else(|| stable_pair(driver.graph(), self.src, self.dst));
        let Some((output, input)) = keys else {
            return Ok(());
        };
        self.keys = Some((output.clone(), input.clone()));
        self.link = driver.connect_by_key_if_missing(&output, &input)?;
        Ok(())
    }

    fn undo(&mut self, driver: &mut dyn GraphDriver) -> Result<(), CommandError> {
        if self.link.is_none() {
            return Ok(());
        }
        let (output, input) = self.keys.as_ref().ok_or(CommandError::MissingUndoLink)?;
        // `disconnect_by_key_if_present` suppresses a pair when it is already
        // absent. This command owns the pair's suppression lifecycle, so make
        // sure a successful undo does not leave that temporary state behind.
        driver.disconnect_by_key_if_present(output, input)?;
        driver.allow_connection(output, input);
        self.link = None;
        Ok(())
    }
}
