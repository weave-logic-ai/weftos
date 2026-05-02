//! Dependency analyzer — parses Cargo.toml and package.json for dependency info.

use std::path::{Path, PathBuf};

use crate::assessment::{analyzer::AnalysisContext, Finding};
use crate::assessment::analyzer::Analyzer;

/// Analyzer that inspects dependency manifests for version info and counts.
pub struct DependencyAnalyzer;

impl Analyzer for DependencyAnalyzer {
    fn id(&self) -> &str {
        "dependency"
    }

    fn name(&self) -> &str {
        "Dependency Analyzer"
    }

    fn categories(&self) -> &[&str] {
        &["dependency"]
    }

    fn analyze(&self, project: &Path, files: &[PathBuf], _context: &AnalysisContext) -> Vec<Finding> {
        let mut findings = Vec::new();

        for path in files {
            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n,
                None => continue,
            };
            let rel = path.strip_prefix(project).unwrap_or(path);
            let rel_str = rel.display().to_string();

            match name {
                "Cargo.toml" => {
                    findings.extend(analyze_cargo_toml(path, &rel_str));
                }
                "package.json" => {
                    findings.extend(analyze_package_json(path, &rel_str));
                }
                _ => {}
            }
        }

        findings
    }
}

fn analyze_cargo_toml(path: &Path, rel_str: &str) -> Vec<Finding> {
    let mut findings = Vec::new();
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return findings,
    };

    let mut in_deps = false;
    let mut dep_count: usize = 0;
    let mut missing_version = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with('[') {
            in_deps = trimmed == "[dependencies]"
                || trimmed == "[dev-dependencies]"
                || trimmed == "[build-dependencies]"
                || trimmed.starts_with("[dependencies.")
                || trimmed.starts_with("[dev-dependencies.")
                || trimmed.starts_with("[build-dependencies.");
            continue;
        }

        if in_deps && !trimmed.is_empty() && !trimmed.starts_with('#')
            && let Some(dep_name) = trimmed.split('=').next().map(|s| s.trim())
                && !dep_name.is_empty() {
                    dep_count += 1;
                    // Check for missing version: `dep = "*"` or dep with no version key
                    let value = trimmed.split_once('=').map(|x| x.1)
                        .map(|s| s.trim())
                        .unwrap_or("");
                    if value == "\"*\"" || value.is_empty() {
                        missing_version.push(dep_name.to_string());
                    }
                }
    }

    findings.push(Finding {
        severity: "info".into(),
        category: "dependency".into(),
        file: rel_str.to_string(),
        line: None,
        message: format!("Cargo.toml has {dep_count} dependencies"),
    });

    for dep in missing_version {
        findings.push(Finding {
            severity: "warning".into(),
            category: "dependency".into(),
            file: rel_str.to_string(),
            line: None,
            message: format!("Dependency '{dep}' has no pinned version"),
        });
    }

    findings
}

fn analyze_package_json(path: &Path, rel_str: &str) -> Vec<Finding> {
    let mut findings = Vec::new();
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return findings,
    };

    // Simple JSON parsing without pulling in a full parser — look for
    // "dependencies" and "devDependencies" object keys and count entries.
    let mut dep_count: usize = 0;
    let mut dev_dep_count: usize = 0;

    // Parse with serde_json if available, otherwise count heuristically
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
        if let Some(deps) = val.get("dependencies").and_then(|v| v.as_object()) {
            dep_count = deps.len();
            for (name, ver) in deps {
                if ver.as_str().map(|s| s == "*").unwrap_or(true) {
                    findings.push(Finding {
                        severity: "warning".into(),
                        category: "dependency".into(),
                        file: rel_str.to_string(),
                        line: None,
                        message: format!("Dependency '{name}' has no pinned version"),
                    });
                }
            }
        }
        if let Some(deps) = val.get("devDependencies").and_then(|v| v.as_object()) {
            dev_dep_count = deps.len();
        }
    }

    findings.push(Finding {
        severity: "info".into(),
        category: "dependency".into(),
        file: rel_str.to_string(),
        line: None,
        message: format!(
            "package.json has {dep_count} dependencies, {dev_dep_count} devDependencies"
        ),
    });

    findings
}
