//! Shared companion widgets: the hook-up checklist (generic connection steps + the app's sensor
//! steps, blocked in order, each with a "?" link into the cog's guide) and a cog settings panel
//! generated from the cog's manifest (`[config]` types, ranges, units, options).

use crate::Probe;
use crate::client::CogState;
use eframe::egui::{self, Color32, RichText};
use serde_json::{Map, Value};

pub const GREEN: Color32 = Color32::from_rgb(70, 190, 110);
pub const AMBER: Color32 = Color32::from_rgb(230, 170, 40);
pub const RED: Color32 = Color32::from_rgb(220, 70, 70);
pub const GREY: Color32 = Color32::from_rgb(140, 140, 140);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    Pass,
    Warn,
    Fail,
    Waiting,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    /// Stable id; the cog guide's `[links]` maps it to a page.
    pub id: &'static str,
    pub title: &'static str,
    pub mark: Mark,
    pub detail: String,
}

pub fn step(id: &'static str, title: &'static str, mark: Mark, detail: impl Into<String>) -> Step {
    Step {
        id,
        title,
        mark,
        detail: detail.into(),
    }
}

/// The four connection steps every sensor cog shares.
pub fn connection_steps(st: &CogState, cog_id: &str, export: &str) -> Vec<Step> {
    vec![
        match &st.seed_api {
            Probe::Ok => step(
                "seed_api",
                "Seed agent reachable",
                Mark::Pass,
                "agent app API answers",
            ),
            Probe::Failed(e) => step(
                "seed_api",
                "Seed agent reachable",
                Mark::Fail,
                format!("{e}. USB: 169.254.42.1; or the Seed's LAN / tailnet IP"),
            ),
            Probe::Unknown => step("seed_api", "Seed agent reachable", Mark::Waiting, "probing"),
        },
        match st.cog_installed {
            Some(true) => step(
                "cog_installed",
                "Cog installed",
                Mark::Pass,
                format!("{cog_id} is in /api/v1/apps"),
            ),
            Some(false) => step(
                "cog_installed",
                "Cog installed",
                Mark::Fail,
                format!("sideload it: scripts/seed-sideload.sh {cog_id} <seed>"),
            ),
            None => step(
                "cog_installed",
                "Cog installed",
                Mark::Waiting,
                "needs the agent",
            ),
        },
        match st.cog_running {
            Some(true) => step(
                "cog_running",
                "Cog running",
                Mark::Pass,
                "continuous mode keeps the export up",
            ),
            _ => step("cog_running", "Cog running", Mark::Fail, "press Start cog"),
        },
        match &st.export {
            Probe::Ok => step(
                "export",
                "Signal export reachable",
                Mark::Pass,
                export.to_string(),
            ),
            Probe::Failed(e) => step(
                "export",
                "Signal export reachable",
                Mark::Fail,
                format!("{e}. Start the cog; check its api_bind is 0.0.0.0"),
            ),
            Probe::Unknown => step(
                "export",
                "Signal export reachable",
                Mark::Waiting,
                "probing",
            ),
        },
    ]
}

/// Steps after the first failure wait for it.
pub fn chain(steps: Vec<Step>) -> Vec<Step> {
    let mut blocked = false;
    steps
        .into_iter()
        .map(|s| {
            let s = if blocked {
                Step {
                    mark: Mark::Waiting,
                    detail: "waiting for the step above".into(),
                    ..s
                }
            } else {
                s
            };
            if s.mark == Mark::Fail {
                blocked = true;
            }
            s
        })
        .collect()
}

pub fn mark_dot(ui: &mut egui::Ui, m: Mark) {
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

/// Renders the checklist; returns the id of a step whose "?" was clicked.
pub fn checklist(ui: &mut egui::Ui, steps: &[Step]) -> Option<&'static str> {
    let mut clicked = None;
    for s in steps {
        ui.horizontal_top(|ui| {
            mark_dot(ui, s.mark);
            if ui
                .small_button("?")
                .on_hover_text("open the guide page for this step")
                .clicked()
            {
                clicked = Some(s.id);
            }
            ui.vertical(|ui| {
                ui.strong(s.title);
                ui.label(RichText::new(&s.detail).small());
            });
        });
        ui.add_space(4.0);
    }
    clicked
}

pub fn opt(v: Option<f64>, fmt: impl Fn(f64) -> String) -> String {
    v.map_or_else(|| "-".into(), fmt)
}

fn as_f64(v: &Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

/// A settings panel generated from the manifest's typed config. Edits accumulate in `draft`;
/// returns the changed keys when the user presses Apply.
pub fn config_panel(
    ui: &mut egui::Ui,
    st: &CogState,
    draft: &mut Option<Map<String, Value>>,
    enabled: bool,
) -> Option<Map<String, Value>> {
    let (Some(manifest), Some(config)) = (st.manifest.as_ref(), st.config.as_ref()) else {
        ui.label(RichText::new("waiting for the agent").small());
        return None;
    };
    let fields = manifest["config"].as_array().cloned().unwrap_or_default();
    let current = config.as_object().cloned().unwrap_or_default();
    let d = draft.get_or_insert_with(|| current.clone());
    let mut show = |ui: &mut egui::Ui, advanced: bool| {
        egui::Grid::new(("cogcfg", advanced))
            .num_columns(2)
            .show(ui, |ui| {
                for f in fields
                    .iter()
                    .filter(|f| f["advanced"].as_bool().unwrap_or(false) == advanced)
                {
                    let Some(key) = f["key"].as_str() else {
                        continue;
                    };
                    let label = f["label"].as_str().unwrap_or(key);
                    let unit = f["unit"].as_str().unwrap_or("");
                    let resp = ui.label(label);
                    if let Some(desc) = f["description"].as_str() {
                        resp.on_hover_text(desc);
                    }
                    let default = f["default"].clone();
                    let v = d.entry(key.to_string()).or_insert(default);
                    match f["type"].as_str().unwrap_or("string") {
                        "boolean" => {
                            let mut b = v.as_bool().unwrap_or(false);
                            if ui.checkbox(&mut b, "").changed() {
                                *v = Value::Bool(b);
                            }
                        }
                        "integer" => {
                            let mut n = as_f64(v).unwrap_or(0.0) as i64;
                            let lo = f["min"].as_i64().unwrap_or(i64::MIN);
                            let hi = f["max"].as_i64().unwrap_or(i64::MAX);
                            if ui
                                .add(
                                    egui::DragValue::new(&mut n)
                                        .range(lo..=hi)
                                        .suffix(format!(" {unit}")),
                                )
                                .changed()
                            {
                                *v = Value::from(n);
                            }
                        }
                        "float" | "number" => {
                            let mut n = as_f64(v).unwrap_or(0.0);
                            let lo = f["min"].as_f64().unwrap_or(f64::MIN);
                            let hi = f["max"].as_f64().unwrap_or(f64::MAX);
                            let step = f["step"].as_f64().unwrap_or(0.1);
                            if ui
                                .add(
                                    egui::DragValue::new(&mut n)
                                        .range(lo..=hi)
                                        .speed(step)
                                        .suffix(format!(" {unit}")),
                                )
                                .changed()
                            {
                                *v = Value::from(n);
                            }
                        }
                        "select" => {
                            let opts: Vec<Value> =
                                f["options"].as_array().cloned().unwrap_or_default();
                            let show_v = |x: &Value| {
                                x.as_str()
                                    .map(String::from)
                                    .unwrap_or_else(|| x.to_string())
                            };
                            egui::ComboBox::from_id_salt(("sel", key))
                                .selected_text(show_v(v))
                                .show_ui(ui, |ui| {
                                    for o in opts {
                                        let txt = show_v(&o);
                                        if ui.selectable_label(*v == o, txt).clicked() {
                                            *v = o.clone();
                                        }
                                    }
                                });
                        }
                        _ => {
                            let mut s = v.as_str().unwrap_or("").to_string();
                            if ui
                                .add(egui::TextEdit::singleline(&mut s).desired_width(140.0))
                                .changed()
                            {
                                *v = Value::String(s);
                            }
                        }
                    }
                    ui.end_row();
                }
            });
    };
    show(ui, false);
    if fields
        .iter()
        .any(|f| f["advanced"].as_bool().unwrap_or(false))
    {
        egui::CollapsingHeader::new("Advanced")
            .default_open(false)
            .show(ui, |ui| show(ui, true));
    }
    let changes: Map<String, Value> = d
        .iter()
        .filter(|(k, v)| current.get(*k) != Some(*v))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let mut apply = None;
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                enabled && !changes.is_empty(),
                egui::Button::new("Apply (restarts cog)"),
            )
            .clicked()
        {
            apply = Some(changes.clone());
        }
        if ui
            .add_enabled(!changes.is_empty(), egui::Button::new("Revert"))
            .clicked()
        {
            *draft = None;
        }
    });
    if apply.is_some() {
        *draft = None;
    }
    apply
}

/// Natively writes `<stem>-<suffix>`; in the browser copies the text to the clipboard.
pub fn save_text(ctx: &egui::Context, stem: &str, suffix: &str, text: String) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_steps_cover_agent_cog_and_export() {
        let st = CogState {
            seed_api: Probe::Ok,
            cog_installed: Some(true),
            cog_running: Some(false),
            ..Default::default()
        };
        let steps = chain(connection_steps(&st, "x", "http://h:1"));
        let marks: Vec<Mark> = steps.iter().map(|s| s.mark).collect();
        assert_eq!(
            marks,
            vec![Mark::Pass, Mark::Pass, Mark::Fail, Mark::Waiting]
        );
        assert_eq!(
            steps.iter().map(|s| s.id).collect::<Vec<_>>(),
            vec!["seed_api", "cog_installed", "cog_running", "export"]
        );
    }

    #[test]
    fn chain_blocks_everything_after_the_first_failure() {
        let s = chain(vec![
            step("a", "A", Mark::Warn, ""),
            step("b", "B", Mark::Fail, "x"),
            step("c", "C", Mark::Pass, ""),
        ]);
        assert_eq!(s[0].mark, Mark::Warn);
        assert_eq!(s[2].mark, Mark::Waiting);
    }
}
