//! weft-ecg-scope: a small WeftOS egui app for the `sen0213-ecg` cog on a Cognitum Seed.
//!
//! It connects to the cog's signal export and the Seed agent's app API over plain HTTP (no
//! SSH, no TLS: the agent serves port 80 with permissive CORS, and so does the cog export),
//! then shows a live ECG graph, a hook-up checklist with the wiring, and calibration tools:
//! baseline, rails, mains hum, R amplitude and polarity, SNR, a tap-along pulse check, and a
//! RuView ADR-293 ground-truth export. Runs natively and in the browser (wasm).

pub mod analysis;
pub mod app;
pub mod checklist;
pub mod model;
pub mod net;
#[cfg(not(target_arch = "wasm32"))]
mod shot;

/// Native entry: opens a window.
#[cfg(not(target_arch = "wasm32"))]
pub fn run_native() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1360.0, 820.0])
            .with_min_inner_size([900.0, 560.0])
            .with_title("WeftOS ECG scope (sen0213-ecg)"),
        ..Default::default()
    };
    eframe::run_native(
        "weft-ecg-scope",
        options,
        Box::new(|_cc| Ok(Box::new(app::ScopeApp::new(net::Settings::default())))),
    )
}

/// Browser entry: mounts the app on `<canvas id=canvas_id>`. `?seed=<host>` picks the Seed.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub async fn ecg_start(canvas_id: String) -> Result<(), wasm_bindgen::JsValue> {
    use wasm_bindgen::JsCast;
    console_error_panic_hook::set_once();
    let document = web_sys::window()
        .and_then(|w| w.document())
        .ok_or_else(|| wasm_bindgen::JsValue::from_str("no document"))?;
    let canvas = document
        .get_element_by_id(&canvas_id)
        .ok_or_else(|| wasm_bindgen::JsValue::from_str("canvas not found"))?
        .dyn_into::<web_sys::HtmlCanvasElement>()?;
    eframe::WebRunner::new()
        .start(
            canvas,
            eframe::WebOptions::default(),
            Box::new(|_cc| Ok(Box::new(app::ScopeApp::new(net::Settings::default())))),
        )
        .await
}
