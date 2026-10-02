//! weft-ecg-scope: companion app for the `sen0213-ecg` cog (DFRobot SEN0213 ECG through an
//! ADS1115 on a Cognitum Seed). Built on `weftos-cog-companion` (connection, checklist, cog
//! settings from the manifest, guide); this crate adds the live ECG plot, the ECG checks, and
//! the calibration tools: baseline, rails, mains hum and notch, R amplitude and polarity, SNR,
//! a tap-along pulse check, and a RuView ADR-293 ground-truth export. Native and wasm.

pub mod analysis;
pub mod app;
pub mod checklist;
pub mod model;

pub use app::EcgApp;

/// Native entry.
#[cfg(not(target_arch = "wasm32"))]
pub fn run_native() -> eframe::Result<()> {
    weftos_cog_companion::run_native(EcgApp::default(), "WeftOS ECG scope (sen0213-ecg)")
}

/// Browser entry: mounts on `<canvas id=canvas_id>`; `?seed=<host>` picks the Seed.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub async fn ecg_start(canvas_id: String) -> Result<(), wasm_bindgen::JsValue> {
    weftos_cog_companion::start_web(EcgApp::default(), &canvas_id).await
}
