//! Security analyzer — scans for hardcoded secrets, .env files, and unsafe blocks.

use std::path::{Path, PathBuf};

use crate::assessment::analyzer::Analyzer;
use crate::assessment::{Finding, analyzer::AnalysisContext};

/// Analyzer that detects common security anti-patterns.
pub struct SecurityAnalyzer;

/// Patterns that suggest hardcoded secrets (case-insensitive prefix match).
const SECRET_PATTERNS: &[&str] = &[
    "api_key=",
    "api_key =",
    "apikey=",
    "apikey =",
    "password=",
    "password =",
    "passwd=",
    "passwd =",
    "secret=",
    "secret =",
    "token=",
    "token =",
    "aws_secret",
    "private_key",
];

impl Analyzer for SecurityAnalyzer {
    fn id(&self) -> &str {
        "security"
    }

    fn name(&self) -> &str {
        "Security Analyzer"
    }

    fn categories(&self) -> &[&str] {
        &["security"]
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
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

            // Detect committed .env files
            if name == ".env" || name.starts_with(".env.") {
                findings.push(Finding {
                    severity: "error".into(),
                    category: "security".into(),
                    file: rel_str.clone(),
                    line: None,
                    message: "Environment file should not be committed to version control".into(),
                });
                continue;
            }

            // Skip test files for secret detection to reduce noise
            let is_test = rel_str.contains("test")
                || rel_str.contains("spec")
                || rel_str.contains("fixture")
                || rel_str.contains("mock");

            let content = match std::fs::read_to_string(path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

            // Hardcoded secret patterns (skip test files)
            if !is_test {
                for (i, line) in content.lines().enumerate() {
                    let lower = line.to_lowercase();
                    // Skip comment lines
                    let trimmed = lower.trim();
                    if trimmed.starts_with("//")
                        || trimmed.starts_with('#')
                        || trimmed.starts_with("/*")
                        || trimmed.starts_with('*')
                    {
                        continue;
                    }

                    for pattern in SECRET_PATTERNS {
                        if lower.contains(pattern) {
                            // Verify there is an actual value (not just the key name)
                            if let Some(pos) = lower.find('=') {
                                let after = lower[pos + 1..].trim();
                                // Skip empty assignments and placeholder values
                                if !after.is_empty()
                                    && after != "\"\""
                                    && after != "''"
                                    && !after.starts_with("env")
                                    && !after.starts_with("std::env")
                                    && !after.starts_with("process.env")
                                    && !after.starts_with("os.environ")
                                {
                                    findings.push(Finding {
                                        severity: "error".into(),
                                        category: "security".into(),
                                        file: rel_str.clone(),
                                        line: Some(i + 1),
                                        message: format!(
                                            "Possible hardcoded secret (matched '{pattern}')"
                                        ),
                                    });
                                    break; // one finding per line
                                }
                            }
                        }
                    }
                }
            }

            // Detect unsafe blocks in Rust files
            if ext == "rs" {
                for (i, line) in content.lines().enumerate() {
                    let trimmed = line.trim();
                    if trimmed.contains("unsafe {")
                        || trimmed.contains("unsafe{")
                        || trimmed == "unsafe {"
                    {
                        findings.push(Finding {
                            severity: "warning".into(),
                            category: "security".into(),
                            file: rel_str.clone(),
                            line: Some(i + 1),
                            message: "Unsafe block detected — review for memory safety".into(),
                        });
                    }
                }
            }
        }

        findings
    }
}
