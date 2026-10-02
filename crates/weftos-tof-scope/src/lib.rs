//! weft-tof-scope: companion app for the `sen0628-tof` cog (DFRobot SEN0628 8x8 matrix ToF on
//! a Cognitum Seed). Built on `weftos-cog-companion` (connection, checklist, settings, guide);
//! this crate adds the zone heatmap, the nearest/motion timeline, the sensor checks and the
//! levelling / noise calibration tools. Runs natively and in the browser.

pub mod model;

use eframe::egui::{self, Color32, RichText};
use egui_plot::{Line, Plot, PlotPoints};
use model::{Frame, Frames};
use std::sync::{Arc, Mutex};
use weftos_cog_companion::widgets::{AMBER, GREEN, GREY, RED, opt, save_text};
use weftos_cog_companion::{CogClient, CogState, Mark, SensorApp, Step, step};

const WIRING: &[(&str, &str)] = &[
    ("Seed pin 1 (3V3)", "SEN0628 +"),
    ("Seed pin 6 (GND)", "SEN0628 -"),
    ("Seed pin 5 (SCL)", "SEN0628 C"),
    ("Seed pin 3 (SDA)", "SEN0628 D"),
    ("Address", "0x33 (factory)"),
    ("Never", "pin 2 / 4 (5 V)"),
];

pub struct TofApp {
    frames: Arc<Mutex<Frames>>,
    show_mm: bool,
    scale_mm: u16,
    frozen: Option<Frame>,
    level: Option<model::Level>,
    save_name: String,
    note: Option<String>,
}

impl Default for TofApp {
    fn default() -> Self {
        Self {
            frames: Arc::new(Mutex::new(Frames::default())),
            show_mm: true,
            scale_mm: 3500,
            frozen: None,
            level: None,
            save_name: "tof-capture".into(),
            note: None,
        }
    }
}

fn status_hint(st: &CogState) -> (String, Option<String>) {
    let s = st.status_str("status").unwrap_or("").to_string();
    let err = st
        .status
        .as_ref()
        .and_then(|r| r["error"].as_str())
        .map(String::from);
    (s, err)
}

impl TofApp {
    fn heatmap(&self, ui: &mut egui::Ui, f: &Frame, st: &CogState) {
        let side = f.side;
        let avail = ui.available_size();
        let cell = ((avail.x.min(avail.y - 40.0)) / side as f32).clamp(20.0, 90.0);
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(cell * side as f32, cell * side as f32),
            egui::Sense::hover(),
        );
        let p = ui.painter_at(rect);
        // Computed from the displayed frame (the cog's report is up to one interval old):
        // occupied = closer than the cog's learned background by more than presence_mm.
        let background: Option<Vec<u16>> = st.status.as_ref().and_then(|r| {
            r["background_mm"]
                .as_array()
                .map(|a| a.iter().map(|v| v.as_u64().unwrap_or(0) as u16).collect())
        });
        let margin = st
            .config
            .as_ref()
            .and_then(|c| c["presence_mm"].as_u64())
            .unwrap_or(150) as u16;
        let occupied = model::occupied(&f.mm, background.as_deref(), margin);
        let nearest = model::nearest(&f.mm).map(|i| (i % side, i / side));
        for (i, &d) in f.mm.iter().enumerate() {
            let (x, y) = (i % side, i / side);
            let cr = egui::Rect::from_min_size(
                rect.min + egui::vec2(x as f32 * cell, y as f32 * cell),
                egui::vec2(cell - 2.0, cell - 2.0),
            );
            if model::valid(d) {
                let [r, g, b] = model::color_for(d, self.scale_mm);
                p.rect_filled(cr, 3.0, Color32::from_rgb(r, g, b));
            } else {
                p.rect_filled(cr, 3.0, Color32::from_gray(45));
                p.line_segment(
                    [cr.left_top(), cr.right_bottom()],
                    egui::Stroke::new(1.0, Color32::from_gray(80)),
                );
            }
            if self.show_mm && cell >= 36.0 {
                let txt = if model::valid(d) {
                    d.to_string()
                } else {
                    "-".into()
                };
                p.text(
                    cr.center(),
                    egui::Align2::CENTER_CENTER,
                    txt,
                    egui::FontId::monospace((cell / 4.2).min(15.0)),
                    Color32::BLACK,
                );
            }
            if occupied.contains(&i) {
                p.rect_stroke(
                    cr.shrink(2.0),
                    3.0,
                    egui::Stroke::new(2.0, Color32::from_rgb(250, 140, 30)),
                    egui::StrokeKind::Inside,
                );
            }
            if nearest == Some((x, y)) {
                p.rect_stroke(
                    cr,
                    3.0,
                    egui::Stroke::new(3.0, Color32::WHITE),
                    egui::StrokeKind::Outside,
                );
            }
        }
        ui.label(RichText::new("Looking out of the sensor: X0 left, Y0 top. White = nearest zone, orange = occupied (closer than background), grey = no target.").small().weak());
    }

    fn timeline(&self, ui: &mut egui::Ui) {
        let fr = match self.frames.lock() {
            Ok(f) => f.frames.iter().cloned().collect::<Vec<_>>(),
            Err(_) => return,
        };
        let Some(t_end) = fr.last().map(|f| f.t_ms) else {
            return;
        };
        let rel = |t: u64| (t as f64 - t_end as f64) / 1000.0;
        let nearest: Vec<[f64; 2]> = fr
            .iter()
            .filter_map(|f| {
                f.mm.iter()
                    .copied()
                    .filter(|d| model::valid(*d))
                    .min()
                    .map(|d| [rel(f.t_ms), f64::from(d)])
            })
            .collect();
        let motion: Vec<[f64; 2]> = fr
            .windows(2)
            .filter_map(|w| {
                let d: Vec<f64> = w[0]
                    .mm
                    .iter()
                    .zip(&w[1].mm)
                    .filter(|(a, b)| model::valid(**a) && model::valid(**b))
                    .map(|(a, b)| (f64::from(*a) - f64::from(*b)).abs())
                    .collect();
                (!d.is_empty()).then(|| [rel(w[1].t_ms), d.iter().sum::<f64>() / d.len() as f64])
            })
            .collect();
        Plot::new("tof_timeline")
            .height(ui.available_height().clamp(120.0, 220.0))
            .x_axis_label("s")
            .y_axis_label("mm")
            .include_x(-30.0)
            .include_x(0.0)
            .include_y(0.0)
            .allow_drag(false)
            .legend(egui_plot::Legend::default())
            .show(ui, |p| {
                p.line(Line::new("nearest", PlotPoints::from(nearest)).color(GREEN));
                p.line(Line::new("motion", PlotPoints::from(motion)).color(AMBER));
            });
    }
}

impl SensorApp for TofApp {
    fn cog_id(&self) -> &'static str {
        "sen0628-tof"
    }

    fn export_port(&self) -> u16 {
        8047
    }

    fn title(&self) -> &'static str {
        "weft-tof-scope"
    }

    fn wiring(&self) -> &'static [(&'static str, &'static str)] {
        WIRING
    }

    fn on_reconnect(&mut self) {
        if let Ok(mut f) = self.frames.lock() {
            *f = Frames::default();
        }
        self.frozen = None;
        self.level = None;
    }

    fn tick(&mut self, ctx: &egui::Context, cog: &mut CogClient) {
        let frames = self.frames.clone();
        cog.poll_export("/frames?seconds=1", 200, ctx, move |r| {
            if let (Ok(v), Ok(mut f)) = (r, frames.lock()) {
                let (fr, src) = model::parse_frames(&v);
                f.merge(fr);
                f.source = src;
            }
        });
    }

    fn steps(&self, st: &CogState) -> Vec<Step> {
        let (status, err) = status_hint(st);
        let valid_pct = st.status_f64("valid_pct");
        let fps = st.status_f64("frame_rate_hz");
        let noise = st.status_f64("mean_zone_noise_mm");
        let learned = st
            .status
            .as_ref()
            .and_then(|r| r["background_learned"].as_bool());
        vec![
            match status.as_str() {
                "no_source" => step(
                    "sensor_found",
                    "SEN0628 found on I2C",
                    Mark::Fail,
                    format!(
                        "{}. Pins 1/6/5/3, I2C mode, address 0x33; power-cycle after changing switches",
                        err.unwrap_or_default()
                    ),
                ),
                "" => step(
                    "sensor_found",
                    "SEN0628 found on I2C",
                    Mark::Waiting,
                    "no report yet",
                ),
                _ => step(
                    "sensor_found",
                    "SEN0628 found on I2C",
                    Mark::Pass,
                    st.status_str("source").unwrap_or("").to_string(),
                ),
            },
            match (status.as_str(), fps) {
                ("no_frames", _) => step(
                    "frames",
                    "Frames arriving",
                    Mark::Fail,
                    "sensor found but no frames: power-cycle the board",
                ),
                (_, Some(f)) if f >= 3.0 => step(
                    "frames",
                    "Frames arriving",
                    Mark::Pass,
                    format!("{f:.1} frames/s, {}", st.status_str("mode").unwrap_or("")),
                ),
                (_, Some(f)) => step(
                    "frames",
                    "Frames arriving",
                    Mark::Warn,
                    format!("only {f:.1} frames/s: the Seed is busy or I2C is slow"),
                ),
                _ => step(
                    "frames",
                    "Frames arriving",
                    Mark::Waiting,
                    "waiting for a report",
                ),
            },
            match valid_pct {
                Some(v) if v >= 70.0 => step(
                    "valid_zones",
                    "Zones see a target",
                    Mark::Pass,
                    format!("{v:.0}% of zones valid"),
                ),
                Some(v) if v > 0.0 => step(
                    "valid_zones",
                    "Zones see a target",
                    Mark::Warn,
                    format!(
                        "{v:.0}% valid: out of range (>3.5 m), glass, sunlight or dark surfaces"
                    ),
                ),
                Some(_) => step(
                    "valid_zones",
                    "Zones see a target",
                    Mark::Fail,
                    "no zone sees anything: lens covered, or everything is beyond 3.5 m",
                ),
                None => step(
                    "valid_zones",
                    "Zones see a target",
                    Mark::Waiting,
                    "needs frames",
                ),
            },
            match learned {
                Some(true) => step(
                    "background",
                    "Background learned",
                    Mark::Pass,
                    "learned at start; restart the cog with the room empty to re-learn",
                ),
                Some(false) => step(
                    "background",
                    "Background learned",
                    Mark::Warn,
                    "learning: keep the view empty for learn_seconds",
                ),
                None => step(
                    "background",
                    "Background learned",
                    Mark::Waiting,
                    "needs frames",
                ),
            },
            match &self.level {
                Some(l) if l.is_level() => step(
                    "level",
                    "Mounting level",
                    Mark::Pass,
                    format!(
                        "flat-wall check: {:+.1}% vertical, {:+.1}% horizontal",
                        l.vertical_pct, l.horizontal_pct
                    ),
                ),
                Some(l) => step(
                    "level",
                    "Mounting level",
                    Mark::Warn,
                    format!(
                        "{:+.1}% vertical, {:+.1}% horizontal: tilt the sensor toward the farther side",
                        l.vertical_pct, l.horizontal_pct
                    ),
                ),
                None => step(
                    "level",
                    "Mounting level",
                    Mark::Waiting,
                    "optional: aim square-on at a flat wall and press Check level",
                ),
            },
            match noise {
                Some(n) if n <= 15.0 => step(
                    "noise",
                    "Zone noise",
                    Mark::Pass,
                    format!("{n:.1} mm mean per-zone noise"),
                ),
                Some(n) => step(
                    "noise",
                    "Zone noise",
                    Mark::Warn,
                    format!("{n:.1} mm: motion in view, sunlight, or dark / shiny targets"),
                ),
                None => step("noise", "Zone noise", Mark::Waiting, "needs frames"),
            },
        ]
    }

    fn live(&mut self, ui: &mut egui::Ui, st: &CogState, _cog: &mut CogClient) {
        ui.horizontal(|ui| {
            ui.heading("Depth zones");
            ui.checkbox(&mut self.show_mm, "mm labels");
            ui.add(egui::Slider::new(&mut self.scale_mm, 500..=3500).text("colour scale (mm)"));
            let frozen = self.frozen.is_some();
            if ui
                .button(if frozen { "Resume" } else { "Freeze" })
                .clicked()
            {
                self.frozen = if frozen {
                    None
                } else {
                    self.frames.lock().ok().and_then(|f| f.latest().cloned())
                };
            }
            if !st.export.is_ok() {
                ui.label(RichText::new("no export").color(RED));
            }
        });
        let frame = self
            .frozen
            .clone()
            .or_else(|| self.frames.lock().ok().and_then(|f| f.latest().cloned()));
        match frame {
            Some(f) => {
                let h = ui.available_height();
                ui.allocate_ui(
                    egui::vec2(ui.available_width(), (h - 230.0).max(200.0)),
                    |ui| self.heatmap(ui, &f, st),
                );
                ui.separator();
                self.timeline(ui);
            }
            None => {
                ui.label("No frames yet. Follow the checklist on the left.");
            }
        }
    }

    fn side(&mut self, ui: &mut egui::Ui, st: &CogState, _cog: &mut CogClient) {
        let near = st.status.as_ref().map(|r| &r["nearest"]);
        let near_mm = near.and_then(|n| n["mm"].as_f64());
        ui.label(
            RichText::new(opt(near_mm, |v| format!("{v:.0} mm")))
                .size(36.0)
                .strong(),
        );
        if let Some((x, y)) = near.and_then(|n| n["x"].as_u64().zip(n["y"].as_u64())) {
            ui.label(format!("nearest at zone X{x} Y{y}"));
        }
        let presence = st.status.as_ref().and_then(|r| r["presence"].as_bool());
        ui.label(match presence {
            Some(true) => RichText::new("PRESENCE").color(AMBER).strong(),
            Some(false) => RichText::new("empty").color(GREEN),
            None => RichText::new("-").color(GREY),
        });
        ui.label(format!(
            "status: {}",
            st.status_str("status").unwrap_or("-")
        ));
        if self
            .frames
            .lock()
            .is_ok_and(|f| f.source.starts_with("simulator"))
        {
            ui.label(
                RichText::new("SIMULATED frames (--simulate)")
                    .color(AMBER)
                    .strong(),
            );
        }
        ui.separator();
        ui.heading("Readings");
        egui::Grid::new("tof_read").num_columns(2).show(ui, |ui| {
            let r = |k: &str| st.status_f64(k);
            let row = |ui: &mut egui::Ui, k: &str, v: String| {
                ui.label(k);
                ui.monospace(v);
                ui.end_row();
            };
            row(ui, "mode", st.status_str("mode").unwrap_or("-").to_string());
            row(
                ui,
                "frame rate",
                opt(r("frame_rate_hz"), |v| format!("{v:.1} Hz")),
            );
            row(
                ui,
                "valid zones",
                opt(r("valid_pct"), |v| format!("{v:.0} %")),
            );
            row(ui, "median", opt(r("median_mm"), |v| format!("{v:.0} mm")));
            row(ui, "motion", opt(r("motion_mm"), |v| format!("{v:.1} mm")));
            row(
                ui,
                "zone noise",
                opt(r("mean_zone_noise_mm"), |v| format!("{v:.1} mm")),
            );
            let sec = st
                .status
                .as_ref()
                .map(|r| r["sectors"].clone())
                .unwrap_or_default();
            let mm = |k: &str| sec[k].as_f64();
            row(
                ui,
                "left / mid / right",
                format!(
                    "{} / {} / {}",
                    opt(mm("left"), |v| format!("{v:.0}")),
                    opt(mm("middle"), |v| format!("{v:.0}")),
                    opt(mm("right"), |v| format!("{v:.0}"))
                ),
            );
            row(
                ui,
                "read errors",
                opt(r("read_errors"), |v| format!("{v:.0}")),
            );
        });
        ui.separator();
        ui.heading("Calibration");
        ui.label(
            RichText::new(
                "Levelling: aim the sensor square-on at a flat wall 1-2 m away, then check.",
            )
            .small(),
        );
        if ui.button("Check level").clicked() {
            self.level = self
                .frames
                .lock()
                .ok()
                .and_then(|f| f.latest().and_then(model::level));
            if self.level.is_none() {
                self.note =
                    Some("level check needs a frame with valid edge rows and columns".into());
            }
        }
        if let Some(l) = &self.level {
            let c = if l.is_level() { GREEN } else { AMBER };
            ui.label(
                RichText::new(format!(
                    "wall {:.0} mm · vertical {:+.1}% · horizontal {:+.1}%",
                    l.mean_mm, l.vertical_pct, l.horizontal_pct
                ))
                .color(c),
            );
        }
        let noise = self
            .frames
            .lock()
            .ok()
            .map(|f| model::zone_noise(&f.last_n(30)))
            .unwrap_or_default();
        let worst = noise
            .iter()
            .enumerate()
            .filter_map(|(i, n)| n.map(|v| (i, v)))
            .max_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((i, v)) = worst {
            ui.label(format!("noisiest zone (last 3 s): #{i} at {v:.1} mm"));
        }
        ui.separator();
        ui.heading("Record");
        ui.horizontal(|ui| {
            ui.label("name");
            ui.add(egui::TextEdit::singleline(&mut self.save_name).desired_width(140.0));
        });
        if ui
            .button("Save 30 s frames CSV")
            .on_hover_text("t_ms, side, z0..zN")
            .clicked()
        {
            let csv = self.frames.lock().map(|f| f.to_csv()).unwrap_or_default();
            self.note = Some(save_text(ui.ctx(), &self.save_name, "frames.csv", csv));
        }
        if let Some(n) = &self.note {
            ui.label(RichText::new(n).small());
        }
    }
}

/// Native entry.
#[cfg(not(target_arch = "wasm32"))]
pub fn run_native() -> eframe::Result<()> {
    weftos_cog_companion::run_native(TofApp::default(), "WeftOS ToF scope (sen0628-tof)")
}

/// Browser entry: mounts on `<canvas id=canvas_id>`; `?seed=<host>` picks the Seed.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub async fn tof_start(canvas_id: String) -> Result<(), wasm_bindgen::JsValue> {
    weftos_cog_companion::start_web(TofApp::default(), &canvas_id).await
}
