//! `weft security` subcommand implementation.
//!
//! Provides security scanning, auditing, and hardening for clawft.
//!
//! # Commands
//!
//! - `weft security scan [PATH]` -- Run 50+ audit checks against a file or directory.
//! - `weft security audit` -- Show audit log for sandbox decisions.
//! - `weft security checks` -- List all available audit checks.

use clap::{Args, Subcommand};
use clawft_rpc::{DaemonClient, Request};
use clawft_security::{AuditSeverity, SecurityScanner};

/// Arguments for `weft security`.
#[derive(Args)]
pub struct SecurityArgs {
    #[command(subcommand)]
    pub action: SecurityAction,
}

/// Subcommands for `weft security`.
#[derive(Subcommand)]
pub enum SecurityAction {
    /// Scan a file or directory for security issues.
    Scan {
        /// File or directory to scan.
        path: String,

        /// Output format: text, json.
        #[arg(long, default_value = "text")]
        format: String,

        /// Minimum severity to report (info, low, medium, high, critical).
        #[arg(long, default_value = "low")]
        min_severity: String,
    },

    /// List all available audit checks.
    Checks,
}

/// Warning printed when falling back to local execution without daemon.
const DAEMON_FALLBACK_WARNING: &str = "Warning: running without kernel daemon — results may not reflect live kernel state. \
     Start daemon with: weaver kernel start";

/// Run the security command.
pub async fn run(args: SecurityArgs) -> anyhow::Result<()> {
    match args.action {
        SecurityAction::Scan {
            path,
            format,
            min_severity,
        } => run_scan(&path, &format, &min_severity).await,
        SecurityAction::Checks => run_checks(),
    }
}

fn parse_min_severity(s: &str) -> AuditSeverity {
    match s.to_lowercase().as_str() {
        "info" => AuditSeverity::Info,
        "low" => AuditSeverity::Low,
        "medium" => AuditSeverity::Medium,
        "high" => AuditSeverity::High,
        "critical" => AuditSeverity::Critical,
        _ => AuditSeverity::Low,
    }
}

async fn run_scan(path: &str, format: &str, min_severity: &str) -> anyhow::Result<()> {
    // Try daemon-first path (ADR-021).
    if let Some(mut client) = DaemonClient::connect().await {
        let params = serde_json::json!({
            "path": path,
            "format": format,
            "min_severity": min_severity,
        });
        let request = Request::with_params("security.scan", params);
        let response = client.call(request).await?;
        let result = response.into_result()?;

        // The daemon returns the formatted output directly.
        if let Some(output) = result.get("output").and_then(|v| v.as_str()) {
            print!("{output}");
        } else {
            // Fallback: dump JSON result.
            println!("{}", serde_json::to_string_pretty(&result)?);
        }

        if result.get("passed").and_then(|v| v.as_bool()) == Some(false) {
            std::process::exit(1);
        }

        return Ok(());
    }

    // No daemon — fall back to local scanner with warning.
    eprintln!("{DAEMON_FALLBACK_WARNING}");

    run_scan_local(path, format, min_severity)
}

fn run_scan_local(path: &str, format: &str, min_severity: &str) -> anyhow::Result<()> {
    let scanner = SecurityScanner::new();
    let min_sev = parse_min_severity(min_severity);
    let path = std::path::Path::new(path);

    let mut all_findings = Vec::new();
    let checks_run = scanner.check_count();

    if path.is_dir() {
        scan_directory(&scanner, path, &min_sev, &mut all_findings)?;
    } else if path.is_file() {
        scan_file(&scanner, path, &min_sev, &mut all_findings)?;
    } else {
        anyhow::bail!("path does not exist: {}", path.display());
    }

    let report = clawft_security::AuditReport::from_findings(all_findings, checks_run);

    match format {
        "json" => {
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        _ => {
            print_text_report(&report);
        }
    }

    if !report.passed {
        std::process::exit(1);
    }

    Ok(())
}

fn scan_directory(
    scanner: &SecurityScanner,
    dir: &std::path::Path,
    min_sev: &AuditSeverity,
    findings: &mut Vec<clawft_security::AuditFinding>,
) -> anyhow::Result<()> {
    let entries = std::fs::read_dir(dir)?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            // Skip hidden directories and target/
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            if name.starts_with('.') || name == "target" || name == "node_modules" {
                continue;
            }
            scan_directory(scanner, &path, min_sev, findings)?;
        } else if path.is_file() {
            scan_file(scanner, &path, min_sev, findings)?;
        }
    }
    Ok(())
}

fn scan_file(
    scanner: &SecurityScanner,
    file: &std::path::Path,
    min_sev: &AuditSeverity,
    findings: &mut Vec<clawft_security::AuditFinding>,
) -> anyhow::Result<()> {
    // Skip binary files and large files
    let metadata = std::fs::metadata(file)?;
    if metadata.len() > 1_048_576 {
        // Skip files > 1MB
        return Ok(());
    }

    // Only scan text-like files
    let ext = file
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase();
    let scannable = matches!(
        ext.as_str(),
        "rs" | "py"
            | "js"
            | "ts"
            | "sh"
            | "bash"
            | "yaml"
            | "yml"
            | "toml"
            | "json"
            | "md"
            | "txt"
            | "cfg"
            | "conf"
            | "env"
            | "ini"
            | "xml"
            | "html"
            | "css"
            | "sql"
            | "rb"
            | "go"
            | "java"
            | "kt"
            | "swift"
            | "c"
            | "cpp"
            | "h"
            | "hpp"
            | "Dockerfile"
            | ""
    );
    if !scannable {
        return Ok(());
    }

    let content = match std::fs::read_to_string(file) {
        Ok(c) => c,
        Err(_) => return Ok(()), // Skip unreadable files
    };

    let file_findings = scanner.scan_content(&content, Some(&file.to_string_lossy()));

    for f in file_findings {
        if f.severity >= *min_sev {
            findings.push(f);
        }
    }

    Ok(())
}

fn print_text_report(report: &clawft_security::AuditReport) {
    println!("Security Scan Report");
    println!("====================");
    println!("Checks run: {}", report.checks_run);
    println!("Findings:   {}", report.total_findings());
    println!(
        "  Critical: {}  High: {}  Medium: {}  Low: {}  Info: {}",
        report.critical_count,
        report.high_count,
        report.medium_count,
        report.low_count,
        report.info_count
    );
    println!(
        "Status:     {}",
        if report.passed { "PASSED" } else { "FAILED" }
    );
    println!();

    for finding in &report.findings {
        println!(
            "[{}] {} ({})",
            finding.severity, finding.check_id, finding.category
        );
        println!("  Name: {}", finding.check_name);
        if let Some(loc) = &finding.location {
            println!("  Location: {loc}");
        }
        if let Some(matched) = &finding.matched_content {
            println!("  Matched: {matched}");
        }
        println!("  Remediation: {}", finding.remediation);
        println!();
    }
}

fn run_checks() -> anyhow::Result<()> {
    let scanner = SecurityScanner::new();
    let by_cat = scanner.checks_by_category();

    println!(
        "Available Security Audit Checks ({} total)",
        scanner.check_count()
    );
    println!("=========================================");

    let mut categories: Vec<_> = by_cat.keys().collect();
    categories.sort_by_key(|c| format!("{c:?}"));

    for category in categories {
        let checks = &by_cat[category];
        println!("\n{category} ({} checks)", checks.len());
        println!("{}", "-".repeat(40));
        for check in checks {
            println!("  [{:>8}] {} -- {}", check.severity, check.id, check.name);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_severity_variants() {
        assert_eq!(parse_min_severity("info"), AuditSeverity::Info);
        assert_eq!(parse_min_severity("low"), AuditSeverity::Low);
        assert_eq!(parse_min_severity("medium"), AuditSeverity::Medium);
        assert_eq!(parse_min_severity("high"), AuditSeverity::High);
        assert_eq!(parse_min_severity("critical"), AuditSeverity::Critical);
        assert_eq!(parse_min_severity("CRITICAL"), AuditSeverity::Critical);
        assert_eq!(parse_min_severity("unknown"), AuditSeverity::Low);
    }
}
