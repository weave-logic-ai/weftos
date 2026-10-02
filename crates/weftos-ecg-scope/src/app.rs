//! The ECG-specific half of weft-ecg-scope on the shared companion shell: the live ECG plot,
//! the sensor checklist steps, and the calibration tools (baseline, rails, hum, R amplitude,
//! SNR, recommended notch, tap-along pulse check, signal and RuView ADR-293 exports).

use crate::analysis::{self, Calibration, TapTempo};
use crate::checklist;
use crate::model::{Sample, SignalBuffer, parse_raw};
use eframe::egui::{self, Color32, RichText};
use egui_plot::{Line, Plot, PlotPoints, Points};
use std::sync::{Arc, Mutex};
use web_time::Instant;
use weftos_cog_companion::widgets::{AMBER, GREEN, RED, opt, save_text};
use weftos_cog_companion::{CogClient, CogState, SensorApp, Step};

const WIRING: &[(&str, &str)] = &[
    ("Seed pin 1 (3V3)", "ADS1115 VDD + SEN0213 +"),
    ("Seed pin 6 (GND)", "ADS1115 GND + SEN0213 -"),
    ("Seed pin 3 (SDA)", "ADS1115 SDA"),
    ("Seed pin 5 (SCL)", "ADS1115 SCL"),
    ("ADS1115 ADDR", "GND (address 0x48)"),
    ("SEN0213 A", "ADS1115 A0"),
    ("Never", "pin 2 / 4 (5 V)"),
];

/// (filtered mV, raw V, R markers) as plot points.
type Series = (Vec<[f64; 2]>, Vec<[f64; 2]>, Vec<[f64; 2]>);

pub struct EcgApp {
    signal: Arc<Mutex<SignalBuffer>>,
    window_s: f64,
    show_raw: bool,
    paused: Option<Series>,
    tap: TapTempo,
    save_name: String,
    note: Option<String>,
}

impl Default for EcgApp {
    fn default() -> Self {
        Self {
            signal: Arc::new(Mutex::new(SignalBuffer::default())),
            window_s: 8.0,
            show_raw: false,
            paused: None,
            tap: TapTempo::default(),
            save_name: "ecg-capture".into(),
            note: None,
        }
    }
}

impl EcgApp {
    fn calibration(&self) -> (Calibration, f64) {
        let Ok(sig) = self.signal.lock() else {
            return (Calibration::default(), 250.0);
        };
        let fs = if sig.sample_rate_hz > 0.0 {
            sig.sample_rate_hz
        } else {
            250.0
        };
        let peaks: Vec<u64> = sig.peaks_ms.iter().copied().collect();
        (analysis::calibrate(&sig.tail(4.0), &peaks, fs), fs)
    }
}

impl SensorApp for EcgApp {
    fn cog_id(&self) -> &'static str {
        "sen0213-ecg"
    }

    fn export_port(&self) -> u16 {
        8046
    }

    fn title(&self) -> &'static str {
        "weft-ecg-scope"
    }

    fn wiring(&self) -> &'static [(&'static str, &'static str)] {
        WIRING
    }

    fn on_reconnect(&mut self) {
        if let Ok(mut s) = self.signal.lock() {
            *s = SignalBuffer::default();
        }
        self.paused = None;
    }

    fn tick(&mut self, ctx: &egui::Context, cog: &mut CogClient) {
        if ctx.input(|i| i.key_pressed(egui::Key::Space)) && !ctx.egui_wants_keyboard_input() {
            self.tap.tap(Instant::now());
        }
        let signal = self.signal.clone();
        cog.poll_export("/raw?seconds=2", 250, ctx, move |r| {
            if let (Ok(body), Ok(mut sig)) = (r, signal.lock()) {
                let (samples, peaks, fs, source) = parse_raw(&body);
                sig.merge(&samples, &peaks);
                sig.sample_rate_hz = fs;
                sig.source = source;
            }
        });
    }

    fn steps(&self, st: &CogState) -> Vec<Step> {
        let (cal, _) = self.calibration();
        let source = self
            .signal
            .lock()
            .map(|s| s.source.clone())
            .unwrap_or_default();
        let configured_mains = st
            .config
            .as_ref()
            .and_then(|c| c["mains_hz"].as_u64())
            .map(|v| v as u32);
        checklist::sensor_steps(st.status.as_ref(), &source, &cal, configured_mains)
    }

    fn live(&mut self, ui: &mut egui::Ui, st: &CogState, _cog: &mut CogClient) {
        let (window, peaks) = match self.signal.lock() {
            Ok(s) => (
                s.tail(self.window_s.max(4.0)),
                s.peaks_ms.iter().copied().collect::<Vec<_>>(),
            ),
            Err(_) => return,
        };
        ui.horizontal(|ui| {
            ui.heading("ECG");
            ui.add(egui::Slider::new(&mut self.window_s, 2.0..=30.0).text("seconds"));
            ui.checkbox(&mut self.show_raw, "raw volts");
            let paused = self.paused.is_some();
            if ui
                .button(if paused { "Resume" } else { "Freeze" })
                .clicked()
            {
                self.paused = if paused {
                    None
                } else {
                    Some(plot_series(&window, &peaks, self.window_s))
                };
            }
            if !st.export.is_ok() {
                ui.label(RichText::new("no signal export").color(RED));
            }
        });
        let (filtered, raw, rpk) = self
            .paused
            .clone()
            .unwrap_or_else(|| plot_series(&window, &peaks, self.window_s));
        let h = if self.show_raw {
            ui.available_height() * 0.62
        } else {
            ui.available_height()
        };
        Plot::new("ecg")
            .height(h)
            .x_axis_label("s")
            .y_axis_label("mV (0.5-40 Hz)")
            .include_x(-self.window_s)
            .include_x(0.0)
            .allow_drag(false)
            .show(ui, |p| {
                p.line(Line::new("filtered", PlotPoints::from(filtered)).color(GREEN));
                p.points(
                    Points::new("R", PlotPoints::from(rpk))
                        .radius(4.0)
                        .color(RED),
                );
            });
        if self.show_raw {
            Plot::new("raw")
                .height(ui.available_height())
                .x_axis_label("s")
                .y_axis_label("V at ADC")
                .include_y(0.0)
                .include_y(analysis::SUPPLY_V)
                .include_x(-self.window_s)
                .include_x(0.0)
                .allow_drag(false)
                .show(ui, |p| {
                    p.line(Line::new("raw", PlotPoints::from(raw)).color(Color32::LIGHT_BLUE));
                });
        }
    }

    fn side(&mut self, ui: &mut egui::Ui, st: &CogState, cog: &mut CogClient) {
        let (calib, fs) = self.calibration();
        let hr = st.status_f64("heart_rate_bpm");
        ui.label(
            RichText::new(opt(hr, |v| format!("{v:.0} bpm")))
                .size(40.0)
                .strong(),
        );
        ui.label(format!(
            "status: {}",
            st.status_str("status").unwrap_or("-")
        ));
        let q = st.status_f64("quality").unwrap_or(0.0);
        ui.add(egui::ProgressBar::new(q as f32).text(format!("quality {q:.2}")));
        if self
            .signal
            .lock()
            .is_ok_and(|s| s.source.starts_with("simulator"))
        {
            ui.label(
                RichText::new("SIMULATED signal (--simulate)")
                    .color(AMBER)
                    .strong(),
            );
        }
        ui.separator();
        ui.heading("Calibration (last 4 s)");
        egui::Grid::new("cal").num_columns(2).show(ui, |ui| {
            let row = |ui: &mut egui::Ui, k: &str, v: String| {
                ui.label(k);
                ui.monospace(v);
                ui.end_row();
            };
            row(
                ui,
                "baseline",
                opt(calib.baseline_v, |v| format!("{v:.3} V")),
            );
            row(
                ui,
                "at rails",
                format!("{:.1} %", calib.rail_fraction * 100.0),
            );
            row(
                ui,
                "hum 50 Hz",
                opt(calib.hum_50_mv, |v| format!("{v:.3} mV rms")),
            );
            row(
                ui,
                "hum 60 Hz",
                opt(calib.hum_60_mv, |v| format!("{v:.3} mV rms")),
            );
            row(
                ui,
                "R amplitude",
                opt(calib.r_amplitude_mv, |v| format!("{v:.2} mV")),
            );
            row(
                ui,
                "noise floor",
                opt(calib.noise_mv, |v| format!("{v:.3} mV rms")),
            );
            row(ui, "SNR", opt(calib.snr_db, |v| format!("{v:.1} dB")));
            let r = |k: &str| st.status_f64(k);
            row(
                ui,
                "SDNN / RMSSD",
                format!(
                    "{} / {}",
                    opt(r("sdnn_ms"), |v| format!("{v:.0}")),
                    opt(r("rmssd_ms"), |v| format!("{v:.0} ms"))
                ),
            );
            row(
                ui,
                "late / errors",
                format!(
                    "{} / {}",
                    opt(r("late_samples"), |v| format!("{v:.0}")),
                    opt(r("read_errors"), |v| format!("{v:.0}"))
                ),
            );
            row(
                ui,
                "max late",
                opt(r("max_late_ms"), |v| format!("{v:.2} ms")),
            );
            row(ui, "sample rate", format!("{fs:.0} Hz"));
        });
        let current = st
            .config
            .as_ref()
            .and_then(|c| c["mains_hz"].as_u64())
            .map(|v| v as u32);
        if let Some(hz) = calib
            .recommended_mains_hz()
            .filter(|hz| Some(*hz) != current)
        {
            ui.label(
                RichText::new(format!(
                    "Hum is strongest at {hz} Hz but the notch is {}.",
                    current.map_or("unset".into(), |c| format!("{c} Hz"))
                ))
                .color(AMBER)
                .small(),
            );
            if ui
                .add_enabled(
                    st.seed_api.is_ok(),
                    egui::Button::new(format!("Apply recommended notch ({hz} Hz)")),
                )
                .clicked()
            {
                let mut ch = serde_json::Map::new();
                ch.insert("mains_hz".into(), serde_json::json!(hz));
                cog.put_config(&ch, ui.ctx());
            }
        }
        ui.separator();
        ui.heading("Pulse cross-check");
        ui.label(
            RichText::new("Feel the pulse at the wrist and tap along (button or Space).").small(),
        );
        ui.horizontal(|ui| {
            if ui
                .add(egui::Button::new(RichText::new("  TAP  ").size(18.0)))
                .clicked()
            {
                self.tap.tap(Instant::now());
            }
            if ui.button("reset").clicked() {
                self.tap.reset();
            }
        });
        match (self.tap.bpm(), hr) {
            (Some(t), Some(h)) => {
                let d = h - t;
                let col = if d.abs() <= 5.0 { GREEN } else { AMBER };
                ui.label(
                    RichText::new(format!("tapped {t:.0} bpm, cog {h:.0} bpm, diff {d:+.0}"))
                        .color(col),
                );
            }
            (Some(t), None) => {
                ui.label(format!("tapped {t:.0} bpm ({} taps)", self.tap.count()));
            }
            _ => {
                ui.label(format!("{} taps (need 4)", self.tap.count()));
            }
        }
        ui.separator();
        ui.heading("Record");
        ui.horizontal(|ui| {
            ui.label("name");
            ui.add(egui::TextEdit::singleline(&mut self.save_name).desired_width(140.0));
        });
        ui.horizontal_wrapped(|ui| {
            if ui.button("Save 60 s signal CSV").on_hover_text("t_ms, raw_v, filtered_mv, r_peak").clicked() {
                let csv = self.signal.lock().map(|s| s.to_csv()).unwrap_or_default();
                self.note = Some(save_text(ui.ctx(), &self.save_name, "signal.csv", csv));
            }
            if ui.button("RuView reference CSV").on_hover_text("ADR-293 ground truth: timestamp_ms,value (beat-to-beat bpm) for wifi-densepose-vitals").clicked() {
                let csv = self.signal.lock().map(|s| s.ruview_reference_csv()).unwrap_or_default();
                self.note = Some(save_text(ui.ctx(), &self.save_name, "hr-reference.csv", csv));
            }
        });
        if let Some(n) = &self.note {
            ui.label(RichText::new(n).small());
        }
        ui.label(
            RichText::new("Not a medical device. Power the Seed from a battery while pads are on.")
                .small()
                .color(AMBER),
        );
    }
}

/// (filtered mV, raw V, R markers) against seconds relative to the newest sample. Markers are
/// snapped to the filtered extreme within +/-40 ms (the cog marks R on its band-passed signal).
fn plot_series(window: &[Sample], peaks: &[u64], window_s: f64) -> Series {
    let Some(newest) = window.last().map(|s| s.t_ms) else {
        return Default::default();
    };
    let rel = |t: u64| (t as f64 - newest as f64) / 1000.0;
    let vis: Vec<Sample> = window
        .iter()
        .filter(|s| rel(s.t_ms) >= -window_s)
        .copied()
        .collect();
    let filtered = vis.iter().map(|s| [rel(s.t_ms), s.filtered_mv]).collect();
    let raw = vis.iter().map(|s| [rel(s.t_ms), s.raw_v]).collect();
    let rpk = peaks
        .iter()
        .filter(|&&p| rel(p) >= -window_s && p <= newest)
        .filter_map(|&p| analysis::snap_peak(&vis, p).map(|s| [rel(s.t_ms), s.filtered_mv]))
        .collect();
    (filtered, raw, rpk)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markers_snap_to_the_local_extreme() {
        let w: Vec<Sample> = (0..100)
            .map(|i| Sample {
                t_ms: i * 4,
                raw_v: 1.5,
                filtered_mv: if i == 50 { 900.0 } else { 0.0 },
            })
            .collect();
        // The cog marked R three samples late; the marker lands on the true peak.
        let (_, _, rpk) = plot_series(&w, &[212], 10.0);
        assert_eq!(rpk.len(), 1);
        assert!((rpk[0][1] - 900.0).abs() < 1e-9);
        assert!((rpk[0][0] - (200.0 - 396.0) / 1000.0).abs() < 1e-9);
    }
}
