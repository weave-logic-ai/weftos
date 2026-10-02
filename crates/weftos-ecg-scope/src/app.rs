//! The egui UI: connection bar, hook-up checklist with wiring reference, the live ECG plot,
//! and the calibration tools (baseline, rails, hum, R amplitude, SNR, tap-along pulse check,
//! RuView ADR-293 reference export).

use crate::analysis::{self, Calibration, TapTempo};
use crate::checklist::{self, Mark};
use crate::model::State;
use crate::net::{Net, Settings};
use eframe::egui::{self, Color32, RichText};
use egui_plot::{Line, Plot, PlotPoints, Points};
use std::sync::{Arc, Mutex};
use web_time::Instant;

const GREEN: Color32 = Color32::from_rgb(70, 190, 110);
const AMBER: Color32 = Color32::from_rgb(230, 170, 40);
const RED: Color32 = Color32::from_rgb(220, 70, 70);
const GREY: Color32 = Color32::from_rgb(140, 140, 140);

const WIRING: [(&str, &str); 7] = [
    ("Seed pin 1 (3V3)", "ADS1115 VDD + SEN0213 +"),
    ("Seed pin 6 (GND)", "ADS1115 GND + SEN0213 -"),
    ("Seed pin 3 (SDA)", "ADS1115 SDA"),
    ("Seed pin 5 (SCL)", "ADS1115 SCL"),
    ("ADS1115 ADDR", "GND (address 0x48)"),
    ("SEN0213 A", "ADS1115 A0"),
    ("Never", "pin 2 / 4 (5 V)"),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Live,
    Guide,
}

/// Native: `ECG_SCOPE_TAB=guide` and `ECG_SCOPE_PAGE=<id>` open a guide page at start
/// (used with ECG_SCOPE_SCREENSHOT for docs and visual checks).
fn start_tab() -> Tab {
    #[cfg(not(target_arch = "wasm32"))]
    if std::env::var("ECG_SCOPE_TAB").is_ok_and(|t| t == "guide") {
        return Tab::Guide;
    }
    Tab::Live
}

fn start_page() -> Option<String> {
    #[cfg(not(target_arch = "wasm32"))]
    return std::env::var("ECG_SCOPE_PAGE").ok();
    #[cfg(target_arch = "wasm32")]
    None
}

/// (filtered mV, raw V, R markers) as plot points.
type Series = (Vec<[f64; 2]>, Vec<[f64; 2]>, Vec<[f64; 2]>);

pub struct ScopeApp {
    state: Arc<Mutex<State>>,
    net: Net,
    settings: Settings,
    /// Settings being edited in the top bar; applied with the Connect button.
    draft: Settings,
    window_s: f64,
    show_raw: bool,
    paused: Option<Series>,
    tap: TapTempo,
    save_path: String,
    note: Option<String>,
    cfg_draft: Option<CogConfig>,
    tab: Tab,
    guide: weftos_sensor_guide::GuideView,
}

impl ScopeApp {
    pub fn new(settings: Settings) -> Self {
        let state = Arc::new(Mutex::new(State::default()));
        Self {
            net: Net::new(state.clone()),
            state,
            draft: settings.clone(),
            settings,
            window_s: 8.0,
            show_raw: false,
            paused: None,
            tap: TapTempo::default(),
            save_path: "ecg-capture".into(),
            note: None,
            cfg_draft: None,
            tab: start_tab(),
            guide: weftos_sensor_guide::GuideView::at(start_page()),
        }
    }
}

/// The cog settings the app edits (the rest of the agent's config is passed through as-is).
#[derive(Clone, Debug, PartialEq)]
struct CogConfig {
    mains_hz: u32,
    i2c_addr: u8,
    channel: u8,
    sample_rate: u32,
    simulate: bool,
}

impl CogConfig {
    fn from_json(v: &serde_json::Value) -> Self {
        let n = |k: &str, d: u64| v[k].as_u64().unwrap_or(d);
        Self {
            mains_hz: n("mains_hz", 60) as u32,
            i2c_addr: n("i2c_addr", 72) as u8,
            channel: n("channel", 0) as u8,
            sample_rate: n("sample_rate", 250) as u32,
            simulate: v["simulate"].as_bool().unwrap_or(false),
        }
    }

    /// The agent's config PUT replaces the whole config (measured on firmware 0.24.2), so the
    /// edited keys are merged into the current config instead of sent alone.
    fn merged_into(&self, base: &serde_json::Value) -> serde_json::Value {
        let mut out = base.as_object().cloned().unwrap_or_default();
        for (k, v) in [
            ("mains_hz", serde_json::json!(self.mains_hz)),
            ("i2c_addr", serde_json::json!(self.i2c_addr)),
            ("channel", serde_json::json!(self.channel)),
            ("sample_rate", serde_json::json!(self.sample_rate)),
            ("simulate", serde_json::json!(self.simulate)),
        ] {
            out.insert(k.to_string(), v);
        }
        serde_json::Value::Object(out)
    }
}

/// Natively writes `<stem>-<suffix>`; in the browser copies the text to the clipboard.
fn save_text(ctx: &egui::Context, stem: &str, suffix: &str, text: String) -> String {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = ctx;
        let path = format!("{stem}-{suffix}");
        match std::fs::write(&path, text) {
            Ok(()) => format!("saved {path}"),
            Err(e) => format!("save failed: {e}"),
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (stem, suffix);
        let n = text.lines().count().saturating_sub(1);
        ctx.copy_text(text);
        format!("copied {n} rows to the clipboard")
    }
}

/// A coloured status dot (painted, so it does not depend on font glyph coverage).
fn mark_dot(ui: &mut egui::Ui, m: Mark) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 16.0), egui::Sense::hover());
    let c = rect.center();
    match m {
        Mark::Pass => _ = ui.painter().circle_filled(c, 5.5, GREEN),
        Mark::Warn => _ = ui.painter().circle_filled(c, 5.5, AMBER),
        Mark::Fail => _ = ui.painter().circle_filled(c, 5.5, RED),
        Mark::Waiting => {
            _ = ui
                .painter()
                .circle_stroke(c, 5.0, egui::Stroke::new(1.5, GREY))
        }
    }
}

fn opt(v: Option<f64>, fmt: impl Fn(f64) -> String) -> String {
    v.map_or_else(|| "–".into(), fmt)
}

impl eframe::App for ScopeApp {
    // eframe 0.34 requires `ui`; the app is driven from `update`, as in clawft-gui-egui.
    fn ui(&mut self, _ui: &mut egui::Ui, _frame: &mut eframe::Frame) {}

    #[allow(deprecated)]
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(web_time::Duration::from_millis(100));
        self.net.tick(&self.settings, ctx);
        #[cfg(not(target_arch = "wasm32"))]
        crate::shot::poll(ctx);
        if ctx.input(|i| i.key_pressed(egui::Key::Space)) && !ctx.wants_keyboard_input() {
            self.tap.tap(Instant::now());
        }

        // Snapshot shared state once per frame.
        let (
            fs,
            window,
            peaks,
            calib,
            steps,
            report,
            seed_ok,
            running,
            action,
            export_ok,
            source,
            cog_cfg,
        ) = {
            let st = match self.state.lock() {
                Ok(s) => s,
                Err(_) => return,
            };
            let fs = if st.signal.sample_rate_hz > 0.0 {
                st.signal.sample_rate_hz
            } else {
                250.0
            };
            let window = st.signal.tail(self.window_s.max(4.0));
            let peaks: Vec<u64> = st.signal.peaks_ms.iter().copied().collect();
            let cal_window = st.signal.tail(4.0);
            let calib: Calibration = analysis::calibrate(&cal_window, &peaks, fs);
            let configured_mains = st
                .cog_config
                .as_ref()
                .and_then(|c| c["mains_hz"].as_u64())
                .map(|v| v as u32);
            let steps = checklist::evaluate(&st, &calib, configured_mains);
            (
                fs,
                window,
                peaks,
                calib,
                steps,
                st.report.clone(),
                st.seed_api.is_ok(),
                st.cog_running,
                st.last_action.clone(),
                st.export.is_ok(),
                st.signal.source.clone(),
                st.cog_config.clone(),
            )
        };

        egui::TopBottomPanel::top("conn").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.strong("weft-ecg-scope");
                ui.separator();
                ui.label("Seed")
                    .on_hover_text("169.254.42.1 over USB, or the Seed's LAN / tailnet IP");
                let host = ui.add(
                    egui::TextEdit::singleline(&mut self.draft.seed_host).desired_width(150.0),
                );
                ui.label("token")
                    .on_hover_text("optional pairing token, if the agent refuses writes");
                ui.add(
                    egui::TextEdit::singleline(&mut self.draft.seed_token)
                        .password(true)
                        .desired_width(80.0),
                );
                let enter = host.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (ui.button("Connect").clicked() || enter) && self.draft != self.settings {
                    self.settings = self.draft.clone();
                    self.net.reset();
                    self.cfg_draft = None;
                    self.paused = None;
                }
                ui.label(
                    RichText::new(format!(
                        "agent {}  ·  export {}",
                        self.settings.agent_base(),
                        self.settings.export_base()
                    ))
                    .small()
                    .weak(),
                );
            });
            ui.horizontal_wrapped(|ui| {
                ui.add_enabled_ui(seed_ok, |ui| {
                    if ui
                        .button("Start cog")
                        .on_hover_text("continuous mode: keeps the signal export up")
                        .clicked()
                    {
                        self.net.action(&self.settings, "start", None, ctx);
                    }
                    if ui.button("Stop cog").clicked() {
                        self.net.action(&self.settings, "stop", None, ctx);
                    }
                    if ui
                        .button("Test run (simulate)")
                        .on_hover_text("console --once --simulate: checks the cog without hardware")
                        .clicked()
                    {
                        self.net.action(
                            &self.settings,
                            "console",
                            Some(serde_json::json!({"command": "--once --simulate"})),
                            ctx,
                        );
                    }
                });
                ui.label(match running {
                    Some(true) => RichText::new("cog running").color(GREEN),
                    Some(false) => RichText::new("cog stopped").color(AMBER),
                    None => RichText::new("cog ?").color(GREY),
                });
                if let Some(a) = &action {
                    ui.separator();
                    ui.label(RichText::new(a).italics());
                }
            });
        });

        egui::SidePanel::left("checklist").resizable(true).default_width(360.0).show(ctx, |ui| {
            ui.heading("Hook-up checklist");
            egui::ScrollArea::vertical().show(ui, |ui| {
                let mut open_step = None;
                for s in &steps {
                    ui.horizontal_top(|ui| {
                        mark_dot(ui, s.mark);
                        if ui.small_button("?").on_hover_text("open the guide page for this step").clicked() {
                            open_step = Some(s.id());
                        }
                        ui.vertical(|ui| {
                            ui.strong(s.title);
                            ui.label(RichText::new(&s.detail).small());
                        });
                    });
                    ui.add_space(4.0);
                }
                if let Some(id) = open_step {
                    let guide = self.state.lock().ok().and_then(|st| st.guide.clone());
                    if let Some(Ok(g)) = guide {
                        self.guide.open_for_step(&g, id);
                    }
                    self.tab = Tab::Guide;
                }
                ui.separator();
                egui::CollapsingHeader::new("Wiring (3.3 V only)").default_open(true).show(ui, |ui| {
                    egui::Grid::new("wiring").striped(true).show(ui, |ui| {
                        for (a, b) in WIRING {
                            ui.label(a);
                            ui.label(b);
                            ui.end_row();
                        }
                    });
                    ui.label(RichText::new("Pads: RA below right collarbone, LA below left, RL right lower ribs.").small());
                    ui.label(RichText::new("Not a medical device. Power the Seed from a battery while pads are on.").small().color(AMBER));
                });
            });
        });

        if self.tab == Tab::Live {
            egui::SidePanel::right("calib").resizable(true).default_width(300.0).show(ctx, |ui| {
            let hr = report.as_ref().and_then(|r| r["heart_rate_bpm"].as_f64());
            let status = report.as_ref().and_then(|r| r["status"].as_str()).unwrap_or("–").to_string();
            ui.label(RichText::new(opt(hr, |v| format!("{v:.0} bpm"))).size(40.0).strong());
            ui.label(format!("status: {status}"));
            let q = report.as_ref().and_then(|r| r["quality"].as_f64()).unwrap_or(0.0);
            ui.add(egui::ProgressBar::new(q as f32).text(format!("quality {q:.2}")));
            if source.starts_with("simulator") {
                ui.label(RichText::new("SIMULATED signal (--simulate)").color(AMBER).strong());
            }
            ui.separator();
            ui.heading("Calibration (last 4 s)");
            egui::Grid::new("cal").num_columns(2).show(ui, |ui| {
                let row = |ui: &mut egui::Ui, k: &str, v: String| {
                    ui.label(k);
                    ui.monospace(v);
                    ui.end_row();
                };
                row(ui, "baseline", opt(calib.baseline_v, |v| format!("{v:.3} V")));
                row(ui, "at rails", format!("{:.1} %", calib.rail_fraction * 100.0));
                row(ui, "hum 50 Hz", opt(calib.hum_50_mv, |v| format!("{v:.3} mV rms")));
                row(ui, "hum 60 Hz", opt(calib.hum_60_mv, |v| format!("{v:.3} mV rms")));
                row(ui, "R amplitude", opt(calib.r_amplitude_mv, |v| format!("{v:.2} mV")));
                row(ui, "noise floor", opt(calib.noise_mv, |v| format!("{v:.3} mV rms")));
                row(ui, "SNR", opt(calib.snr_db, |v| format!("{v:.1} dB")));
                let r = |k: &str| report.as_ref().and_then(|r| r[k].as_f64());
                row(ui, "SDNN / RMSSD", format!("{} / {}", opt(r("sdnn_ms"), |v| format!("{v:.0}")), opt(r("rmssd_ms"), |v| format!("{v:.0} ms"))));
                row(ui, "late / errors", format!("{} / {}", opt(r("late_samples"), |v| format!("{v:.0}")), opt(r("read_errors"), |v| format!("{v:.0}"))));
                row(ui, "max late", opt(r("max_late_ms"), |v| format!("{v:.2} ms")));
                row(ui, "sample rate", format!("{fs:.0} Hz"));
            });
            ui.separator();
            egui::CollapsingHeader::new("Cog settings").default_open(true).show(ui, |ui| {
                let Some(cfg) = cog_cfg.as_ref() else {
                    ui.label(RichText::new("waiting for the agent").small());
                    return;
                };
                if self.cfg_draft.is_none() {
                    self.cfg_draft = Some(CogConfig::from_json(cfg));
                }
                let current = CogConfig::from_json(cfg);
                if let Some(d) = self.cfg_draft.as_mut() {
                    egui::Grid::new("cogcfg").num_columns(2).show(ui, |ui| {
                        ui.label("mains notch");
                        egui::ComboBox::from_id_salt("mains").selected_text(format!("{} Hz", d.mains_hz)).show_ui(ui, |ui| {
                            for hz in [60, 50, 0] {
                                ui.selectable_value(&mut d.mains_hz, hz, if hz == 0 { "off".to_string() } else { format!("{hz} Hz") });
                            }
                        });
                        ui.end_row();
                        ui.label("ADS1115 address");
                        egui::ComboBox::from_id_salt("addr").selected_text(format!("0x{:02x}", d.i2c_addr)).show_ui(ui, |ui| {
                            for (a, pin) in [(0x48, "ADDR to GND"), (0x49, "ADDR to VDD"), (0x4A, "ADDR to SDA"), (0x4B, "ADDR to SCL")] {
                                ui.selectable_value(&mut d.i2c_addr, a, format!("0x{a:02x}  {pin}"));
                            }
                        });
                        ui.end_row();
                        ui.label("ADC input");
                        egui::ComboBox::from_id_salt("ch").selected_text(format!("A{}", d.channel)).show_ui(ui, |ui| {
                            for c in 0..4u8 {
                                ui.selectable_value(&mut d.channel, c, format!("A{c}"));
                            }
                        });
                        ui.end_row();
                        ui.label("sample rate");
                        ui.add(egui::Slider::new(&mut d.sample_rate, 100..=500).suffix(" Hz"));
                        ui.end_row();
                        ui.label("simulate");
                        ui.checkbox(&mut d.simulate, "synthetic 72 bpm (no hardware)");
                        ui.end_row();
                    });
                    let changed = *d != current;
                    ui.horizontal(|ui| {
                        if ui.add_enabled(changed && seed_ok, egui::Button::new("Apply (restarts cog)")).clicked() {
                            self.net.put_config(&self.settings, d.merged_into(cfg), ctx);
                        }
                        if ui.add_enabled(changed, egui::Button::new("Revert")).clicked() {
                            *d = current.clone();
                        }
                    });
                    if let Some(hz) = calib.recommended_mains_hz().filter(|hz| *hz != current.mains_hz) {
                        {
                            ui.label(RichText::new(format!("Hum is strongest at {hz} Hz but the notch is {} Hz.", current.mains_hz)).color(AMBER).small());
                            if ui.add_enabled(seed_ok, egui::Button::new(format!("Apply recommended notch ({hz} Hz)"))).clicked() {
                                let fixed = CogConfig { mains_hz: hz, ..current.clone() };
                                self.net.put_config(&self.settings, fixed.merged_into(cfg), ctx);
                                *d = fixed;
                            }
                        }
                    }
                }
            });
            ui.separator();
            ui.heading("Pulse cross-check");
            ui.label(RichText::new("Feel the pulse at the wrist and tap along (button or Space).").small());
            ui.horizontal(|ui| {
                if ui.add(egui::Button::new(RichText::new("  TAP  ").size(18.0))).clicked() {
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
                    ui.label(RichText::new(format!("tapped {t:.0} bpm, cog {h:.0} bpm, diff {d:+.0}")).color(col));
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
                ui.add(egui::TextEdit::singleline(&mut self.save_path).desired_width(140.0));
            });
            ui.horizontal_wrapped(|ui| {
                if ui.button("Save 60 s signal CSV").on_hover_text("t_ms, raw_v, filtered_mv, r_peak").clicked() {
                    let csv = self.state.lock().map(|s| s.signal.to_csv()).unwrap_or_default();
                    self.note = Some(save_text(ctx, &self.save_path, "signal.csv", csv));
                }
                if ui.button("RuView reference CSV").on_hover_text("ADR-293 ground truth: timestamp_ms,value (beat-to-beat bpm) for wifi-densepose-vitals").clicked() {
                    let csv = self.state.lock().map(|s| s.signal.ruview_reference_csv()).unwrap_or_default();
                    self.note = Some(save_text(ctx, &self.save_path, "hr-reference.csv", csv));
                }
            });
            if let Some(n) = &self.note {
                ui.label(RichText::new(n).small());
            }
        });
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::Live, RichText::new("Live").size(16.0));
                ui.selectable_value(&mut self.tab, Tab::Guide, RichText::new("Guide").size(16.0));
            });
            ui.separator();
            if self.tab == Tab::Guide {
                let guide = self.state.lock().ok().and_then(|st| st.guide.clone());
                match guide {
                    Some(Ok(g)) => self.guide.show(ui, &g),
                    Some(Err(e)) => {
                        ui.label(RichText::new(format!("The cog's guide could not be loaded: {e}")).color(RED));
                        ui.label("It is served by the cog at :8046/guide (cog 0.1.2+). Start the cog, or set ECG_GUIDE_DIR to a local guide folder.");
                    }
                    None => {
                        ui.label("Loading the guide from the cog...");
                    }
                }
                return;
            }
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
                if !export_ok {
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
        });
    }
}

/// (filtered mV, raw V, R markers) against seconds relative to the newest sample.
fn plot_series(window: &[crate::model::Sample], peaks: &[u64], window_s: f64) -> Series {
    let Some(newest) = window.last().map(|s| s.t_ms) else {
        return Default::default();
    };
    let rel = |t: u64| (t as f64 - newest as f64) / 1000.0;
    let vis: Vec<_> = window.iter().filter(|s| rel(s.t_ms) >= -window_s).collect();
    let filtered = vis.iter().map(|s| [rel(s.t_ms), s.filtered_mv]).collect();
    let raw = vis.iter().map(|s| [rel(s.t_ms), s.raw_v]).collect();
    let rpk = peaks
        .iter()
        .filter(|&&p| rel(p) >= -window_s && p <= newest)
        .filter_map(|&p| {
            vis.iter()
                .min_by_key(|s| s.t_ms.abs_diff(p))
                .map(|s| [rel(p), s.filtered_mv])
        })
        .collect();
    (filtered, raw, rpk)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_edits_merge_into_the_full_config() {
        let base = serde_json::json!({"api_bind":"0.0.0.0:8046","interval":5,"mains_hz":60,"simulate":false});
        let mut c = CogConfig::from_json(&base);
        assert_eq!(c.mains_hz, 60);
        c.mains_hz = 50;
        let out = c.merged_into(&base);
        assert_eq!(out["mains_hz"], 50);
        assert_eq!(out["api_bind"], "0.0.0.0:8046", "untouched keys survive");
        assert_eq!(out["interval"], 5);
    }
}
