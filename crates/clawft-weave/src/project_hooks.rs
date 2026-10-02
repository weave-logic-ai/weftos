//! The three seams a per-project kernel hangs off `daemon::run`
//! (ADR-103 A6, Phase 2 package A).
//!
//! `daemon.rs` calls these and nothing else, so later packages never edit it
//! again for project behaviour: F fills [`adjust_services`] (via
//! `project_profile.rs`), H fills [`pre_boot`], [`post_boot`] and
//! [`pre_shutdown`] (via `project_boot.rs` and `project_boot_run.rs`), G adds
//! adoption to [`post_boot`]. Every body is a no-op outside the `project`
//! profile.

use clawft_platform::NativePlatform;
use clawft_kernel::Kernel;
use clawft_types::config::{Config, KernelConfig};

pub use crate::project_boot::PreBoot;

/// Top of `run()`, before the runtime dir is locked or anything is opened:
/// for a `project`-profile kernel the spawn handshake, key, registration and
/// certificate (see `project_boot`). A no-op for every other profile.
pub async fn pre_boot(config: &mut Config, kernel_config: &KernelConfig) -> anyhow::Result<PreBoot> {
    Ok(crate::project_boot::pre_boot(config, kernel_config).await?)
}

/// Right after [`pre_boot`]: switch off services a `project`-profile kernel
/// takes from its parent (mesh listener, voice, local embedding model).
///
/// `kernel_config` is the copy the kernel boots with (the caller clones it
/// out of `config.kernel`), so both are adjusted.
pub fn adjust_services(config: &mut Config, kernel_config: &mut KernelConfig) {
    crate::project_profile::adjust_services(config, kernel_config);
}

/// After the kernel has booted: for a project kernel, genesis, the
/// heartbeat and the anchor task.
pub fn post_boot(kernel: &Kernel<NativePlatform>, pre: &PreBoot) -> anyhow::Result<()> {
    crate::project_boot_run::post_boot(kernel, pre)
}

/// Before the kernel shuts down: final anchor and unregister (project kernel).
pub async fn pre_shutdown() {
    crate::project_boot_run::pre_shutdown().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn hooks_do_not_change_the_config() {
        let mut cfg = Config::default();
        let before = serde_json::to_value(&cfg).unwrap();
        let pre = pre_boot(&mut cfg, &KernelConfig::default()).await.unwrap();
        assert!(pre.identity.is_none() && pre.child.is_none());
        adjust_services(&mut cfg, &mut KernelConfig::default());
        assert_eq!(serde_json::to_value(&cfg).unwrap(), before);
    }
}
