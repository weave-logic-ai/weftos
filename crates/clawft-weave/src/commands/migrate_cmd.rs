//! `weaver migrate`: one-way local state migrations (ADR-103).
//!
//! `weaver migrate user-key [--dry-run]` copies the seed of the migrated
//! `~/.weftos/chain/chain.key` to `~/.weftos/user.key` (decision D-5: one
//! identity, same user id). It is idempotent and never modifies or deletes
//! `chain.key`.

use clap::{Args, Subcommand};

use crate::user_key::{MigrateOutcome, migrate_user_key};

/// Arguments for `weaver migrate`.
#[derive(Args, Debug)]
pub struct MigrateArgs {
    #[command(subcommand)]
    pub command: MigrateCmd,
}

/// `weaver migrate` subcommands.
#[derive(Subcommand, Debug)]
pub enum MigrateCmd {
    /// Copy chain.key's seed to ~/.weftos/user.key (same public key, same user id).
    UserKey {
        /// Show what would happen without writing anything.
        #[arg(long)]
        dry_run: bool,
    },
}

/// Run `weaver migrate`.
pub async fn run(args: MigrateArgs) -> anyhow::Result<()> {
    match args.command {
        MigrateCmd::UserKey { dry_run } => {
            let home = clawft_types::runtime_paths::home_dir()
                .ok_or_else(|| anyhow::anyhow!("cannot determine the home directory"))?;
            println!("{}", describe(&migrate_user_key(&home, dry_run)?));
        }
    }
    Ok(())
}

fn describe(outcome: &MigrateOutcome) -> String {
    match outcome {
        MigrateOutcome::WouldCopy { from, to } => format!(
            "dry run: would copy the seed of {} to {} (0600) and write {}; {} is never modified",
            from.display(),
            to.display(),
            crate::user_key::MIGRATED_FROM_FILE,
            from.display()
        ),
        MigrateOutcome::Copied { from, to } => format!(
            "copied the seed of {} to {}; public keys verified equal, user id unchanged. \
             {} is kept (remove it yourself after one release)",
            from.display(),
            to.display(),
            from.display()
        ),
        MigrateOutcome::AlreadyMigrated { to } => {
            format!("{} already holds the same key; nothing to do", to.display())
        }
    }
}
