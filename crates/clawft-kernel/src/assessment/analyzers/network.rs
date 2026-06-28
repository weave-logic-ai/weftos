//! Network analyzer — discovers HTTP endpoints, webhooks, gRPC, and WebSocket URLs.

use std::path::{Path, PathBuf};

use crate::assessment::Finding;
use crate::assessment::analyzer::{AnalysisContext, Analyzer};

/// Analyzer that scans for network endpoint references in config and source files.
pub struct NetworkAnalyzer;

/// File extensions considered config files for network scanning.
fn is_config_ext(path: &Path) -> bool {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    matches!(ext, "toml" | "json" | "yaml" | "yml" | "env")
        || path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.starts_with(".env"))
            .unwrap_or(false)
}

/// File extensions considered source code (for hardcoded-URL detection).
fn is_source_ext(path: &Path) -> bool {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    matches!(
        ext,
        "rs" | "ts" | "tsx" | "js" | "jsx" | "py" | "go" | "rb" | "java"
    )
}

/// Webhook path patterns.
const WEBHOOK_PATTERNS: &[&str] = &["/webhook", "/hook", "/callback"];

/// API base-URL variable name patterns (case-insensitive match).
const API_VAR_PATTERNS: &[&str] = &["api_url", "base_url", "endpoint"];

impl Analyzer for NetworkAnalyzer {
    fn id(&self) -> &str {
        "network"
    }

    fn name(&self) -> &str {
        "Network Analyzer"
    }

    fn categories(&self) -> &[&str] {
        &["network"]
    }

    fn analyze(
        &self,
        project: &Path,
        files: &[PathBuf],
        _context: &AnalysisContext,
    ) -> Vec<Finding> {
        let mut findings = Vec::new();

        for path in files {
            let is_config = is_config_ext(path);
            let is_source = is_source_ext(path);

            if !is_config && !is_source {
                continue;
            }

            let rel = path.strip_prefix(project).unwrap_or(path);
            let rel_str = rel.display().to_string();

            let content = match std::fs::read_to_string(path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            for (i, line) in content.lines().enumerate() {
                let trimmed = line.trim();
                let lower = line.to_lowercase();
                let line_num = i + 1;

                // Skip comment lines
                if trimmed.starts_with("//")
                    || trimmed.starts_with('#')
                    || trimmed.starts_with("/*")
                    || trimmed.starts_with('*')
                {
                    continue;
                }

                // --- HTTP/HTTPS URLs ---
                let has_http = lower.contains("http://") || lower.contains("https://");
                if has_http {
                    // Extract URLs via simple scan
                    let urls = extract_urls(line);
                    for url in &urls {
                        // Determine severity: production URLs in source = medium,
                        // URLs in config = info
                        let severity = if is_source && is_production_url(url) {
                            "medium"
                        } else {
                            "info"
                        };

                        findings.push(Finding {
                            severity: severity.into(),
                            category: "network".into(),
                            file: rel_str.clone(),
                            line: Some(line_num),
                            message: format!("HTTP endpoint: {url}"),
                        });

                        // Check for webhook patterns in the URL
                        let url_lower = url.to_lowercase();
                        for pattern in WEBHOOK_PATTERNS {
                            if url_lower.contains(pattern) {
                                findings.push(Finding {
                                    severity: "info".into(),
                                    category: "network".into(),
                                    file: rel_str.clone(),
                                    line: Some(line_num),
                                    message: format!("Webhook URL detected: {url}"),
                                });
                                break;
                            }
                        }
                    }
                }

                // --- API base URL variable names in config ---
                if is_config {
                    for var_pat in API_VAR_PATTERNS {
                        if lower.contains(var_pat) {
                            findings.push(Finding {
                                severity: "info".into(),
                                category: "network".into(),
                                file: rel_str.clone(),
                                line: Some(line_num),
                                message: format!(
                                    "API base URL variable detected (matched '{var_pat}')"
                                ),
                            });
                            break;
                        }
                    }
                }

                // --- gRPC endpoints ---
                if lower.contains("grpc://") {
                    findings.push(Finding {
                        severity: "info".into(),
                        category: "network".into(),
                        file: rel_str.clone(),
                        line: Some(line_num),
                        message: "gRPC endpoint reference detected".into(),
                    });
                }
                if lower.contains("50051") {
                    findings.push(Finding {
                        severity: "info".into(),
                        category: "network".into(),
                        file: rel_str.clone(),
                        line: Some(line_num),
                        message: "Possible gRPC port (50051) reference".into(),
                    });
                }

                // --- WebSocket URLs ---
                if lower.contains("ws://") || lower.contains("wss://") {
                    findings.push(Finding {
                        severity: "info".into(),
                        category: "network".into(),
                        file: rel_str.clone(),
                        line: Some(line_num),
                        message: "WebSocket endpoint detected".into(),
                    });
                }
            }
        }

        findings
    }
}

/// Extract `http://` and `https://` URLs from a line of text.
fn extract_urls(line: &str) -> Vec<String> {
    let mut urls = Vec::new();
    let mut search = line;

    loop {
        let start = if let Some(pos) = search.find("https://") {
            pos
        } else if let Some(pos) = search.find("http://") {
            pos
        } else {
            break;
        };

        let rest = &search[start..];
        // URL ends at whitespace or quote/backtick delimiter
        let end = rest
            .find(|c: char| {
                c.is_whitespace()
                    || c == '"'
                    || c == '\''
                    || c == '`'
                    || c == '>'
                    || c == ')'
                    || c == ']'
            })
            .unwrap_or(rest.len());
        let url = &rest[..end];
        if url.len() > 8 {
            urls.push(url.to_string());
        }
        search = &rest[end..];
    }

    urls
}

/// Heuristic: does this URL look like a production endpoint?
fn is_production_url(url: &str) -> bool {
    let lower = url.to_lowercase();
    // Exclude localhost, example.com, placeholder domains
    if lower.contains("localhost")
        || lower.contains("127.0.0.1")
        || lower.contains("0.0.0.0")
        || lower.contains("example.com")
        || lower.contains("example.org")
        || lower.contains("placeholder")
        || lower.contains("{")
    {
        return false;
    }
    // If it looks like a real domain with a TLD, treat as production
    // Simple heuristic: contains a dot after the scheme
    let after_scheme = if let Some(rest) = lower.strip_prefix("https://") {
        rest
    } else if let Some(rest) = lower.strip_prefix("http://") {
        rest
    } else {
        return false;
    };
    after_scheme.contains('.')
}
