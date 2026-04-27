//! `weaver` — WeftOS operator CLI.
//!
//! The human-facing CLI for kernel management, agent orchestration,
//! and system administration. Complement to `weft` (the agent CLI).
//!
//! # Commands
//!
//! - `weaver kernel` — Boot, status, process table, services.
//! - `weaver agent` — Spawn, stop, restart, inspect agents (planned).
//! - `weaver app` — Install, start, stop applications (planned).
//! - `weaver ipc` — Send messages, manage topics (planned).

use clap::{Parser, Subcommand};

use clawft_weave::commands;

/// WeftOS operator CLI.
#[derive(Parser)]
#[command(
    name = "weaver",
    about = "WeftOS operator CLI — kernel, agents, and system management",
    version,
    disable_help_subcommand = true
)]
struct Cli {
    /// Enable verbose (debug-level) logging.
    #[arg(short, long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Commands,
}

/// Top-level subcommands.
#[derive(Subcommand)]
enum Commands {
    /// Kernel management (boot, status, services, processes).
    Kernel(commands::kernel_cmd::KernelArgs),

    /// Agent lifecycle management (spawn, stop, restart, inspect).
    Agent(commands::agent_cmd::AgentArgs),

    /// Application management (install, start, stop, list).
    App(commands::app_cmd::AppArgs),

    /// Cluster management (nodes, shards, health).
    Cluster(commands::cluster_cmd::ClusterArgs),

    /// Chain management (status, events, checkpoints).
    Chain(commands::chain_cmd::ChainArgs),

    /// Custody attestation (signed proof of system state).
    Custody(commands::custody_cmd::CustodyArgs),

    /// Resource tree management (tree, inspect, stats).
    Resource(commands::resource_cmd::ResourceArgs),

    /// Cron job management (add, list, remove).
    Cron(commands::cron_cmd::CronArgs),

    /// IPC management (topics, subscribe, publish).
    Ipc(commands::ipc_cmd::IpcArgs),

    /// Interactive kernel console (boot + REPL, or attach to running kernel).
    #[cfg(unix)]
    Console(commands::console_cmd::ConsoleArgs),

    /// ECC cognitive substrate management (status, calibrate, search).
    Ecc(commands::ecc_cmd::EccArgs),

    /// Knowledge graph extraction, query, and export (graphify).
    Graphify(commands::graphify_cmd::GraphifyArgs),

    /// Obsidian vault cultivation (frontmatter, links, graph analysis).
    Vault(commands::vault_cmd::VaultArgs),

    /// Topology layout, schema validation, and geometry detection.
    Topology(commands::topology_cmd::TopologyArgs),

    /// Leaf device control (push audio, display, effects).
    Leaf(commands::leaf_cmd::LeafArgs),

    /// Run standardized kernel performance benchmark.
    Benchmark {
        #[command(subcommand)]
        cmd: commands::bench_cmd::BenchCmd,
    },

    /// Initialize development environment (install skills, verify tools).
    Init(commands::init_cmd::InitArgs),

    /// Update both weft and weaver binaries to latest release.
    Update {
        #[command(subcommand)]
        cmd: Option<commands::update_cmd::UpdateCmd>,
    },

    /// Show version and build info.
    Version,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Best-effort load of `.env` from the current working directory,
    // before any subcommand reads env vars. Lets `OPENROUTER_API_KEY`,
    // `LLM_SERVICE_URL`, `LLM_MODEL`, etc. live in a project-local
    // `.env` (which is gitignored) without forcing shell exports.
    // Silently ignored if no file exists.
    let _ = dotenvy::dotenv();

    let cli = Cli::parse();

    let default_filter = if cli.verbose { "debug" } else { "warn" };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| default_filter.into()),
        )
        .init();

    // Check for updates (non-blocking, cached 24h)
    clawft_rpc::version_check::check_for_updates();

    match cli.command {
        Commands::Kernel(args) => commands::kernel_cmd::run(args).await?,
        Commands::Agent(args) => commands::agent_cmd::run(args).await?,
        Commands::App(args) => commands::app_cmd::run(args).await?,
        Commands::Cluster(args) => commands::cluster_cmd::run(args).await?,
        Commands::Chain(args) => commands::chain_cmd::run(args).await?,
        Commands::Custody(args) => commands::custody_cmd::run(args).await?,
        Commands::Resource(args) => commands::resource_cmd::run(args).await?,
        Commands::Cron(args) => commands::cron_cmd::run(args).await?,
        Commands::Ipc(args) => commands::ipc_cmd::run(args).await?,
        #[cfg(unix)]
        Commands::Console(args) => commands::console_cmd::run(args).await?,
        Commands::Ecc(args) => commands::ecc_cmd::run(args).await?,
        Commands::Graphify(args) => commands::graphify_cmd::run(args).await?,
        Commands::Vault(args) => commands::vault_cmd::run(args).await?,
        Commands::Topology(args) => commands::topology_cmd::run(args).await?,
        Commands::Leaf(args) => commands::leaf_cmd::run(args).await?,
        Commands::Benchmark { cmd } => commands::bench_cmd::run(cmd).await?,
        Commands::Update { cmd } => match cmd {
            Some(c) => commands::update_cmd::run(c).await?,
            None => commands::update_cmd::run_default().await?,
        },
        Commands::Init(args) => commands::init_cmd::run(args).await?,
        Commands::Version => {
            println!(
                "weaver {} (WeftOS) · git {} · built {}",
                env!("CARGO_PKG_VERSION"),
                env!("BUILD_GIT_HASH"),
                env!("BUILD_TIMESTAMP"),
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_parses_without_error() {
        Cli::command().debug_assert();
    }

    #[test]
    fn cli_help_contains_binary_name() {
        let help = Cli::command().render_help().to_string();
        assert!(help.contains("weaver"));
    }
}
