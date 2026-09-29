//! Human-readable explanation of a [`Decision`] (the `--explain` output).
//!
//! Deterministic: candidates are already ranked, numbers use two decimals,
//! flags are sorted. Snapshot tests pin the format.

use std::fmt::Write as _;

use super::decision::{Decision, NodeReport, ScoreBreakdown};

fn score_line(s: &ScoreBreakdown) -> String {
    format!(
        "execution {:.2} + locality {:.2} + accel_fit {:.2} + perf {:.2} + load {:.2} + stickiness {:.2}",
        s.execution, s.locality, s.accel_fit, s.perf, s.load, s.stickiness
    )
}

fn flags(r: &NodeReport) -> String {
    if r.flags.is_empty() {
        String::new()
    } else {
        let f: Vec<&str> = r.flags.iter().map(|f| f.as_str()).collect();
        format!(" [{}]", f.join(", "))
    }
}

/// Render the decision as plain text.
pub fn explain(d: &Decision) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "workload {} (kind {})", d.name, d.kind);
    let _ = writeln!(
        out,
        "  allow_emulated: {}   pin: {}",
        d.allow_emulated,
        d.pin.as_deref().unwrap_or("-")
    );
    match &d.placement {
        Some(p) => {
            let _ = writeln!(
                out,
                "decision: PLACED on {} via {} (tier {}, emulated {}) score {:.2}{}",
                p.node_id,
                p.variant,
                p.tier.as_str(),
                p.emulated(),
                p.score.total(),
                p.accelerator
                    .as_deref()
                    .map(|a| format!(", accelerator {a}"))
                    .unwrap_or_default()
            );
        }
        None => {
            let _ = writeln!(out, "decision: UNPLACEABLE (no eligible node)");
        }
    }
    let _ = writeln!(out, "candidates:");
    for (rank, c) in d.candidates.iter().enumerate() {
        match &c.score {
            Some(s) if c.eligible() => {
                let _ = writeln!(
                    out,
                    "  {}. {} eligible via {} (tier {}) score {:.2}{}",
                    rank + 1,
                    c.node_id,
                    c.variant.as_deref().unwrap_or("-"),
                    c.tier.map(|t| t.as_str()).unwrap_or("-"),
                    s.total(),
                    flags(c)
                );
                let _ = writeln!(out, "       {}", score_line(s));
            }
            _ => {
                let _ = writeln!(out, "  -  {} rejected", c.node_id);
                for r in &c.rejections {
                    let _ = writeln!(out, "       {}: {}", r.constraint.as_str(), r.detail);
                }
            }
        }
    }
    out
}
