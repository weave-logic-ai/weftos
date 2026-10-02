//! The three seams a per-project kernel hangs off `daemon::run`
//! (ADR-103 A6, Phase 2 package A).
//!
//! `daemon.rs` calls these and nothing else, so later packages never edit it
//! again for project behaviour: F fills [`adjust_services`] (via
//! `project_profile.rs`), H fills [`pre_boot`] and [`post_boot`] (via
//! `project_boot.rs`), G adds adoption to [`post_boot`]. In package A every
//! body is a no-op.

use clawft_platform::NativePlatform;
use clawft_kernel::Kernel;
use clawft_types::config::{Config, KernelConfig};

/// What [`pre_boot`] hands back to `run()`. Empty until package H adds the
/// child's node key seed and chain signing key.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct PreBoot {}

/// Top of `run()`, before the runtime dir is locked or anything is opened.
pub fn pre_boot(_config: &mut Config) -> PreBoot {
    PreBoot::default()
}

/// Right after [`pre_boot`]: switch off services a `project`-profile kernel
/// takes from its parent (mesh listener, voice, local embedding model).
///
/// `kernel_config` is the copy the kernel boots with (the caller clones it
/// out of `config.kernel`), so both are adjusted.
pub fn adjust_services(config: &mut Config, kernel_config: &mut KernelConfig) {
    crate::project_profile::adjust_services(config, kernel_config);
}

/// After the kernel has booted.
pub fn post_boot(_kernel: &Kernel<NativePlatform>) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hooks_do_not_change_the_config() {
        let mut cfg = Config::default();
        let before = serde_json::to_value(&cfg).unwrap();
        let _ = pre_boot(&mut cfg);
        adjust_services(&mut cfg, &mut KernelConfig::default());
        assert_eq!(serde_json::to_value(&cfg).unwrap(), before);
    }
}
