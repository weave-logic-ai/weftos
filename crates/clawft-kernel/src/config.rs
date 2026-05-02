//! Kernel configuration re-exports and extensions.
//!
//! The base [`KernelConfig`] is defined in `clawft-types` so it can
//! be embedded in the root `Config` without circular dependencies.
//! This module re-exports it and provides kernel-specific extensions.

pub use clawft_types::config::KernelConfig;

use crate::capability::AgentCapabilities;

/// Extended kernel configuration with capability defaults.
///
/// This wraps the base `KernelConfig` from `clawft-types` with
/// kernel-specific fields that reference types only available
/// in this crate (e.g. `AgentCapabilities`).
#[derive(Debug, Clone, Default)]
pub struct KernelConfigExt {
    /// Base configuration from the config file.
    pub base: KernelConfig,

    /// Default capabilities assigned to new agents when none are
    /// specified explicitly.
    pub default_capabilities: Option<AgentCapabilities>,
}

impl From<KernelConfig> for KernelConfigExt {
    fn from(base: KernelConfig) -> Self {
        Self {
            base,
            default_capabilities: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernel_config_ext_from_base() {
        let base = KernelConfig {
            enabled: true,
            max_processes: 128,
            health_check_interval_secs: 10,
            cluster: None,
            chain: None,
            resource_tree: None,
            vector: None,
            profiles: None,
            pairing: None,
            mesh: None,
            anchor: None,
            ipc_tcp: None,
        };
        let ext = KernelConfigExt::from(base.clone());
        assert!(ext.base.enabled);
        assert_eq!(ext.base.max_processes, 128);
        assert!(ext.default_capabilities.is_none());
    }

    #[test]
    fn kernel_config_ext_default() {
        let ext = KernelConfigExt::default();
        assert!(ext.base.enabled);
        assert_eq!(ext.base.max_processes, 64);
    }

    #[test]
    fn kernel_config_reexport() {
        // Verify the re-export works
        let cfg = KernelConfig::default();
        assert!(cfg.enabled);
    }
}
