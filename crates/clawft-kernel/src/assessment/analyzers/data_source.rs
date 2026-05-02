//! Data-source analyzer — detects database URIs, S3 buckets, and API base URLs.

use std::path::{Path, PathBuf};

use crate::assessment::{analyzer::AnalysisContext, Finding};
use crate::assessment::analyzer::Analyzer;

/// Analyzer that greps for connection strings and external service references.
pub struct DataSourceAnalyzer;

/// URI scheme prefixes that indicate database or storage connections.
const DATA_PREFIXES: &[(&str, &str)] = &[
    ("postgres://", "PostgreSQL"),
    ("postgresql://", "PostgreSQL"),
    ("mongodb://", "MongoDB"),
    ("mongodb+srv://", "MongoDB Atlas"),
    ("redis://", "Redis"),
    ("rediss://", "Redis (TLS)"),
    ("mysql://", "MySQL"),
    ("sqlite://", "SQLite"),
    ("s3://", "S3 bucket"),
    ("amqp://", "RabbitMQ"),
    ("nats://", "NATS"),
];

/// File extensions we consider as config/source that might contain URLs.
fn is_scannable(path: &Path) -> bool {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    matches!(
        ext,
        "rs" | "ts"
            | "tsx"
            | "js"
            | "jsx"
            | "py"
            | "go"
            | "rb"
            | "toml"
            | "yaml"
            | "yml"
            | "json"
            | "env"
            | "cfg"
            | "ini"
            | "conf"
            | "properties"
    ) || path
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.starts_with(".env"))
        .unwrap_or(false)
}

impl Analyzer for DataSourceAnalyzer {
    fn id(&self) -> &str {
        "data_source"
    }

    fn name(&self) -> &str {
        "Data Source Analyzer"
    }

    fn categories(&self) -> &[&str] {
        &["data_source"]
    }

    fn analyze(&self, project: &Path, files: &[PathBuf], _context: &AnalysisContext) -> Vec<Finding> {
        let mut findings = Vec::new();

        for path in files {
            if !is_scannable(path) {
                continue;
            }

            let rel = path.strip_prefix(project).unwrap_or(path);
            let rel_str = rel.display().to_string();

            let content = match std::fs::read_to_string(path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            for (i, line) in content.lines().enumerate() {
                let lower = line.to_lowercase();

                // Check data source prefixes
                for &(prefix, label) in DATA_PREFIXES {
                    if lower.contains(prefix) {
                        findings.push(Finding {
                            severity: "info".into(),
                            category: "data_source".into(),
                            file: rel_str.clone(),
                            line: Some(i + 1),
                            message: format!("{label} connection reference detected"),
                        });
                    }
                }

                // Detect HTTP API base URLs in config-like files
                // Look for patterns like BASE_URL, API_URL, ENDPOINT assigned to http(s)://
                let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                if (matches!(
                    ext,
                    "toml" | "yaml" | "yml" | "json" | "env" | "cfg" | "ini" | "conf" | "properties"
                ) || path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.starts_with(".env"))
                    .unwrap_or(false))
                    && (lower.contains("base_url")
                        || lower.contains("api_url")
                        || lower.contains("endpoint")
                        || lower.contains("base-url")
                        || lower.contains("api-url"))
                        && (lower.contains("http://") || lower.contains("https://"))
                    {
                        findings.push(Finding {
                            severity: "info".into(),
                            category: "data_source".into(),
                            file: rel_str.clone(),
                            line: Some(i + 1),
                            message: "HTTP API base URL reference detected".into(),
                        });
                    }
            }
        }

        findings
    }
}
