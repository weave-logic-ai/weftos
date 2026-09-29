//! Default locations for ExoChain storage (checkpoint, RVF, signing key,
//! anchor ledger).
//!
//! A daemon started with `WEFTOS_RUNTIME_DIR` set (probe, demo and test
//! daemons) must never load or append to the operator's real chain under
//! `~/.clawft/`. When that variable is set, every *default* chain path
//! resolves under the runtime dir instead; the daemon still chains all of
//! its events, just to its own isolated chain file and key.
//!
//! Precedence, highest first:
//! 1. an explicit path in config (`kernel.chain.checkpoint_path`,
//!    `kernel.chain.external_anchor.ledger_path`) always wins;
//! 2. `$WEFTOS_RUNTIME_DIR` when set and non-empty;
//! 3. `~/.clawft/` (unchanged behaviour for operators who set neither).
//!
//! The RVF file, signing key and resource-tree checkpoint are all derived
//! from the checkpoint path by extension (`chain.rvf`, `chain.key`,
//! `chain.tree.json`), so isolating the checkpoint path isolates them all.

use std::path::{Path, PathBuf};

/// Environment variable that points a daemon at an isolated runtime dir.
pub const RUNTIME_DIR_ENV: &str = "WEFTOS_RUNTIME_DIR";

/// File name of the chain JSON checkpoint inside the chain root.
pub const CHAIN_CHECKPOINT_FILE: &str = "chain.json";

/// Resolve the directory that holds default chain files.
///
/// `runtime_dir` is the raw value of [`RUNTIME_DIR_ENV`] (if any); an empty
/// or whitespace-only value counts as unset. Falls back to `<home>/.clawft`.
pub fn chain_root(runtime_dir: Option<&str>, home: Option<&Path>) -> Option<PathBuf> {
    match runtime_dir.map(str::trim) {
        Some(dir) if !dir.is_empty() => Some(PathBuf::from(dir)),
        _ => home.map(|h| h.join(".clawft")),
    }
}

/// Resolve the chain checkpoint path (see module docs for precedence).
pub fn resolve_checkpoint_path(
    explicit: Option<&str>,
    runtime_dir: Option<&str>,
    home: Option<&Path>,
) -> Option<PathBuf> {
    if let Some(p) = explicit {
        return Some(PathBuf::from(p));
    }
    chain_root(runtime_dir, home).map(|root| root.join(CHAIN_CHECKPOINT_FILE))
}

/// Resolve the external-anchor ledger path (see module docs for precedence).
pub fn resolve_anchor_ledger_path(
    explicit: Option<&str>,
    runtime_dir: Option<&str>,
    home: Option<&Path>,
) -> Option<PathBuf> {
    if let Some(p) = explicit {
        return Some(PathBuf::from(p));
    }
    chain_root(runtime_dir, home).map(|root| root.join("chain").join("anchors.jsonl"))
}

/// Current value of [`RUNTIME_DIR_ENV`], if set.
pub fn runtime_dir_from_env() -> Option<String> {
    std::env::var(RUNTIME_DIR_ENV).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> PathBuf {
        PathBuf::from("/home/op")
    }

    #[test]
    fn no_runtime_dir_keeps_operator_home_default() {
        let p = resolve_checkpoint_path(None, None, Some(&home())).unwrap();
        assert_eq!(p, PathBuf::from("/home/op/.clawft/chain.json"));
        let l = resolve_anchor_ledger_path(None, None, Some(&home())).unwrap();
        assert_eq!(l, PathBuf::from("/home/op/.clawft/chain/anchors.jsonl"));
    }

    #[test]
    fn runtime_dir_isolates_checkpoint_rvf_and_key() {
        let p = resolve_checkpoint_path(None, Some("/run/probe"), Some(&home())).unwrap();
        assert_eq!(p, PathBuf::from("/run/probe/chain.json"));
        // RVF and key are derived by extension, so they follow.
        assert_eq!(p.with_extension("rvf"), PathBuf::from("/run/probe/chain.rvf"));
        assert_eq!(p.with_extension("key"), PathBuf::from("/run/probe/chain.key"));
        assert!(!p.starts_with("/home/op"));
    }

    #[test]
    fn runtime_dir_isolates_anchor_ledger() {
        let l = resolve_anchor_ledger_path(None, Some("/run/probe"), Some(&home())).unwrap();
        assert_eq!(l, PathBuf::from("/run/probe/chain/anchors.jsonl"));
    }

    #[test]
    fn explicit_config_path_wins_over_runtime_dir() {
        let p = resolve_checkpoint_path(Some("/data/c.json"), Some("/run/probe"), Some(&home()))
            .unwrap();
        assert_eq!(p, PathBuf::from("/data/c.json"));
        let l = resolve_anchor_ledger_path(Some("/data/a.jsonl"), Some("/run/probe"), None)
            .unwrap();
        assert_eq!(l, PathBuf::from("/data/a.jsonl"));
    }

    #[test]
    fn empty_runtime_dir_counts_as_unset() {
        let p = resolve_checkpoint_path(None, Some("  "), Some(&home())).unwrap();
        assert_eq!(p, PathBuf::from("/home/op/.clawft/chain.json"));
    }

    #[test]
    fn no_home_and_no_runtime_dir_yields_none() {
        assert!(resolve_checkpoint_path(None, None, None).is_none());
        assert!(resolve_checkpoint_path(None, Some("/run/p"), None).is_some());
    }
}
