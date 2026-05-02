//! Top-level app: boot splash → desktop shell with tray + floating windows.

use std::sync::Arc;

use eframe::egui;

use crate::live::Live;
use crate::shell::{self, desktop::Desktop, Phase};

pub struct ClawftApp {
    phase: Phase,
    desktop: Desktop,
    live: Arc<Live>,
}

impl ClawftApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        crate::theming::apply(&cc.egui_ctx);
        egui_extras::install_image_loaders(&cc.egui_ctx);

        // Preload the boot logo so the splash appears on frame 1 instead
        // of flashing in once the async image loader catches up.
        cc.egui_ctx.include_bytes(
            "bytes://weftos-gold.png",
            crate::shell::boot::LOGO_PNG,
        );

        Self {
            phase: Phase::boot(),
            desktop: Desktop::default(),
            live: Live::spawn(),
        }
    }
}

impl eframe::App for ClawftApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 1.0]
    }

    // eframe 0.34 added a required `ui` method on `App`. We still drive
    // the app from the (now-deprecated) `update` method below — eframe
    // calls both each frame, so an empty `ui` keeps the trait satisfied
    // without restructuring the app body.
    fn ui(&mut self, _ui: &mut egui::Ui, _frame: &mut eframe::Frame) {}

    #[allow(deprecated)]
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let snap = self.live.snapshot();

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(egui::Color32::BLACK))
            .show(ctx, |ui| match &mut self.phase {
                Phase::Boot {
                    started,
                    sfx_played,
                } => {
                    let done = shell::boot::show(ui, *started, sfx_played);
                    if done {
                        self.phase = Phase::Desktop;
                        self.desktop.boot_started = web_time::Instant::now();
                    }
                }
                Phase::Desktop => {
                    shell::desktop::show(ui, &mut self.desktop, &self.live, &snap);
                }
            });

        // ~60 fps for the grid animation / fade.
        ctx.request_repaint_after(std::time::Duration::from_millis(16));
    }
}

