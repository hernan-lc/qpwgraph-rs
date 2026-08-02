//! Persistent patchbay profiles and file operations.

use super::QpwgraphApp;
use pw_graph_patchbay::Patchbay;
use rfd::FileDialog;
use std::path::PathBuf;

impl QpwgraphApp {
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

    pub(crate) fn snapshot_patchbay(&mut self) {
        self.patchbay
            .snapshot_graph(self.driver.graph(), self.config.patchbay_auto_pin);
        self.status = self.tf(
            "status.snapshot",
            &[("count", self.patchbay.connections.len().to_string())],
        );
    }
}
