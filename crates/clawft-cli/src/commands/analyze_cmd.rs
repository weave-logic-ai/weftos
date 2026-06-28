//! `weft analyze` — Analysis commands that delegate to weaver.
//!
//! These commands proxy to `weaver topology` and `weaver vault` so agents
//! can run analysis without needing the full kernel/graphify stack in the
//! agent binary.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(about = "Code analysis, topology extraction, and vault cultivation")]
pub struct AnalyzeArgs {
    #[command(subcommand)]
    pub action: AnalyzeAction,
}

#[derive(Subcommand)]
pub enum AnalyzeAction {
    /// Extract a knowledge graph from a codebase.
    Extract {
        /// Path to source code.
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Output path for graph JSON.
        #[arg(short, long, default_value = "graphify-out/graph.json")]
        output: PathBuf,
        /// Also generate navigator slices.
        #[arg(long)]
        slices: Option<PathBuf>,
    },
    /// Detect the topology structure of a knowledge graph.
    Detect {
        /// Path to graph JSON.
        graph: PathBuf,
    },
    /// Infer a topology schema from a knowledge graph.
    Infer {
        /// Path to graph JSON.
        graph: PathBuf,
        /// Output path for schema YAML.
        #[arg(short, long, default_value = "inferred.topology.yaml")]
        output: PathBuf,
    },
    /// Diff declared schema against inferred from graph.
    Diff {
        /// Declared schema YAML.
        schema: PathBuf,
        /// Knowledge graph JSON.
        graph: PathBuf,
    },
    /// Generate drill-down slices for the navigator.
    Slice {
        /// Path to graph JSON.
        graph: PathBuf,
        /// Output directory.
        #[arg(short, long, default_value = "slices")]
        output: PathBuf,
    },
    /// Export graph as VOWL JSON for the navigator widget.
    Vowl {
        /// Path to graph JSON.
        graph: PathBuf,
        /// Output path.
        #[arg(short, long, default_value = "vowl-graph.json")]
        output: PathBuf,
    },
    /// Enrich markdown files with YAML frontmatter.
    Enrich {
        /// Path to document directory.
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Overwrite existing frontmatter.
        #[arg(long)]
        force: bool,
    },
    /// Analyze vault link graph (orphans, clusters, density).
    Links {
        /// Path to document directory.
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Output format: table or json.
        #[arg(short, long, default_value = "table")]
        format: String,
    },
    /// Suggest new connections between documents.
    Suggest {
        /// Path to document directory.
        #[arg(default_value = ".")]
        path: PathBuf,
    },
}

pub async fn run(args: AnalyzeArgs) -> anyhow::Result<()> {
    let (subcommand, sub_args) = match args.action {
        AnalyzeAction::Extract {
            path,
            output,
            slices,
        } => {
            let mut a = vec![
                "topology".into(),
                "extract".into(),
                path.to_string_lossy().into(),
                "--output".into(),
                output.to_string_lossy().into(),
            ];
            if let Some(s) = slices {
                a.push("--slices".into());
                a.push(s.to_string_lossy().into());
            }
            ("weaver", a)
        }
        AnalyzeAction::Detect { graph } => (
            "weaver",
            vec![
                "topology".into(),
                "detect".into(),
                graph.to_string_lossy().into(),
            ],
        ),
        AnalyzeAction::Infer { graph, output } => (
            "weaver",
            vec![
                "topology".into(),
                "infer".into(),
                graph.to_string_lossy().into(),
                "--output".into(),
                output.to_string_lossy().into(),
            ],
        ),
        AnalyzeAction::Diff { schema, graph } => (
            "weaver",
            vec![
                "topology".into(),
                "diff".into(),
                schema.to_string_lossy().into(),
                graph.to_string_lossy().into(),
            ],
        ),
        AnalyzeAction::Slice { graph, output } => (
            "weaver",
            vec![
                "topology".into(),
                "slice".into(),
                graph.to_string_lossy().into(),
                "--output".into(),
                output.to_string_lossy().into(),
            ],
        ),
        AnalyzeAction::Vowl { graph, output } => (
            "weaver",
            vec![
                "topology".into(),
                "vowl".into(),
                graph.to_string_lossy().into(),
                "--output".into(),
                output.to_string_lossy().into(),
            ],
        ),
        AnalyzeAction::Enrich { path, force } => {
            let mut a = vec![
                "vault".into(),
                "enrich".into(),
                path.to_string_lossy().into(),
            ];
            if force {
                a.push("--force".into());
            }
            ("weaver", a)
        }
        AnalyzeAction::Links { path, format } => (
            "weaver",
            vec![
                "vault".into(),
                "analyze".into(),
                path.to_string_lossy().into(),
                "--format".into(),
                format,
            ],
        ),
        AnalyzeAction::Suggest { path } => (
            "weaver",
            vec![
                "vault".into(),
                "suggest".into(),
                path.to_string_lossy().into(),
            ],
        ),
    };

    // Find weaver binary.
    let weaver = which::which(subcommand).map_err(|_| {
        anyhow::anyhow!(
            "'weaver' not found in PATH. Install with: cargo install --path crates/clawft-weave"
        )
    })?;

    let status = tokio::process::Command::new(&weaver)
        .args(&sub_args)
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
        .await?;

    if !status.success() {
        anyhow::bail!("weaver exited with status {}", status);
    }

    Ok(())
}
