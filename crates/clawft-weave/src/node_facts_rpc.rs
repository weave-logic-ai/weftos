//! Daemon side of node facts (ADR-099 section 2, card mesh-placement-03).
//!
//! At boot the daemon probes this machine off the async runtime, signs the
//! facts with its node key (`<runtime>/node.key`), and caches them in
//! `ClusterMembership` as the local node (`trust_tier: pinned`). A
//! background task re-probes before the TTL runs out. `cluster.facts`
//! returns every fresh cached entry (local and verified peers) with its
//! signed envelope, so a client can re-verify the signature itself.
//!
//! Optional operator inputs in the runtime directory:
//! - `perf.measured.json`: conformance-harness output (`capabilities.json`
//!   or a `probe` document); only `measured` `perf.*` entries are used.
//! - `feeds.declared.json`: a list of `feed.*` capabilities, advertised as
//!   `claimed`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use clawft_kernel::boot::Kernel;
use clawft_kernel::cluster::ClusterMembership;
use clawft_kernel::node_facts::{
    CachedNodeFacts, DEFAULT_FACTS_TTL_SECS, ProbeConfig, SystemHost, measured, probe_and_sign,
};
use clawft_platform::NativePlatform;
use clawft_rpc::Response;
use clawft_types::placement::{Capability, TrustTier};
use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{info, warn};

/// Measured results file under the runtime dir.
pub const MEASURED_FILE: &str = "perf.measured.json";
/// Declared feeds file under the runtime dir.
pub const FEEDS_FILE: &str = "feeds.declared.json";

struct LocalSigner {
    key: SigningKey,
    runtime_dir: PathBuf,
    last_seq: Mutex<u64>,
}

static LOCAL: OnceLock<Arc<LocalSigner>> = OnceLock::new();

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn read_small(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() as usize > measured::MAX_MEASURED_BYTES {
        warn!(path = %path.display(), "node facts input too large; ignored");
        return None;
    }
    std::fs::read_to_string(path).ok()
}

/// Build the probe config from the runtime directory's optional inputs.
pub fn probe_config(runtime_dir: &Path) -> ProbeConfig {
    let mut cfg = ProbeConfig::default();
    if let Some(text) = read_small(&runtime_dir.join(MEASURED_FILE)) {
        match measured::parse_measured(&text) {
            Ok(m) => {
                if m.skipped > 0 {
                    warn!(
                        skipped = m.skipped,
                        "measured results: non-perf or invalid entries skipped"
                    );
                }
                cfg.measured = m.caps;
            }
            Err(e) => warn!(error = %e, "measured results unreadable; ignored"),
        }
    }
    if let Some(text) = read_small(&runtime_dir.join(FEEDS_FILE)) {
        match serde_json::from_str::<Vec<Capability>>(&text) {
            Ok(feeds) => cfg.declared_feeds = feeds,
            Err(e) => warn!(error = %e, "declared feeds unreadable; ignored"),
        }
    }
    cfg
}

/// Probe, sign and cache this node's facts. Blocking: call off the runtime.
fn refresh_blocking(signer: &LocalSigner, membership: &ClusterMembership) -> Result<u64, String> {
    let cfg = probe_config(&signer.runtime_dir);
    let now = now_secs();
    let seq = {
        let mut last = signer.last_seq.lock().map_err(|_| "seq lock poisoned")?;
        *last = (*last + 1).max(now);
        *last
    };
    let signed = probe_and_sign(
        &SystemHost::default(),
        &cfg,
        &signer.key,
        now,
        seq,
        DEFAULT_FACTS_TTL_SECS,
    )
    .map_err(|e| e.to_string())?;
    membership
        .facts()
        .insert(signed, TrustTier::Pinned, now)
        .map_err(|e| e.to_string())?;
    Ok(seq)
}

async fn refresh(
    signer: Arc<LocalSigner>,
    membership: Arc<ClusterMembership>,
) -> Result<u64, String> {
    tokio::task::spawn_blocking(move || refresh_blocking(&signer, &membership))
        .await
        .map_err(|e| format!("probe task failed: {e}"))?
}

/// Start local facts: first probe now, then refresh at 80% of the TTL.
/// Idempotent: only the first call starts the task.
pub fn init(key: SigningKey, runtime_dir: PathBuf, membership: Arc<ClusterMembership>) {
    let signer = Arc::new(LocalSigner {
        key,
        runtime_dir,
        last_seq: Mutex::new(0),
    });
    if LOCAL.set(signer.clone()).is_err() {
        return;
    }
    tokio::spawn(async move {
        let every = Duration::from_secs(DEFAULT_FACTS_TTL_SECS * 4 / 5);
        loop {
            match refresh(signer.clone(), membership.clone()).await {
                Ok(seq) => info!(seq, "node facts probed and signed"),
                Err(e) => warn!(error = %e, "node facts probe failed"),
            }
            membership.facts().evict_expired(now_secs());
            tokio::time::sleep(every).await;
        }
    });
}

/// `cluster.facts` params.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FactsParams {
    /// Re-probe the local node before answering.
    #[serde(default)]
    pub refresh: bool,
    /// Only this node.
    #[serde(default)]
    pub node_id: Option<String>,
}

/// One entry of the `cluster.facts` result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactsEntry {
    /// Node id.
    pub node_id: String,
    /// True for this daemon's own node.
    pub local: bool,
    /// Receiver-assigned trust tier.
    pub trust_tier: TrustTier,
    /// When this daemon accepted the facts, unix seconds.
    pub received_at: u64,
    /// When the facts expire, unix seconds.
    pub expires_at: u64,
    /// Last applied delta seq.
    pub delta_seq: u64,
    /// Current facts (base plus deltas).
    pub facts: clawft_types::placement::NodeFacts,
    /// Signed base envelope, for independent verification.
    pub signed: clawft_kernel::node_facts_advert::SignedNodeFacts,
}

fn entry(c: CachedNodeFacts, local_id: Option<&str>) -> FactsEntry {
    FactsEntry {
        node_id: c.node_id().to_string(),
        local: local_id == Some(c.node_id()),
        trust_tier: c.trust_tier,
        received_at: c.received_at,
        expires_at: c.facts.expires_at(),
        delta_seq: c.delta_seq,
        facts: c.facts,
        signed: c.signed,
    }
}

/// Handle `cluster.facts`.
pub async fn handle(
    params: Value,
    kernel: Arc<tokio::sync::RwLock<Kernel<NativePlatform>>>,
) -> Response {
    let p: FactsParams = if params.is_null() {
        FactsParams::default()
    } else {
        match serde_json::from_value(params) {
            Ok(p) => p,
            Err(e) => return Response::error(format!("invalid params: {e}")),
        }
    };
    let membership = kernel.read().await.cluster_membership().clone();
    let local = LOCAL.get().cloned();
    if p.refresh {
        let Some(signer) = local.clone() else {
            return Response::error("local node facts not initialised");
        };
        if let Err(e) = refresh(signer, membership.clone()).await {
            return Response::error(format!("probe failed: {e}"));
        }
    }
    let local_id = local
        .as_ref()
        .map(|s| clawft_kernel::node_id_from_pubkey(&s.key.verifying_key().to_bytes()));
    let now = now_secs();
    let entries: Vec<FactsEntry> = membership
        .facts()
        .list(now)
        .into_iter()
        .filter(|c| p.node_id.as_deref().is_none_or(|n| n == c.node_id()))
        .map(|c| entry(c, local_id.as_deref()))
        .collect();
    match serde_json::to_value(entries) {
        Ok(v) => Response::success(v),
        Err(e) => Response::error(format!("encode failed: {e}")),
    }
}
