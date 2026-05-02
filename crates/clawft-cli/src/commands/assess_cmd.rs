//! `weft assess` — SOP assessment workflow.
//!
//! Run continuous assessment against a codebase to maintain the WeftOS
//! knowledge graph. Supports multiple scopes (full, commit, CI, dependency)
//! and output formats (table, JSON, GitHub annotations).
//!
//! # Usage
//!
//! ```bash
//! weft assess                          # full assessment, table output
//! weft assess run --scope commit       # only files in last commit
//! weft assess run --scope ci --format github-annotations
//! weft assess status                   # show last assessment results
//! weft assess init                     # initialize .weftos/ assessment config
//! ```

use std::path::{Path, PathBuf};
use std::process::Command;

use clap::{Args, Subcommand, ValueEnum};
use clawft_rpc::{DaemonClient, Request};

/// Arguments for `weft assess`.
#[derive(Args)]
pub struct AssessArgs {
    #[command(subcommand)]
    pub action: Option<AssessAction>,

    /// Assessment scope (when running without a subcommand).
    #[arg(short, long, default_value = "full")]
    pub scope: AssessScope,

    /// Output format.
    #[arg(short, long, default_value = "table")]
    pub format: AssessFormat,

    /// Project directory to assess (defaults to current directory).
    #[arg(short, long)]
    pub dir: Option<String>,
}

/// Subcommands for `weft assess`.
#[derive(Subcommand)]
pub enum AssessAction {
    /// Run an assessment (default if no subcommand given).
    Run {
        /// Assessment scope.
        #[arg(short, long, default_value = "full")]
        scope: AssessScope,

        /// Output format.
        #[arg(short, long, default_value = "table")]
        format: AssessFormat,

        /// Project directory to assess.
        #[arg(short, long)]
        dir: Option<String>,

        /// PR number for github-pr format.
        #[arg(long)]
        pr_number: Option<u64>,
    },

    /// Show status of the last assessment.
    Status {
        /// Project directory.
        #[arg(short, long)]
        dir: Option<String>,
    },

    /// Initialize assessment configuration in .weftos/.
    Init {
        /// Project directory.
        #[arg(short, long)]
        dir: Option<String>,

        /// Overwrite existing config.
        #[arg(long)]
        force: bool,
    },

    /// Link two projects for cross-project coordination.
    Link {
        /// Name for the peer project.
        name: String,

        /// Path to the peer project's .weftos/ directory.
        path: String,

        /// Project directory (this project).
        #[arg(short, long)]
        dir: Option<String>,
    },

    /// Show linked peers and cross-project status.
    Peers {
        /// Project directory.
        #[arg(short, long)]
        dir: Option<String>,
    },

    /// Compare assessment results across linked projects.
    Compare {
        /// Peer name to compare against.
        peer: String,

        /// Project directory.
        #[arg(short, long)]
        dir: Option<String>,
    },

    /// Install git hooks for automatic assessment on commit.
    Hooks {
        /// Hook type: "post-commit" or "pre-push".
        #[arg(long, default_value = "post-commit")]
        hook_type: String,

        /// Project directory.
        #[arg(short, long)]
        dir: Option<String>,

        /// Remove installed hooks.
        #[arg(long)]
        uninstall: bool,
    },

    /// Review assessment trends and generate SOP improvement recommendations.
    Review {
        /// Number of past assessments to analyze.
        #[arg(short = 'n', long, default_value_t = 5)]
        history: usize,

        /// Project directory.
        #[arg(short, long)]
        dir: Option<String>,
    },
}

/// Assessment scope — what to scan.
#[derive(Clone, ValueEnum)]
pub enum AssessScope {
    /// Full rescan of all files matching configured patterns.
    Full,
    /// Only files changed in the last git commit.
    Commit,
    /// All files changed in the current PR/push (CI mode).
    Ci,
    /// Only dependency manifests (Cargo.toml, package.json, etc.).
    Dependency,
}

impl std::fmt::Display for AssessScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Full => write!(f, "full"),
            Self::Commit => write!(f, "commit"),
            Self::Ci => write!(f, "ci"),
            Self::Dependency => write!(f, "dependency"),
        }
    }
}

/// Output format for assessment results.
#[derive(Clone, ValueEnum)]
pub enum AssessFormat {
    /// Human-readable summary table.
    Table,
    /// Machine-readable JSON.
    Json,
    /// GitHub Actions annotation format.
    GithubAnnotations,
}

impl std::fmt::Display for AssessFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Table => write!(f, "table"),
            Self::Json => write!(f, "json"),
            Self::GithubAnnotations => write!(f, "github-annotations"),
        }
    }
}

/// Run the assess command.
///
/// For `run`, `link`, and `compare` subcommands, tries daemon RPC first
/// (ADR-021). Falls back to local execution with a warning if no daemon
/// is available. `init` and `status` always run locally (bootstrap
/// exception and pure display).
pub async fn run(args: AssessArgs) -> anyhow::Result<()> {
    match args.action {
        Some(AssessAction::Run {
            scope,
            format,
            dir,
            pr_number,
        }) => run_assessment_with_daemon(&scope, &format, dir.as_deref(), pr_number).await,
        Some(AssessAction::Status { dir }) => run_status(dir.as_deref()),
        Some(AssessAction::Init { dir, force }) => run_init(dir.as_deref(), force),
        Some(AssessAction::Link { name, path, dir }) => {
            run_link_with_daemon(&name, &path, dir.as_deref()).await
        }
        Some(AssessAction::Peers { dir }) => run_peers(dir.as_deref()),
        Some(AssessAction::Compare { peer, dir }) => {
            run_compare_with_daemon(&peer, dir.as_deref()).await
        }
        Some(AssessAction::Hooks {
            hook_type,
            dir,
            uninstall,
        }) => run_hooks(&hook_type, dir.as_deref(), uninstall),
        Some(AssessAction::Review { history, dir }) => {
            run_review_with_daemon(history, dir.as_deref()).await
        }
        // No subcommand — run assessment with top-level args.
        None => run_assessment_with_daemon(&args.scope, &args.format, args.dir.as_deref(), None).await,
    }
}

// ---------------------------------------------------------------------------
// Daemon-first wrappers (ADR-021)
// ---------------------------------------------------------------------------

const NO_DAEMON_WARNING: &str =
    "Warning: running without kernel daemon. Assessment not logged to ExoChain. \
     Start daemon with: weaver kernel start";

/// Try daemon RPC for `assess.run`; fall back to local execution.
async fn run_assessment_with_daemon(
    scope: &AssessScope,
    format: &AssessFormat,
    dir: Option<&str>,
    pr_number: Option<u64>,
) -> anyhow::Result<()> {
    if let Some(mut client) = DaemonClient::connect().await {
        let mut params = serde_json::json!({
            "scope": scope.to_string(),
            "format": format.to_string(),
        });
        if let Some(d) = dir {
            params["dir"] = serde_json::json!(d);
        }
        if let Some(pr) = pr_number {
            params["pr_number"] = serde_json::json!(pr);
        }
        let resp = client
            .call(Request::with_params("assess.run", params))
            .await?;
        if resp.ok {
            if let Some(data) = resp.result {
                println!("{}", serde_json::to_string_pretty(&data)?);
            }
            return Ok(());
        }
        // If daemon doesn't support the method yet, fall through.
        if let Some(ref err) = resp.error
            && !err.contains("unknown method") {
                anyhow::bail!("{err}");
            }
    } else {
        eprintln!("{NO_DAEMON_WARNING}");
    }

    run_assessment(scope, format, dir, pr_number)
}

/// Try daemon RPC for `assess.link`; fall back to local execution.
async fn run_link_with_daemon(
    name: &str,
    location: &str,
    dir: Option<&str>,
) -> anyhow::Result<()> {
    if let Some(mut client) = DaemonClient::connect().await {
        let mut params = serde_json::json!({
            "name": name,
            "location": location,
        });
        if let Some(d) = dir {
            params["dir"] = serde_json::json!(d);
        }
        let resp = client
            .call(Request::with_params("assess.link", params))
            .await?;
        if resp.ok {
            if let Some(data) = resp.result {
                println!("{}", serde_json::to_string_pretty(&data)?);
            }
            return Ok(());
        }
        if let Some(ref err) = resp.error
            && !err.contains("unknown method") {
                anyhow::bail!("{err}");
            }
    } else {
        eprintln!("{NO_DAEMON_WARNING}");
    }

    run_link(name, location, dir)
}

/// Try daemon RPC for `assess.compare`; fall back to local execution.
async fn run_compare_with_daemon(
    peer_name: &str,
    dir: Option<&str>,
) -> anyhow::Result<()> {
    if let Some(mut client) = DaemonClient::connect().await {
        let mut params = serde_json::json!({
            "peer": peer_name,
        });
        if let Some(d) = dir {
            params["dir"] = serde_json::json!(d);
        }
        let resp = client
            .call(Request::with_params("assess.compare", params))
            .await?;
        if resp.ok {
            if let Some(data) = resp.result {
                println!("{}", serde_json::to_string_pretty(&data)?);
            }
            return Ok(());
        }
        if let Some(ref err) = resp.error
            && !err.contains("unknown method") {
                anyhow::bail!("{err}");
            }
    } else {
        eprintln!("{NO_DAEMON_WARNING}");
    }

    run_compare(peer_name, dir)
}

/// Try daemon RPC for `assess.review`; fall back to local execution.
async fn run_review_with_daemon(
    history: usize,
    dir: Option<&str>,
) -> anyhow::Result<()> {
    if let Some(mut client) = DaemonClient::connect().await {
        let mut params = serde_json::json!({
            "history": history,
        });
        if let Some(d) = dir {
            params["dir"] = serde_json::json!(d);
        }
        let resp = client
            .call(Request::with_params("assess.review", params))
            .await?;
        if resp.ok {
            if let Some(data) = resp.result {
                println!("{}", serde_json::to_string_pretty(&data)?);
            }
            return Ok(());
        }
        if let Some(ref err) = resp.error
            && !err.contains("unknown method") {
                anyhow::bail!("{err}");
            }
    } else {
        eprintln!("{NO_DAEMON_WARNING}");
    }

    run_review(history, dir)
}

/// Analyze assessment history and generate SOP improvement recommendations.
fn run_review(history: usize, dir: Option<&str>) -> anyhow::Result<()> {
    let project = resolve_project_dir(dir);
    let artifacts_dir = project.join(".weftos/artifacts");

    if !artifacts_dir.exists() {
        anyhow::bail!(
            "No .weftos/artifacts/ directory at {}. Run `weft assess` first.",
            project.display()
        );
    }

    // Collect assessment JSON files, sorted by name (which includes timestamps).
    let mut assessment_files: Vec<PathBuf> = std::fs::read_dir(&artifacts_dir)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension().is_some_and(|e| e == "json")
                && p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("assessment"))
        })
        .collect();

    assessment_files.sort();

    // Take only the last `history` files.
    if assessment_files.len() > history {
        let start = assessment_files.len() - history;
        assessment_files = assessment_files.split_off(start);
    }

    if assessment_files.is_empty() {
        anyhow::bail!("No assessment files found in {}", artifacts_dir.display());
    }

    // Parse all reports into JSON values.
    let reports: Vec<serde_json::Value> = assessment_files
        .iter()
        .filter_map(|p| {
            std::fs::read_to_string(p)
                .ok()
                .and_then(|c| serde_json::from_str(&c).ok())
        })
        .collect();

    if reports.is_empty() {
        anyhow::bail!("Could not parse any assessment files.");
    }

    let count = reports.len();

    // --- Extract per-report metrics ---
    let mut finding_counts: Vec<usize> = Vec::new();
    let mut coherence_scores: Vec<f64> = Vec::new();
    let mut complexity_counts: Vec<usize> = Vec::new();
    let mut file_occurrence: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    let mut analyzer_findings: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();

    for report in &reports {
        // Findings count
        let findings = report.get("findings").and_then(|v| v.as_array());
        let fc = findings.map(|a| a.len()).unwrap_or(0);
        finding_counts.push(fc);

        // Coherence
        let cs = report
            .get("summary")
            .and_then(|s| s.get("coherence_score"))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        coherence_scores.push(cs);

        // Complexity
        let cw = report
            .get("summary")
            .and_then(|s| s.get("complexity_warnings"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;
        complexity_counts.push(cw);

        // Per-file and per-category occurrence
        if let Some(findings_arr) = findings {
            for finding in findings_arr {
                if let Some(file) = finding.get("file").and_then(|v| v.as_str()) {
                    *file_occurrence.entry(file.to_string()).or_insert(0) += 1;
                }
                if let Some(cat) = finding.get("category").and_then(|v| v.as_str()) {
                    *analyzer_findings.entry(cat.to_string()).or_insert(0) += 1;
                }
            }
        }
    }

    // --- Compute trends ---
    let first_findings = finding_counts.first().copied().unwrap_or(0);
    let last_findings = finding_counts.last().copied().unwrap_or(0);
    let findings_pct = if first_findings > 0 {
        ((last_findings as f64 - first_findings as f64) / first_findings as f64) * 100.0
    } else if last_findings > 0 {
        100.0
    } else {
        0.0
    };

    let first_coherence = coherence_scores.first().copied().unwrap_or(0.0);
    let last_coherence = coherence_scores.last().copied().unwrap_or(0.0);
    let coherence_delta = last_coherence - first_coherence;

    let first_complexity = complexity_counts.first().copied().unwrap_or(0);
    let last_complexity = complexity_counts.last().copied().unwrap_or(0);
    let complexity_trend = if first_complexity == last_complexity {
        "stable".to_string()
    } else if last_complexity > first_complexity {
        format!("+{}", last_complexity - first_complexity)
    } else {
        format!("-{}", first_complexity - last_complexity)
    };

    // Repeat offenders: files appearing in every assessment
    let repeat_offenders: Vec<&String> = file_occurrence
        .iter()
        .filter(|&(_, &c)| c >= count)
        .map(|(f, _)| f)
        .collect();

    // Analyzers with 0 findings across all reports
    let known_categories = [
        "complexity",
        "technical-debt",
        "security",
        "dependency",
        "documentation",
    ];
    let zero_categories: Vec<&str> = known_categories
        .iter()
        .filter(|c| !analyzer_findings.contains_key(**c))
        .copied()
        .collect();

    // --- Build recommendations ---
    let mut recommendations: Vec<(String, String)> = Vec::new();

    if !repeat_offenders.is_empty() {
        recommendations.push((
            "SUGGEST".into(),
            format!(
                "{} files appear in every assessment — consider splitting or adding to exclude",
                repeat_offenders.len()
            ),
        ));
    }

    for cat in &zero_categories {
        recommendations.push((
            "SUGGEST".into(),
            format!(
                "{cat} analyzer found 0 issues in {count} runs — scan frequency could be reduced"
            ),
        ));
    }

    if last_coherence < 30.0 {
        recommendations.push((
            "SUGGEST".into(),
            format!(
                "Coherence below 30% ({last_coherence:.1}%) — add documentation for undocumented modules"
            ),
        ));
    }

    if findings_pct > 10.0 {
        recommendations.push((
            "ACTION".into(),
            format!(
                "Findings increased by {findings_pct:.1}% over {count} runs — investigate root causes"
            ),
        ));
    }

    if last_complexity > first_complexity && count > 1 {
        recommendations.push((
            "ACTION".into(),
            format!(
                "Complexity warnings rose from {first_complexity} to {last_complexity} — review large files"
            ),
        ));
    }

    // --- Print report ---
    let findings_arrow = finding_counts
        .iter()
        .map(|c| c.to_string())
        .collect::<Vec<_>>()
        .join(" → ");

    println!("SOP Review ({count} assessments analyzed)");
    println!("====================================");
    println!();
    println!("Trends:");
    println!(
        "  Findings:   {findings_arrow} ({findings_pct:+.1}% over {count} runs)"
    );
    println!(
        "  Coherence:  {first_coherence:.1}% → {last_coherence:.1}% ({coherence_delta:+.1}%)"
    );
    println!(
        "  Complexity: {last_complexity} warnings ({complexity_trend})"
    );

    if !recommendations.is_empty() {
        println!();
        println!("Recommendations:");
        for (tag, msg) in &recommendations {
            println!("  [{tag}] {msg}");
        }
    } else {
        println!();
        println!("No recommendations — assessment trends look healthy.");
    }

    // --- Save structured output ---
    let review_output = serde_json::json!({
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "assessments_analyzed": count,
        "trends": {
            "findings": {
                "values": finding_counts,
                "change_pct": findings_pct,
            },
            "coherence": {
                "first": first_coherence,
                "last": last_coherence,
                "delta": coherence_delta,
            },
            "complexity": {
                "first": first_complexity,
                "last": last_complexity,
                "trend": complexity_trend,
            },
        },
        "repeat_offenders": repeat_offenders,
        "zero_finding_categories": zero_categories,
        "recommendations": recommendations.iter().map(|(tag, msg)| {
            serde_json::json!({ "tag": tag, "message": msg })
        }).collect::<Vec<_>>(),
    });

    let review_json = serde_json::to_string_pretty(&review_output)?;
    let output_path = artifacts_dir.join("sop-review-latest.json");
    std::fs::write(&output_path, review_json.as_bytes())?;

    println!();
    println!("Review saved to .weftos/artifacts/sop-review-latest.json");

    Ok(())
}

// ---------------------------------------------------------------------------
// Assessment runner
// ---------------------------------------------------------------------------

fn resolve_project_dir(dir: Option<&str>) -> PathBuf {
    dir.map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

fn run_assessment(
    scope: &AssessScope,
    format: &AssessFormat,
    dir: Option<&str>,
    _pr_number: Option<u64>,
) -> anyhow::Result<()> {
    let project = resolve_project_dir(dir);
    let weftos_dir = project.join(".weftos");

    if !weftos_dir.exists() {
        eprintln!("No .weftos/ directory found in {}", project.display());
        eprintln!("Run `weft assess init` to initialize assessment configuration.");
        std::process::exit(1);
    }

    // Determine files to scan based on scope
    let files = match scope {
        AssessScope::Full => collect_all_files(&project)?,
        AssessScope::Commit => collect_commit_files(&project)?,
        AssessScope::Ci => collect_ci_files(&project)?,
        AssessScope::Dependency => collect_dependency_files(&project)?,
    };

    // Run the assessment pipeline: SCOPE -> SCAN -> ANALYZE -> REPORT
    let report = assess_files(&project, &files, scope)?;

    // Output in requested format
    match format {
        AssessFormat::Table => print_table_report(&report),
        AssessFormat::Json => print_json_report(&report)?,
        AssessFormat::GithubAnnotations => print_github_annotations(&report),
    }

    // Write latest assessment to .weftos/artifacts/
    let artifacts_dir = weftos_dir.join("artifacts");
    std::fs::create_dir_all(&artifacts_dir)?;
    let json = serde_json::to_string_pretty(&report)?;
    std::fs::write(
        artifacts_dir.join("assessment-latest.json"),
        json.as_bytes(),
    )?;

    Ok(())
}

// ---------------------------------------------------------------------------
// File collection by scope
// ---------------------------------------------------------------------------

fn collect_all_files(project: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let patterns = ["rs", "ts", "tsx", "js", "json", "toml", "md", "mdx"];
    collect_files_recursive(project, &patterns, &mut files);
    Ok(files)
}

fn collect_commit_files(project: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let output = Command::new("git")
        .args(["diff", "--name-only", "HEAD~1", "HEAD"])
        .current_dir(project)
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| project.join(l))
        .filter(|p| p.exists())
        .collect())
}

fn collect_ci_files(project: &Path) -> anyhow::Result<Vec<PathBuf>> {
    // Try to find the merge base with main/master
    let base_branch = if Command::new("git")
        .args(["rev-parse", "--verify", "origin/main"])
        .current_dir(project)
        .output()
        .is_ok_and(|o| o.status.success())
    {
        "origin/main"
    } else {
        "origin/master"
    };

    let output = Command::new("git")
        .args(["diff", "--name-only", &format!("{base_branch}...HEAD")])
        .current_dir(project)
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| project.join(l))
        .filter(|p| p.exists())
        .collect())
}

fn collect_dependency_files(project: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let dep_files = [
        "Cargo.toml",
        "Cargo.lock",
        "package.json",
        "package-lock.json",
        "yarn.lock",
        "pnpm-lock.yaml",
    ];
    Ok(dep_files
        .iter()
        .map(|f| project.join(f))
        .filter(|p| p.exists())
        .collect())
}

fn collect_files_recursive(dir: &Path, extensions: &[&str], out: &mut Vec<PathBuf>) {
    let skip = [
        "node_modules",
        "target",
        ".git",
        ".next",
        ".weftos",
        ".claude",
        ".planning",
    ];

    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if path.is_dir() {
            if !skip.contains(&name_str.as_ref()) {
                collect_files_recursive(&path, extensions, out);
            }
        } else if let Some(ext) = path.extension()
            && extensions.contains(&ext.to_string_lossy().as_ref()) {
                out.push(path);
            }
    }
}

// ---------------------------------------------------------------------------
// Assessment pipeline
// ---------------------------------------------------------------------------

#[derive(serde::Serialize)]
struct AssessmentReport {
    timestamp: String,
    scope: String,
    project: String,
    files_scanned: usize,
    summary: AssessmentSummary,
    findings: Vec<Finding>,
}

#[derive(serde::Serialize)]
struct AssessmentSummary {
    total_files: usize,
    lines_of_code: usize,
    rust_files: usize,
    typescript_files: usize,
    config_files: usize,
    doc_files: usize,
    dependency_files: usize,
    complexity_warnings: usize,
    coherence_score: f64,
}

#[derive(serde::Serialize)]
struct Finding {
    severity: String,
    category: String,
    file: String,
    line: Option<usize>,
    message: String,
}

fn assess_files(
    project: &Path,
    files: &[PathBuf],
    scope: &AssessScope,
) -> anyhow::Result<AssessmentReport> {
    let mut summary = AssessmentSummary {
        total_files: files.len(),
        lines_of_code: 0,
        rust_files: 0,
        typescript_files: 0,
        config_files: 0,
        doc_files: 0,
        dependency_files: 0,
        complexity_warnings: 0,
        coherence_score: 0.0,
    };

    let mut findings = Vec::new();

    for file in files {
        let ext = file
            .extension()
            .map(|e| e.to_string_lossy().to_string())
            .unwrap_or_default();

        // Count lines
        if let Ok(content) = std::fs::read_to_string(file) {
            let line_count = content.lines().count();
            summary.lines_of_code += line_count;

            // Categorize
            match ext.as_str() {
                "rs" => {
                    summary.rust_files += 1;
                    // Check for large files
                    if line_count > 500 {
                        findings.push(Finding {
                            severity: "medium".into(),
                            category: "complexity".into(),
                            file: file.strip_prefix(project).unwrap_or(file).display().to_string(),
                            line: None,
                            message: format!("{line_count} lines — consider splitting (target: <500)"),
                        });
                        summary.complexity_warnings += 1;
                    }
                    // Check for TODO/FIXME
                    for (i, line) in content.lines().enumerate() {
                        if line.contains("TODO") || line.contains("FIXME") {
                            findings.push(Finding {
                                severity: "info".into(),
                                category: "technical-debt".into(),
                                file: file.strip_prefix(project).unwrap_or(file).display().to_string(),
                                line: Some(i + 1),
                                message: line.trim().to_string(),
                            });
                        }
                    }
                }
                "ts" | "tsx" | "js" | "jsx" => {
                    summary.typescript_files += 1;
                    if line_count > 500 {
                        findings.push(Finding {
                            severity: "medium".into(),
                            category: "complexity".into(),
                            file: file.strip_prefix(project).unwrap_or(file).display().to_string(),
                            line: None,
                            message: format!("{line_count} lines — consider splitting"),
                        });
                        summary.complexity_warnings += 1;
                    }
                }
                "toml" | "json" => {
                    if file
                        .file_name()
                        .is_some_and(|n| {
                            let s = n.to_string_lossy();
                            s.contains("Cargo") || s.contains("package")
                        })
                    {
                        summary.dependency_files += 1;
                    } else {
                        summary.config_files += 1;
                    }
                }
                "md" | "mdx" => {
                    summary.doc_files += 1;
                }
                _ => {}
            }
        }
    }

    // Coherence score: ratio of documented modules to total modules
    let documented = summary.doc_files as f64;
    let code = (summary.rust_files + summary.typescript_files).max(1) as f64;
    summary.coherence_score = (documented / code * 100.0).min(100.0);

    Ok(AssessmentReport {
        timestamp: chrono::Utc::now().to_rfc3339(),
        scope: scope.to_string(),
        project: project.display().to_string(),
        files_scanned: files.len(),
        summary,
        findings,
    })
}

// ---------------------------------------------------------------------------
// Output formatters
// ---------------------------------------------------------------------------

fn print_table_report(report: &AssessmentReport) {
    println!("WeftOS Assessment Report");
    println!("========================");
    println!("  Timestamp:    {}", report.timestamp);
    println!("  Scope:        {}", report.scope);
    println!("  Project:      {}", report.project);
    println!();
    println!("Summary");
    println!("-------");
    println!("  Files scanned:      {}", report.summary.total_files);
    println!("  Lines of code:      {}", report.summary.lines_of_code);
    println!("  Rust files:         {}", report.summary.rust_files);
    println!("  TypeScript files:   {}", report.summary.typescript_files);
    println!("  Config files:       {}", report.summary.config_files);
    println!("  Doc files:          {}", report.summary.doc_files);
    println!("  Dependency files:   {}", report.summary.dependency_files);
    println!(
        "  Coherence score:    {:.1}%",
        report.summary.coherence_score
    );
    println!(
        "  Complexity warns:   {}",
        report.summary.complexity_warnings
    );

    if !report.findings.is_empty() {
        println!();
        println!("Findings ({} total)", report.findings.len());
        println!("---------");
        for finding in &report.findings {
            let loc = finding
                .line
                .map(|l| format!(":{l}"))
                .unwrap_or_default();
            println!(
                "  [{:>8}] {}{} — {}",
                finding.severity, finding.file, loc, finding.message
            );
        }
    }

    println!();
    println!("Assessment saved to .weftos/artifacts/assessment-latest.json");
}

fn print_json_report(report: &AssessmentReport) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string_pretty(report)?);
    Ok(())
}

fn print_github_annotations(report: &AssessmentReport) {
    for finding in &report.findings {
        let level = match finding.severity.as_str() {
            "critical" | "high" => "error",
            "medium" => "warning",
            _ => "notice",
        };
        let line = finding.line.unwrap_or(1);
        println!(
            "::{level} file={},line={line}::{}",
            finding.file, finding.message
        );
    }
}

// ---------------------------------------------------------------------------
// Git hooks
// ---------------------------------------------------------------------------

fn run_hooks(hook_type: &str, dir: Option<&str>, uninstall: bool) -> anyhow::Result<()> {
    let valid_hooks = ["post-commit", "pre-push"];
    if !valid_hooks.contains(&hook_type) {
        anyhow::bail!(
            "unsupported hook type '{hook_type}' — use one of: {}",
            valid_hooks.join(", ")
        );
    }

    let project = resolve_project_dir(dir);
    let hooks_dir = project.join(".git/hooks");

    if !hooks_dir.exists() {
        anyhow::bail!(
            "No .git/hooks/ directory at {} — is this a git repository?",
            project.display()
        );
    }

    let hook_path = hooks_dir.join(hook_type);

    if uninstall {
        if hook_path.exists() {
            std::fs::remove_file(&hook_path)?;
            println!("Removed git hook: {}", hook_path.display());
        } else {
            println!("No {hook_type} hook installed.");
        }
        return Ok(());
    }

    let hook_script = r#"#!/bin/sh
# WeftOS assessment hook — installed by `weft assess hooks`
# Runs assessment scoped to the latest commit.
weft assess run --scope commit 2>&1 || true
"#.to_string();

    std::fs::write(&hook_path, hook_script)?;

    // Make executable (Unix)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&hook_path)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&hook_path, perms)?;
    }

    println!("Installed {hook_type} hook at {}", hook_path.display());
    println!("  Assessment will run automatically on each {hook_type}.");
    println!("  To remove: weft assess hooks --hook-type {hook_type} --uninstall");

    Ok(())
}

// ---------------------------------------------------------------------------
// Status + Init
// ---------------------------------------------------------------------------

fn run_status(dir: Option<&str>) -> anyhow::Result<()> {
    let project = resolve_project_dir(dir);
    let latest = project.join(".weftos/artifacts/assessment-latest.json");

    if !latest.exists() {
        println!("No assessment results found.");
        println!("Run `weft assess` to perform an assessment.");
        return Ok(());
    }

    let content = std::fs::read_to_string(&latest)?;
    let report: serde_json::Value = serde_json::from_str(&content)?;

    println!("Last Assessment");
    println!("===============");
    if let Some(ts) = report.get("timestamp").and_then(|v| v.as_str()) {
        println!("  Timestamp: {ts}");
    }
    if let Some(scope) = report.get("scope").and_then(|v| v.as_str()) {
        println!("  Scope:     {scope}");
    }
    if let Some(n) = report.get("files_scanned").and_then(|v| v.as_u64()) {
        println!("  Files:     {n}");
    }
    if let Some(summary) = report.get("summary") {
        if let Some(loc) = summary.get("lines_of_code").and_then(|v| v.as_u64()) {
            println!("  LOC:       {loc}");
        }
        if let Some(cs) = summary.get("coherence_score").and_then(|v| v.as_f64()) {
            println!("  Coherence: {cs:.1}%");
        }
    }
    if let Some(findings) = report.get("findings").and_then(|v| v.as_array()) {
        println!("  Findings:  {}", findings.len());
    }

    Ok(())
}

fn run_init(dir: Option<&str>, force: bool) -> anyhow::Result<()> {
    let project = resolve_project_dir(dir);
    let weftos_dir = project.join(".weftos");
    let config_path = weftos_dir.join("weave.toml");

    if config_path.exists() && !force {
        println!(
            ".weftos/weave.toml already exists at {}",
            config_path.display()
        );
        println!("Use --force to overwrite.");
        return Ok(());
    }

    std::fs::create_dir_all(weftos_dir.join("artifacts"))?;
    std::fs::create_dir_all(weftos_dir.join("memory"))?;

    // Derive a default project name from the directory basename.
    let project_name = project
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("my-project");

    let config = format!(
        r#"# WeftOS Assessment Configuration
# See: https://weftos.weavelogic.ai/docs/weftos/guides/assessment

[project]
name = "{project_name}"
org = "weavelogic"
environment = "development"

[assessment]
version = 1

[assessment.sources.files]
patterns = ["**/*.rs", "**/*.ts", "**/*.tsx", "**/*.json"]
exclude = ["node_modules/**", "target/**", ".weftos/**", ".git/**"]

[assessment.triggers.filesystem]
enabled = false
debounce_ms = 2000
patterns = ["**/*.rs", "**/*.ts", "**/*.json"]
exclude = ["node_modules/**", "target/**"]

[assessment.triggers.scheduled]
enabled = false
cron = "0 2 * * *"
scope = "full"

[assessment.reporting]
default_format = "table"
save_artifacts = true
"#
    );

    std::fs::write(&config_path, config)?;

    println!("Initialized WeftOS assessment at {}", weftos_dir.display());
    println!("  Created: .weftos/weave.toml");
    println!("  Created: .weftos/artifacts/");
    println!("  Created: .weftos/memory/");
    println!();
    // Create peers.json for cross-project coordination
    let peers_path = weftos_dir.join("peers.json");
    if !peers_path.exists() {
        std::fs::write(&peers_path, "[]")?;
        println!("  Created: .weftos/peers.json");
    }

    println!();
    println!("Next steps:");
    println!("  weft assess              # run your first assessment");
    println!("  weft assess status       # view results");
    println!("  weft assess link <name> <path-or-url>  # link a peer project");

    Ok(())
}

// ---------------------------------------------------------------------------
// Cross-project coordination
// ---------------------------------------------------------------------------

#[derive(serde::Serialize, serde::Deserialize, Clone)]
struct Peer {
    name: String,
    /// Local filesystem path or HTTP(S) URL to the peer's .weftos/ artifacts.
    ///
    /// Local:  /path/to/project/.weftos
    /// Remote: https://example.com/api/weftos/artifacts
    location: String,
    /// When this peer was linked.
    linked_at: String,
    /// Last known assessment timestamp from this peer.
    last_assessment: Option<String>,
}

impl Peer {
    fn is_remote(&self) -> bool {
        self.location.starts_with("http://") || self.location.starts_with("https://")
    }

    /// Fetch the latest assessment report from this peer.
    fn fetch_latest(&self) -> anyhow::Result<serde_json::Value> {
        if self.is_remote() {
            // HTTP fetch — works across servers
            let url = format!(
                "{}/assessment-latest.json",
                self.location.trim_end_matches('/')
            );
            let output = Command::new("curl")
                .args(["-fsSL", &url])
                .output()
                .map_err(|e| anyhow::anyhow!("failed to fetch {}: {}", url, e))?;
            if !output.status.success() {
                anyhow::bail!(
                    "failed to fetch {}: {}",
                    url,
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            let report: serde_json::Value =
                serde_json::from_slice(&output.stdout)?;
            Ok(report)
        } else {
            // Local filesystem
            let path = Path::new(&self.location).join("artifacts/assessment-latest.json");
            if !path.exists() {
                anyhow::bail!(
                    "no assessment found at {} — run `weft assess` in that project",
                    path.display()
                );
            }
            let content = std::fs::read_to_string(&path)?;
            let report: serde_json::Value = serde_json::from_str(&content)?;
            Ok(report)
        }
    }
}

fn load_peers(project: &Path) -> anyhow::Result<Vec<Peer>> {
    let peers_path = project.join(".weftos/peers.json");
    if !peers_path.exists() {
        return Ok(Vec::new());
    }
    let content = std::fs::read_to_string(&peers_path)?;
    let peers: Vec<Peer> = serde_json::from_str(&content)?;
    Ok(peers)
}

fn save_peers(project: &Path, peers: &[Peer]) -> anyhow::Result<()> {
    let peers_path = project.join(".weftos/peers.json");
    let json = serde_json::to_string_pretty(peers)?;
    std::fs::write(&peers_path, json)?;
    Ok(())
}

fn run_link(name: &str, location: &str, dir: Option<&str>) -> anyhow::Result<()> {
    let project = resolve_project_dir(dir);
    let weftos_dir = project.join(".weftos");

    if !weftos_dir.exists() {
        anyhow::bail!("No .weftos/ directory. Run `weft assess init` first.");
    }

    // Validate the peer is reachable
    let is_remote = location.starts_with("http://") || location.starts_with("https://");
    if !is_remote {
        let peer_path = Path::new(location);
        if !peer_path.exists() {
            anyhow::bail!("Peer path does not exist: {location}");
        }
        if !peer_path.join("artifacts").exists() {
            anyhow::bail!(
                "No artifacts/ directory at {location} — is this a .weftos/ directory?"
            );
        }
    }

    let mut peers = load_peers(&project)?;

    // Remove existing peer with same name
    peers.retain(|p| p.name != name);

    peers.push(Peer {
        name: name.to_string(),
        location: location.to_string(),
        linked_at: chrono::Utc::now().to_rfc3339(),
        last_assessment: None,
    });

    save_peers(&project, &peers)?;

    if is_remote {
        println!("Linked remote peer '{name}' at {location}");
    } else {
        println!("Linked local peer '{name}' at {location}");
    }
    println!("  Run `weft assess peers` to see all linked projects.");
    println!("  Run `weft assess compare {name}` to compare assessments.");

    Ok(())
}

fn run_peers(dir: Option<&str>) -> anyhow::Result<()> {
    let project = resolve_project_dir(dir);
    let peers = load_peers(&project)?;

    if peers.is_empty() {
        println!("No linked peers.");
        println!();
        println!("Link a project:");
        println!("  weft assess link <name> <path>    # local project");
        println!("  weft assess link <name> <url>     # remote server");
        println!();
        println!("Examples:");
        println!("  weft assess link frontend /path/to/frontend/.weftos");
        println!("  weft assess link api https://api.example.com/weftos/artifacts");
        return Ok(());
    }

    println!("Linked Peers ({} total)", peers.len());
    println!("=============");
    for peer in &peers {
        let kind = if peer.is_remote() { "remote" } else { "local " };
        let last = peer
            .last_assessment
            .as_deref()
            .unwrap_or("(none)");
        println!("  [{kind}] {:<20} {}", peer.name, peer.location);
        println!("           linked: {}  last assessment: {last}", peer.linked_at);
    }

    Ok(())
}

fn run_compare(peer_name: &str, dir: Option<&str>) -> anyhow::Result<()> {
    let project = resolve_project_dir(dir);
    let peers = load_peers(&project)?;

    let peer = peers
        .iter()
        .find(|p| p.name == peer_name)
        .ok_or_else(|| {
            anyhow::anyhow!("peer '{peer_name}' not found — run `weft assess link` first")
        })?;

    // Load local assessment
    let local_path = project.join(".weftos/artifacts/assessment-latest.json");
    if !local_path.exists() {
        anyhow::bail!("No local assessment found. Run `weft assess` first.");
    }
    let local_content = std::fs::read_to_string(&local_path)?;
    let local: serde_json::Value = serde_json::from_str(&local_content)?;

    // Load peer assessment
    println!("Fetching assessment from '{}'...", peer.name);
    let remote = peer.fetch_latest()?;

    // Update last_assessment timestamp
    let mut peers_mut = peers.clone();
    if let Some(p) = peers_mut.iter_mut().find(|p| p.name == peer_name) {
        p.last_assessment = remote
            .get("timestamp")
            .and_then(|v| v.as_str())
            .map(String::from);
    }
    save_peers(&project, &peers_mut)?;

    // Print comparison
    println!();
    println!("Cross-Project Comparison");
    println!("========================");
    println!();

    let local_name = local
        .get("project")
        .and_then(|v| v.as_str())
        .unwrap_or("this project");
    let remote_name = remote
        .get("project")
        .and_then(|v| v.as_str())
        .unwrap_or(&peer.name);

    println!("  {:<30} {:<15} {:<15}", "", "Local", &peer.name);
    println!("  {:<30} {:<15} {:<15}", "", "-----", "-----");

    // Compare summaries
    let ls = local.get("summary").cloned().unwrap_or_default();
    let rs = remote.get("summary").cloned().unwrap_or_default();

    let fields = [
        ("total_files", "Files"),
        ("lines_of_code", "Lines of code"),
        ("rust_files", "Rust files"),
        ("typescript_files", "TypeScript files"),
        ("doc_files", "Doc files"),
        ("dependency_files", "Dependency files"),
        ("complexity_warnings", "Complexity warnings"),
    ];

    for (key, label) in &fields {
        let lv = ls.get(*key).and_then(|v| v.as_u64()).unwrap_or(0);
        let rv = rs.get(*key).and_then(|v| v.as_u64()).unwrap_or(0);
        println!("  {:<30} {:<15} {:<15}", label, lv, rv);
    }

    let lcs = ls
        .get("coherence_score")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let rcs = rs
        .get("coherence_score")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    println!(
        "  {:<30} {:<15} {:<15}",
        "Coherence score",
        format!("{lcs:.1}%"),
        format!("{rcs:.1}%")
    );

    let lf = local
        .get("findings")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    let rf = remote
        .get("findings")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    println!("  {:<30} {:<15} {:<15}", "Findings", lf, rf);

    // Shared dependency analysis
    println!();
    println!("Shared Dependencies");
    println!("-------------------");

    let local_deps = extract_dependency_names(&local);
    let remote_deps = extract_dependency_names(&remote);
    let shared: Vec<&str> = local_deps
        .iter()
        .filter(|d| remote_deps.contains(d))
        .copied()
        .collect();

    if shared.is_empty() {
        println!("  (no shared dependencies detected in assessment data)");
    } else {
        for dep in &shared {
            println!("  - {dep}");
        }
    }

    println!();
    println!("Assessments compared: {} vs {}", local_name, remote_name);

    Ok(())
}

fn extract_dependency_names(report: &serde_json::Value) -> Vec<&str> {
    // Extract dependency file names from findings
    report
        .get("findings")
        .and_then(|v| v.as_array())
        .map(|findings| {
            findings
                .iter()
                .filter_map(|f| {
                    if f.get("category").and_then(|c| c.as_str()) == Some("dependency") {
                        f.get("file").and_then(|f| f.as_str())
                    } else {
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}
