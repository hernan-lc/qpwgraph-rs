use super::QpwgraphApp;
use eframe::egui;
use std::time::{Duration, Instant};

#[cfg(all(target_os = "linux", feature = "tray"))]
use crate::tray::tray_support;

impl QpwgraphApp {
    pub(crate) fn refresh_graph(&mut self) {
        match self.driver.refresh() {
            Ok(nodes) => {
                self.last_graph_refresh = Instant::now();
                self.status = self.tf("status.refreshed", &[("count", nodes.len().to_string())])
            }
            Err(error) => {
                self.status = self.tf("status.refresh_failed", &[("error", error.to_string())])
            }
        }
    }

    fn refresh_graph_if_dirty(&mut self) {
        if self.last_graph_refresh.elapsed() < Duration::from_millis(100)
            || !self.driver.graph_dirty()
        {
            return;
        }
        match self.driver.refresh() {
            Ok(_) => self.last_graph_refresh = Instant::now(),
            Err(error) => {
                // Keep the dirty bit eligible for the next frame. A failed
                // retry must not hide a short-lived PipeWire transition for
                // another half second.
                self.status = self.tf("status.refresh_failed", &[("error", error.to_string())]);
            }
        }
    }

    #[cfg(all(target_os = "linux", feature = "tray"))]
    fn poll_tray(&mut self, ctx: &egui::Context) {
        let Some(tray) = self.tray.as_ref() else {
            return;
        };
        while let Ok(command) = tray.receiver.try_recv() {
            match command {
                tray_support::Command::Show => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                tray_support::Command::Hide => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                }
                tray_support::Command::Quit => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
    }
}

impl eframe::App for QpwgraphApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.apply_ui_text_scale(ctx);
        self.sync_meter_policy();
        self.refresh_graph_if_dirty();
        if self.last_meter_refresh.elapsed() >= Duration::from_millis(50) {
            self.refresh_audio_meters();
            self.last_meter_refresh = Instant::now();
        }
        ctx.request_repaint_after(Duration::from_millis(50));
        #[cfg(all(target_os = "linux", feature = "tray"))]
        self.poll_tray(ctx);
        if self.start_minimized {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            self.start_minimized = false;
        }
        self.handle_shortcuts(ctx);
        self.update_window_size(ctx);
        self.show_gui_panels(ctx);
        self.update_canvas_from_config();
        self.sync_effect_controls();

        if self.config.statusbar {
            egui::TopBottomPanel::bottom("statusbar")
                .frame(
                    egui::Frame::none()
                        .fill(egui::Color32::from_rgb(29, 33, 40))
                        .inner_margin(egui::Margin::symmetric(8.0, 4.0)),
                )
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(&self.status);
                        if self.debug {
                            ui.separator();
                            ui.monospace(self.i18n.format(
                                "debug.backend",
                                &[
                                    ("backend", self.backend_name.clone()),
                                    ("enabled", (!self.no_alsa_midi).to_string()),
                                ],
                            ));
                        }
                    });
                });
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            let actions = if self.any_modal_open() {
                Vec::new()
            } else {
                self.canvas.show_with_keyboard_shortcuts(
                    ui,
                    self.driver.graph(),
                    &self.i18n,
                    !self.text_input_focused(ctx),
                )
            };
            self.handle_canvas_actions(actions);
        });

        // Runs after the canvas so the request reflects what this frame drew.
        self.request_visible_meters(ctx);
        self.show_shortcuts_modal(ctx);
        self.show_history_modal(ctx);
        self.show_effect_gallery_modal(ctx);
        self.show_preferences_modal(ctx);
        self.autosave_config();
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        #[cfg(all(target_os = "linux", feature = "tray"))]
        if let Some(tray) = self.tray.as_ref() {
            tray.shutdown();
        }
        self.sync_config();
        if let Err(error) = self.config.save_to(&self.config_file) {
            eprintln!(
                "{}",
                self.tf("status.config_save_failed", &[("error", error.to_string())])
            );
        }
    }
}

pub(crate) fn run(args: crate::args::Args) -> eframe::Result<()> {
    let app = QpwgraphApp::new(args);
    let window_title = app.i18n.text("app.title");
    let viewport = egui::ViewportBuilder::default()
        .with_inner_size([app.config.window_width, app.config.window_height]);
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        &window_title,
        options,
        Box::new(move |creation_context| {
            egui_extras::install_image_loaders(&creation_context.egui_ctx);
            Ok(Box::new(app))
        }),
    )
}
