//! Operator policy files for the daemon's placement control plane and
//! `workload-host` (ADR-099 sections 3, 4 and 7; card mesh-placement-12).
//! Every file lives in the daemon's runtime directory, is size-capped and
//! validated here, at the boundary:
//!
//! - `workload-permits.json`: `[WorkloadPermitRule, ...]`; missing means no
//!   permits (default deny);
//! - `workload-trust.json`: package signers; missing means the compiled-in
//!   WeftOS signer set only;
//! - `workload-peers.json`: `[{"addr": "host:port", "tier": "paired",
//!   "key": "<64 hex>"}]`. The tier is given to the node `key` names, or,
//!   without a key, to the key first seen at the address; a different key
//!   answering there later never inherits it;
//! - `workload-container.json`: serve a container adapter
//!   (`{"engine": "docker", "base_image": "name@sha256:<64 hex>",
//!   "arches_native": ["aarch64"], "arches_emulated": [],
//!   "allow_emulated": false}`); missing means native only, and the
//!   controller never offers this node a container variant;
//! - `workload-seeds.json`: Cognitum Seeds on operator-assigned node ids
//!   (`[{"node_id": "seed-kitchen", "url": "https://seed:8443",
//!   "tls_sha256": "sha256:<64 hex>", "tier": "paired",
//!   "pins": [{"id": "fall-detect", "version": "1.0.0"}]}]`); tokens are
//!   read from `secrets/workload.seed/<node_id>.token` under the runtime dir.

use std::path::Path;
use std::sync::Arc;

use clawft_kernel::chain::ChainManager;
use clawft_kernel::workload_ctl::governance_tier;
use clawft_kernel::workload_ctl::{MEM_SCHEME, OperatorPeer};
use clawft_kernel::workload_governance::{NodeTrustTier, WorkloadGate, WorkloadPermitRule};
use clawft_kernel::workload_pkg::TrustAnchors;
use clawft_kernel::workload_pkg::codec::hex_decode_exact;
use clawft_kernel::workload_runtime::seed_creds::SEED_SECRETS_SUBDIR;
use clawft_kernel::workload_runtime::seed_tls::SeedTls;
use clawft_kernel::workload_runtime::{
    ContainerRuntime, ContainerRuntimeConfig, Engine, FileCredentials, HttpSeedTransport,
    SeedApiRuntime, SeedConfig, SeedPin, SystemRunner, WorkloadHost,
};
use clawft_types::placement::TrustTier;
use serde::Deserialize;

/// Permit file under the runtime dir.
pub const PERMITS_FILE: &str = "workload-permits.json";
/// Trust file under the runtime dir.
pub const TRUST_FILE: &str = "workload-trust.json";
/// Peers file under the runtime dir.
pub const PEERS_FILE: &str = "workload-peers.json";
/// Container adapter file under the runtime dir.
pub const CONTAINER_FILE: &str = "workload-container.json";
/// Seed file under the runtime dir.
pub const SEEDS_FILE: &str = "workload-seeds.json";
/// Controller state (targets, placements) under the runtime dir.
pub const STATE_FILE: &str = "workload-placements.json";
const MAX_POLICY_BYTES: u64 = 256 * 1024;
const MAX_PEERS: usize = 256;
const MAX_SEEDS: usize = 64;

fn read_policy(path: &Path) -> Result<Option<String>, String> {
    match std::fs::metadata(path) {
        Err(_) => Ok(None),
        Ok(m) if m.len() > MAX_POLICY_BYTES => Err(format!("{} is too large", path.display())),
        Ok(_) => std::fs::read_to_string(path)
            .map(Some)
            .map_err(|e| e.to_string()),
    }
}

fn parse<T: serde::de::DeserializeOwned>(dir: &Path, file: &str) -> Result<Option<T>, String> {
    match read_policy(&dir.join(file))? {
        None => Ok(None),
        Some(t) => serde_json::from_str(&t)
            .map(Some)
            .map_err(|e| format!("{file}: {e}")),
    }
}

/// Permits from the runtime dir (none if the file is absent).
pub fn load_permits(dir: &Path) -> Result<Vec<WorkloadPermitRule>, String> {
    Ok(parse(dir, PERMITS_FILE)?.unwrap_or_default())
}

/// Trust anchors from the runtime dir (WeftOS defaults if absent).
pub fn load_anchors(dir: &Path) -> Result<TrustAnchors, String> {
    match read_policy(&dir.join(TRUST_FILE))? {
        None => TrustAnchors::weftos_default(),
        Some(t) => TrustAnchors::from_trust_json(t.as_bytes()),
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct PeerEntry {
    addr: String,
    #[serde(default = "paired")]
    tier: TrustTier,
    /// The node key (64 hex) this tier is for.
    #[serde(default)]
    key: Option<String>,
}

fn paired() -> TrustTier {
    TrustTier::Paired
}

fn host_port(a: &str) -> bool {
    !a.is_empty()
        && a.len() <= 260
        && a.contains(':')
        && !a.starts_with(MEM_SCHEME)
        && a.chars()
            .all(|c| c.is_ascii_alphanumeric() || ".-:[]_".contains(c))
}

/// Operator peers from the runtime dir (none if the file is absent).
pub fn load_peers(dir: &Path) -> Result<Vec<OperatorPeer>, String> {
    let peers: Vec<PeerEntry> = parse(dir, PEERS_FILE)?.unwrap_or_default();
    if peers.len() > MAX_PEERS {
        return Err(format!("{PEERS_FILE}: more than {MAX_PEERS} peers"));
    }
    peers
        .into_iter()
        .map(|p| {
            if !host_port(&p.addr) {
                return Err(format!("{PEERS_FILE}: {:?} is not host:port", p.addr));
            }
            let peer = OperatorPeer::new(p.addr.clone(), p.tier);
            match p.key {
                None => Ok(peer),
                Some(k) => hex_decode_exact::<32>(&k)
                    .map(|k| peer.with_key(k))
                    .ok_or_else(|| format!("{PEERS_FILE}: key for {:?} is not 64 hex", p.addr)),
            }
        })
        .collect()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ContainerFile {
    engine: String,
    base_image: String,
    #[serde(default = "aarch64")]
    arches_native: Vec<String>,
    #[serde(default)]
    arches_emulated: Vec<String>,
    #[serde(default)]
    allow_emulated: bool,
}

fn aarch64() -> Vec<String> {
    vec!["aarch64".into()]
}

const ARCHES: &[&str] = &["aarch64", "armv7", "x86_64"];

/// The operator's container adapter config (none if the file is absent).
pub fn load_container(
    dir: &Path,
    work_root: &Path,
) -> Result<Option<ContainerRuntimeConfig>, String> {
    let Some(f) = parse::<ContainerFile>(dir, CONTAINER_FILE)? else {
        return Ok(None);
    };
    let engine = match f.engine.as_str() {
        "docker" => Engine::Docker,
        "podman" => Engine::Podman,
        "apple" => Engine::Apple,
        other => return Err(format!("{CONTAINER_FILE}: unknown engine {other:?}")),
    };
    clawft_kernel::workload_runtime::container_cmd::validate_base_image(&f.base_image)
        .map_err(|e| format!("{CONTAINER_FILE}: {e}"))?;
    let known = |v: &[String]| v.iter().all(|a| ARCHES.contains(&a.as_str()));
    if f.arches_native.is_empty() || !known(&f.arches_native) || !known(&f.arches_emulated) {
        return Err(format!("{CONTAINER_FILE}: arches must be from {ARCHES:?}"));
    }
    let mut cfg = ContainerRuntimeConfig::new(engine, f.base_image, work_root);
    cfg.arches_native = f.arches_native;
    cfg.arches_emulated = f.arches_emulated;
    cfg.allow_emulated = f.allow_emulated;
    Ok(Some(cfg))
}

/// The container adapter under this node's governance, and the routes it
/// serves (`container`, plus `emulated` when the operator opted in).
pub fn container_host(
    cfg: ContainerRuntimeConfig,
    gate: Arc<WorkloadGate>,
    node_id: &str,
    chain: &Arc<ChainManager>,
) -> (Arc<WorkloadHost>, Vec<&'static str>) {
    let mut routes = vec!["container"];
    if cfg.allow_emulated && !cfg.arches_emulated.is_empty() {
        routes.push("emulated");
    }
    let rt = ContainerRuntime::new(cfg, Arc::new(SystemRunner));
    let host = WorkloadHost::new(Arc::new(rt), gate, node_id, NodeTrustTier::Pinned)
        .with_chain(chain.clone());
    (Arc::new(host), routes)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct SeedEntry {
    node_id: String,
    url: String,
    #[serde(default)]
    tls_sha256: Option<String>,
    #[serde(default = "paired")]
    tier: TrustTier,
    pins: Vec<SeedPin>,
}

/// One operator-assigned Seed, ready for `PlacementControlPlane::add_seed`.
pub struct SeedTarget {
    /// Operator-assigned node id.
    pub node_id: String,
    /// Operator-assigned tier.
    pub tier: TrustTier,
    /// Card 09's `remote.api` adapter under this node's governance.
    pub host: Arc<WorkloadHost>,
}

/// Seeds from the runtime dir (none if the file is absent).
pub fn load_seeds(
    dir: &Path,
    gate: Arc<WorkloadGate>,
    chain: &Arc<ChainManager>,
) -> Result<Vec<SeedTarget>, String> {
    let seeds: Vec<SeedEntry> = parse(dir, SEEDS_FILE)?.unwrap_or_default();
    if seeds.len() > MAX_SEEDS {
        return Err(format!("{SEEDS_FILE}: more than {MAX_SEEDS} seeds"));
    }
    let creds = Arc::new(FileCredentials::new(dir.join(SEED_SECRETS_SUBDIR)));
    seeds
        .into_iter()
        .map(|s| {
            let tls = match &s.tls_sha256 {
                Some(f) => SeedTls::pinned(f).map_err(|e| format!("{SEEDS_FILE}: {e}"))?,
                None => SeedTls::WebPki,
            };
            let transport = HttpSeedTransport::new(&s.url, tls)
                .map_err(|e| format!("{SEEDS_FILE}: {}: {e}", s.node_id))?;
            let rt = SeedApiRuntime::new(
                SeedConfig {
                    node_id: s.node_id.clone(),
                    pins: s.pins,
                    concurrency_cap: clawft_kernel::workload_runtime::seed::SEED_CONCURRENCY_CAP,
                },
                Arc::new(transport),
                creds.clone(),
            )
            .map_err(|e| format!("{SEEDS_FILE}: {}: {e}", s.node_id))?;
            let host = WorkloadHost::new(
                Arc::new(rt),
                gate.clone(),
                s.node_id.clone(),
                governance_tier(s.tier),
            )
            .with_chain(chain.clone());
            Ok(SeedTarget {
                node_id: s.node_id,
                tier: s.tier,
                host: Arc::new(host),
            })
        })
        .collect()
}
