//! Browser popup for one ADR-104 sensor guide.
//!
//! Sensor Explorer serves the same JSON a cog serves at `GET /guide`. This crate mounts
//! [`weftos_sensor_guide::GuideView`] on a canvas. The native build is empty; `scripts/build.sh
//! guide-web` produces the wasm package.

#[cfg(target_arch = "wasm32")]
mod popup {
    use std::cell::RefCell;

    use eframe::egui;
    use wasm_bindgen::prelude::*;
    use wasm_bindgen::JsCast;
    use weftos_sensor_guide::{GuideBundle, GuideView};

    struct Popup {
        bundle: GuideBundle,
        view: GuideView,
    }

    impl eframe::App for Popup {
        // eframe 0.34 requires `ui`. The guide shell is driven from `update`, same as the companion.
        fn ui(&mut self, _ui: &mut egui::Ui, _frame: &mut eframe::Frame) {}

        #[allow(deprecated)]
        fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
            egui::CentralPanel::default().show(ctx, |ui| {
                self.view.show(ui, &self.bundle);
            });
        }
    }

    thread_local! {
        static RUNNER: RefCell<Option<&'static eframe::WebRunner>> = const { RefCell::new(None) };
    }

    fn runner() -> &'static eframe::WebRunner {
        RUNNER.with(|slot| {
            if let Some(existing) = *slot.borrow() {
                return existing;
            }
            let leaked: &'static eframe::WebRunner = Box::leak(Box::new(eframe::WebRunner::new()));
            *slot.borrow_mut() = Some(leaked);
            leaked
        })
    }

    fn canvas(canvas_id: &str) -> Result<web_sys::HtmlCanvasElement, JsValue> {
        let document = web_sys::window()
            .and_then(|w| w.document())
            .ok_or_else(|| JsValue::from_str("no document"))?;
        document
            .get_element_by_id(canvas_id)
            .ok_or_else(|| JsValue::from_str("canvas not found"))?
            .dyn_into::<web_sys::HtmlCanvasElement>()
            .map_err(|_| JsValue::from_str("canvas is not a canvas"))
    }

    /// Parse a `/guide` JSON document and paint it on `canvas_id`.
    #[wasm_bindgen]
    pub async fn open_guide(canvas_id: String, json: String) -> Result<(), JsValue> {
        console_error_panic_hook::set_once();
        let value: serde_json::Value = serde_json::from_str(&json)
            .map_err(|e| JsValue::from_str(&format!("guide JSON: {e}")))?;
        let bundle = GuideBundle::from_json(&value).map_err(|e| JsValue::from_str(&e))?;
        let canvas = canvas(&canvas_id)?;
        runner()
            .start(
                canvas,
                eframe::WebOptions::default(),
                Box::new(move |cc| {
                    // set_visuals alone loses to ThemePreference::System on the next frame.
                    cc.egui_ctx.set_theme(egui::ThemePreference::Dark);
                    Ok(Box::new(Popup {
                        bundle,
                        view: GuideView::default(),
                    }))
                }),
            )
            .await
    }

    /// Stop the viewer so a hidden canvas does not keep painting.
    #[wasm_bindgen]
    pub fn close_guide() {
        runner().destroy();
    }
}
