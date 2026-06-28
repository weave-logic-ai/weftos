//! Logs — `derived/logs/*` stream + Witness chain mode. DESIGN.md §9
//! sidebar 8, archetype `stream`, group expandable in sidebar.
//!
//! Graduated under WEFT-586 from the Phase 3 stub. The System tab
//! renders a newest-at-top monospace stream of log entries from
//! `snap.logs`, with a top-of-pane filter strip (All / Info / Warn /
//! Error). Each entry's level drives the row colour through the token
//! palette. The Witness chain tab delegates to the existing ExoChain
//! chip-detail surface for `substrate/chain/status`.

use std::sync::Arc;

use eframe::egui;
use serde_json::Value;

use crate::live::{Live, Snapshot};
use crate::shell::desktop::{self, Desktop};
use crate::shell::sidebar::LogsTab;
use crate::shell::tray;
use crate::theming::Tokens;

/// Persistent filter state for the System logs stream. Kept on
/// `Desktop` so the user's choice survives across paints and tab
/// switches. The filter drops any row whose `level` field doesn't
/// match the chosen severity.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum LogLevelFilter {
    #[default]
    All,
    Info,
    Warn,
    Error,
}

impl LogLevelFilter {
    /// Variants in the order the filter strip paints them.
    const STRIP: [(LogLevelFilter, &'static str); 4] = [
        (LogLevelFilter::All, "All"),
        (LogLevelFilter::Info, "Info"),
        (LogLevelFilter::Warn, "Warn"),
        (LogLevelFilter::Error, "Error"),
    ];

    /// Whether this filter accepts a row whose normalized level is
    /// `row_level`. Unknown / missing levels are treated as "info" —
    /// the most common default for tools that don't tag every entry —
    /// so they pass under both `All` and `Info`.
    fn accepts(self, row_level: &str) -> bool {
        match self {
            LogLevelFilter::All => true,
            LogLevelFilter::Info => matches!(row_level, "info" | "" | "debug" | "trace"),
            LogLevelFilter::Warn => row_level == "warn" || row_level == "warning",
            LogLevelFilter::Error => matches!(row_level, "error" | "err" | "crit" | "fatal"),
        }
    }
}

pub fn show(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    desk: &mut Desktop,
    live: &Arc<Live>,
    snap: &Snapshot,
    tab: LogsTab,
) {
    let tab_name = match tab {
        LogsTab::System => "System",
        LogsTab::WitnessChain => "Witness chain",
    };
    super::paint_heading(ui, rect, &format!("Logs · {}", tab_name));

    let body = egui::Rect::from_min_max(egui::pos2(rect.left(), rect.top() + 64.0), rect.max);

    let has_data = match tab {
        LogsTab::System => snap.logs.as_ref().map(|v| !v.is_empty()).unwrap_or(false),
        LogsTab::WitnessChain => snap.chain_status.is_some(),
    };
    let (what, hint) = match tab {
        LogsTab::System => (
            "No logs published yet",
            "Logs flow through `derived/logs/*` once a service writes.",
        ),
        LogsTab::WitnessChain => (
            "Witness chain not initialised",
            "Run `weaver chain init` to create the chain.",
        ),
    };

    if super::state::render_if_needed(ui, body, snap, has_data, what, Some(hint)) {
        return;
    }

    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(body)
            .layout(egui::Layout::top_down(egui::Align::LEFT)),
    );

    match tab {
        LogsTab::System => render_system(&mut child, &mut desk.log_filter, snap),
        LogsTab::WitnessChain => {
            // Re-use the canonical chip-detail surface for
            // `substrate/chain/status` — same data the ExoChain chip
            // showed, now living inside the Logs · Witness chain tab.
            desktop::render_chip_detail(&mut child, desk, tray::ChipId::ExoChain, live, snap);
        }
    }
}

/// Render the System logs tab — filter strip + stream view.
fn render_system(ui: &mut egui::Ui, filter: &mut LogLevelFilter, snap: &Snapshot) {
    let tokens = Tokens::default();

    // ── Filter strip ────────────────────────────────────────────────
    ui.horizontal(|ui| {
        for (variant, label) in LogLevelFilter::STRIP {
            if ui.selectable_label(*filter == variant, label).clicked() {
                *filter = variant;
            }
        }
    });
    ui.separator();

    // ── Stream view ─────────────────────────────────────────────────
    // `render_if_needed` already guaranteed `snap.logs` is `Some` and
    // non-empty. Reverse the iter so the newest entry is on top — the
    // canonical stream-archetype convention from DESIGN.md §4.1.
    let Some(rows) = snap.logs.as_ref() else {
        return;
    };

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for row in rows.iter().rev() {
                let level = row_level(row);
                if !filter.accepts(&level) {
                    continue;
                }
                let line = format_row(row, &level);
                let color = level_color(&level, &tokens);
                ui.label(
                    egui::RichText::new(line)
                        .monospace()
                        .size(12.0)
                        .color(color),
                );
            }
        });
}

/// Pull the row's level from the JSON object, lower-cased. Falls back
/// to `""` (empty) when no recognised key is present — the filter and
/// colourer treat that as "info-ish".
fn row_level(row: &Value) -> String {
    row.get("level")
        .or_else(|| row.get("severity"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default()
}

/// Format one log row for the stream view. Prefers a recognised
/// message field (`msg`/`message`/`text`), otherwise falls back to a
/// compact JSON dump so nothing is silently dropped. Always tags the
/// line with `[level]` so the colour-blind path remains readable.
fn format_row(row: &Value, level: &str) -> String {
    let msg = row
        .get("msg")
        .or_else(|| row.get("message"))
        .or_else(|| row.get("text"))
        .and_then(|v| v.as_str());
    let level_tag = if level.is_empty() {
        "log".to_string()
    } else {
        level.to_string()
    };
    match msg {
        Some(m) => format!("[{level_tag}] {m}"),
        None => format!("[{level_tag}] {row}"),
    }
}

/// Map a normalized level string to a Tokens palette colour. Stays in
/// the design-token universe — no raw `Color32::from_rgb` literals.
fn level_color(level: &str, tokens: &Tokens) -> egui::Color32 {
    match level {
        "warn" | "warning" => tokens.warn,
        "error" | "err" | "crit" | "fatal" => tokens.crit,
        _ => tokens.text_dim,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn filter_all_accepts_everything() {
        for lvl in ["", "info", "warn", "error", "debug", "weird"] {
            assert!(LogLevelFilter::All.accepts(lvl), "All should accept {lvl}");
        }
    }

    #[test]
    fn filter_info_includes_unlabelled_rows() {
        // Rows without a level still need somewhere to land — Info is
        // the default fallback bucket so unattributed rows aren't
        // silently dropped when the user picks Info.
        assert!(LogLevelFilter::Info.accepts(""));
        assert!(LogLevelFilter::Info.accepts("info"));
        assert!(!LogLevelFilter::Info.accepts("warn"));
        assert!(!LogLevelFilter::Info.accepts("error"));
    }

    #[test]
    fn filter_warn_strict() {
        assert!(LogLevelFilter::Warn.accepts("warn"));
        assert!(LogLevelFilter::Warn.accepts("warning"));
        assert!(!LogLevelFilter::Warn.accepts("info"));
        assert!(!LogLevelFilter::Warn.accepts("error"));
    }

    #[test]
    fn filter_error_covers_aliases() {
        for lvl in ["error", "err", "crit", "fatal"] {
            assert!(
                LogLevelFilter::Error.accepts(lvl),
                "Error should accept {lvl}"
            );
        }
        assert!(!LogLevelFilter::Error.accepts("warn"));
    }

    #[test]
    fn row_level_reads_known_keys() {
        assert_eq!(row_level(&json!({"level": "WARN"})), "warn");
        assert_eq!(row_level(&json!({"severity": "Error"})), "error");
        assert_eq!(row_level(&json!({})), "");
    }

    #[test]
    fn format_row_prefers_msg_then_message_then_text() {
        let row_msg = json!({"level": "info", "msg": "A"});
        let row_message = json!({"level": "info", "message": "B"});
        let row_text = json!({"level": "info", "text": "C"});
        let row_none = json!({"level": "info", "extra": 1});
        assert_eq!(format_row(&row_msg, "info"), "[info] A");
        assert_eq!(format_row(&row_message, "info"), "[info] B");
        assert_eq!(format_row(&row_text, "info"), "[info] C");
        // Falls through to compact JSON on no-known-field rows.
        let dumped = format_row(&row_none, "info");
        assert!(dumped.starts_with("[info] "));
        assert!(dumped.contains("\"extra\""));
    }

    #[test]
    fn level_color_resolves_to_tokens() {
        let t = Tokens::default();
        assert_eq!(level_color("warn", &t), t.warn);
        assert_eq!(level_color("warning", &t), t.warn);
        assert_eq!(level_color("error", &t), t.crit);
        assert_eq!(level_color("crit", &t), t.crit);
        assert_eq!(level_color("info", &t), t.text_dim);
        assert_eq!(level_color("", &t), t.text_dim);
    }
}
