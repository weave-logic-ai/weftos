//! Service configuration: `mesh.toml` (root:service 0640 in production),
//! `WEFTOS_MESH_*` environment overrides and command-line flags (plan 1.1, 2 S).
//!
//! Precedence, lowest first: built-in defaults, `mesh.toml`, environment,
//! flags. Tests and probes isolate themselves with `WEFTOS_MESH_STATE_DIR` and
//! `WEFTOS_MESH_SOCKET`; production units never set them.

use std::net::ToSocketAddrs;
use std::path::{Path, PathBuf};

use clawft_types::config::{MeshAdmissionMode, DEFAULT_MESH_PORT};
use serde::Deserialize;

/// Environment override of the state directory.
pub const ENV_STATE_DIR: &str = "WEFTOS_MESH_STATE_DIR";
/// Environment override of the mesh-local socket path.
pub const ENV_SOCKET: &str = "WEFTOS_MESH_SOCKET";

/// Production state directory.
pub const DEFAULT_STATE_DIR: &str = "/var/lib/weftos/mesh";
/// Production socket path.
pub const DEFAULT_SOCKET: &str = "/var/run/weftos/mesh.sock";
/// Default loopback health port.
pub const DEFAULT_HEALTH_LISTEN: &str = "127.0.0.1:9490";

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read {path}: {source}")]
    Read { path: String, source: std::io::Error },
    #[error("{path}: {message}")]
    Parse { path: String, message: String },
    #[error("invalid configuration: {0}")]
    Invalid(String),
}

/// What happens when an unknown uid registers for the first time (ADR-103 D3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BindPolicy {
    /// Trust on first use: the first key a uid presents is bound.
    #[default]
    Tofu,
    /// The bind stays pending until `weaver mesh bind approve <uid>`.
    Approve,
}

impl BindPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            BindPolicy::Tofu => "tofu",
            BindPolicy::Approve => "approve",
        }
    }
}

/// Raw `mesh.toml`. Unknown keys are an error so a typo cannot silently weaken
/// a policy.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    state_dir: Option<PathBuf>,
    socket: Option<PathBuf>,
    listen: Option<String>,
    transport: Option<String>,
    noise: Option<bool>,
    discovery: Option<bool>,
    seed_peers: Option<Vec<String>>,
    bind_policy: Option<BindPolicy>,
    admission: Option<MeshAdmissionMode>,
    genesis_hash: Option<String>,
    cluster_owner_uid: Option<u32>,
    admin_uids: Option<Vec<u32>>,
    cert_ttl_s: Option<u64>,
    verdict_timeout_s: Option<u64>,
    stale_grace_s: Option<u64>,
    health_listen: Option<String>,
    facts_ttl_s: Option<u64>,
    probe_facts: Option<bool>,
}

/// Command-line overrides (`--state-dir --socket --listen --config`).
#[derive(Debug, Default, Clone)]
pub struct Overrides {
    pub state_dir: Option<PathBuf>,
    pub socket: Option<PathBuf>,
    pub listen: Option<String>,
}

/// Effective configuration.
#[derive(Debug, Clone)]
pub struct MeshServiceConfig {
    pub state_dir: PathBuf,
    pub socket: PathBuf,
    /// Mesh listener address (default `0.0.0.0:9489`, ADR-103 D1).
    pub listen: String,
    pub transport: String,
    pub noise: bool,
    pub discovery: bool,
    pub seed_peers: Vec<String>,
    pub bind_policy: BindPolicy,
    pub admission: MeshAdmissionMode,
    pub genesis_hash: Option<[u8; 32]>,
    pub cluster_owner_uid: Option<u32>,
    /// uids allowed to use admin verbs, besides root.
    pub admin_uids: Vec<u32>,
    pub cert_ttl_s: u64,
    pub verdict_timeout_s: u64,
    /// How long an already-granted verdict stays usable when the cluster owner
    /// cannot be asked (D-3).
    pub stale_grace_s: u64,
    /// Loopback-only health endpoint; `None` disables it.
    pub health_listen: Option<String>,
    pub facts_ttl_s: u64,
    /// Probe the host for capabilities when signing facts. Tests turn it off.
    pub probe_facts: bool,
    pub build_sha: String,
}

impl Default for MeshServiceConfig {
    fn default() -> Self {
        Self {
            state_dir: PathBuf::from(DEFAULT_STATE_DIR),
            socket: PathBuf::from(DEFAULT_SOCKET),
            listen: format!("0.0.0.0:{DEFAULT_MESH_PORT}"),
            transport: "tcp".into(),
            noise: false,
            discovery: false,
            seed_peers: Vec::new(),
            bind_policy: BindPolicy::Tofu,
            admission: MeshAdmissionMode::Observe,
            genesis_hash: None,
            cluster_owner_uid: None,
            admin_uids: Vec::new(),
            cert_ttl_s: clawft_mesh_local::cert::DEFAULT_TTL_S,
            verdict_timeout_s: 3,
            stale_grace_s: 600,
            health_listen: Some(DEFAULT_HEALTH_LISTEN.into()),
            facts_ttl_s: 3600,
            probe_facts: true,
            build_sha: option_env!("WEFTOS_BUILD_SHA")
                .unwrap_or(env!("CARGO_PKG_VERSION"))
                .to_string(),
        }
    }
}

fn parse_genesis(h: &str) -> Result<[u8; 32], ConfigError> {
    clawft_mesh_local::hexser::decode::<32>(h.trim())
        .ok_or_else(|| ConfigError::Invalid("genesis_hash must be 64 lowercase hex characters".into()))
}

impl MeshServiceConfig {
    /// Load `path` (when given), then apply the environment and `overrides`.
    pub fn load(path: Option<&Path>, overrides: &Overrides) -> Result<Self, ConfigError> {
        let mut cfg = Self::default();
        if let Some(p) = path {
            let text = std::fs::read_to_string(p).map_err(|source| ConfigError::Read {
                path: p.display().to_string(),
                source,
            })?;
            cfg.apply_toml(&text, &p.display().to_string())?;
        }
        cfg.apply_env(|k| std::env::var_os(k).map(PathBuf::from));
        cfg.apply_overrides(overrides);
        cfg.validate()?;
        Ok(cfg)
    }

    pub(crate) fn apply_toml(&mut self, text: &str, origin: &str) -> Result<(), ConfigError> {
        let f: FileConfig = toml::from_str(text)
            .map_err(|e| ConfigError::Parse { path: origin.into(), message: e.to_string() })?;
        macro_rules! take {
            ($($field:ident),*) => {$(if let Some(v) = f.$field { self.$field = v; })*};
        }
        take!(
            state_dir, socket, listen, transport, noise, discovery, seed_peers, bind_policy,
            admission, admin_uids, cert_ttl_s, verdict_timeout_s, stale_grace_s, facts_ttl_s,
            probe_facts
        );
        if let Some(h) = f.genesis_hash {
            self.genesis_hash = Some(parse_genesis(&h)?);
        }
        if f.cluster_owner_uid.is_some() {
            self.cluster_owner_uid = f.cluster_owner_uid;
        }
        if let Some(h) = f.health_listen {
            self.health_listen = if h.trim().is_empty() || h == "off" { None } else { Some(h) };
        }
        Ok(())
    }

    pub(crate) fn apply_env(&mut self, get: impl Fn(&str) -> Option<PathBuf>) {
        if let Some(p) = get(ENV_STATE_DIR) {
            self.state_dir = p;
        }
        if let Some(p) = get(ENV_SOCKET) {
            self.socket = p;
        }
    }

    pub(crate) fn apply_overrides(&mut self, o: &Overrides) {
        if let Some(p) = &o.state_dir {
            self.state_dir = p.clone();
        }
        if let Some(p) = &o.socket {
            self.socket = p.clone();
        }
        if let Some(l) = &o.listen {
            self.listen = l.clone();
        }
    }

    /// Reject configurations that would silently weaken the service.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let bad = |m: &str| Err(ConfigError::Invalid(m.to_string()));
        if self.cert_ttl_s < 10 {
            return bad("cert_ttl_s must be at least 10 seconds");
        }
        if self.verdict_timeout_s == 0 {
            return bad("verdict_timeout_s must be at least 1 second");
        }
        if self.admission == MeshAdmissionMode::Enforce {
            if self.genesis_hash.is_none() {
                return bad("admission = \"enforce\" requires genesis_hash");
            }
            if !self.noise {
                return bad(
                    "admission = \"enforce\" requires noise = true (a plaintext channel has no \
                     handshake hash to bind a peer's identity to)",
                );
            }
        }
        if let Some(h) = &self.health_listen {
            let addrs = h
                .to_socket_addrs()
                .map_err(|e| ConfigError::Invalid(format!("health_listen {h:?}: {e}")))?;
            if addrs.clone().next().is_none() || !addrs.into_iter().all(|a| a.ip().is_loopback()) {
                return bad("health_listen must be a loopback address (127.0.0.1 or ::1)");
            }
        }
        if self.socket.file_name().is_none() || self.socket.parent().is_none() {
            return bad("socket must be a file path inside a directory");
        }
        Ok(())
    }

    /// Whether `uid` may use admin verbs: root or listed in `admin_uids`.
    pub fn is_admin(&self, uid: u32) -> bool {
        uid == 0 || self.admin_uids.contains(&uid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_listen_on_the_weave_port() {
        assert_eq!(MeshServiceConfig::default().listen, "0.0.0.0:9489");
    }

    #[test]
    fn toml_env_and_flags_layer_in_order() {
        let mut c = MeshServiceConfig::default();
        c.apply_toml("state_dir = \"/a\"\nsocket = \"/a/s\"\nlisten = \"127.0.0.1:1\"", "t").unwrap();
        c.apply_env(|k| (k == ENV_STATE_DIR).then(|| PathBuf::from("/env")));
        c.apply_overrides(&Overrides { listen: Some("127.0.0.1:2".into()), ..Default::default() });
        assert_eq!(c.state_dir, PathBuf::from("/env"));
        assert_eq!(c.socket, PathBuf::from("/a/s"));
        assert_eq!(c.listen, "127.0.0.1:2");
    }

    #[test]
    fn unknown_keys_are_rejected() {
        let mut c = MeshServiceConfig::default();
        assert!(c.apply_toml("bind_polcy = \"tofu\"", "t").is_err());
    }

    #[test]
    fn enforce_needs_genesis_and_noise() {
        let mut c = MeshServiceConfig { admission: MeshAdmissionMode::Enforce, ..Default::default() };
        assert!(c.validate().is_err());
        c.genesis_hash = Some([1; 32]);
        assert!(c.validate().is_err(), "noise still off");
        c.noise = true;
        assert!(c.validate().is_ok());
    }

    #[test]
    fn health_must_be_loopback() {
        let c = MeshServiceConfig { health_listen: Some("0.0.0.0:9490".into()), ..Default::default() };
        assert!(c.validate().is_err());
        let c = MeshServiceConfig { health_listen: Some("127.0.0.1:0".into()), ..Default::default() };
        assert!(c.validate().is_ok());
    }

    #[test]
    fn admin_is_root_or_listed() {
        let c = MeshServiceConfig { admin_uids: vec![501], ..Default::default() };
        assert!(c.is_admin(0) && c.is_admin(501) && !c.is_admin(502));
    }
}
