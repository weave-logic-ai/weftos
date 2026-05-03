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

/// Subset of DejaVu Sans covering the canonical sidebar icon glyphs
/// (Geometric Shapes / Math / Block Elements / Arrows blocks). egui's
/// default proportional font (Ubuntu-Light + NotoEmoji) doesn't carry
/// these blocks, so painting glyphs like ▢ ≣ ↯ ◯ ◷ ▥ ≡ ▌ ▦ rendered
/// as tofu. Registering this font as a fallback in every family
/// closes the gap. Built from
/// `/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf` via
/// `pyftsubset --unicodes=U+25A2,U+2263,...` so the bundled bytes
/// are ~3 KB instead of the full 700 KB.
const SYMBOLS_FONT: &[u8] = include_bytes!(
    "../assets/fonts/DejaVuSans-WeftSymbols.ttf"
);

/// Append the symbol-font subset to every family's fallback list so
/// painting an icon glyph that the primary font lacks falls through
/// to a font that has it.
fn install_symbol_font(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "weft-symbols".to_string(),
        std::sync::Arc::new(egui::FontData::from_static(SYMBOLS_FONT)),
    );
    for family in [
        egui::FontFamily::Proportional,
        egui::FontFamily::Monospace,
    ] {
        fonts
            .families
            .entry(family)
            .or_default()
            .push("weft-symbols".to_string());
    }
    ctx.set_fonts(fonts);
}

impl ClawftApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        crate::theming::apply(&cc.egui_ctx);
        install_symbol_font(&cc.egui_ctx);
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

