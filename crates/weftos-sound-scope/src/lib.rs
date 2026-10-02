//! weft-sound-scope: standalone companion app for the `sound-detect` cog (KY-038 / LM393 sound
//! sensor read through an ADS1115 on the Seed). Built on `weftos-cog-companion` (Seed connection,
//! hook-up checklist, wiring, Live/Guide tabs); this crate adds the live level trace with the
//! threshold line and the sound/quiet readout. Runs natively and in the browser.

use eframe::egui::{self, RichText};
use egui_plot::{HLine, Line, Plot, PlotPoints};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use weftos_cog_companion::widgets::{AMBER, GREEN, GREY, RED};
use weftos_cog_companion::{step, CogClient, CogState, Mark, SensorApp, Step};

const WIRING: &[(&str, &str)] = &[
    ("Sensor VCC", "3V3 (Seed pin 1)"),
    ("Sensor GND", "GND (Seed pin 6)"),
    ("Sensor OUT", "ADS1115 A1"),
    ("ADS1115 VDD / GND", "3V3 / GND"),
    ("ADS1115 SCL", "Seed pin 5 (SCL)"),
    ("ADS1115 SDA", "Seed pin 3 (SDA)"),
    ("ADS1115 ADDR", "GND = 0x48"),
    ("Trimpot", "turn until the sensor's LED just blinks on a clap"),
];

#[derive(Default)]
pub struct SoundApp {
    /// Recent (t_ms, volts) from the cog's /raw, for the level trace.
    trace: Arc<Mutex<VecDeque<(u64, f64)>>>,
}

impl SensorApp for SoundApp {
    fn cog_id(&self) -> &'static str {
        "sound-detect"
    }
    fn export_port(&self) -> u16 {
        8049
    }
    fn title(&self) -> &'static str {
        "weft-sound-scope"
    }
    fn test_command(&self) -> &'static str {
        "--once --simulate"
    }
    fn wiring(&self) -> &'static [(&'static str, &'static str)] {
        WIRING
    }
    fn on_reconnect(&mut self) {
        if let Ok(mut t) = self.trace.lock() {
            t.clear();
        }
    }

    fn tick(&mut self, ctx: &egui::Context, cog: &mut CogClient) {
        let trace = self.trace.clone();
        cog.poll_export("/raw", 400, ctx, move |r| {
            if let (Ok(v), Ok(mut t)) = (r, trace.lock()) {
                if let Some(arr) = v["samples"].as_array() {
                    t.clear();
                    for s in arr {
                        if let (Some(tm), Some(vv)) = (s["t_ms"].as_u64(), s["v"].as_f64()) {
                            t.push_back((tm, vv));
                        }
                    }
                }
            }
        });
    }

    fn steps(&self, st: &CogState) -> Vec<Step> {
        let status = st.status_str("status").unwrap_or("");
        let src = st.status_str("source").unwrap_or("");
        let events = st.status_f64("total_events").unwrap_or(0.0);
        let present = st.status.as_ref().and_then(|r| r["present"].as_bool()).unwrap_or(false);
        vec![
            match status {
                "no_source" => step("adc", "ADS1115 found on I2C", Mark::Fail, "no ADC: check wiring, I2C enabled (dtparam=i2c_arm=on), address 0x48".to_string()),
                "" => step("adc", "ADS1115 found on I2C", Mark::Waiting, "no report yet".to_string()),
                _ => step("adc", "ADS1115 found on I2C", Mark::Pass, src.to_string()),
            },
            if events > 0.0 || present {
                step("sound", "Sound detected", Mark::Pass, format!("{} events so far", events as u64))
            } else if status == "quiet" {
                step("sound", "Sound detected", Mark::Warn, "reading OK but no events — make a noise, turn up the trimpot, or lower --threshold".to_string())
            } else {
                step("sound", "Sound detected", Mark::Waiting, "waiting for a report".to_string())
            },
        ]
    }

    fn live(&mut self, ui: &mut egui::Ui, st: &CogState, _cog: &mut CogClient) {
        let status = st.status_str("status").unwrap_or("");
        let present = st.status.as_ref().and_then(|r| r["present"].as_bool()).unwrap_or(false);
        let epm = st.status_f64("events_per_min").unwrap_or(0.0);
        let act = st.status_f64("activity_pct").unwrap_or(0.0);
        let thr = st.status_f64("threshold_v").unwrap_or(1.5);
        let peak = st.status_f64("peak_v").unwrap_or(0.0);
        let quiet = st.status_f64("quiet_s");

        let (c, label) = if status == "no_source" {
            (RED, "no sensor")
        } else if present {
            (GREEN, "SOUND")
        } else {
            (GREY, "quiet")
        };
        ui.horizontal(|ui| {
            ui.label(RichText::new("●").color(c).size(44.0));
            ui.vertical(|ui| {
                ui.heading(RichText::new(label).color(c));
                ui.label(RichText::new(format!("{epm:.0} events/min · {act:.0}% active")).color(GREY));
            });
        });
        ui.separator();
        ui.horizontal(|ui| {
            ui.label(format!("peak {peak:.2} V"));
            ui.separator();
            ui.label(format!("threshold {thr:.2} V"));
            ui.separator();
            ui.label(match quiet {
                Some(q) => format!("quiet {q:.0} s"),
                None => "no events yet".to_string(),
            });
        });

        let pts: Vec<[f64; 2]> = {
            let t = self.trace.lock().unwrap();
            let t0 = t.front().map(|&(tm, _)| tm).unwrap_or(0);
            t.iter().map(|&(tm, v)| [tm.saturating_sub(t0) as f64 / 1000.0, v]).collect()
        };
        Plot::new("level").height(220.0).show(ui, |p| {
            p.line(Line::new("OUT (V)", PlotPoints::from(pts)).color(GREEN));
            p.hline(HLine::new("threshold", thr).color(AMBER));
        });
        if self.trace.lock().map(|t| t.is_empty()).unwrap_or(true) {
            ui.label(RichText::new("No trace yet — start the cog and make a sound (or use Test run / --simulate).").color(GREY).small());
        }
    }

    fn side(&mut self, ui: &mut egui::Ui, st: &CogState, _cog: &mut CogClient) {
        ui.label(RichText::new("Sound Detector").strong());
        ui.label(RichText::new("Make a sound near the mic; the OUT line crosses the threshold and an event is counted. Tune the trimpot (or --threshold) for the room.").color(GREY).small());
        ui.separator();
        if let Some(tot) = st.status_f64("total_events") {
            ui.label(format!("{} events total", tot as u64));
        }
        if let Some(e) = st.status_str("source") {
            ui.label(RichText::new(e).color(GREY).small());
        }
    }
}

/// Native entry point.
#[cfg(not(target_arch = "wasm32"))]
pub fn run_native() -> eframe::Result<()> {
    weftos_cog_companion::run_native(SoundApp::default(), "weft-sound-scope")
}

/// Browser entry exported to JS (`www/index.html` calls `sound_start`).
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub async fn sound_start(canvas_id: String) -> Result<(), wasm_bindgen::JsValue> {
    weftos_cog_companion::start_web(SoundApp::default(), &canvas_id).await
}
