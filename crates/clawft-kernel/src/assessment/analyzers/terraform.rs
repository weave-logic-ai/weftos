//! Terraform analyzer — discovers IaC resources, providers, and state files.

use std::path::{Path, PathBuf};

use crate::assessment::analyzer::{AnalysisContext, Analyzer};
use crate::assessment::Finding;

/// Analyzer that identifies Terraform configuration, resources, providers, and state files.
pub struct TerraformAnalyzer;

impl Analyzer for TerraformAnalyzer {
    fn id(&self) -> &str {
        "terraform"
    }

    fn name(&self) -> &str {
        "Terraform Analyzer"
    }

    fn categories(&self) -> &[&str] {
        &["infrastructure", "security"]
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
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

            // terraform.tfstate — committed state file is a security risk
            if name == "terraform.tfstate" || name == "terraform.tfstate.backup" {
                findings.push(Finding {
                    severity: "high".into(),
                    category: "security".into(),
                    file: rel_str.clone(),
                    line: None,
                    message: "Terraform state file committed — may contain secrets".into(),
                });
                continue;
            }

            // .terraform.lock.hcl — extract provider versions
            if name == ".terraform.lock.hcl" {
                let content = match std::fs::read_to_string(path) {
                    Ok(c) => c,
                    Err(_) => continue,
                };
                findings.push(Finding {
                    severity: "info".into(),
                    category: "infrastructure".into(),
                    file: rel_str.clone(),
                    line: None,
                    message: "Terraform lock file detected".into(),
                });
                extract_lock_providers(&content, &rel_str, &mut findings);
                continue;
            }

            // *.tf files — extract resource, provider, data, variable, output blocks
            if ext == "tf" {
                let content = match std::fs::read_to_string(path) {
                    Ok(c) => c,
                    Err(_) => continue,
                };
                findings.push(Finding {
                    severity: "info".into(),
                    category: "infrastructure".into(),
                    file: rel_str.clone(),
                    line: None,
                    message: "Terraform configuration file detected".into(),
                });
                extract_tf_blocks(&content, &rel_str, &mut findings);
                continue;
            }

            // *.tfvars files — extract variable assignments
            if ext == "tfvars" {
                let content = match std::fs::read_to_string(path) {
                    Ok(c) => c,
                    Err(_) => continue,
                };
                findings.push(Finding {
                    severity: "info".into(),
                    category: "infrastructure".into(),
                    file: rel_str.clone(),
                    line: None,
                    message: "Terraform variables file detected".into(),
                });
                extract_tfvars(&content, &rel_str, &mut findings);
                continue;
            }
        }

        findings
    }
}

/// Extract `resource`, `provider`, `data`, `variable`, and `output` blocks from a `.tf` file.
fn extract_tf_blocks(content: &str, rel_str: &str, findings: &mut Vec<Finding>) {
    for (i, line) in content.lines().enumerate() {
        let trimmed = line.trim();

        // resource "type" "name" {
        if let Some(rest) = trimmed.strip_prefix("resource ") {
            if let Some((rtype, rname)) = parse_two_quoted(rest) {
                findings.push(Finding {
                    severity: "info".into(),
                    category: "infrastructure".into(),
                    file: rel_str.to_string(),
                    line: Some(i + 1),
                    message: format!("Terraform resource: {rtype}.{rname}"),
                });
            }
            continue;
        }

        // data "type" "name" {
        if let Some(rest) = trimmed.strip_prefix("data ") {
            if let Some((dtype, dname)) = parse_two_quoted(rest) {
                findings.push(Finding {
                    severity: "info".into(),
                    category: "infrastructure".into(),
                    file: rel_str.to_string(),
                    line: Some(i + 1),
                    message: format!("Terraform data source: {dtype}.{dname}"),
                });
            }
            continue;
        }

        // provider "name" {  — may have version inside block, but we capture inline version too
        if let Some(rest) = trimmed.strip_prefix("provider ") {
            if let Some(pname) = parse_one_quoted(rest) {
                // Try to find version on same line or nearby (simple heuristic)
                let version = extract_inline_version(rest);
                let msg = match version {
                    Some(v) => format!("Terraform provider: {pname} {v}"),
                    None => format!("Terraform provider: {pname}"),
                };
                findings.push(Finding {
                    severity: "info".into(),
                    category: "infrastructure".into(),
                    file: rel_str.to_string(),
                    line: Some(i + 1),
                    message: msg,
                });
            }
            continue;
        }

        // variable "name" {
        if let Some(rest) = trimmed.strip_prefix("variable ") {
            if let Some(vname) = parse_one_quoted(rest) {
                findings.push(Finding {
                    severity: "info".into(),
                    category: "infrastructure".into(),
                    file: rel_str.to_string(),
                    line: Some(i + 1),
                    message: format!("Terraform variable: {vname}"),
                });
            }
            continue;
        }

        // output "name" {
        if let Some(rest) = trimmed.strip_prefix("output ") {
            if let Some(oname) = parse_one_quoted(rest) {
                findings.push(Finding {
                    severity: "info".into(),
                    category: "infrastructure".into(),
                    file: rel_str.to_string(),
                    line: Some(i + 1),
                    message: format!("Terraform output: {oname}"),
                });
            }
            continue;
        }

        // version constraint inside provider/required_providers block
        if trimmed.starts_with("version") && trimmed.contains('=')
            && let Some(v) = extract_version_value(trimmed) {
                findings.push(Finding {
                    severity: "info".into(),
                    category: "infrastructure".into(),
                    file: rel_str.to_string(),
                    line: Some(i + 1),
                    message: format!("Terraform version constraint: {v}"),
                });
            }
    }
}

/// Parse two quoted strings: `"foo" "bar"` -> Some(("foo", "bar")).
fn parse_two_quoted(s: &str) -> Option<(&str, &str)> {
    let first_start = s.find('"')? + 1;
    let first_end = s[first_start..].find('"')? + first_start;
    let rest = &s[first_end + 1..];
    let second_start = rest.find('"')? + 1;
    let second_end = rest[second_start..].find('"')? + second_start;
    Some((&s[first_start..first_end], &rest[second_start..second_end]))
}

/// Parse one quoted string: `"foo" {` -> Some("foo").
fn parse_one_quoted(s: &str) -> Option<&str> {
    let start = s.find('"')? + 1;
    let end = s[start..].find('"')? + start;
    Some(&s[start..end])
}

/// Try to extract an inline `version = "..."` from remaining text on the same line.
fn extract_inline_version(s: &str) -> Option<&str> {
    let idx = s.find("version")?;
    let after = &s[idx + 7..];
    let eq = after.find('=')?;
    let after_eq = &after[eq + 1..];
    let q_start = after_eq.find('"')? + 1;
    let q_end = after_eq[q_start..].find('"')? + q_start;
    Some(&after_eq[q_start..q_end])
}

/// Extract the version value from a line like `version = "~> 4.0"`.
fn extract_version_value(line: &str) -> Option<&str> {
    let eq_pos = line.find('=')?;
    let after_eq = &line[eq_pos + 1..];
    let q_start = after_eq.find('"')? + 1;
    let q_end = after_eq[q_start..].find('"')? + q_start;
    Some(&after_eq[q_start..q_end])
}

/// Extract variable assignments from `.tfvars` files.
fn extract_tfvars(content: &str, rel_str: &str, findings: &mut Vec<Finding>) {
    for (i, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // Simple pattern: key = value
        if let Some(eq_pos) = trimmed.find('=') {
            let key = trimmed[..eq_pos].trim();
            if !key.is_empty() && key.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-')
            {
                findings.push(Finding {
                    severity: "info".into(),
                    category: "infrastructure".into(),
                    file: rel_str.to_string(),
                    line: Some(i + 1),
                    message: format!("Terraform variable assignment: {key}"),
                });
            }
        }
    }
}

/// Extract provider versions from `.terraform.lock.hcl`.
fn extract_lock_providers(content: &str, rel_str: &str, findings: &mut Vec<Finding>) {
    // Lines like: provider "registry.terraform.io/hashicorp/aws" {
    // Then later:   version = "4.67.0"
    let mut current_provider: Option<String> = None;

    for (i, line) in content.lines().enumerate() {
        let trimmed = line.trim();

        if let Some(rest) = trimmed.strip_prefix("provider ") {
            if let Some(pname) = parse_one_quoted(rest) {
                current_provider = Some(pname.to_string());
                findings.push(Finding {
                    severity: "info".into(),
                    category: "infrastructure".into(),
                    file: rel_str.to_string(),
                    line: Some(i + 1),
                    message: format!("Terraform locked provider: {pname}"),
                });
            }
            continue;
        }

        if trimmed.starts_with("version") && trimmed.contains('=')
            && let Some(v) = extract_version_value(trimmed)
                && let Some(ref prov) = current_provider {
                    findings.push(Finding {
                        severity: "info".into(),
                        category: "infrastructure".into(),
                        file: rel_str.to_string(),
                        line: Some(i + 1),
                        message: format!("Terraform locked provider version: {prov} = {v}"),
                    });
                }

        if trimmed == "}" {
            current_provider = None;
        }
    }
}
