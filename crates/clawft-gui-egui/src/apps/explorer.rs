//! Explorer — first-class sidebar app (WEFT-590). Substrate tree
//! browser graduated from `crates/clawft-gui-egui/src/explorer/mod.rs`.
//! DESIGN.md §9 sidebar 12.
//!
//! Layout: a small "intelligence summary" band sits above the
//! two-pane substrate browser. The summary surfaces the on-device
//! RNN + vector-DB state — HNSW entry count, search count, causal
//! graph node/edge totals, cognitive-tick interval — pulled from
//! `ecc.status` (cached on `snap.ecc_status`). When ECC is disabled
//! or hasn't reported yet the summary collapses to a single
//! "intelligence offline" line so it doesn't dominate the panel.
//!
//! All the real tree-walk work — left-tree navigation, right-detail
//! viewer cascade, viewer dispatch, `substrate.list` / `substrate.read`
//! polling, activity tracking, copy-actions — lives in
//! `crate::explorer::Explorer`, owned by the `Desktop`
//! (`desk.explorer`). This module paints the canonical heading + the
//! intelligence band, then delegates the body to
//! `desktop::render_explorer`, the helper that puts a connection
//! pill above the two-pane Explorer layout.
//!
//! Lifecycle: subscription cleanup on nav-AWAY is handled centrally
//! by [`crate::apps::dispatch`].

use std::sync::Arc;

use eframe::egui;
use serde_json::Value;

use crate::live::{Live, Snapshot};
use crate::shell::desktop::{self, Desktop};
use crate::theming::Tokens;

const HEADING_BAND_H: f32 = 64.0;
const INTEL_BAND_H: f32 = 56.0;

pub fn show(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    desk: &mut Desktop,
    live: &Arc<Live>,
    snap: &Snapshot,
) {
    super::paint_heading(ui, rect, "Explorer · substrate/");

    let intel_rect = egui::Rect::from_min_max(
        egui::pos2(rect.left(), rect.top() + HEADING_BAND_H),
        egui::pos2(rect.right(), rect.top() + HEADING_BAND_H + INTEL_BAND_H),
    );
    paint_intelligence_band(ui, intel_rect, snap);

    let body =
        egui::Rect::from_min_max(egui::pos2(rect.left(), intel_rect.bottom() + 4.0), rect.max);
    ui.scope_builder(egui::UiBuilder::new().max_rect(body), |ui| {
        desktop::render_explorer(ui, desk, live, snap);
    });
}

/// Render the inline RNN + vector-DB summary band. Pulls fields out of
/// `snap.ecc_status` (populated by the live driver's `ecc.status`
/// poll) and lays them out as four monospace KPI tiles. The band
/// always renders at fixed height — when ECC is disabled, the tiles
/// show "—" so the layout below doesn't shift between paints.
fn paint_intelligence_band(ui: &egui::Ui, rect: egui::Rect, snap: &Snapshot) {
    let painter = ui.painter_at(rect);
    let tokens = Tokens::default();

    let inner = rect.shrink2(egui::vec2(16.0, 8.0));
    painter.rect_filled(inner, tokens.rounding, tokens.bg_panel);
    painter.rect_stroke(
        inner,
        egui::CornerRadius::same(tokens.rounding.round() as u8),
        egui::Stroke::new(1.0, tokens.stroke_soft),
        egui::epaint::StrokeKind::Inside,
    );

    let kpis = build_intel_kpis(snap.ecc_status.as_ref());

    // Lay tiles left-to-right with a fixed pitch. Width chosen so four
    // tiles fit comfortably in a typical sidebar-app body width (~960
    // px); they wrap to "..." truncation if the host narrows further.
    let tile_w = 200.0_f32;
    let mut x = inner.left() + 12.0;
    let y = inner.top() + 6.0;
    for (label, value) in &kpis {
        if x + tile_w > inner.right() {
            break;
        }
        painter.text(
            egui::pos2(x, y),
            egui::Align2::LEFT_TOP,
            *label,
            egui::FontId::proportional(11.0),
            tokens.text_dim,
        );
        painter.text(
            egui::pos2(x, y + 16.0),
            egui::Align2::LEFT_TOP,
            value,
            egui::FontId::monospace(15.0),
            tokens.text_primary,
        );
        x += tile_w;
    }
}

fn build_intel_kpis(ecc: Option<&Value>) -> Vec<(&'static str, String)> {
    let Some(v) = ecc else {
        return vec![
            ("RNN tick", "—".into()),
            ("Vector entries", "—".into()),
            ("Causal graph", "—".into()),
            ("Crossrefs", "—".into()),
        ];
    };
    let enabled = v.get("enabled").and_then(|b| b.as_bool()).unwrap_or(false);
    if !enabled {
        return vec![
            ("RNN tick", "off".into()),
            ("Vector entries", "off".into()),
            ("Causal graph", "off".into()),
            ("Crossrefs", "off".into()),
        ];
    }
    let hnsw_entries = v.get("hnsw_entries").and_then(|n| n.as_u64()).unwrap_or(0);
    let crossref_count = v
        .get("crossref_count")
        .and_then(|n| n.as_u64())
        .unwrap_or(0);

    let tick = match v.get("cognitive_tick") {
        Some(t) if !t.is_null() => {
            let ms = t.get("interval_ms").and_then(|n| n.as_u64()).unwrap_or(0);
            let running = t.get("running").and_then(|b| b.as_bool()).unwrap_or(false);
            if running {
                format!("{ms} ms")
            } else {
                "paused".into()
            }
        }
        _ => "off".into(),
    };

    let causal = match v.get("causal_graph") {
        Some(g) if !g.is_null() => {
            let nodes = g.get("nodes").and_then(|n| n.as_u64()).unwrap_or(0);
            let edges = g.get("edges").and_then(|n| n.as_u64()).unwrap_or(0);
            format!("{nodes}n / {edges}e")
        }
        _ => "—".into(),
    };

    vec![
        ("RNN tick", tick),
        ("Vector entries", hnsw_entries.to_string()),
        ("Causal graph", causal),
        ("Crossrefs", crossref_count.to_string()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn build_kpis_uses_dashes_when_no_status() {
        let kpis = build_intel_kpis(None);
        assert_eq!(kpis.len(), 4);
        assert!(kpis.iter().all(|(_, v)| v == "—"));
    }

    #[test]
    fn build_kpis_marks_disabled_ecc() {
        let v = json!({"enabled": false, "hnsw_entries": 0});
        let kpis = build_intel_kpis(Some(&v));
        assert!(kpis.iter().all(|(_, v)| v == "off"));
    }

    #[test]
    fn build_kpis_pulls_real_numbers() {
        let v = json!({
            "enabled": true,
            "hnsw_entries": 1234,
            "cognitive_tick": {"interval_ms": 50, "running": true},
            "causal_graph": {"nodes": 80, "edges": 215},
            "crossref_count": 17,
        });
        let kpis = build_intel_kpis(Some(&v));
        assert_eq!(kpis[0].1, "50 ms");
        assert_eq!(kpis[1].1, "1234");
        assert_eq!(kpis[2].1, "80n / 215e");
        assert_eq!(kpis[3].1, "17");
    }

    #[test]
    fn build_kpis_handles_missing_subobjects() {
        let v = json!({"enabled": true, "hnsw_entries": 7});
        let kpis = build_intel_kpis(Some(&v));
        assert_eq!(kpis[0].1, "off"); // no cognitive_tick
        assert_eq!(kpis[1].1, "7");
        assert_eq!(kpis[2].1, "—"); // no causal_graph
        assert_eq!(kpis[3].1, "0"); // missing crossref_count -> 0
    }
}
