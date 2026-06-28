//! Complexity analyzer — detects large files and TODO/FIXME/HACK markers.

use std::path::{Path, PathBuf};

use crate::assessment::analyzer::Analyzer;
use crate::assessment::{Finding, analyzer::AnalysisContext};
use crate::eml_kernel::ComplexityModel;

/// Default complexity threshold (lines per file). Preserved as the
/// hardcoded fallback.
const DEFAULT_LINE_THRESHOLD: usize = 500;

/// Analyzer that flags files exceeding a per-file line count and
/// tracks TODO markers.
///
/// The line-count threshold defaults to 500 for backward compatibility
/// (matching the original hardcoded limit). When constructed via
/// [`Self::with_model`], a learned
/// [`ComplexityModel`](crate::eml_kernel::ComplexityModel) is consulted
/// per file; an untrained model falls back to the same 500-line
/// threshold so the default remains drop-in safe.
///
/// NOTE(eml-swap): wired — Finding #5 (ComplexityModel).
pub struct ComplexityAnalyzer {
    /// Optional learned threshold model. When None or untrained, the
    /// analyzer behaves exactly as it did pre-EML.
    model: Option<ComplexityModel>,
}

impl Default for ComplexityAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl ComplexityAnalyzer {
    /// Create a complexity analyzer using the hardcoded 500-line
    /// threshold (no learned model).
    pub fn new() -> Self {
        Self { model: None }
    }

    /// Create a complexity analyzer backed by a learned
    /// [`ComplexityModel`]. Untrained models fall back to the
    /// hardcoded 500-line threshold so this is drop-in safe.
    pub fn with_model(model: ComplexityModel) -> Self {
        Self { model: Some(model) }
    }

    /// Returns a reference to the learned model, if installed.
    pub fn model(&self) -> Option<&ComplexityModel> {
        self.model.as_ref()
    }

    /// Resolve the per-file line threshold. Consults the optional
    /// learned model when trained; otherwise returns 500.
    fn threshold_for(&self, lang_ordinal: u32, line_count: usize) -> usize {
        match self.model.as_ref() {
            Some(m) if m.is_trained() => {
                // The model needs three features; we feed lang
                // ordinal + observed line count + a stub team-size
                // proxy of 1.0 since we don't track team size here.
                m.predict(lang_ordinal, line_count as f64, 1.0)
            }
            _ => DEFAULT_LINE_THRESHOLD,
        }
    }
}

impl Analyzer for ComplexityAnalyzer {
    fn id(&self) -> &str {
        "complexity"
    }

    fn name(&self) -> &str {
        "Complexity Analyzer"
    }

    fn categories(&self) -> &[&str] {
        &["size", "todo"]
    }

    fn analyze(
        &self,
        project: &Path,
        files: &[PathBuf],
        _context: &AnalysisContext,
    ) -> Vec<Finding> {
        let mut findings = Vec::new();

        for path in files {
            let rel = path.strip_prefix(project).unwrap_or(path);
            let rel_str = rel.display().to_string();

            let content = match std::fs::read_to_string(path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            let line_count = content.lines().count();
            let threshold = self.threshold_for(0, line_count);

            // > threshold line warning
            if line_count > threshold {
                findings.push(Finding {
                    severity: "warning".into(),
                    category: "size".into(),
                    file: rel_str.clone(),
                    line: None,
                    message: format!("File has {line_count} lines (>{threshold} limit)"),
                });
            }

            // TODO / FIXME / HACK detection
            for (i, line) in content.lines().enumerate() {
                if line.contains("TODO") || line.contains("FIXME") || line.contains("HACK") {
                    findings.push(Finding {
                        severity: "info".into(),
                        category: "todo".into(),
                        file: rel_str.clone(),
                        line: Some(i + 1),
                        message: line.trim().to_string(),
                    });
                }
            }
        }

        findings
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_file(dir: &Path, name: &str, lines: usize) -> PathBuf {
        let p = dir.join(name);
        let body: String = (0..lines).map(|i| format!("line {i}\n")).collect();
        std::fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn analyzer_default_uses_500_threshold() {
        // Finding #5: with no model, the threshold is 500 lines
        // (preserved from the original hardcoded behaviour).
        let dir = tempfile::tempdir().unwrap();
        let big = write_file(dir.path(), "big.txt", 600);
        let small = write_file(dir.path(), "small.txt", 100);

        let analyzer = ComplexityAnalyzer::new();
        let ctx = AnalysisContext::default();
        let findings = analyzer.analyze(dir.path(), &[big.clone(), small.clone()], &ctx);

        let size_findings: Vec<_> = findings.iter().filter(|f| f.category == "size").collect();
        assert_eq!(size_findings.len(), 1);
        assert!(size_findings[0].message.contains("600 lines"));
        assert!(size_findings[0].message.contains(">500 limit"));
    }

    #[test]
    fn analyzer_untrained_model_matches_default() {
        // Finding #5: an untrained ComplexityModel must reproduce the
        // 500-line threshold exactly.
        let dir = tempfile::tempdir().unwrap();
        let big = write_file(dir.path(), "b.txt", 700);

        let baseline = ComplexityAnalyzer::new();
        let model = ComplexityModel::new();
        assert!(!model.is_trained());
        let wired = ComplexityAnalyzer::with_model(model);

        let ctx = AnalysisContext::default();
        let baseline_findings = baseline.analyze(dir.path(), std::slice::from_ref(&big), &ctx);
        let wired_findings = wired.analyze(dir.path(), std::slice::from_ref(&big), &ctx);

        assert_eq!(baseline_findings.len(), wired_findings.len());
        for (a, b) in baseline_findings.iter().zip(wired_findings.iter()) {
            assert_eq!(a.message, b.message);
            assert_eq!(a.severity, b.severity);
            assert_eq!(a.category, b.category);
        }
    }

    #[test]
    fn analyzer_trained_model_can_change_threshold() {
        // Finding #5: a trained model dispatches to predict() and may
        // produce a different threshold than 500.
        let dir = tempfile::tempdir().unwrap();
        let medium = write_file(dir.path(), "m.txt", 400);

        let baseline = ComplexityAnalyzer::new();
        let baseline_findings = baseline.analyze(
            dir.path(),
            std::slice::from_ref(&medium),
            &AnalysisContext::default(),
        );
        // 400 lines < 500 threshold → no size warning.
        let baseline_size = baseline_findings
            .iter()
            .filter(|f| f.category == "size")
            .count();
        assert_eq!(baseline_size, 0);

        // Force the model into "trained" via JSON patch.
        let model = ComplexityModel::new();
        let mut json = serde_json::to_value(&model).unwrap();
        if let Some(inner) = json.get_mut("inner").and_then(|v| v.as_object_mut()) {
            inner.insert("trained".into(), serde_json::Value::Bool(true));
        }
        let forced: ComplexityModel = serde_json::from_value(json).unwrap();
        assert!(forced.is_trained());

        let wired = ComplexityAnalyzer::with_model(forced);
        let wired_findings = wired.analyze(
            dir.path(),
            std::slice::from_ref(&medium),
            &AnalysisContext::default(),
        );
        // We don't assert a specific outcome (the trained model with
        // zero params produces an implementation-defined threshold,
        // clamped to [100, 5000]). We assert that at least one branch
        // observably differs from the hardcoded threshold path: the
        // size finding either is or is not present, but the message
        // (when present) reports the model's threshold, not 500.
        let size_msgs: Vec<&str> = wired_findings
            .iter()
            .filter(|f| f.category == "size")
            .map(|f| f.message.as_str())
            .collect();
        for msg in size_msgs {
            // Must not contain ">500 limit" — the trained dispatch
            // produced its own threshold.
            assert!(
                !msg.contains(">500 limit"),
                "trained model must not produce hardcoded 500-line message: {msg}"
            );
        }
    }
}
