//! weft-cog-manager (COG-009): the WeftOS appliance console.
//!
//! This is the OS showing its own interface — the cogs it runs and the marketplace it can pull
//! from — talking to a `weft-cog-host` over its lifecycle API. A left nav has **Cogs** (running +
//! marketplace), **Apps** (a placeholder for what lands next), and **System**. Runs natively and in
//! the browser; `?host=<url>` (wasm) or `WEFTOS_HOST` (native) points it at a host.

// egui 0.34 panel API (`.show(ctx)`, `.default_width`) is deprecated upstream but is what the
// companion/scope crates use; silence it crate-wide rather than scatter per-method allows.
#![allow(deprecated)]
pub mod client;
mod app;
mod hw_dex;
mod hw_identify;
mod sensor_detail;
mod sensor_install;
mod sensor_link;
mod sensor_mesh;
mod sensor_software;
mod style;
mod views;

use app::Manager;
use eframe::egui::{self, Color32};

const GREEN: Color32 = Color32::from_rgb(0x4c, 0xc2, 0x7a);
const RED: Color32 = Color32::from_rgb(0xe0, 0x5a, 0x5a);
const GREY: Color32 = Color32::from_rgb(0x88, 0x88, 0x88);
const AMBER: Color32 = Color32::from_rgb(0xd8, 0xa0, 0x3a);
const WL: Color32 = Color32::from_rgb(0x6c, 0x9c, 0xe8); // WeaveLogic blue
const COG: Color32 = Color32::from_rgb(0xb0, 0x82, 0xd8); // Cognitum purple

/// Native entry point.
#[cfg(not(target_arch = "wasm32"))]
pub fn run_native() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 780.0])
            .with_min_inner_size([820.0, 520.0])
            .with_title("WeftOS — appliance console"),
        ..Default::default()
    };
    eframe::run_native("WeftOS console", options, Box::new(|cc| {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        Ok(Box::new(Manager::new()))
    }))
}

/// Browser entry exported to JS (`www/index.html` calls `mgr_start`).
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub async fn mgr_start(canvas_id: String) -> Result<(), wasm_bindgen::JsValue> {
    start_web(&canvas_id).await
}

/// Browser entry: mounts on `<canvas id=canvas_id>`. `?host=<url>` picks the cog-host.
#[cfg(target_arch = "wasm32")]
pub async fn start_web(canvas_id: &str) -> Result<(), wasm_bindgen::JsValue> {
    use wasm_bindgen::JsCast;
    console_error_panic_hook::set_once();
    let document = web_sys::window().and_then(|w| w.document()).ok_or_else(|| wasm_bindgen::JsValue::from_str("no document"))?;
    let canvas = document
        .get_element_by_id(canvas_id)
        .ok_or_else(|| wasm_bindgen::JsValue::from_str("canvas not found"))?
        .dyn_into::<web_sys::HtmlCanvasElement>()?;
    eframe::WebRunner::new()
        .start(canvas, eframe::WebOptions::default(), Box::new(|cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
            Ok(Box::new(Manager::new()))
        }))
        .await
}
