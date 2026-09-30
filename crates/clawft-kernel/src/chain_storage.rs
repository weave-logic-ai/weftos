//! Pin the chain storage location once at boot.
//!
//! The kernel reads the chain checkpoint path in several places: boot
//! (signing key, RVF restore, tree checkpoint), the daemon's `chain.verify`
//! RPC, and shutdown persistence. [`pin_chain_storage`] resolves the path a
//! single time and writes it into the kernel config, so all of them agree
//! even if the environment changes after boot.
//!
//! Resolution follows [`clawft_types::runtime_paths::RuntimePaths`]: an
//! explicit config path wins, then `chain.json` under the runtime root
//! (`$WEFTOS_RUNTIME_DIR`, else the project's `.weftos/runtime`, else legacy
//! `~/.clawft`). In this crate's own unit tests the default is a fresh temp
//! dir per boot, so `cargo test` never touches the operator chain.

use std::path::PathBuf;

use clawft_types::config::KernelConfig;
use clawft_types::runtime_paths::{RuntimePaths, legacy_chain_left_behind};

/// The runtime paths this boot uses for every non-chain runtime file
/// (cluster peers, apps, revoked hosts).
///
/// Production: the one shared resolver. Unit tests: the directory holding
/// the pinned per-boot temp chain, so nothing lands in the operator's
/// runtime dir.
pub fn boot_runtime_paths(pinned_chain: Option<&std::path::Path>) -> RuntimePaths {
    #[cfg(test)]
    if let Some(dir) = pinned_chain.and_then(std::path::Path::parent) {
        return RuntimePaths::at(dir);
    }
    let _ = pinned_chain;
    RuntimePaths::resolve()
}

/// WARN text when a project-rooted boot leaves the legacy `~/.clawft` chain
/// behind (ADR-103 D4 behavior change). `None` when nothing is stranded.
///
/// Only meaningful for a chain path that came from the resolver, not from an
/// explicit `kernel.chain.checkpoint_path`.
pub fn legacy_chain_warning(
    paths: &RuntimePaths,
    home: Option<&std::path::Path>,
) -> Option<String> {
    legacy_chain_left_behind(paths, home).map(|legacy| {
        format!(
            "chain not found at {} but a legacy chain exists at {}; \
             continuing with a fresh chain at the resolved path (nothing was moved). \
             Copy chain.* there to keep the old history, or set kernel.chain.checkpoint_path.",
            paths.chain_checkpoint().display(),
            legacy.display()
        )
    })
}

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
        dir.join(clawft_types::runtime_paths::CHAIN_CHECKPOINT_FILE)
            .to_string_lossy()
            .into_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use clawft_types::config::ChainConfig;

    #[test]
    fn legacy_chain_warning_names_both_paths() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path().join("home");
        std::fs::create_dir_all(home.join(".clawft")).unwrap();
        std::fs::write(home.join(".clawft/chain.json"), "{}").unwrap();
        let proj = t.path().join("proj");
        std::fs::create_dir_all(proj.join(".weftos")).unwrap();
        std::fs::write(proj.join(".weftos/project.toml"), "").unwrap();
        let paths = RuntimePaths::resolve_with(None, Some(&proj), Some(&home));
        let w = legacy_chain_warning(&paths, Some(&home)).expect("warns");
        assert!(w.contains(".weftos/runtime/chain.json"), "{w}");
        assert!(w.contains(".clawft/chain.json"), "{w}");
        let iso = RuntimePaths::resolve_with(Some("/x"), Some(&proj), Some(&home));
        assert!(legacy_chain_warning(&iso, Some(&home)).is_none());
    }

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
