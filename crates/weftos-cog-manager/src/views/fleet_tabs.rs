//! The node detail tabs that only read the snapshot: Health (fleet P3), Trust / licence and
//! Software / firmware (P4), and Raw. Admin actions are shown as commands to copy, never run.

use super::fleet::prov_pill;
use crate::fleet::{self, Sample};
use crate::style;
use crate::{AMBER, GREEN, GREY, RED, WL};
use eframe::egui::{self, RichText, Ui};
use serde_json::Value;
use std::collections::VecDeque;

fn grid(ui: &mut Ui, salt: &str, rows: Vec<(String, String, Option<&str>)>) {
    egui::Grid::new(salt).num_columns(3).spacing([14.0, 4.0]).show(ui, |ui| {
        for (k, v, p) in rows {
            ui.label(style::dim(ui, k));
            ui.add(egui::Label::new(style::body(&v)).truncate()).on_hover_text(&v);
            prov_pill(ui, p);
            ui.end_row();
        }
    });
}

fn bytes(b: u64) -> String {
    const G: f64 = 1024.0 * 1024.0 * 1024.0;
    if b as f64 >= G { format!("{:.1} GiB", b as f64 / G) } else { format!("{:.0} MiB", b as f64 / (1024.0 * 1024.0)) }
}

/// A small line chart of `values` (gaps where a reading is missing).
fn sparkline(ui: &mut Ui, label: &str, values: &[Option<f64>], unit: &str) {
    ui.horizontal(|ui| {
        ui.label(style::dim(ui, label));
        let (rect, _) = ui.allocate_exact_size(egui::vec2(220.0, 34.0), egui::Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_stroke(rect, 2.0, egui::Stroke::new(1.0, GREY.gamma_multiply(0.4)), egui::StrokeKind::Inside);
        let known: Vec<f64> = values.iter().flatten().copied().collect();
        let Some(max) = known.iter().copied().reduce(f64::max) else {
            painter.text(rect.center(), egui::Align2::CENTER_CENTER, "no readings yet", egui::FontId::proportional(11.0), GREY);
            return;
        };
        let max = if max <= 0.0 { 1.0 } else { max };
        let n = values.len().max(2) as f32 - 1.0;
        let mut seg: Vec<egui::Pos2> = Vec::new();
        for (i, v) in values.iter().enumerate() {
            match v {
                Some(v) => seg.push(egui::pos2(
                    rect.left() + rect.width() * i as f32 / n,
                    rect.bottom() - 3.0 - (rect.height() - 6.0) * (*v / max) as f32,
                )),
                None if !seg.is_empty() => {
                    painter.add(egui::Shape::line(std::mem::take(&mut seg), egui::Stroke::new(1.5, WL)));
                }
                None => {}
            }
        }
        if seg.len() == 1 {
            painter.circle_filled(seg[0], 2.0, WL);
        } else if !seg.is_empty() {
            painter.add(egui::Shape::line(seg, egui::Stroke::new(1.5, WL)));
        }
        let last = known.last().copied().unwrap_or(0.0);
        ui.label(RichText::new(format!("{last:.1}{unit} (max {max:.1})")).small());
    });
}

pub(crate) fn node_health(ui: &mut Ui, n: &Value, now: u64, hist: &VecDeque<Sample>) {
    let mesh = fleet::val(n, "mesh");
    let cluster = fleet::val(n, "cluster");
    let age = |v: &Value| v.as_u64().map_or_else(|| "-".into(), |t| fleet::age(now, t));
    let s = |v: &Value| v.as_str().map_or_else(|| "-".into(), str::to_owned);
    let pc = fleet::provenance(n, "cluster");
    let pm = fleet::provenance(n, "mesh");
    let mut rows: Vec<(String, String, Option<&str>)> = vec![
        ("cluster state".into(), s(&cluster["state"]), pc),
        ("last announce".into(), age(&cluster["last_announce_unix"]), pc),
    ];
    if mesh.is_object() {
        let seen = mesh["last_seen_unix"].as_u64();
        let stale = seen.is_some_and(|t| now.saturating_sub(t) > 60);
        rows.extend([
            ("mesh class".into(), s(&mesh["class"]), pm),
            ("verified".into(), mesh["verified"].to_string(), pm),
            ("heartbeat".into(), s(&mesh["heartbeat"]), pm),
            ("connected at".into(), s(&mesh["connected_at"]), pm),
            ("last pong".into(), format!("{}{}", age(&mesh["last_seen_unix"]), if stale { " (stale)" } else { "" }), pm),
            ("rtt (smoothed)".into(), mesh["rtt_ms"].as_f64().map_or_else(|| "not measured yet".into(), |v| format!("{v:.1} ms")), pm),
            ("missed pongs".into(), mesh["missed_pongs"].as_u64().map_or_else(|| "-".into(), |m| m.to_string()), pm),
        ]);
    } else {
        rows.push(("mesh".into(), "not connected to this daemon now".into(), None));
    }
    match fleet::load(n) {
        Some(l) => {
            let p = fleet::provenance(n, "load");
            let cores = l.cores.map(|c| format!(" on {c} cores")).unwrap_or_default();
            rows.push(("load (1m / 5m)".into(), format!("{:.2} / {:.2}{cores}", l.load1, l.load5), p));
            if let (Some(a), Some(t)) = (l.mem_avail, l.mem_total) {
                rows.push(("memory available".into(), format!("{} of {}", bytes(a), bytes(t)), p));
            }
        }
        None => rows.push(("load".into(), "not reported by this node".into(), None)),
    }
    if let Some((busy, total, free)) = fleet::capacity(n) {
        let free = free.map(|f| format!(", {} free", bytes(f))).unwrap_or_default();
        rows.push(("capabilities in use".into(), format!("{busy} of {total}{free}"), fleet::provenance(n, "facts")));
    }
    grid(ui, "node_health", rows);
    if fleet::provenance(n, "load") == Some("peer_claimed") {
        ui.label(RichText::new("load is what the peer reports about itself over its verified connection; it is not signed").color(AMBER).small());
    }
    ui.add_space(style::GAP_S);
    let rtt: Vec<Option<f64>> = hist.iter().map(|s| s.rtt_ms).collect();
    let load: Vec<Option<f64>> = hist.iter().map(|s| s.load1).collect();
    sparkline(ui, "rtt    ", &rtt, " ms");
    sparkline(ui, "load 1m", &load, "");
    ui.label(style::dim(ui, format!("{} reading(s) since the console connected, one per snapshot", hist.len())));
}

pub(crate) fn node_trust(ui: &mut Ui, n: &Value, id: &str, now: u64, licence: Option<&Value>) {
    let t = fleet::trust(n);
    let pf = fleet::provenance(n, "facts");
    let at = |v: Option<u64>| v.map_or_else(|| "-".into(), |x| if x > now { format!("in {}", fleet::age(x, now).trim_end_matches(" ago")) } else { fleet::age(now, x) });
    grid(
        ui,
        "node_trust",
        vec![
            ("trust tier".into(), t.tier.clone().unwrap_or_else(|| "no facts from this node".into()), pf),
            ("tier source".into(), t.tier_source.clone().unwrap_or_else(|| "-".into()), pf),
            ("facts received".into(), at(t.received_at), pf),
            ("facts expire".into(), at(t.expires_at), pf),
            ("delta seq".into(), t.delta_seq.map_or_else(|| "-".into(), |d| d.to_string()), pf),
        ],
    );
    if t.signed {
        ui.label(RichText::new("✔ facts signature verified by the daemon (the signed envelope is in Raw)").color(GREEN).small());
    } else if t.tier.is_some() {
        ui.label(RichText::new("facts present but not labelled signed").color(AMBER).small());
    }
    if let Some(reason) = &t.revoked {
        ui.colored_label(RED, format!("revoked{}", if reason.is_empty() { String::new() } else { format!(": {reason}") }));
    }
    let local = n["local"] == true;
    if let (true, Some(l)) = (local, licence) {
        ui.add_space(style::GAP_S);
        style::subhead(ui, "Licence (this node)");
        let lv = &l["value"];
        if lv.is_null() {
            ui.label(style::dim(ui, l["unavailable"].as_str().unwrap_or("licence status not available on this daemon")));
        } else {
            let b = &lv["binding"]["value"];
            let s = |v: &Value| if v.is_null() { "-".to_string() } else { v.as_str().map_or_else(|| v.to_string(), str::to_owned) };
            grid(
                ui,
                "node_licence",
                vec![
                    ("mesh id".into(), s(&lv["mesh_id"]), l["provenance"].as_str()),
                    ("genesis pinned".into(), s(&lv["genesis_pinned"]), l["provenance"].as_str()),
                    ("binding".into(), if b.is_null() { "no Seed bound".into() } else { s(&b["state"]) }, lv["binding"]["provenance"].as_str()),
                    ("binding seq".into(), s(&b["seq"]), lv["binding"]["provenance"].as_str()),
                    ("grant key".into(), s(&b["grant_fingerprint"]), lv["binding"]["provenance"].as_str()),
                    ("orphaned".into(), s(&b["orphaned"]), lv["binding"]["provenance"].as_str()),
                ],
            );
        }
    }
    ui.add_space(style::GAP_S);
    style::subhead(ui, "Admin commands (copy; the console never runs them)");
    for (what, cmd) in fleet::admin_commands(id, t.revoked.is_some(), local) {
        ui.horizontal_wrapped(|ui| {
            ui.label(style::dim(ui, what));
            ui.code(&cmd);
            if ui.small_button("copy").clicked() {
                ui.ctx().copy_text(cmd.clone());
            }
        });
    }
}

pub(crate) fn node_software(ui: &mut Ui, n: &Value) {
    let rows = fleet::software(n);
    if rows.is_empty() {
        ui.label(style::dim(ui, "no signed facts from this node, so no OS, CPU or board details"));
    } else {
        let p = fleet::provenance(n, "facts");
        grid(ui, "node_software", rows.into_iter().map(|(k, v)| (k, v, p)).collect());
    }
    ui.add_space(style::GAP_S);
    ui.label(style::dim(ui, "WeftOS version: not in node facts yet. ESP32 firmware is the edge heartbeat's self-reported `fw` (Network tab, edge nodes). Seed upgrade checks wait for a verified Seed API capture."));
}

pub(crate) fn node_raw(ui: &mut Ui, n: &Value) {
    ui.horizontal_wrapped(|ui| {
        for (section, p) in fleet::sections(n) {
            ui.label(style::dim(ui, section));
            prov_pill(ui, Some(&p));
        }
    });
    let text = serde_json::to_string_pretty(n).unwrap_or_default();
    if ui.small_button("copy JSON").clicked() {
        ui.ctx().copy_text(text.clone());
    }
    egui::ScrollArea::vertical().max_height(360.0).id_salt("node_raw").show(ui, |ui| {
        ui.add(egui::Label::new(RichText::new(text).monospace().small()).selectable(true));
    });
}
