//! Shared service state: the journal and its bindings, the registry, the
//! router, verdicts, facts and the admission gate, built once at start.

use std::collections::HashMap;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use clawft_kernel::revocation::RevocationList;
use clawft_mesh_local::proto::{PROTO_MAX, PROTO_MIN};
use clawft_mesh_local::{hexser, Principal};
use clawft_types::config::MeshAdmissionMode;
use ed25519_dalek::SigningKey;
use serde_json::{json, Value};

use crate::config::MeshServiceConfig;
use crate::facts::Facts;
use crate::force_revoked::ForceRevoked;
use crate::gate::ServiceGate;
use crate::limits::{LimitConfig, Limiter};
use crate::registry::Registry;
use crate::router::TenantRouter;
use crate::verdicts::VerdictBroker;
use crate::{Bindings, Journal};

pub fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// The journal and the bindings folded from it. One lock: a binding change
/// and its journal record are one step.
pub struct Core {
    pub journal: Journal,
    pub bindings: Bindings,
}

struct PolicyInner {
    owner_uid: Option<u32>,
    owner_explicit: bool,
    admission: MeshAdmissionMode,
}

/// Mutable policy: which uid is the cluster owner and the admission mode.
/// Initial values come from `mesh.toml`, overridden by the newest
/// `policy.set` records in the journal (the journal is the source of truth
/// for admin changes). The owner is never inferred: a TOFU bind must not be
/// able to make a user the authority on cluster membership. With no owner,
/// verdicts answer `Unavailable` (observe-only) and enforce is refused.
pub struct PolicyCell {
    inner: RwLock<PolicyInner>,
}

impl PolicyCell {
    pub fn new(owner: Option<u32>, admission: MeshAdmissionMode) -> Arc<Self> {
        Arc::new(Self {
            inner: RwLock::new(PolicyInner { owner_uid: owner, owner_explicit: owner.is_some(), admission }),
        })
    }

    /// Apply the journal over the configured values.
    pub fn from_journal(cfg: &MeshServiceConfig, journal: &Journal) -> Arc<Self> {
        let cell = Self::new(cfg.cluster_owner_uid, cfg.admission);
        for r in journal.iter().filter(|r| r.kind == "policy.set") {
            match r.body["key"].as_str() {
                Some("admission") => {
                    if let Some(m) = r.body["new"].as_str().and_then(parse_admission) {
                        cell.set_admission(m);
                    }
                }
                Some("cluster_owner_uid") => {
                    if let Some(u) = r.body["new"].as_u64().and_then(|u| u32::try_from(u).ok()) {
                        cell.set_owner(u);
                    }
                }
                _ => {}
            }
        }
        cell
    }

    pub fn owner_uid(&self) -> Option<u32> {
        self.inner.read().expect("policy lock").owner_uid
    }

    pub fn admission(&self) -> MeshAdmissionMode {
        self.inner.read().expect("policy lock").admission
    }

    pub fn set_owner(&self, uid: u32) {
        let mut g = self.inner.write().expect("policy lock");
        g.owner_uid = Some(uid);
        g.owner_explicit = true;
    }

    pub fn set_admission(&self, m: MeshAdmissionMode) {
        self.inner.write().expect("policy lock").admission = m;
    }
}

pub fn parse_admission(s: &str) -> Option<MeshAdmissionMode> {
    match s {
        "off" => Some(MeshAdmissionMode::Off),
        "observe" => Some(MeshAdmissionMode::Observe),
        "enforce" => Some(MeshAdmissionMode::Enforce),
        _ => None,
    }
}

pub fn admission_str(m: MeshAdmissionMode) -> &'static str {
    match m {
        MeshAdmissionMode::Off => "off",
        MeshAdmissionMode::Observe => "observe",
        MeshAdmissionMode::Enforce => "enforce",
    }
}

pub struct ServiceState {
    pub cfg: MeshServiceConfig,
    pub machine_key: SigningKey,
    pub machine_pubkey: [u8; 32],
    pub node_id: String,
    pub core: Arc<Mutex<Core>>,
    pub registry: Arc<Registry>,
    pub limiter: Arc<Limiter>,
    pub policy: Arc<PolicyCell>,
    pub verdicts: Arc<VerdictBroker>,
    pub router: Arc<TenantRouter>,
    pub revocations: Arc<RevocationList>,
    pub facts: Arc<Facts>,
    pub gate: Arc<ServiceGate>,
    /// The key each principal most recently offered that conflicted with its
    /// binding; lets `bind.rebind <uid>` name it without retyping. In memory.
    pub conflicts: Mutex<HashMap<Principal, [u8; 32]>>,
    /// Newest certificate per user id, reused by a reconnect while it has more
    /// than half its life left (no new journal record per reconnect). In memory.
    pub last_certs: Mutex<HashMap<String, clawft_mesh_local::UserCert>>,
    /// Principals revoked while the journal could not record it: enforced in
    /// memory (registration and renewal refuse them) until a restart.
    pub force_revoked: ForceRevoked,
    pub conn_seq: AtomicU64,
    pub started_at: u64,
    /// Where the mesh listener actually bound.
    pub listen_addr: Mutex<Option<String>>,
}

impl ServiceState {
    /// Assemble the state from an opened journal.
    pub fn build(
        cfg: MeshServiceConfig,
        machine_key: SigningKey,
        journal: Journal,
        revocations: Arc<RevocationList>,
        limits: LimitConfig,
    ) -> Result<Arc<Self>, String> {
        let machine_pubkey = machine_key.verifying_key().to_bytes();
        let node_id = clawft_mesh_local::node_id_from_pubkey(&machine_pubkey);
        let bindings = Bindings::fold_lenient(&journal);
        if let Some(why) = bindings.degraded() {
            tracing::error!(reason = why, "bindings are degraded: serving read-only, refusing binds and certs");
        }
        let force_revoked = ForceRevoked::load(&cfg.state_dir)?;
        let policy = PolicyCell::from_journal(&cfg, &journal);
        if policy.admission() == MeshAdmissionMode::Enforce && policy.owner_uid().is_none() {
            return Err("the effective admission mode is enforce but no cluster owner is set \
                        (set cluster_owner_uid in mesh.toml)"
                .into());
        }
        let core = Arc::new(Mutex::new(Core { journal, bindings }));
        let registry = Arc::new(Registry::new());
        let verdicts = VerdictBroker::new(
            Arc::clone(&registry),
            Arc::clone(&policy),
            std::time::Duration::from_secs(cfg.verdict_timeout_s),
            std::time::Duration::from_secs(cfg.stale_grace_s),
        );
        let router = TenantRouter::new(Arc::clone(&registry), Arc::clone(&policy), node_id.clone());
        let facts = Facts::new(&cfg, machine_key.clone(), node_id.clone());
        let gate = ServiceGate::new(
            cfg.genesis_hash,
            cfg.noise,
            Arc::clone(&revocations),
            Arc::clone(&verdicts),
            Arc::clone(&core),
            policy.admission(),
        )
        .map_err(|e| format!("the effective admission mode ({}) cannot be applied: {e}", admission_str(policy.admission())))?;
        Ok(Arc::new(Self {
            cfg,
            machine_key,
            machine_pubkey,
            node_id,
            core,
            registry,
            limiter: Limiter::new(limits),
            policy,
            verdicts,
            router,
            revocations,
            facts,
            gate,
            conflicts: Mutex::new(HashMap::new()),
            last_certs: Mutex::new(HashMap::new()),
            conn_seq: AtomicU64::new(1),
            started_at: unix_now(),
            listen_addr: Mutex::new(None),
            force_revoked,
        }))
    }

    /// Append a non-binding record. A failure is logged and swallowed: the
    /// decision it records has already been made.
    pub fn note(&self, kind: &str, body: Value) {
        let mut core = self.core.lock().expect("core lock");
        if let Err(e) = core.journal.append(kind, body) {
            tracing::error!(kind, error = %e, "journal append failed");
        }
    }

    /// Append a record and report failure: for admin verbs, which must
    /// journal first and change state only when the append succeeded.
    pub fn try_note(&self, kind: &str, body: Value) -> Result<(), crate::JournalError> {
        let mut core = self.core.lock().expect("core lock");
        core.journal.append(kind, body).map(|_| ())
    }

    /// Whether the journal refuses trust-increasing changes (quarantined tail).
    pub fn journal_read_only(&self) -> bool {
        self.core.lock().expect("core lock").journal.read_only()
    }

    /// Re-sign facts after a revocation change (best effort, logged).
    pub fn refresh_facts(&self) {
        if let Err(e) = self.facts.refresh(&self.core, &self.force_revoked, unix_now()) {
            tracing::error!(error = %e, "facts refresh failed");
        }
    }

    /// The `status` reply. Registrations are listed for admins only; the peer
    /// table needs a registered daemon or an admin.
    pub fn status_json(&self, caller_uid: u32, admin: bool, registered: bool) -> Value {
        let (head, records, read_only, degraded, pending) = {
            let c = self.core.lock().expect("core lock");
            (
                c.journal.head(),
                c.journal.len(),
                c.journal.read_only(),
                c.bindings.degraded().map(str::to_string),
                c.journal.pending_quarantines(),
            )
        };
        let regs: Vec<Value> = if admin {
            self.registry
                .all()
                .iter()
                .map(|r| {
                    let (projects, prefixes) = self.registry.owned(&r.user_id);
                    json!({
                        "principal": r.principal, "user_id": r.user_id, "pid": r.pid, "exe": r.exe,
                        "projects": projects, "topic_prefixes": prefixes, "registered_at": r.registered_at,
                        "capabilities": r.capabilities,
                        "delivered": r.counters.delivered.load(std::sync::atomic::Ordering::Relaxed),
                        "dropped_full": r.counters.dropped_full.load(std::sync::atomic::Ordering::Relaxed),
                    })
                })
                .collect()
        } else {
            Vec::new()
        };
        let c = &self.router.counters;
        let get = |a: &AtomicU64| a.load(std::sync::atomic::Ordering::Relaxed);
        json!({
            "node_id": self.node_id,
            "machine_pubkey": hexser::encode(&self.machine_pubkey),
            "proto": {"min": PROTO_MIN, "max": PROTO_MAX},
            "build_sha": self.cfg.build_sha,
            "started_at": self.started_at,
            "listen": self.listen_addr.lock().expect("listen lock").clone().unwrap_or_else(|| self.cfg.listen.clone()),
            "socket": self.cfg.socket.display().to_string(),
            "admission": admission_str(self.policy.admission()),
            "bind_policy": self.cfg.bind_policy.as_str(),
            "cluster_owner_uid": self.policy.owner_uid(),
            "you": caller_uid,
            "registered": self.registry.len(),
            "force_revoked": if admin { self.force_revoked.list() } else { Vec::new() },
            "registrations": regs,
            "peers": if admin || registered { self.router.runtime().map(|r| r.peer_ids()).unwrap_or_default() } else { Vec::new() },
            "journal": {
                "seq": head.as_ref().map(|h| h.seq), "hash": head.map(|h| h.hash),
                "records": records, "read_only": read_only, "degraded": degraded,
                "pending_quarantines": pending,
            },
            "router": {
                "delivered": get(&c.delivered), "scope_required": get(&c.scope_required),
                "unknown_scope": get(&c.unknown_scope), "denied_scope": get(&c.denied_scope),
                "no_tenant": get(&c.no_tenant), "dropped_full": get(&c.dropped_full),
                "sent_remote": get(&c.sent_remote), "sent_local": get(&c.sent_local),
            },
        })
    }
}
