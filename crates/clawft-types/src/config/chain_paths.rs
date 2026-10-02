//! Chain storage locations.
//!
//! Resolution now lives in [`crate::runtime_paths::RuntimePaths`], the single
//! resolver for every runtime file (ADR-103 D4). Precedence for chain files,
//! highest first:
//!
//! 1. an explicit path in config (`kernel.chain.checkpoint_path`,
//!    `kernel.chain.external_anchor.ledger_path`);
//! 2. `RuntimePaths::resolve()` (`$WEFTOS_RUNTIME_DIR`, else the project's
//!    `.weftos/runtime`, else legacy `~/.clawft`).
//!
//! The RVF file, signing key and resource-tree checkpoint are derived from the
//! checkpoint path by extension (`chain.rvf`, `chain.key`, `chain.tree.json`).
//!
//! The user key (`~/.weftos/user.key`, ADR-103 D-5) carries the same seed as
//! the migrated `chain.key`; [`chain_key_for_checkpoint`] makes the chain
//! loader prefer it.

use std::path::{Path, PathBuf};

pub use crate::runtime_paths::{CHAIN_CHECKPOINT_FILE, RUNTIME_DIR_ENV};

/// File name of the user key under `~/.weftos`.
pub const USER_KEY_FILE: &str = "user.key";

/// `<home>/.weftos/user.key`, the user's Ed25519 seed (0600).
pub fn user_key_path(home: &Path) -> PathBuf {
    crate::runtime_paths::user_weftos_dir(home).join(USER_KEY_FILE)
}

/// Where the chain signing key for `checkpoint` lives.
///
/// The default is the checkpoint path with a `.key` extension. For the user
/// chain (`<home>/.weftos/chain/chain.json`) an existing
/// `<home>/.weftos/user.key` wins; `chain.key` stays the fallback and is
/// never deleted by code.
pub fn chain_key_for_checkpoint(checkpoint: &Path) -> PathBuf {
    let fallback = checkpoint.with_extension("key");
    let user_key = checkpoint
        .parent()
        .filter(|chain_dir| chain_dir.file_name().is_some_and(|n| n == "chain"))
        .and_then(Path::parent)
        .filter(|weftos| weftos.file_name().is_some_and(|n| n == ".weftos"))
        .map(|weftos| weftos.join(USER_KEY_FILE));
    match user_key {
        Some(p) if std::fs::symlink_metadata(&p).is_ok() => p,
        _ => fallback,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_key_hangs_off_weftos_dir() {
        assert_eq!(user_key_path(Path::new("/h")), Path::new("/h/.weftos/user.key"));
    }

    #[test]
    fn loader_prefers_user_key_and_falls_back_to_chain_key() {
        let t = tempfile::tempdir().unwrap();
        let weftos = t.path().join(".weftos");
        let ckpt = weftos.join("chain").join("chain.json");
        std::fs::create_dir_all(ckpt.parent().unwrap()).unwrap();
        // No user.key: chain.key.
        assert_eq!(chain_key_for_checkpoint(&ckpt), weftos.join("chain").join("chain.key"));
        std::fs::write(weftos.join("user.key"), [1u8; 32]).unwrap();
        assert_eq!(chain_key_for_checkpoint(&ckpt), weftos.join("user.key"));
    }

    #[test]
    fn other_checkpoints_always_use_their_own_key() {
        let t = tempfile::tempdir().unwrap();
        // A user.key beside an unrelated chain must not hijack it.
        std::fs::write(t.path().join("user.key"), [1u8; 32]).unwrap();
        let ckpt = t.path().join("runtime").join("chain.json");
        assert_eq!(chain_key_for_checkpoint(&ckpt), t.path().join("runtime").join("chain.key"));
    }
}
