//! Pin the chain storage location once at boot.
//!
//! The kernel reads the chain checkpoint path in several places: boot
//! (signing key, RVF restore, tree checkpoint), the daemon's `chain.verify`
//! RPC, and shutdown persistence. [`pin_chain_storage`] resolves the path a
//! single time and writes it into the kernel config, so all of them agree
//! even if the environment changes after boot.
//!
//! Resolution follows [`clawft_types::config::chain_paths`]: an explicit
//! config path wins, then `$WEFTOS_RUNTIME_DIR/chain.json` (probe, demo and
//! test daemons get their own isolated chain and key), then the operator's
//! `~/.clawft/chain.json`. In this crate's own unit tests the default is a
//! fresh temp dir per boot, so `cargo test` never touches the operator
//! chain.

use std::path::PathBuf;

use clawft_types::config::KernelConfig;

/// Resolve the chain checkpoint path and pin it into `kernel_config.chain`.
///
/// Returns the pinned checkpoint path, or `None` when the chain is disabled
/// or no location can be resolved (no home dir, no runtime dir).
pub fn pin_chain_storage(kernel_config: &mut KernelConfig) -> Option<PathBuf> {
    let mut chain = kernel_config.chain.clone().unwrap_or_default();
    if !chain.enabled {
        return None;
    }
    if chain.checkpoint_path.is_none() {
        chain.checkpoint_path = default_checkpoint_path(&chain);
    }
    #[cfg(test)]
    if let (Some(anchor), Some(ckpt)) = (chain.external_anchor.as_mut(), &chain.checkpoint_path)
        && anchor.ledger_path.is_none()
    {
        let dir = PathBuf::from(ckpt);
        let dir = dir.parent().unwrap_or(std::path::Path::new("."));
        anchor.ledger_path = Some(
            dir.join("chain")
                .join("anchors.jsonl")
                .to_string_lossy()
                .into_owned(),
        );
    }
    let pinned = chain.checkpoint_path.clone().map(PathBuf::from);
    kernel_config.chain = Some(chain);
    pinned
}

#[cfg(not(test))]
fn default_checkpoint_path(chain: &clawft_types::config::ChainConfig) -> Option<String> {
    chain.effective_checkpoint_path()
}

/// Unit-test default: a fresh temp dir per boot, never `~/.clawft`.
#[cfg(test)]
fn default_checkpoint_path(_chain: &clawft_types::config::ChainConfig) -> Option<String> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "weftos-kernel-unit-chain-{}-{n}",
        std::process::id()
    ));
    Some(
        dir.join(clawft_types::config::chain_paths::CHAIN_CHECKPOINT_FILE)
            .to_string_lossy()
            .into_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use clawft_types::config::ChainConfig;

    #[test]
    fn explicit_path_is_kept() {
        let mut k = KernelConfig {
            chain: Some(ChainConfig {
                checkpoint_path: Some("/data/c.json".into()),
                ..ChainConfig::default()
            }),
            ..KernelConfig::default()
        };
        assert_eq!(pin_chain_storage(&mut k), Some(PathBuf::from("/data/c.json")));
    }

    #[test]
    fn disabled_chain_is_not_pinned() {
        let mut k = KernelConfig {
            chain: Some(ChainConfig {
                enabled: false,
                ..ChainConfig::default()
            }),
            ..KernelConfig::default()
        };
        assert_eq!(pin_chain_storage(&mut k), None);
        assert!(k.chain.unwrap().checkpoint_path.is_none());
    }

    #[test]
    fn unit_test_default_is_isolated_and_pinned() {
        let mut k = KernelConfig::default();
        let p = pin_chain_storage(&mut k).expect("pinned");
        assert!(p.starts_with(std::env::temp_dir()), "{}", p.display());
        if let Some(home) = std::env::var_os("HOME") {
            assert!(!p.starts_with(PathBuf::from(home).join(".clawft")));
        }
        assert_eq!(
            k.chain.unwrap().checkpoint_path.as_deref(),
            Some(p.to_string_lossy().as_ref())
        );
    }
}
