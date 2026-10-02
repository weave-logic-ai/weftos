//! Verdict sources and the cryptographic admission gate (P3-K1).
//!
//! Split from [`mesh_admit`](crate::mesh_admit), which re-exports
//! everything here.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use clawft_types::config::MeshAdmissionMode;

use crate::mesh_admit::{
    Admission, AdmissionGate, AdmitContext, Grant, HelloFailure, PeerClass, PeerLimits, Refusal,
    VerifiedHello,
};
use crate::revocation::RevocationList;

/// Default verdict cache lifetime.
pub const VERDICT_TTL: Duration = Duration::from_secs(300);
/// How long a denial is cached (short: an owner who changes their mind
/// should not wait five minutes).
pub const DENY_TTL: Duration = Duration::from_secs(5);
const MAX_CACHE: usize = 1024;
const WARN_EVERY: Duration = Duration::from_secs(60);
const MAX_RECORDS: usize = 256;

/// Request to the cluster-owner for a join verdict.
#[derive(Debug, Clone)]
pub struct VerdictRequest {
    /// Verified node id.
    pub node_id: String,
    /// Verified key.
    pub pubkey: [u8; 32],
    /// Peer class.
    pub class: PeerClass,
    /// Platform string.
    pub platform: String,
}

/// Verdict on a join request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Peer may join as its declared class.
    Permit,
    /// Peer may join, served as this class (overrides the self-declared
    /// class under `enforce`).
    PermitAs(PeerClass),
    /// Peer may not join (deferred decisions count as denials).
    Deny(String),
    /// No decision could be made (source not ready). Refused like a denial
    /// but never cached.
    Unavailable(String),
}

/// Source of `peer.admit` verdicts. In service mode this is the
/// cluster-owner daemon; in collapsed mode [`GateVerdictSource`].
#[async_trait]
pub trait VerdictSource: Send + Sync + 'static {
    /// Decide on `req`.
    async fn verdict(&self, req: &VerdictRequest) -> Verdict;

    /// True when this source permits everybody (no real membership
    /// decision). `enforce` over an open source needs an explicit opt-in.
    fn is_open(&self) -> bool {
        false
    }
}

/// Permits every request (no governance configured).
#[derive(Debug, Default, Clone, Copy)]
pub struct OpenVerdicts;

#[async_trait]
impl VerdictSource for OpenVerdicts {
    async fn verdict(&self, _: &VerdictRequest) -> Verdict {
        Verdict::Permit
    }
    fn is_open(&self) -> bool {
        true
    }
}

#[cfg(feature = "exochain")]
enum GateState {
    Gate(Arc<dyn crate::gate::GateBackend>),
    Open,
    Closed,
}

/// Collapsed-mode verdict source: the local governance gate, asked the
/// `cluster.join` action, so collapsed and service mode share one path.
/// The gate is bound late because the mesh listener starts before the
/// governance gate is built at boot; until bound, requests are
/// [`Verdict::Unavailable`].
#[cfg(feature = "exochain")]
#[derive(Default)]
pub struct GateVerdictSource {
    gate: std::sync::OnceLock<GateState>,
}

#[cfg(feature = "exochain")]
impl GateVerdictSource {
    /// Unbound source.
    pub fn late() -> Self {
        Self::default()
    }
    /// Bind the governance gate.
    pub fn bind(&self, gate: Arc<dyn crate::gate::GateBackend>) {
        let _ = self.gate.set(GateState::Gate(gate));
    }
    /// Bind "no governance available": permit everything.
    pub fn bind_open(&self) {
        let _ = self.gate.set(GateState::Open);
    }
    /// Bind "no governance available and open membership not allowed":
    /// refuse everything.
    pub fn bind_closed(&self) {
        let _ = self.gate.set(GateState::Closed);
    }
}

#[cfg(feature = "exochain")]
#[async_trait]
impl VerdictSource for GateVerdictSource {
    async fn verdict(&self, req: &VerdictRequest) -> Verdict {
        use crate::gate::GateDecision;
        match self.gate.get() {
            None => Verdict::Unavailable("governance gate not ready".into()),
            Some(GateState::Open) => Verdict::Permit,
            Some(GateState::Closed) => Verdict::Unavailable(
                "no governance gate; set kernel.mesh.admission_open_membership to allow".into(),
            ),
            Some(GateState::Gate(g)) => {
                let ctx = serde_json::json!({
                    "node_id": req.node_id,
                    "platform": req.platform,
                    "class": format!("{:?}", req.class),
                });
                match g.check(&req.node_id, "cluster.join", &ctx) {
                    GateDecision::Permit { .. } => Verdict::Permit,
                    GateDecision::Defer { reason } => Verdict::Deny(format!("deferred: {reason}")),
                    GateDecision::Deny { reason, .. } => Verdict::Deny(reason),
                }
            }
        }
    }
    fn is_open(&self) -> bool {
        matches!(self.gate.get(), Some(GateState::Open))
    }
}

/// One admission outcome, kept for `doctor` / journaling by the owner.
#[derive(Debug, Clone)]
pub struct AdmissionRecord {
    /// Verified node id, when a hello verified.
    pub node_id: Option<String>,
    /// Refusal / would-refuse code.
    pub code: &'static str,
    /// Detail text.
    pub detail: String,
    /// Mode in force.
    pub mode: MeshAdmissionMode,
    /// True when the peer was served anyway (observe).
    pub admitted: bool,
}

type CacheKey = (String, [u8; 32], PeerClass, String);

/// Cryptographic admission: genesis pin, revocation, verdict, mode.
///
/// `genesis_hash` is a cluster *label*, not a credential: anyone who knows
/// it can present it. Membership is the verdict source's decision.
pub struct CryptoGate {
    genesis: [u8; 32],
    revocations: Arc<RevocationList>,
    verdicts: Arc<dyn VerdictSource>,
    mode: MeshAdmissionMode,
    ttl: Duration,
    // Keyed by everything the verdict depends on (id, key, declared class,
    // platform): a cached permit never applies to a different key or class.
    cache: Mutex<HashMap<CacheKey, (Instant, Verdict)>>,
    records: Mutex<VecDeque<AdmissionRecord>>,
    last_warn: Mutex<HashMap<String, Instant>>,
}

impl CryptoGate {
    /// Build a gate pinned to `genesis`.
    pub fn new(
        genesis: [u8; 32],
        revocations: Arc<RevocationList>,
        verdicts: Arc<dyn VerdictSource>,
        mode: MeshAdmissionMode,
    ) -> Self {
        Self {
            genesis,
            revocations,
            verdicts,
            mode,
            ttl: VERDICT_TTL,
            cache: Mutex::new(HashMap::new()),
            records: Mutex::new(VecDeque::new()),
            last_warn: Mutex::new(HashMap::new()),
        }
    }

    /// Override the verdict cache lifetime.
    pub fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
    }

    /// Recent observe/refuse records, oldest first.
    pub fn records(&self) -> Vec<AdmissionRecord> {
        self.records.lock().unwrap().iter().cloned().collect()
    }

    /// Number of cached verdicts (for tests and diagnostics).
    pub fn cache_len(&self) -> usize {
        self.cache.lock().unwrap().len()
    }

    fn warn_due(&self, key: &str) -> bool {
        let mut w = self.last_warn.lock().unwrap();
        if w.len() >= MAX_CACHE {
            w.retain(|_, t| t.elapsed() < WARN_EVERY);
        }
        match w.get(key) {
            Some(t) if t.elapsed() < WARN_EVERY => false,
            _ => {
                w.insert(key.to_owned(), Instant::now());
                true
            }
        }
    }

    fn finish(&self, node_id: Option<&str>, grant: Grant, refusal: Refusal) -> Admission {
        let admitted = self.mode != MeshAdmissionMode::Enforce;
        if self.warn_due(&format!("{}/{}", node_id.unwrap_or("-"), refusal.code)) {
            tracing::warn!(
                node = node_id.unwrap_or("-"), code = refusal.code, detail = %refusal.detail,
                mode = ?self.mode, admitted, "mesh admission failure"
            );
        }
        let mut rec = self.records.lock().unwrap();
        if rec.len() >= MAX_RECORDS {
            rec.pop_front();
        }
        rec.push_back(AdmissionRecord {
            node_id: node_id.map(str::to_owned),
            code: refusal.code,
            detail: refusal.detail.clone(),
            mode: self.mode,
            admitted,
        });
        if admitted {
            Admission::Admit(Grant {
                observed: Some(refusal),
                trust_scope: false,
                admitted: false,
                ..grant
            })
        } else {
            Admission::Refuse(refusal)
        }
    }

    async fn cached_verdict(&self, req: &VerdictRequest) -> Verdict {
        let key: CacheKey = (req.node_id.clone(), req.pubkey, req.class, req.platform.clone());
        if let Some((at, v)) = self.cache.lock().unwrap().get(&key) {
            let ttl = if matches!(v, Verdict::Deny(_)) { DENY_TTL.min(self.ttl) } else { self.ttl };
            if at.elapsed() < ttl {
                return v.clone();
            }
        }
        let v = self.verdicts.verdict(req).await;
        if !matches!(v, Verdict::Unavailable(_)) {
            let mut c = self.cache.lock().unwrap();
            if c.len() >= MAX_CACHE {
                let ttl = self.ttl;
                c.retain(|_, (t, _)| t.elapsed() < ttl);
                // Still full: evict the oldest entries, not everything.
                while c.len() >= MAX_CACHE {
                    let Some(oldest) = c.iter().min_by_key(|(_, (t, _))| *t).map(|(k, _)| k.clone())
                    else {
                        break;
                    };
                    c.remove(&oldest);
                }
            }
            c.insert(key, (Instant::now(), v.clone()));
        }
        v
    }
}

#[async_trait]
impl AdmissionGate for CryptoGate {
    /// Only `enforce` bounds connections: observe must stay a no-op for
    /// legacy peers (quiet leaves, large NAT'd fleets).
    fn strict(&self) -> bool {
        self.mode == MeshAdmissionMode::Enforce
    }

    async fn admit(&self, hello: &VerifiedHello, ctx: &AdmitContext) -> Admission {
        if self.mode == MeshAdmissionMode::Off {
            return Admission::Admit(Grant::open(ctx.class));
        }
        let enforce = self.mode == MeshAdmissionMode::Enforce;
        let id = hello.node_id.as_str();
        let base = Grant::open(ctx.class);
        if hello.genesis_hash != self.genesis {
            return self.finish(Some(id), base, Refusal {
                code: "wrong_genesis",
                detail: "hello genesis differs from the pinned cluster genesis".into(),
            });
        }
        if self.revocations.is_revoked(id) {
            return self.finish(Some(id), base, Refusal {
                code: "revoked",
                detail: "node id is on the revocation list".into(),
            });
        }
        let req = VerdictRequest {
            node_id: id.to_owned(),
            pubkey: hello.pubkey,
            class: ctx.class,
            platform: hello.platform.clone(),
        };
        let class = match self.cached_verdict(&req).await {
            Verdict::Permit => ctx.class,
            Verdict::PermitAs(c) if enforce => c,
            Verdict::PermitAs(_) => ctx.class,
            Verdict::Deny(why) => {
                return self.finish(Some(id), base, Refusal { code: "verdict_denied", detail: why });
            }
            Verdict::Unavailable(why) => {
                return self.finish(Some(id), base, Refusal { code: "verdict_unavailable", detail: why });
            }
        };
        Admission::Admit(Grant {
            limits: if enforce && class == PeerClass::Leaf { PeerLimits::Leaf } else { PeerLimits::None },
            class,
            admitted: enforce,
            trust_scope: enforce && class == PeerClass::Node,
            observed: None,
        })
    }

    async fn admit_unverified(&self, failure: &HelloFailure, ctx: &AdmitContext) -> Admission {
        if self.mode == MeshAdmissionMode::Off {
            return Admission::Admit(Grant::open(ctx.class));
        }
        let refusal = Refusal { code: failure.code(), detail: format!("{failure:?}") };
        self.finish(None, Grant::open(ctx.class), refusal)
    }
}
