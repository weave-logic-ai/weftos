//! The service's admission gate: a swappable wrapper over the kernel's
//! `CryptoGate` (genesis pin, revocation list, cluster-owner verdicts) that
//! journals what it decides (`peer.admit`, `peer.refuse`).
//!
//! Journalling is bounded: a refusal is triggered by an unauthenticated
//! network peer, so every record costs an fsync the peer chose to cause. At
//! most [`MAX_NOTES_PER_MINUTE`] records are written per minute, and the same
//! (kind, node, code) at most once per [`DEDUPE`].

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use clawft_kernel::mesh_admit::{
    Admission, AdmissionGate, AdmitContext, AllowAll, CryptoGate, HelloFailure, VerifiedHello,
};
use clawft_kernel::revocation::RevocationList;
use clawft_mesh_local::hexser;
use clawft_types::config::MeshAdmissionMode;
use serde_json::json;

use crate::state::{admission_str, Core};
use crate::verdicts::{OwnerVerdicts, VerdictBroker};

/// Journal records the gate may write per minute.
pub const MAX_NOTES_PER_MINUTE: usize = 30;
/// Same (kind, node, code) at most once per this long.
pub const DEDUPE: Duration = Duration::from_secs(300);
const MAX_SEEN: usize = 1024;

struct NoteLimiter {
    window_start: Instant,
    in_window: usize,
    seen: HashMap<String, Instant>,
}

impl NoteLimiter {
    fn new() -> Self {
        Self { window_start: Instant::now(), in_window: 0, seen: HashMap::new() }
    }

    fn allow(&mut self, key: String) -> bool {
        let now = Instant::now();
        if now.duration_since(self.window_start) >= Duration::from_secs(60) {
            self.window_start = now;
            self.in_window = 0;
        }
        if self.in_window >= MAX_NOTES_PER_MINUTE {
            return false;
        }
        if self.seen.get(&key).is_some_and(|t| now.duration_since(*t) < DEDUPE) {
            return false;
        }
        if self.seen.len() >= MAX_SEEN {
            self.seen.retain(|_, t| now.duration_since(*t) < DEDUPE);
            if self.seen.len() >= MAX_SEEN {
                self.seen.clear();
            }
        }
        self.seen.insert(key, now);
        self.in_window += 1;
        true
    }
}

pub struct ServiceGate {
    inner: RwLock<Arc<dyn AdmissionGate>>,
    genesis: Option<[u8; 32]>,
    noise: bool,
    revocations: Arc<RevocationList>,
    verdicts: Arc<VerdictBroker>,
    core: Arc<Mutex<Core>>,
    mode: RwLock<MeshAdmissionMode>,
    /// `peer.refuse` records (attacker-triggerable) and `peer.admit` records
    /// have separate budgets so refusals cannot starve the admission audit.
    refuse_notes: Mutex<NoteLimiter>,
    admit_notes: Mutex<NoteLimiter>,
}

impl ServiceGate {
    pub fn new(
        genesis: Option<[u8; 32]>,
        noise: bool,
        revocations: Arc<RevocationList>,
        verdicts: Arc<VerdictBroker>,
        core: Arc<Mutex<Core>>,
        mode: MeshAdmissionMode,
    ) -> Result<Arc<Self>, String> {
        let gate = Arc::new(Self {
            inner: RwLock::new(Arc::new(AllowAll)),
            genesis,
            noise,
            revocations,
            verdicts,
            core,
            mode: RwLock::new(mode),
            refuse_notes: Mutex::new(NoteLimiter::new()),
            admit_notes: Mutex::new(NoteLimiter::new()),
        });
        // Fail closed: an effective mode that cannot be built (journalled
        // `enforce` with an edited mesh.toml missing the genesis hash or noise)
        // must stop the service, not silently run as AllowAll.
        gate.set_mode(mode)?;
        Ok(gate)
    }

    fn build(&self, mode: MeshAdmissionMode) -> Result<Arc<dyn AdmissionGate>, String> {
        match (mode, self.genesis) {
            (MeshAdmissionMode::Off, _) | (MeshAdmissionMode::Observe, None) => Ok(Arc::new(AllowAll)),
            (MeshAdmissionMode::Enforce, None) => Err("enforce requires genesis_hash".into()),
            (MeshAdmissionMode::Enforce, Some(_)) if !self.noise => {
                Err("enforce requires noise = true".into())
            }
            (m, Some(g)) => Ok(Arc::new(CryptoGate::new(
                g,
                Arc::clone(&self.revocations),
                Arc::new(OwnerVerdicts(Arc::clone(&self.verdicts))),
                m,
            ))),
        }
    }

    /// Whether `mode` could be applied (genesis and noise present for enforce).
    pub fn validate_mode(&self, mode: MeshAdmissionMode) -> Result<(), String> {
        self.build(mode).map(|_| ())
    }

    /// Switch the admission mode for new admissions. Listener-level limits
    /// (`strict`) are read once when the listener starts and change only on a
    /// restart.
    pub fn set_mode(&self, mode: MeshAdmissionMode) -> Result<(), String> {
        let built = self.build(mode)?;
        *self.inner.write().expect("gate lock") = built;
        *self.mode.write().expect("gate mode lock") = mode;
        Ok(())
    }

    pub fn mode(&self) -> MeshAdmissionMode {
        *self.mode.read().expect("gate mode lock")
    }

    fn current(&self) -> Arc<dyn AdmissionGate> {
        Arc::clone(&self.inner.read().expect("gate lock"))
    }

    fn note(&self, kind: &str, node: &str, code: &str, body: serde_json::Value) {
        let budget = if kind == "peer.admit" { &self.admit_notes } else { &self.refuse_notes };
        if !budget.lock().expect("notes lock").allow(format!("{kind}/{node}/{code}")) {
            return;
        }
        let mut c = self.core.lock().expect("core lock");
        if let Err(e) = c.journal.append(kind, body) {
            tracing::error!(kind, error = %e, "journal append failed");
        }
    }

    fn record(&self, hello: Option<&VerifiedHello>, ctx: &AdmitContext, adm: &Admission) {
        let mode = admission_str(self.mode());
        let class = format!("{:?}", ctx.class).to_lowercase();
        let (node_id, pubkey) = hello
            .map(|h| (h.node_id.clone(), hexser::encode(&h.pubkey)))
            .unwrap_or_default();
        match adm {
            Admission::Refuse(r) => self.note(
                "peer.refuse",
                &node_id,
                r.code,
                json!({"node_id": node_id, "pubkey": pubkey, "class": class, "reason": r.code, "mode": mode}),
            ),
            Admission::Admit(g) => {
                if let Some(r) = &g.observed {
                    self.note(
                        "peer.refuse",
                        &node_id,
                        r.code,
                        json!({"node_id": node_id, "pubkey": pubkey, "class": class, "reason": r.code, "mode": "observe"}),
                    );
                } else if g.admitted {
                    let rule = self.verdicts.rule_hash_for(&node_id);
                    self.note(
                        "peer.admit",
                        &node_id,
                        "admit",
                        json!({"node_id": node_id, "pubkey": pubkey, "class": class,
                               "verdict_rule_hash": rule, "mode": mode}),
                    );
                }
            }
        }
    }
}

#[async_trait]
impl AdmissionGate for ServiceGate {
    fn strict(&self) -> bool {
        self.current().strict()
    }

    fn is_revoked(&self, node_id: &str) -> bool {
        self.current().is_revoked(node_id)
    }

    async fn admit(&self, hello: &VerifiedHello, ctx: &AdmitContext) -> Admission {
        let adm = self.current().admit(hello, ctx).await;
        self.record(Some(hello), ctx, &adm);
        adm
    }

    async fn admit_unverified(&self, failure: &HelloFailure, ctx: &AdmitContext) -> Admission {
        let adm = self.current().admit_unverified(failure, ctx).await;
        self.record(None, ctx, &adm);
        adm
    }
}
