//! Which mesh mode a kernel boots in (P3-K0).
//!
//! Only the collapsed mode (mesh runs inside the kernel process) exists
//! today. A later package adds an external-service variant behind
//! `kernel.mesh.service`; `Kernel::boot` and `weft kernel boot
//! --foreground` both call [`select`] so they cannot diverge.

use clawft_types::config::MeshConfig;

/// The mesh mode selected for this boot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MeshMode {
    /// Mesh disabled: no listener, no runtime.
    Off,
    /// Mesh runs in this kernel (listener, seeds, `MeshService`).
    Collapsed,
}

/// Choose the mesh mode from configuration.
pub fn select(cfg: &MeshConfig) -> MeshMode {
    if cfg.enabled {
        MeshMode::Collapsed
    } else {
        MeshMode::Off
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_is_off() {
        assert_eq!(select(&MeshConfig::default()), MeshMode::Off);
    }

    #[test]
    fn enabled_is_collapsed() {
        let cfg = MeshConfig {
            enabled: true,
            ..MeshConfig::default()
        };
        assert_eq!(select(&cfg), MeshMode::Collapsed);
    }
}
