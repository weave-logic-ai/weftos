//! Windows-deadlock reproduction for `CanonResponse::from_egui`.
//!
//! Build & run (native Windows target):
//!
//! ```text
//! cargo run --example canon_slider_windows_deadlock_repro \
//!     --target x86_64-pc-windows-gnu --release
//! ```
//!
//! Expected behaviour **without** the fix on this branch:
//!   - Windows (SRWLock-backed `std::sync::RwLock`): the moment the mouse
//!     presses and drags the slider handle, the UI thread freezes.
//!     No panic, no log line, no stack dump.
//!   - Linux (futex-backed): slider drags normally. The same code path
//!     does recursive `RwLock::read()` and futex happens to permit it,
//!     so the bug is invisible on Linux CI.
//!
//! Expected behaviour **with** the fix on this branch:
//!   - Both platforms: slider drags normally.
//!
//! The bug is in `CanonResponse::from_egui`:
//!
//! ```ignore
//! // BUGGY: `inner.drag_delta()` internally calls `ctx.input(|i| ...)`,
//! //        so this nests two read-locks on egui's Context RwLock.
//! let (pointer, drag) = ctx.input(|i| (i.pointer.delta(), inner.drag_delta()));
//!
//! // FIXED: read the drag delta BEFORE entering the closure.
//! let drag    = inner.drag_delta();
//! let pointer = ctx.input(|i| i.pointer.delta());
//! ```
//!
//! This example doesn't try to *detect* the deadlock — it just places a
//! canon `Slider` on screen and lets the user drag it. On Windows the
//! freeze is immediate and obvious; on Linux everything works. That's
//! the whole reproduction.

use clawft_gui_egui::canon::{CanonWidget, Slider};
use eframe::egui;

struct ReproApp {
    value: f64,
}

impl Default for ReproApp {
    fn default() -> Self {
        Self { value: 0.5 }
    }
}

impl eframe::App for ReproApp {
    fn ui(&mut self, _ui: &mut egui::Ui, _frame: &mut eframe::Frame) {}

    #[allow(deprecated)]
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("canon::Slider — Windows drag-deadlock repro");
            ui.label(
                "Drag the slider. On a Windows release build without the \
                 `CanonResponse::from_egui` fix, the UI thread freezes on \
                 the first drag frame. Linux is unaffected.",
            );
            ui.add_space(12.0);
            Slider::new("repro-slider", "value", &mut self.value, 0.0, 1.0).show(ui);
            ui.add_space(8.0);
            ui.label(format!("value = {:.4}", self.value));
        });
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([480.0, 220.0])
            .with_title("canon::Slider Windows deadlock repro"),
        ..Default::default()
    };
    eframe::run_native(
        "canon slider repro",
        options,
        Box::new(|_cc| Ok(Box::new(ReproApp::default()))),
    )
}
