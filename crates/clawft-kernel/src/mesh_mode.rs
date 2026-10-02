//! Which mesh mode a kernel boots in (P3-K0, P3-U).
//!
//! The mesh runs either inside the kernel process (`Collapsed`), in the
//! machine mesh service the daemon is a client of (`Service`), or not at all
//! (`Off`). `kernel.mesh.service = auto | required | off` picks between the
//! first two (ADR-103 D2).
//!
//! [`select`] is pure configuration: it never probes a socket, so a kernel
//! booted without the daemon's service glue (one-shot CLI boots, tests) is
//! never silently left without a mesh. A caller that has probed the service
//! passes the outcome to [`decide`].

use std::path::{Path, PathBuf};

use clawft_types::config::{MeshConfig, MeshServicePolicy};

/// Default socket of the machine mesh service.
pub const DEFAULT_SERVICE_SOCKET: &str = "/var/run/weftos/mesh.sock";
/// Environment override for the service socket (tests and probes).
pub const SERVICE_SOCKET_ENV: &str = "WEFTOS_MESH_SOCKET";

/// The mesh mode selected for this boot.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum MeshMode {
    /// Mesh disabled: no listener, no runtime.
    Off,
    /// Mesh runs in this kernel (listener, seeds, `MeshService`).
    Collapsed,
    /// The machine mesh service owns the listener; this kernel is a client
    /// reaching it over `sock`.
    Service {
        /// The service's mesh-local socket.
        sock: PathBuf,
    },
}

/// What probing the service socket found (`auto` and `required` only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceProbe {
    /// A service answered and verified.
    Present,
    /// Nothing is there (no socket, connection refused).
    Absent,
    /// Something answered but failed verification or refused us; the text is
    /// the reason. Fatal under every policy.
    Refused(String),
}

/// The service socket: config override, else `WEFTOS_MESH_SOCKET`, else the
/// default.
pub fn service_socket(cfg: &MeshConfig) -> PathBuf {
    resolve_socket(cfg, std::env::var(SERVICE_SOCKET_ENV).ok().as_deref())
}

fn resolve_socket(cfg: &MeshConfig, env: Option<&str>) -> PathBuf {
    cfg.service_socket
        .as_deref()
        .filter(|s| !s.is_empty())
        .or(env.filter(|s| !s.is_empty()))
        .map_or_else(|| PathBuf::from(DEFAULT_SERVICE_SOCKET), PathBuf::from)
}

/// Choose the mesh mode from configuration alone (no probing).
///
/// Service mode is never returned here: it needs a verified service, which
/// only [`decide`] can confirm.
pub fn select(cfg: &MeshConfig) -> MeshMode {
    if cfg.enabled {
        MeshMode::Collapsed
    } else {
        MeshMode::Off
    }
}

/// Whether the policy asks for a probe at all.
pub fn wants_probe(cfg: &MeshConfig) -> bool {
    match cfg.service {
        MeshServicePolicy::Off => false,
        MeshServicePolicy::Auto => cfg.enabled,
        MeshServicePolicy::Required => true,
    }
}

/// Combine configuration with a probe outcome. The probe is `None` under
/// `off`. Errors carry the reason the boot must fail with.
///
/// Mesh disabled with `service = auto` stays off: a daemon that did not ask
/// for a mesh does not register with one ([`wants_probe`] is false, so no
/// probe is made). `required` always needs a service, whatever `enabled` says.
pub fn decide(cfg: &MeshConfig, probe: Option<ServiceProbe>) -> Result<MeshMode, String> {
    let sock = service_socket(cfg);
    decide_at(cfg, probe, &sock)
}

fn decide_at(cfg: &MeshConfig, probe: Option<ServiceProbe>, sock: &Path) -> Result<MeshMode, String> {
    match (cfg.service, probe) {
        (MeshServicePolicy::Off, _) => Ok(select(cfg)),
        (MeshServicePolicy::Auto, _) if !cfg.enabled => Ok(MeshMode::Off),
        (_, Some(ServiceProbe::Refused(why))) => Err(format!(
            "the machine mesh service at {} refused or failed verification: {why}",
            sock.display()
        )),
        (_, Some(ServiceProbe::Present)) => Ok(MeshMode::Service { sock: sock.to_path_buf() }),
        (MeshServicePolicy::Required, _) => Err(format!(
            "kernel.mesh.service = \"required\" but no machine mesh service answers at {}; \
             start it (`weaver mesh serve`, or the installed service) or set \
             kernel.mesh.service = \"auto\"",
            sock.display()
        )),
        (_, _) => Ok(select(cfg)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(policy: MeshServicePolicy, enabled: bool) -> MeshConfig {
        MeshConfig { enabled, service: policy, ..MeshConfig::default() }
    }

    #[test]
    fn disabled_is_off() {
        assert_eq!(select(&MeshConfig::default()), MeshMode::Off);
    }

    #[test]
    fn enabled_is_collapsed() {
        assert_eq!(select(&cfg(MeshServicePolicy::Auto, true)), MeshMode::Collapsed);
    }

    #[test]
    fn auto_uses_a_present_service_else_collapses() {
        let c = cfg(MeshServicePolicy::Auto, true);
        assert!(matches!(
            decide(&c, Some(ServiceProbe::Present)),
            Ok(MeshMode::Service { .. })
        ));
        assert_eq!(decide(&c, Some(ServiceProbe::Absent)), Ok(MeshMode::Collapsed));
    }

    #[test]
    fn required_without_a_service_fails_with_the_reason() {
        let c = cfg(MeshServicePolicy::Required, true);
        let e = decide(&c, Some(ServiceProbe::Absent)).unwrap_err();
        assert!(e.contains("required") && e.contains("mesh.sock"), "{e}");
    }

    #[test]
    fn a_refusing_service_is_fatal_even_under_auto() {
        let c = cfg(MeshServicePolicy::Auto, true);
        let e = decide(&c, Some(ServiceProbe::Refused("machine_key_changed".into()))).unwrap_err();
        assert!(e.contains("machine_key_changed"), "{e}");
    }

    #[test]
    fn auto_with_the_mesh_disabled_stays_off_without_probing() {
        let c = cfg(MeshServicePolicy::Auto, false);
        assert!(!wants_probe(&c));
        assert_eq!(decide(&c, Some(ServiceProbe::Present)), Ok(MeshMode::Off));
    }

    #[test]
    fn off_never_uses_the_service() {
        let c = cfg(MeshServicePolicy::Off, true);
        assert_eq!(decide(&c, None), Ok(MeshMode::Collapsed));
        assert!(!wants_probe(&c));
    }

    #[test]
    fn socket_resolution_order() {
        let mut c = MeshConfig::default();
        assert_eq!(resolve_socket(&c, None), Path::new(DEFAULT_SERVICE_SOCKET));
        assert_eq!(resolve_socket(&c, Some("/e.sock")), Path::new("/e.sock"));
        c.service_socket = Some("/c.sock".into());
        assert_eq!(resolve_socket(&c, Some("/e.sock")), Path::new("/c.sock"));
    }
}
