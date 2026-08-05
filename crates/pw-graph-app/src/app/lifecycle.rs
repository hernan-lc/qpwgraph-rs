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
            Err(error) => self.status_error("status.refresh_failed", &error),
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
                self.status_error("status.refresh_failed", &error);
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
        self.ui_document.begin_frame();
        self.apply_ui_text_scale(ctx);
        self.sync_meter_policy();
        #[cfg(feature = "relay")]
        {
            self.with_relay(|app, relay| relay.poll(app));
        }
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
        #[cfg(feature = "relay")]
        self.show_relay_panel(ctx);
        self.update_canvas_from_config();
        self.sync_effect_controls();

        if self.config.statusbar {
            let fill = self
                .ui_document
                .theme_color(pw_graph_ui::ThemeToken::Background);
            egui::TopBottomPanel::bottom("statusbar")
                .frame(
                    egui::Frame::none()
                        .fill(fill)
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
            let modal_open = self.any_modal_open();
            let keyboard_shortcuts_enabled = !modal_open && !self.text_input_focused(ctx);
            // Keep the graph rendered beneath dialogs so the modal backdrop
            // can dim real application content instead of exposing the
            // central panel's plain background.
            let actions = self.canvas.show_with_keyboard_shortcuts(
                ui,
                self.driver.graph(),
                &self.i18n,
                &mut self.ui_document,
                keyboard_shortcuts_enabled,
            );
            let actions = if modal_open { Vec::new() } else { actions };
            self.handle_canvas_actions(actions);
        });

        // Runs after the canvas so the request reflects what this frame drew.
        self.request_visible_meters(ctx);
        self.show_shortcuts_modal(ctx);
        self.show_history_modal(ctx);
        self.show_effect_gallery_modal(ctx);
        self.show_preferences_modal(ctx);
        self.ui_document.dispatch_pending_events();
        self.autosave_config();
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        #[cfg(all(target_os = "linux", feature = "tray"))]
        if let Some(tray) = self.tray.as_ref() {
            tray.shutdown();
        }
        #[cfg(feature = "relay")]
        self.driver.relay_discovery_stop();
        self.sync_config();
        self.sync_patchbay_connections();
        self.autosave_patchbay();
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
