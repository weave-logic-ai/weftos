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
//! - `facts.config.json`: `{"docker_probe_image": "alpine:3.20" | null}`, the
//!   local image the probe runs inside a VM-backed container engine to list
//!   emulated architectures (for example armv7). `null` disables it. The
//!   `WEFTOS_FACTS_PROBE_IMAGE` environment variable overrides the file
//!   (empty disables). Default `alpine:3.20`; it is never pulled.
//!
//! With a mesh runtime attached, each probe goes through
//! [`FactsExchange`](clawft_kernel::node_facts_exchange::FactsExchange): a
//! changed capability set becomes a new signed base, a live-state change a
//! small signed delta, and both are sent to every connected or joining peer.
//! The live probe reads free memory only; no probe sets a capability's
//! busy/free state, so those deltas appear only when something else (the
//! workload host) marks one. Peers' facts arrive on the same exchange and
//! are cached with a receiver-assigned trust tier.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use clawft_kernel::boot::Kernel;
use clawft_kernel::cluster::ClusterMembership;
use clawft_kernel::mesh_runtime::MeshRuntime;
use clawft_kernel::node_facts::{
    CachedNodeFacts, DEFAULT_FACTS_TTL_SECS, EmulationCache, ProbeConfig, SystemHost, build_facts,
    measured, probe_and_sign, probe_capabilities, refresh_live, valid_image_ref,
};
use clawft_kernel::node_facts_exchange::{FactsExchange, FactsTrustPolicy};
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
/// Probe settings file under the runtime dir.
pub const CONFIG_FILE: &str = "facts.config.json";
/// Environment override for the container-engine probe image.
pub const PROBE_IMAGE_ENV: &str = "WEFTOS_FACTS_PROBE_IMAGE";
/// How often a connected node re-reads live state (free memory only: no
/// container engine, no privileged command).
pub const LIVE_PROBE_SECS: u64 = 60;
/// How often the full probe runs (80% of the facts TTL).
pub const FULL_PROBE_SECS: u64 = DEFAULT_FACTS_TTL_SECS * 4 / 5;
/// How long the privileged emulation (binfmt) listing is reused.
pub const EMULATION_CACHE_SECS: u64 = 3600;

struct LocalSigner {
    key: SigningKey,
    runtime_dir: PathBuf,
    last_seq: Mutex<u64>,
    exchange: Option<Arc<FactsExchange>>,
    /// The last full probe and when it ran: the base live refreshes start from.
    last_full: Mutex<Option<(std::time::Instant, clawft_types::placement::NodeFacts)>>,
    emulation: Arc<EmulationCache>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FactsConfig {
    /// Absent: keep the default. `null`: no probe image.
    #[serde(default, deserialize_with = "some_or_null")]
    docker_probe_image: Option<Option<String>>,
}

fn some_or_null<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(d).map(Some)
}

/// The probe image: environment, then `facts.config.json`, then `default`.
fn probe_image(runtime_dir: &Path, env: Option<String>, default: Option<String>) -> Option<String> {
    let checked = |v: Option<String>| {
        v.filter(|v| !v.trim().is_empty()).filter(|v| {
            let ok = valid_image_ref(v);
            if !ok {
                warn!(image = %v, "probe image is not a plain image reference; no probe image used");
            }
            ok
        })
    };
    if let Some(v) = env {
        return checked(Some(v));
    }
    if let Some(text) = read_small(&runtime_dir.join(CONFIG_FILE)) {
        match serde_json::from_str::<FactsConfig>(&text) {
            Ok(FactsConfig { docker_probe_image: Some(v) }) => {
                return checked(v);
            }
            Ok(_) => {}
            Err(e) => warn!(error = %e, "facts config unreadable; ignored"),
        }
    }
    default
}

static LOCAL: OnceLock<Arc<LocalSigner>> = OnceLock::new();
static INIT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

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
    cfg.docker_probe_image = probe_image(
        runtime_dir,
        std::env::var(PROBE_IMAGE_ENV).ok(),
        cfg.docker_probe_image.take(),
    );
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

/// Probe this machine into unsigned facts. Blocking: call off the runtime.
fn probe_blocking(signer: &LocalSigner) -> clawft_types::placement::NodeFacts {
    let mut cfg = probe_config(&signer.runtime_dir);
    cfg.emulation_cache = Some(signer.emulation.clone());
    let node_id = clawft_kernel::node_id_from_pubkey(&signer.key.verifying_key().to_bytes());
    let facts = build_facts(
        &node_id,
        now_secs(),
        DEFAULT_FACTS_TTL_SECS,
        0,
        probe_capabilities(&SystemHost::default(), &cfg),
    );
    *signer.last_full.lock().unwrap_or_else(|p| p.into_inner()) =
        Some((std::time::Instant::now(), facts.clone()));
    facts
}

/// Facts for this tick: a cheap live refresh of the last full probe, or a
/// full probe when asked, when none exists or when it is due.
fn facts_blocking(signer: &LocalSigner, full: bool) -> clawft_types::placement::NodeFacts {
    let base = signer
        .last_full
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone()
        .filter(|(at, _)| at.elapsed() < Duration::from_secs(FULL_PROBE_SECS));
    match base {
        Some((_, base)) if !full => refresh_live(&SystemHost::default(), &base, now_secs()),
        _ => probe_blocking(signer),
    }
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
    full: bool,
) -> Result<u64, String> {
    if let Some(exchange) = signer.exchange.clone() {
        let s = signer.clone();
        let facts = tokio::task::spawn_blocking(move || facts_blocking(&s, full))
            .await
            .map_err(|e| format!("probe task failed: {e}"))?;
        let published = exchange
            .update_live(facts, now_secs())
            .await
            .map_err(|e| e.to_string())?;
        use clawft_kernel::node_facts_exchange::Published;
        return Ok(match published {
            Published::Base(seq) => seq,
            Published::Delta { seq, .. } => seq,
            Published::Nothing => 0,
        });
    }
    tokio::task::spawn_blocking(move || refresh_blocking(&signer, &membership))
        .await
        .map_err(|e| format!("probe task failed: {e}"))?
}

/// Start local facts: first probe now, then refresh at 80% of the TTL.
/// Idempotent: only the first call starts the task.
pub fn init(
    key: SigningKey,
    runtime_dir: PathBuf,
    membership: Arc<ClusterMembership>,
    mesh: Option<Arc<MeshRuntime>>,
) {
    // Claim first: a second call must not build (and start) a second exchange.
    if INIT.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let exchange = mesh.map(|rt| {
        let ex = FactsExchange::new(
            key.clone(),
            membership.clone(),
            rt,
            FactsTrustPolicy::default(),
        );
        ex.start();
        ex
    });
    let signer = Arc::new(LocalSigner {
        key,
        runtime_dir,
        last_seq: Mutex::new(0),
        exchange,
        last_full: Mutex::new(None),
        emulation: Arc::new(EmulationCache::new(Duration::from_secs(EMULATION_CACHE_SECS))),
    });
    if LOCAL.set(signer.clone()).is_err() {
        return;
    }
    tokio::spawn(async move {
        // Connected nodes re-probe often enough to turn live-state changes into
        // deltas; the exchange re-signs a base by itself before its TTL.
        let every = if signer.exchange.is_some() {
            Duration::from_secs(LIVE_PROBE_SECS)
        } else {
            Duration::from_secs(DEFAULT_FACTS_TTL_SECS * 4 / 5)
        };
        loop {
            match refresh(signer.clone(), membership.clone(), false).await {
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
        if let Err(e) = refresh(signer, membership.clone(), true).await {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn dir_with(config: Option<&str>) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        if let Some(c) = config {
            std::fs::write(d.path().join(CONFIG_FILE), c).unwrap();
        }
        d
    }

    fn alpine() -> Option<String> {
        Some("alpine:3.20".into())
    }

    #[test]
    fn probe_image_defaults_then_file_then_environment() {
        let none = dir_with(None);
        assert_eq!(probe_image(none.path(), None, alpine()), alpine());

        let file = dir_with(Some(r#"{"docker_probe_image": "armv7/probe:1"}"#));
        assert_eq!(
            probe_image(file.path(), None, alpine()).as_deref(),
            Some("armv7/probe:1")
        );
        // Environment wins over the file.
        assert_eq!(
            probe_image(file.path(), Some("local/img:2".into()), alpine()).as_deref(),
            Some("local/img:2")
        );
    }

    #[test]
    fn probe_image_can_be_disabled() {
        let null = dir_with(Some(r#"{"docker_probe_image": null}"#));
        assert_eq!(probe_image(null.path(), None, alpine()), None);
        let none = dir_with(None);
        assert_eq!(probe_image(none.path(), Some("  ".into()), alpine()), None);
        // An empty config object keeps the default.
        let empty = dir_with(Some("{}"));
        assert_eq!(probe_image(empty.path(), None, alpine()), alpine());
    }

    #[test]
    fn an_image_reference_that_docker_would_read_as_an_option_is_not_used() {
        let none = dir_with(None);
        assert_eq!(probe_image(none.path(), Some("--privileged".into()), alpine()), None);
        assert_eq!(probe_image(none.path(), Some("a b".into()), alpine()), None);
        let file = dir_with(Some(r#"{"docker_probe_image": "-v /:/host"}"#));
        assert_eq!(probe_image(file.path(), None, alpine()), None);
    }

    #[test]
    fn unreadable_config_falls_back_to_the_default() {
        let bad = dir_with(Some("{not json"));
        assert_eq!(probe_image(bad.path(), None, alpine()), alpine());
        let unknown = dir_with(Some(r#"{"surprise": 1}"#));
        assert_eq!(probe_image(unknown.path(), None, alpine()), alpine());
    }
}
