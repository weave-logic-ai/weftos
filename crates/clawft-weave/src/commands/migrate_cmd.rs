//! `weaver migrate` subcommands (ADR-103 Phase 1).
//!
//! `weaver migrate user-chain [--dry-run] [--from DIR] [--to DIR]` copies the
//! legacy `~/.clawft` chain to `~/.weftos/chain`, verified, source untouched.

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use clawft_kernel::chain_migrate::{MigrateOptions, Outcome, Plan, migrate_user_chain};
use clawft_types::runtime_paths::{home_dir, user_chain_root};

/// Migration subcommand.
#[derive(Parser)]
#[command(about = "Migrate WeftOS state to the current layout")]
pub struct MigrateArgs {
    /// What to migrate.
    #[command(subcommand)]
    pub action: MigrateAction,
}

/// Migration targets.
#[derive(Subcommand)]
pub enum MigrateAction {
    /// Copy the legacy ~/.clawft chain to ~/.weftos/chain (verified; source kept).
    #[command(name = "user-chain")]
    UserChain {
        /// Print what would happen (sizes, head seq/hash) and write nothing.
        #[arg(long)]
        dry_run: bool,
        /// Legacy chain directory (default: ~/.clawft).
        #[arg(long)]
        from: Option<PathBuf>,
        /// Destination chain directory (default: ~/.weftos/chain).
        #[arg(long)]
        to: Option<PathBuf>,
    },
}

fn print_plan(p: &Plan) {
    println!("  from:  {}", p.from.display());
    println!("  to:    {}", p.to.display());
    for f in &p.files {
        println!("  file:  {:<22} {:>12} bytes  sha256 {}", f.name, f.bytes, f.sha256);
    }
    println!(
        "  head:  seq {}  events {}  hash {}  signature {}",
        p.head.sequence, p.head.events, p.head.hash, p.head.signature
    );
}

/// Run the migrate subcommand.
pub fn run(args: MigrateArgs) -> anyhow::Result<()> {
    match args.action {
        MigrateAction::UserChain { dry_run, from, to } => {
            let home = home_dir();
            let from = from
                .or_else(|| home.as_ref().map(|h| h.join(".clawft")))
                .ok_or_else(|| anyhow::anyhow!("cannot resolve HOME; pass --from"))?;
            let to = to
                .or_else(|| home.as_deref().map(user_chain_root))
                .ok_or_else(|| anyhow::anyhow!("cannot resolve HOME; pass --to"))?;
            let opts = MigrateOptions {
                from: &from,
                to: &to,
                dry_run,
                now: std::time::SystemTime::now(),
                tool_version: env!("CARGO_PKG_VERSION"),
            };
            match migrate_user_chain(&opts).map_err(|e| anyhow::anyhow!("{e}"))? {
                Outcome::DryRun(p) => {
                    println!("dry run: would copy, verify, then atomically place the chain");
                    print_plan(&p);
                    println!("  steps: lock source chain.lock, copy to a temp dir beside the destination,");
                    println!("         fsync, verify (hashes, head, integrity, signature), rename, write markers.");
                    println!("  nothing was written.");
                }
                Outcome::Migrated(p) => {
                    println!("migrated and verified");
                    print_plan(&p);
                    println!("  source {} is unchanged (a MIGRATED-TO-WEFTOS.txt marker was added).", p.from.display());
                    println!("  rollback: rm -r {} ; the legacy chain is intact.", p.to.display());
                }
                Outcome::AlreadyMigrated(_) => {
                    println!("already migrated: {} holds this chain; nothing to do", to.display());
                }
            }
        }
    }
    Ok(())
}
