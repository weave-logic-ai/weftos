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
    /// Peer may join.
    Permit,
    /// Peer may not join (deferred decisions count as denials).
    Deny(String),
}

/// Source of `peer.admit` verdicts. In service mode this is the
/// cluster-owner daemon; in collapsed mode [`GateVerdictSource`].
#[async_trait]
pub trait VerdictSource: Send + Sync + 'static {
    /// Decide on `req`.
    async fn verdict(&self, req: &VerdictRequest) -> Verdict;
}

/// Permits every request (no governance configured).
#[derive(Debug, Default, Clone, Copy)]
pub struct OpenVerdicts;

#[async_trait]
impl VerdictSource for OpenVerdicts {
    async fn verdict(&self, _: &VerdictRequest) -> Verdict {
        Verdict::Permit
    }
}

/// Collapsed-mode verdict source: the local governance gate, asked the
/// `cluster.join` action, so collapsed and service mode share one path.
/// The gate is bound late because the mesh listener starts before the
/// governance gate is built at boot; until bound, requests are denied.
#[cfg(feature = "exochain")]
#[derive(Default)]
pub struct GateVerdictSource {
    gate: std::sync::OnceLock<Option<Arc<dyn crate::gate::GateBackend>>>,
}

#[cfg(feature = "exochain")]
impl GateVerdictSource {
    /// Unbound source (denies until [`bind`](Self::bind) / [`bind_open`](Self::bind_open)).
    pub fn late() -> Self {
        Self::default()
    }
    /// Bind the governance gate.
    pub fn bind(&self, gate: Arc<dyn crate::gate::GateBackend>) {
        let _ = self.gate.set(Some(gate));
    }
    /// Bind "no governance available": permit everything.
    pub fn bind_open(&self) {
        let _ = self.gate.set(None);
    }
}

#[cfg(feature = "exochain")]
#[async_trait]
impl VerdictSource for GateVerdictSource {
    async fn verdict(&self, req: &VerdictRequest) -> Verdict {
        use crate::gate::GateDecision;
        match self.gate.get() {
            None => Verdict::Deny("governance gate not ready".into()),
            Some(None) => Verdict::Permit,
            Some(Some(g)) => {
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

/// Cryptographic admission: genesis pin, revocation, verdict, mode.
pub struct CryptoGate {
    genesis: [u8; 32],
    revocations: Arc<RevocationList>,
    verdicts: Arc<dyn VerdictSource>,
    mode: MeshAdmissionMode,
    ttl: Duration,
    // Keyed by (node id, pubkey): a cached permit for an id must never
    // apply to a different key claiming it.
    cache: Mutex<HashMap<(String, [u8; 32]), (Instant, Verdict)>>,
    records: Mutex<VecDeque<AdmissionRecord>>,
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

    fn finish(&self, node_id: Option<&str>, grant: Grant, failure: Option<Refusal>) -> Admission {
        let Some(refusal) = failure else {
            return Admission::Admit(grant);
        };
        let admitted = self.mode != MeshAdmissionMode::Enforce;
        tracing::warn!(
            node = node_id.unwrap_or("-"), code = refusal.code, detail = %refusal.detail,
            mode = ?self.mode, admitted, "mesh admission failure"
        );
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
            Admission::Admit(Grant { observed: Some(refusal), trust_scope: false, ..grant })
        } else {
            Admission::Refuse(refusal)
        }
    }

    async fn cached_verdict(&self, req: &VerdictRequest) -> Verdict {
        let key = (req.node_id.clone(), req.pubkey);
        if let Some((at, v)) = self.cache.lock().unwrap().get(&key) {
            if at.elapsed() < self.ttl {
                return v.clone();
            }
        }
        let v = self.verdicts.verdict(req).await;
        self.cache.lock().unwrap().insert(key, (Instant::now(), v.clone()));
        v
    }
}

fn open_grant() -> Grant {
    Grant { limits: PeerLimits::None, trust_scope: false, observed: None }
}

#[async_trait]
impl AdmissionGate for CryptoGate {
    async fn admit(&self, hello: &VerifiedHello, ctx: &AdmitContext) -> Admission {
        if self.mode == MeshAdmissionMode::Off {
            return Admission::Admit(open_grant());
        }
        let id = hello.node_id.as_str();
        let grant = Grant {
            limits: if self.mode == MeshAdmissionMode::Enforce && ctx.class == PeerClass::Leaf {
                PeerLimits::Leaf
            } else {
                PeerLimits::None
            },
            trust_scope: ctx.class == PeerClass::Node,
            observed: None,
        };
        if hello.genesis_hash != self.genesis {
            return self.finish(Some(id), grant, Some(Refusal {
                code: "wrong_genesis",
                detail: "hello genesis differs from the pinned cluster genesis".into(),
            }));
        }
        if self.revocations.is_revoked(id) {
            return self.finish(Some(id), grant, Some(Refusal {
                code: "revoked",
                detail: "node id is on the revocation list".into(),
            }));
        }
        let req = VerdictRequest {
            node_id: id.to_owned(),
            pubkey: hello.pubkey,
            class: ctx.class,
            platform: hello.platform.clone(),
        };
        match self.cached_verdict(&req).await {
            Verdict::Permit => Admission::Admit(grant),
            Verdict::Deny(why) => self.finish(Some(id), grant, Some(Refusal {
                code: "verdict_denied",
                detail: why,
            })),
        }
    }

    async fn admit_unverified(&self, failure: &HelloFailure, _ctx: &AdmitContext) -> Admission {
        if self.mode == MeshAdmissionMode::Off {
            return Admission::Admit(open_grant());
        }
        let refusal = Refusal { code: failure.code(), detail: format!("{failure:?}") };
        self.finish(None, open_grant(), Some(refusal))
    }
}
