//! Pluggable analyzer trait and registry for the assessment pipeline.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{AssessmentReport, Finding};

/// Context passed to each analyzer during a scan.
#[derive(Debug, Clone, Default)]
pub struct AnalysisContext {
    /// The assessment scope (full, commit, ci, dependency).
    pub scope: String,
    /// The previous assessment report, if available.
    pub previous_report: Option<AssessmentReport>,
}

/// Trait for pluggable assessment analyzers.
///
/// Implement this trait to add a new analyzer to the assessment pipeline.
/// Each analyzer receives the project root, the list of scoped files, and
/// an `AnalysisContext`, and returns zero or more `Finding` values.
pub trait Analyzer: Send + Sync {
    /// Unique identifier for the analyzer (e.g. "complexity").
    fn id(&self) -> &str;
    /// Human-readable display name.
    fn name(&self) -> &str;
    /// Categories of findings this analyzer may produce.
    fn categories(&self) -> &[&str];
    /// Run analysis over the given files and return findings.
    fn analyze(&self, project: &Path, files: &[PathBuf], context: &AnalysisContext)
    -> Vec<Finding>;
}

/// Registry that holds all pluggable analyzers.
pub struct AnalyzerRegistry {
    analyzers: Vec<Box<dyn Analyzer>>,
}

impl AnalyzerRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            analyzers: Vec::new(),
        }
    }

    /// Create a registry pre-loaded with all built-in analyzers.
    pub fn with_defaults() -> Self {
        let mut reg = Self::new();
        reg.register(Box::new(super::analyzers::ComplexityAnalyzer::new()));
        reg.register(Box::new(super::analyzers::DependencyAnalyzer));
        reg.register(Box::new(super::analyzers::SecurityAnalyzer));
        reg.register(Box::new(super::analyzers::TopologyAnalyzer));
        reg.register(Box::new(super::analyzers::DataSourceAnalyzer));
        reg.register(Box::new(super::analyzers::NetworkAnalyzer));
        reg.register(Box::new(super::analyzers::RabbitMQAnalyzer));
        reg.register(Box::new(super::analyzers::TerraformAnalyzer));
        reg
    }

    /// Register a new analyzer.
    pub fn register(&mut self, analyzer: Box<dyn Analyzer>) {
        self.analyzers.push(analyzer);
    }

    /// Run all registered analyzers and collect their findings.
    pub fn run_all(
        &self,
        project: &Path,
        files: &[PathBuf],
        context: &AnalysisContext,
    ) -> Vec<Finding> {
        let mut findings = Vec::new();
        for analyzer in &self.analyzers {
            findings.extend(analyzer.analyze(project, files, context));
        }
        findings
    }

    /// Return the ids of all registered analyzers.
    pub fn analyzer_ids(&self) -> Vec<String> {
        self.analyzers.iter().map(|a| a.id().to_string()).collect()
    }
}

impl Default for AnalyzerRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Diff between two assessment reports.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssessmentDiff {
    /// Files present in current but not in previous.
    pub files_added: Vec<String>,
    /// Files present in previous but not in current.
    pub files_removed: Vec<String>,
    /// Findings in current that were not in previous.
    pub findings_new: Vec<Finding>,
    /// Findings in previous that are no longer in current.
    pub findings_resolved: Vec<Finding>,
    /// Change in complexity warnings (positive = more warnings).
    pub complexity_delta: i64,
    /// Change in coherence score (positive = improved).
    pub coherence_delta: f64,
}

/// Compute a diff between two assessment reports.
pub fn diff_reports(current: &AssessmentReport, previous: &AssessmentReport) -> AssessmentDiff {
    use std::collections::HashSet;

    // Collect file sets from findings + all scanned context
    let current_files: HashSet<String> = current.findings.iter().map(|f| f.file.clone()).collect();
    let previous_files: HashSet<String> =
        previous.findings.iter().map(|f| f.file.clone()).collect();

    let files_added: Vec<String> = current_files.difference(&previous_files).cloned().collect();
    let files_removed: Vec<String> = previous_files.difference(&current_files).cloned().collect();

    // Finding identity: (category, file, message)
    type FindingKey = (String, String, String);
    fn finding_key(f: &Finding) -> FindingKey {
        (f.category.clone(), f.file.clone(), f.message.clone())
    }

    let prev_keys: HashSet<FindingKey> = previous.findings.iter().map(finding_key).collect();
    let curr_keys: HashSet<FindingKey> = current.findings.iter().map(finding_key).collect();

    let findings_new: Vec<Finding> = current
        .findings
        .iter()
        .filter(|f| !prev_keys.contains(&finding_key(f)))
        .cloned()
        .collect();

    let findings_resolved: Vec<Finding> = previous
        .findings
        .iter()
        .filter(|f| !curr_keys.contains(&finding_key(f)))
        .cloned()
        .collect();

    let complexity_delta =
        current.summary.complexity_warnings as i64 - previous.summary.complexity_warnings as i64;

    let coherence_delta = current.summary.coherence_score - previous.summary.coherence_score;

    AssessmentDiff {
        files_added,
        files_removed,
        findings_new,
        findings_resolved,
        complexity_delta,
        coherence_delta,
    }
}
