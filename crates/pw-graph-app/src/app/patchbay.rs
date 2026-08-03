//! Persistent patchbay profiles and file operations.

use super::QpwgraphApp;
use pw_graph_core::PortKey;
use pw_graph_patchbay::Patchbay;
use rfd::FileDialog;
use std::path::PathBuf;

impl QpwgraphApp {
    /// Persist the current in-memory patchbay without opening a dialog. Graph
    /// edits are user actions, so losing them merely because the profile was
    /// not manually saved is particularly surprising for effect nodes.
    pub(crate) fn autosave_patchbay(&mut self) {
        if let Err(error) = self.patchbay.save_to(&self.patchbay_file) {
            self.status = self.tf(
                "status.patchbay_save_failed",
                &[("error", error.to_string())],
            );
        }
    }

    /// Bring saved rules in line with the live graph after undo/redo and at
    /// shutdown. Keep the original endpoints of inserted effects even though
    /// their direct link is intentionally absent while the effect is active.
    pub(crate) fn sync_patchbay_connections(&mut self) {
        let live: Vec<(PortKey, PortKey)> = self
            .driver
            .graph()
            .links
            .values()
            .filter_map(|link| {
                self.driver
                    .graph()
                    .port_key(link.output_port)
                    .zip(self.driver.graph().port_key(link.input_port))
            })
            .collect();
        let protected: Vec<(PortKey, PortKey)> = self
            .config
            .effects
            .iter()
            .filter_map(|effect| effect.source.clone().zip(effect.destination.clone()))
            .collect();

        self.patchbay.connections.retain(|connection| {
            if connection.output_node.is_empty()
                || connection.output_name.is_empty()
                || connection.input_node.is_empty()
                || connection.input_name.is_empty()
            {
                return true;
            }
            live.iter()
                .any(|(output, input)| same_endpoint_names(connection, output, input))
                || protected
                    .iter()
                    .any(|(output, input)| same_endpoint_names(connection, output, input))
        });

        for (output, input) in live {
            let Some(output_id) = self.driver.graph().resolve_port_key(&output) else {
                continue;
            };
            let Some(input_id) = self.driver.graph().resolve_port_key(&input) else {
                continue;
            };
            self.patchbay.add_graph_connection(
                self.driver.graph(),
                output_id,
                input_id,
                self.config.patchbay_auto_pin,
            );
        }
    }

    pub(crate) fn save_patchbay(&mut self) {
        let directory = self
            .config
            .patchbay_dir
            .clone()
            .or_else(|| self.patchbay_file.parent().map(PathBuf::from));
        let selected = FileDialog::new()
            .set_directory(directory.unwrap_or_else(|| PathBuf::from(".")))
            .set_file_name(
                self.patchbay_file
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("default.qpwgraph"),
            )
            .add_filter("Patchbay", &["qpwgraph", "xml", "json"])
            .save_file();
        let Some(path) = selected else {
            return;
        };
        self.select_patchbay_path(path);
        match self.patchbay.save_to(&self.patchbay_file) {
            Ok(()) => {
                self.status = self.tf(
                    "status.saved_patchbay",
                    &[("path", self.patchbay_file.display().to_string())],
                )
            }
            Err(error) => {
                self.status = self.tf(
                    "status.patchbay_save_failed",
                    &[("error", error.to_string())],
                )
            }
        }
    }

    pub(crate) fn load_patchbay(&mut self) {
        let directory = self
            .config
            .patchbay_dir
            .clone()
            .or_else(|| self.patchbay_file.parent().map(PathBuf::from));
        let selected = FileDialog::new()
            .set_directory(directory.unwrap_or_else(|| PathBuf::from(".")))
            .add_filter("Patchbay", &["qpwgraph", "xml", "json"])
            .pick_file();
        let Some(path) = selected else {
            return;
        };
        match Patchbay::load_from(&path) {
            Ok(patchbay) => {
                self.select_patchbay_path(path);
                self.patchbay = patchbay;
                self.status = self.tf(
                    "status.loaded",
                    &[("path", self.patchbay_file.display().to_string())],
                );
            }
            Err(error) => {
                self.status = self.tf(
                    "status.patchbay_load_failed",
                    &[("error", error.to_string())],
                )
            }
        }
    }

    pub(crate) fn select_patchbay_path(&mut self, path: PathBuf) {
        self.patchbay_file = path.clone();
        self.config.patchbay_dir = path.parent().map(PathBuf::from);
        self.config
            .recent_patchbay_paths
            .retain(|item| item != &path);
        self.config.recent_patchbay_paths.insert(0, path.clone());
        self.config.recent_patchbay_paths.truncate(8);
        self.config.patchbay_path = Some(path);
        self.config.patchbay_profiles.insert(
            self.config.active_patchbay_profile.clone(),
            self.patchbay_file.clone(),
        );
    }

    pub(crate) fn choose_patchbay_directory(&mut self) {
        let initial = self
            .config
            .patchbay_dir
            .clone()
            .or_else(|| self.patchbay_file.parent().map(PathBuf::from));
        if let Some(path) = FileDialog::new()
            .set_directory(initial.unwrap_or_else(|| PathBuf::from(".")))
            .pick_folder()
        {
            self.config.patchbay_dir = Some(path);
        }
    }

    pub(crate) fn use_recent_patchbay(&mut self, path: PathBuf) {
        if Patchbay::load_from(&path).is_ok() {
            self.select_patchbay_path(path);
            let _ = self.load_patchbay_from_current();
        }
    }

    pub(crate) fn load_patchbay_from_current(&mut self) -> Result<(), String> {
        match Patchbay::load_from(&self.patchbay_file) {
            Ok(patchbay) => {
                self.patchbay = patchbay;
                self.status = self.tf(
                    "status.loaded",
                    &[("path", self.patchbay_file.display().to_string())],
                );
                Ok(())
            }
            Err(error) => Err(error.to_string()),
        }
    }

    pub(crate) fn activate_patchbay(&mut self) {
        match self.patchbay.activate(
            self.driver.as_mut(),
            self.config.patchbay_exclusive,
            self.config.patchbay_auto_disconnect,
        ) {
            Ok(report) => {
                self.status = self.tf(
                    "status.activated",
                    &[
                        ("connected", report.connected.to_string()),
                        ("present", report.already_present.to_string()),
                        ("disconnected", report.disconnected.to_string()),
                    ],
                )
            }
            Err(error) => {
                self.status = self.tf("status.activation_failed", &[("error", error.to_string())])
            }
        }
    }

    /// Effect links are persisted together with the effect nodes. Restore
    /// those links even when the user has disabled automatic activation of the
    /// general patchbay profile.
    pub(crate) fn restore_effect_connections(&mut self) {
        let effect_patchbay = self.patchbay.effect_connections();
        if effect_patchbay.connections.is_empty() {
            return;
        }
        match effect_patchbay.activate(self.driver.as_mut(), false, false) {
            Ok(report) if report.failed.is_empty() => {}
            Ok(report) => {
                self.status = self.tf(
                    "status.activation_failed",
                    &[("error", report.failed.join("; "))],
                );
            }
            Err(error) => {
                self.status = self.tf("status.activation_failed", &[("error", error.to_string())])
            }
        }
    }

    pub(crate) fn snapshot_patchbay(&mut self) {
        self.patchbay
            .snapshot_graph(self.driver.graph(), self.config.patchbay_auto_pin);
        self.status = self.tf(
            "status.snapshot",
            &[("count", self.patchbay.connections.len().to_string())],
        );
    }
}

fn same_endpoint_names(
    connection: &pw_graph_patchbay::PatchConnection,
    output: &PortKey,
    input: &PortKey,
) -> bool {
    connection.output_node == output.node_name
        && connection.output_name == output.port_name
        && connection.input_node == input.node_name
        && connection.input_name == input.port_name
}
