//! Phase 3 — ops dashboard MVP.
//!
//! Renders the gap sweep + store coherence scores into a plain-text report
//! and a structured JSON report. The text format is what the operations team
//! would see in their morning queue; the JSON is what drives the UI (future
//! phase) and the CoherenceAlert impulses pushed back into ECC.

use crate::coherence::StoreCoherence;
use crate::gaps::{Gap, GapPattern, GapReport, GapSeverity};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dashboard {
    pub summary: Summary,
    pub top_alerts: Vec<Gap>,
    pub worst_stores: Vec<StoreCoherence>,
    pub best_stores: Vec<StoreCoherence>,
    pub pattern_breakdown: Vec<PatternRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Summary {
    pub total_stores: usize,
    pub total_gaps: usize,
    pub critical_gaps: usize,
    pub high_gaps: usize,
    pub medium_gaps: usize,
    pub low_gaps: usize,
    pub avg_ops_health: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatternRow {
    pub pattern: String,
    pub count: usize,
}

pub fn build(gaps: &GapReport, mut scores: Vec<StoreCoherence>, top_n: usize) -> Dashboard {
    crate::coherence::rank_by_health(&mut scores);
    let total_stores = scores.len();
    let avg = if total_stores > 0 {
        scores.iter().map(|s| s.ops_health).sum::<f64>() / total_stores as f64
    } else {
        0.0
    };

    let summary = Summary {
        total_stores,
        total_gaps: gaps.total(),
        critical_gaps: gaps.count_severity(GapSeverity::Critical),
        high_gaps: gaps.count_severity(GapSeverity::High),
        medium_gaps: gaps.count_severity(GapSeverity::Medium),
        low_gaps: gaps.count_severity(GapSeverity::Low),
        avg_ops_health: (avg * 1000.0).round() / 1000.0,
    };

    let mut top_alerts: Vec<Gap> = gaps
        .gaps
        .iter()
        .filter(|g| g.severity == GapSeverity::Critical || g.severity == GapSeverity::High)
        .cloned()
        .collect();
    top_alerts.sort_by(|a, b| b.severity.cmp(&a.severity));
    top_alerts.truncate(top_n);

    let worst_stores: Vec<_> = scores.iter().take(top_n).cloned().collect();
    let best_stores: Vec<_> = scores.iter().rev().take(top_n).cloned().collect();

    let pattern_breakdown: Vec<PatternRow> = GapPattern::all()
        .iter()
        .map(|p| PatternRow {
            pattern: p.as_str().into(),
            count: gaps.count(*p),
        })
        .collect();

    Dashboard {
        summary,
        top_alerts,
        worst_stores,
        best_stores,
        pattern_breakdown,
    }
}

pub fn render_text(dash: &Dashboard) -> String {
    let mut s = String::new();
    s.push_str("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");
    s.push_str("  QSR OPS DASHBOARD — Phase 3 Gap Sweep\n");
    s.push_str("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n\n");

    s.push_str(&format!("  Stores: {}\n", dash.summary.total_stores));
    s.push_str(&format!(
        "  Gaps:   {} total  (critical={} high={} medium={} low={})\n",
        dash.summary.total_gaps,
        dash.summary.critical_gaps,
        dash.summary.high_gaps,
        dash.summary.medium_gaps,
        dash.summary.low_gaps,
    ));
    s.push_str(&format!(
        "  Avg ops health: {:.3}\n\n",
        dash.summary.avg_ops_health
    ));

    s.push_str("  ── Pattern breakdown ────────────────────────────────\n");
    for row in &dash.pattern_breakdown {
        s.push_str(&format!("    {:32}  {:>4}\n", row.pattern, row.count));
    }
    s.push('\n');

    s.push_str("  ── Worst-performing stores (by ops_health) ──────────\n");
    for sc in &dash.worst_stores {
        s.push_str(&format!(
            "    {:42}  health={:.3}  λ₂={:.3}  gaps={:.2}\n",
            truncate_label(&sc.store_ref, 42),
            sc.ops_health,
            sc.org_lambda_2,
            sc.gap_weight,
        ));
    }
    s.push('\n');

    s.push_str("  ── Top critical/high alerts ─────────────────────────\n");
    for gap in &dash.top_alerts {
        let sev = format!("{:?}", gap.severity).to_uppercase();
        s.push_str(&format!(
            "    [{:8}] {:34}  {}\n",
            sev,
            truncate_label(gap.pattern.as_str(), 34),
            gap.message,
        ));
    }
    s.push('\n');
    s
}

fn truncate_label(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(n - 1).collect();
        t.push('…');
        t
    }
}
