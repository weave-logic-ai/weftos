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

pub use crate::runtime_paths::{
    CHAIN_CHECKPOINT_FILE, LEGACY_MIGRATED_MARKER, MIGRATED_FROM_FILE, RUNTIME_DIR_ENV,
    user_chain_root,
};
