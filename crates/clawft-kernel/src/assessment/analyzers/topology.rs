//! Topology analyzer — discovers infrastructure descriptors (Docker, k8s, .env).

use std::path::{Path, PathBuf};

use crate::assessment::{analyzer::AnalysisContext, Finding};
use crate::assessment::analyzer::Analyzer;

/// Analyzer that identifies infrastructure topology files and extracts metadata.
pub struct TopologyAnalyzer;

impl Analyzer for TopologyAnalyzer {
    fn id(&self) -> &str {
        "topology"
    }

    fn name(&self) -> &str {
        "Topology Analyzer"
    }

    fn categories(&self) -> &[&str] {
        &["topology"]
    }

    fn analyze(&self, project: &Path, files: &[PathBuf], _context: &AnalysisContext) -> Vec<Finding> {
        let mut findings = Vec::new();

        for path in files {
            let rel = path.strip_prefix(project).unwrap_or(path);
            let rel_str = rel.display().to_string();
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");

            // docker-compose files
            if name.starts_with("docker-compose") && (name.ends_with(".yml") || name.ends_with(".yaml")) {
                let content = match std::fs::read_to_string(path) {
                    Ok(c) => c,
                    Err(_) => continue,
                };
                findings.push(Finding {
                    severity: "info".into(),
                    category: "topology".into(),
                    file: rel_str.clone(),
                    line: None,
                    message: "Docker Compose file detected".into(),
                });
                extract_compose_services(&content, &rel_str, &mut findings);
                continue;
            }

            // Dockerfile
            if name == "Dockerfile" || name.starts_with("Dockerfile.") {
                let content = match std::fs::read_to_string(path) {
                    Ok(c) => c,
                    Err(_) => continue,
                };
                findings.push(Finding {
                    severity: "info".into(),
                    category: "topology".into(),
                    file: rel_str.clone(),
                    line: None,
                    message: "Dockerfile detected".into(),
                });
                // Extract image references from FROM lines
                for (i, line) in content.lines().enumerate() {
                    let trimmed = line.trim();
                    if let Some(rest) = trimmed.strip_prefix("FROM ") {
                        let image = rest.split_whitespace().next().unwrap_or(rest);
                        findings.push(Finding {
                            severity: "info".into(),
                            category: "topology".into(),
                            file: rel_str.clone(),
                            line: Some(i + 1),
                            message: format!("Base image: {image}"),
                        });
                    }
                    if let Some(rest) = trimmed.strip_prefix("EXPOSE ") {
                        findings.push(Finding {
                            severity: "info".into(),
                            category: "topology".into(),
                            file: rel_str.clone(),
                            line: Some(i + 1),
                            message: format!("Exposed port: {}", rest.trim()),
                        });
                    }
                }
                continue;
            }

            // Kubernetes manifests (YAML with apiVersion)
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if (ext == "yaml" || ext == "yml") && !name.starts_with("docker-compose") {
                let content = match std::fs::read_to_string(path) {
                    Ok(c) => c,
                    Err(_) => continue,
                };
                if content.contains("apiVersion:") && content.contains("kind:") {
                    findings.push(Finding {
                        severity: "info".into(),
                        category: "topology".into(),
                        file: rel_str.clone(),
                        line: None,
                        message: "Kubernetes manifest detected".into(),
                    });
                }
            }

            // .env files (just note existence; SecurityAnalyzer handles the warning)
            if name == ".env" || name.starts_with(".env.") {
                findings.push(Finding {
                    severity: "info".into(),
                    category: "topology".into(),
                    file: rel_str.clone(),
                    line: None,
                    message: "Environment configuration file detected".into(),
                });
            }
        }

        findings
    }
}

/// Extract service names and ports from a docker-compose YAML (heuristic parse).
fn extract_compose_services(content: &str, rel_str: &str, findings: &mut Vec<Finding>) {
    let mut in_services = false;
    let mut current_service: Option<String> = None;

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed == "services:" {
            in_services = true;
            continue;
        }

        if in_services {
            // Top-level key under services (indented exactly 2 spaces typically)
            if !line.starts_with(' ') && !line.is_empty() {
                // We've left the services block
                in_services = false;
                current_service = None;
                continue;
            }

            // Service name: line starts with some indent and ends with ':'
            let stripped = line.trim_start();
            if stripped.ends_with(':') && !stripped.contains(' ') {
                let svc_name = stripped.trim_end_matches(':');
                current_service = Some(svc_name.to_string());
                findings.push(Finding {
                    severity: "info".into(),
                    category: "topology".into(),
                    file: rel_str.to_string(),
                    line: None,
                    message: format!("Service: {svc_name}"),
                });
            }

            // Image reference
            if let Some(rest) = stripped.strip_prefix("image:") {
                let image = rest.trim().trim_matches('"').trim_matches('\'');
                if let Some(ref svc) = current_service {
                    findings.push(Finding {
                        severity: "info".into(),
                        category: "topology".into(),
                        file: rel_str.to_string(),
                        line: None,
                        message: format!("Service '{svc}' image: {image}"),
                    });
                }
            }

            // Port mappings (e.g. - "8080:80")
            if stripped.starts_with("- \"") || stripped.starts_with("- '") || stripped.starts_with("- ") {
                let val = stripped
                    .trim_start_matches("- ")
                    .trim_matches('"')
                    .trim_matches('\'');
                if val.contains(':') && val.chars().all(|c| c.is_ascii_digit() || c == ':')
                    && let Some(ref svc) = current_service {
                        findings.push(Finding {
                            severity: "info".into(),
                            category: "topology".into(),
                            file: rel_str.to_string(),
                            line: None,
                            message: format!("Service '{svc}' port mapping: {val}"),
                        });
                    }
            }
        }
    }
}
