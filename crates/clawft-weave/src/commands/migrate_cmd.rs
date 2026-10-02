//! `weaver migrate` subcommands (ADR-103 Phase 1).
//!
//! `weaver migrate user-chain [--dry-run] [--from DIR] [--to DIR]` copies the
//! legacy `~/.clawft` chain to `~/.weftos/chain`, verified, source untouched.
//!
//! `weaver migrate user-key [--dry-run]` (Phase 3, D-5) copies the seed of the
//! migrated `chain.key` to `~/.weftos/user.key`; idempotent, `chain.key` is
//! never modified.

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
        /// Migrate even if chain.key is missing or the chain signature cannot
        /// be verified against it.
        #[arg(long)]
        allow_unsigned: bool,
    },
    /// Copy chain.key's seed to ~/.weftos/user.key (same public key, same user id).
    ///
    /// With --rotate: replace the user key instead. The handover is recorded
    /// in the manifest store, signed by the old and the new key, so
    /// certificates, anchor records and policies sealed by the old key keep
    /// verifying up to the rotation point (ADR-103 A13). Stop the user daemon
    /// first; afterwards restart it, restart project kernels, and run
    /// `weaver mesh bind rebind` if the machine mesh service is installed.
    #[command(name = "user-key")]
    UserKey {
        /// Show what would happen without writing anything.
        #[arg(long)]
        dry_run: bool,
        /// Rotate the user key (new key, dual-signed handover) instead of migrating.
        #[arg(long)]
        rotate: bool,
    },
}

fn describe_user_key(outcome: &crate::user_key::MigrateOutcome) -> String {
    use crate::user_key::{MIGRATED_FROM_FILE, MigrateOutcome};
    match outcome {
        MigrateOutcome::WouldCopy { from, to } => format!(
            "dry run: would copy the seed of {} to {} (0600) and write {MIGRATED_FROM_FILE}; {} is never modified",
            from.display(),
            to.display(),
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

fn describe_rotation(outcome: &crate::user_key_rotate::RotateOutcome) -> String {
    use crate::user_key_rotate::RotateOutcome;
    match outcome {
        RotateOutcome::WouldRotate { old_key_id } => format!(
            "dry run: would retire user key {old_key_id}, create a new one and record a dual-signed handover; nothing written"
        ),
        RotateOutcome::Rotated { seq, old_key_id, new_key_id, retired } => format!(
            "rotated the user key (record {seq}): {old_key_id} -> {new_key_id}.{} Next: start the user daemon \
             (it chains the handover), restart each project kernel (`weaver kernel restart --project <id>`; \
             a running child still pins the old key), and run `weaver mesh bind rebind` if the machine mesh \
             service is installed. Delete the retired key yourself once you are satisfied; code never does.",
            retired.as_ref().map_or(String::new(), |p| format!(" The old private key is kept at {}.", p.display()))
        ),
    }
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

/// The `kernel.chain.checkpoint_path` set in a `config.json` body, if any
/// (snake or camel case). An explicit path bypasses the chain guards, so the
/// user daemon would keep writing the chain this command is about to retire.
fn config_checkpoint_path(config_json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(config_json).ok()?;
    let chain = v.get("kernel")?.get("chain")?;
    ["checkpoint_path", "checkpointPath"]
        .iter()
        .find_map(|k| chain.get(*k)?.as_str())
        .filter(|p| !p.trim().is_empty())
        .map(str::to_owned)
}

/// Warn when `~/.clawft/config.json` pins `kernel.chain.checkpoint_path`.
fn warn_explicit_checkpoint(home: Option<&std::path::Path>) {
    let Some(cfg) = home.map(|h| h.join(".clawft").join("config.json")) else {
        return;
    };
    let Some(path) = std::fs::read_to_string(&cfg)
        .ok()
        .and_then(|t| config_checkpoint_path(&t))
    else {
        return;
    };
    eprintln!(
        "warning: {} sets kernel.chain.checkpoint_path = {path}. An explicit path bypasses the \
         chain guards, so a kernel started with this config keeps writing that chain after the \
         migration and the migrated copy goes stale. Remove the key or point it at the migrated \
         chain before starting the user daemon (a path inside a migrated directory is refused at \
         boot unless --adopt-legacy-chain is passed).",
        cfg.display()
    );
}

/// Run the migrate subcommand.
pub fn run(args: MigrateArgs) -> anyhow::Result<()> {
    match args.action {
        MigrateAction::UserKey { dry_run, rotate } => {
            let home = home_dir().ok_or_else(|| anyhow::anyhow!("cannot determine the home directory"))?;
            if rotate {
                let manifests = crate::user_daemon::manifests_dir(&home);
                println!("{}", describe_rotation(&crate::user_key_rotate::rotate_user_key(&home, &manifests, dry_run, chrono::Utc::now())?));
            } else {
                println!("{}", describe_user_key(&crate::user_key::migrate_user_key(&home, dry_run)?));
            }
        }
        MigrateAction::UserChain {
            dry_run,
            from,
            to,
            allow_unsigned,
        } => {
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
                allow_unsigned,
            };
            warn_explicit_checkpoint(home.as_deref());
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
                    println!(
                        "  rollback: rm -r {} ; the legacy chain is intact. Remove {}/MIGRATED-TO-WEFTOS.txt too, \
                         or pass --adopt-legacy-chain to start a kernel on the legacy chain.",
                        p.to.display(),
                        p.from.display()
                    );
                }
                Outcome::AlreadyMigrated(_, marked) => {
                    println!("already migrated: {} holds this chain", to.display());
                    if marked {
                        println!("  wrote the missing MIGRATED-TO-WEFTOS.txt marker beside {}", from.display());
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::config_checkpoint_path;

    #[test]
    fn finds_an_explicit_checkpoint_path() {
        let j = r#"{"kernel":{"chain":{"checkpoint_path":"/h/.clawft/chain.json"}}}"#;
        assert_eq!(config_checkpoint_path(j).as_deref(), Some("/h/.clawft/chain.json"));
        let j = r#"{"kernel":{"chain":{"checkpointPath":"/x/chain.json"}}}"#;
        assert_eq!(config_checkpoint_path(j).as_deref(), Some("/x/chain.json"));
    }

    #[test]
    fn ignores_absent_blank_or_malformed() {
        assert_eq!(config_checkpoint_path(r#"{"kernel":{"chain":{}}}"#), None);
        assert_eq!(config_checkpoint_path(r#"{"kernel":{"chain":{"checkpoint_path":" "}}}"#), None);
        assert_eq!(config_checkpoint_path("not json"), None);
        assert_eq!(config_checkpoint_path("{}"), None);
    }
}
