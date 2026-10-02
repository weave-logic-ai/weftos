//! A project kernel's live governance: boot, reload and parent updates
//! (ADR-103 A6, D8, Phase 2 package E).
//!
//! * [`prepare`] runs early in boot, before any subsystem is built: it reads
//!   the project certificate, the signed parent policy and the overlay, checks
//!   them and merges. Any failure is a boot refusal (fail closed, decision 2);
//!   a project never starts with fewer rules than its parent.
//! * [`Prepared::apply_limits`] enforces the merged `max_processes` (the
//!   process table's cap) and `spawn_budget` (the concurrent sub-agent spawn
//!   cap, `kernel.agent.subagents.max_per_conv`) by lowering the kernel config
//!   before it is used. Both are boot-time caps: a pushed policy that lowers
//!   them reports `restart_required` instead of pretending to apply live.
//! * [`OverlayRuntime`] owns the running state. [`OverlayRuntime::reload`] is
//!   the only thing that re-reads the files; an overlay edited on disk does
//!   nothing until it is called. [`OverlayRuntime::apply_parent_update`]
//!   accepts a pushed policy, which must carry a valid user signature and a
//!   version no older than the newest accepted. Every accepted change appends
//!   `governance.overlay.applied` and publishes the new effective hash to the
//!   chain's `rule_hash` provider; every refused one appends
//!   `governance.overlay.rejected` and leaves the running rules alone.
//!
//! The chain stamps `rule_hash` through a lock-free [`RuleHashCell`]; the gate
//! swap and the cell write happen under the gate's write lock, so a decision
//! is never chained with the other generation's hash. Lock order: the gate
//! lock is taken before the chain lock (a check appends under it); nothing
//! takes the gate lock while holding the chain lock.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

use chrono::Utc;
use clawft_types::config::KernelConfig;
use clawft_types::project::ProjectCert;
use clawft_types::project::canon::{hex_decode, hex_encode};
use clawft_types::runtime_paths::RuntimePaths;
use serde_json::json;

use crate::chain::ChainManager;
use crate::chain_rule_hash::RuleHashCell;
use crate::gate::{GateBackend, GateDecision, GovernanceGate, GovernanceSnapshot};
use crate::governance_overlay::{
    Effective, Overlay, OverlayError, load_overlay, merge, read_capped,
};
use crate::parent_policy::{
    ParentPolicy, ParentPolicyError, load_parent_policy, verify_parent_policy, write_atomic_0600,
};

/// Fallback engine threshold when neither parent nor overlay set one.
pub const DEFAULT_RISK_THRESHOLD: f64 = 0.7;
/// File (under `<root>/.weftos/state/`) holding the newest accepted parent
/// policy version.
pub const VERSION_PIN_FILE: &str = "parent-policy.version";

#[cfg(test)]
thread_local! {
    /// Tests boot with `RuntimePaths::at`; this lets one name a child root.
    pub(crate) static TEST_CHILD_PATHS: std::cell::RefCell<Option<RuntimePaths>> =
        const { std::cell::RefCell::new(None) };
}

/// The child's paths: `paths` itself when it is a child root. A project
/// profile on any other root is refused (package H makes
/// `RuntimePaths::resolve()` return the child root for a spawned child).
pub fn child_paths(paths: &RuntimePaths) -> Result<RuntimePaths, OverlayError> {
    #[cfg(test)]
    if let Some(p) = TEST_CHILD_PATHS.with(|c| c.borrow().clone()) {
        return Ok(p);
    }
    paths.child_id().ok_or(OverlayError::NotAChild)?;
    Ok(paths.clone())
}

/// A gate whose inner [`GovernanceGate`] is replaced atomically on update.
pub struct OverlayGate {
    inner: RwLock<GovernanceGate>,
}

impl OverlayGate {
    fn new(gate: GovernanceGate) -> Self {
        Self {
            inner: RwLock::new(gate),
        }
    }

    /// Replace the rules and publish `hash` in one step (see module docs).
    fn swap(&self, gate: GovernanceGate, hash: [u8; 32], cell: &RuleHashCell) {
        let mut w = self.inner.write().unwrap_or_else(|e| e.into_inner());
        *w = gate;
        cell.set(Some(hash));
    }
}

impl GateBackend for OverlayGate {
    fn check(&self, agent_id: &str, action: &str, context: &serde_json::Value) -> GateDecision {
        // The read lock spans the whole evaluation: a concurrent swap waits.
        self.inner
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .check(agent_id, action, context)
    }

    fn governance_snapshot(&self) -> Option<GovernanceSnapshot> {
        self.inner
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .governance_snapshot()
    }
}

/// The hashes and version of the rules a kernel is running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    /// Hex.
    pub parent_hash: String,
    /// Hex.
    pub overlay_hash: String,
    /// Hex; the chain's `rule_hash`.
    pub effective_hash: String,
    /// Parent policy version.
    pub parent_version: u64,
    /// Rules in the engine.
    pub rule_count: usize,
    /// `max_processes` / `spawn_budget` changed since boot and only a restart
    /// applies them.
    pub restart_required: bool,
}

impl Applied {
    fn of(e: &Effective, boot: &Effective) -> Self {
        let restart_required = e.limits.max_processes != boot.limits.max_processes
            || e.limits.spawn_budget != boot.limits.spawn_budget;
        Self {
            parent_hash: hex_encode(&e.parent_hash),
            overlay_hash: hex_encode(&e.overlay_hash),
            effective_hash: hex_encode(&e.effective_hash),
            parent_version: e.parent_version,
            rule_count: e.rules.len(),
            restart_required,
        }
    }

    /// JSON for RPC replies and chain payloads.
    pub fn to_json(&self) -> serde_json::Value {
        json!({
            "parent_hash": self.parent_hash,
            "overlay_hash": self.overlay_hash,
            "effective_hash": self.effective_hash,
            "parent_version": self.parent_version,
            "rule_count": self.rule_count,
            "restart_required": self.restart_required,
        })
    }
}

/// Everything boot learned about a project's governance, before the chain
/// exists.
pub struct Prepared {
    paths: RuntimePaths,
    user_pubkey: [u8; 32],
    parent: ParentPolicy,
    overlay: Overlay,
    effective: Effective,
    cell: Arc<RuleHashCell>,
}

fn load_user_pubkey(paths: &RuntimePaths) -> Result<[u8; 32], OverlayError> {
    let cert_err = |m: String| OverlayError::Cert(m);
    let path = paths
        .project_cert()
        .ok_or_else(|| cert_err("no certificate path for this root".into()))?;
    let text = read_capped(&path)?
        .ok_or_else(|| cert_err(format!("{} does not exist", path.display())))?;
    let cert: ProjectCert =
        serde_json::from_str(&text).map_err(|e| cert_err(format!("does not parse: {e}")))?;
    let user_pk: [u8; 32] = hex_decode(&cert.user_pubkey)
        .ok_or_else(|| cert_err("`user_pubkey` is not 64 lowercase hex".into()))?;
    cert.verify(&user_pk, Utc::now())
        .map_err(|e| cert_err(e.to_string()))?;
    if Some(cert.project_id.as_str()) != paths.child_id() {
        return Err(cert_err("`project_id` is not this kernel's project".into()));
    }
    // The key beside the certificate must be the certified one.
    if let Some(kp) = paths.project_key()
        && let Ok(bytes) = std::fs::read(&kp)
        && let Ok(seed) = <[u8; 32]>::try_from(bytes.as_slice())
    {
        let pk = ed25519_dalek::SigningKey::from_bytes(&seed)
            .verifying_key()
            .to_bytes();
        if hex_encode(&pk) != cert.project_pubkey {
            return Err(cert_err("project.key is not the certified project key".into()));
        }
    }
    Ok(user_pk)
}

fn read_pin(paths: &RuntimePaths) -> Option<u64> {
    let p = paths.state_dir()?.join(VERSION_PIN_FILE);
    std::fs::read_to_string(p).ok()?.trim().parse().ok()
}

fn write_pin(paths: &RuntimePaths, version: u64) -> Result<(), OverlayError> {
    let p = paths
        .state_dir()
        .ok_or(OverlayError::NotAChild)?
        .join(VERSION_PIN_FILE);
    write_atomic_0600(&p, version.to_string().as_bytes()).map_err(|e| OverlayError::Io {
        path: p.display().to_string(),
        reason: e.to_string(),
    })
}

fn check_version(parent: &ParentPolicy, pinned: Option<u64>) -> Result<(), OverlayError> {
    match pinned {
        Some(pinned) if parent.version < pinned => Err(ParentPolicyError::Rollback {
            have: parent.version,
            pinned,
        }
        .into()),
        _ => Ok(()),
    }
}

/// Read, verify and merge the certificate, parent policy and overlay of the
/// child at `paths`. Fails closed.
pub fn prepare(paths: &RuntimePaths) -> Result<Prepared, OverlayError> {
    let paths = child_paths(paths)?;
    let user_pubkey = load_user_pubkey(&paths)?;
    let parent = load_parent_policy(&paths.parent_policy())?;
    verify_parent_policy(&parent, &user_pubkey)?;
    check_version(&parent, read_pin(&paths))?;
    let overlay = match paths.overlay() {
        Some(p) => load_overlay(&p)?,
        None => Overlay::empty(),
    };
    let effective = merge(&parent, &overlay)?;
    let cell = Arc::new(RuleHashCell::new());
    cell.set(Some(effective.effective_hash));
    Ok(Prepared {
        paths,
        user_pubkey,
        parent,
        overlay,
        effective,
        cell,
    })
}

impl Prepared {
    /// Lower `kc` to the merged limits. A limit can only go down.
    pub fn apply_limits(&self, kc: &mut KernelConfig) {
        let l = &self.effective.limits;
        if let Some(n) = l.max_processes {
            kc.max_processes = kc.max_processes.min(u32::try_from(n).unwrap_or(u32::MAX));
        }
        if let Some(b) = l.spawn_budget {
            let sub = &mut kc.agent.get_or_insert_with(Default::default).subagents;
            sub.max_per_conv = sub.max_per_conv.min(u32::try_from(b).unwrap_or(u32::MAX));
            if b == 0 {
                sub.enabled = false;
            }
        }
    }

    /// The merged rules (they replace the default set).
    pub fn rules(&self) -> &[crate::governance::GovernanceRule] {
        &self.effective.rules
    }

    /// Engine threshold and human-approval flag for the gate.
    pub fn engine_params(&self) -> (f64, bool) {
        (
            self.effective.risk_threshold(DEFAULT_RISK_THRESHOLD),
            self.effective.human_approval(false),
        )
    }

    /// Hashes of what boot merged.
    pub fn applied(&self) -> Applied {
        Applied::of(&self.effective, &self.effective)
    }

    /// Start stamping `rule_hash` on every chain append. Call before the
    /// first governance event so genesis carries it.
    pub fn install_provider(&self, chain: &ChainManager) {
        chain.set_rule_hash_provider(self.cell.provider());
    }

    /// Wrap the boot gate (already built from [`Self::rules`] and
    /// [`Self::engine_params`]) so it can be replaced, and record the
    /// applied rules on the chain.
    pub fn into_runtime(
        self,
        gate: GovernanceGate,
        chain: Arc<ChainManager>,
    ) -> (Arc<dyn GateBackend>, Arc<OverlayRuntime>) {
        let gate = Arc::new(OverlayGate::new(gate));
        let applied = self.applied();
        chain.append(
            "governance",
            "governance.overlay.applied",
            Some(with_source(applied.to_json(), "boot")),
        );
        let rt = Arc::new(OverlayRuntime {
            gate: Arc::clone(&gate),
            cell: self.cell,
            chain,
            paths: self.paths,
            user_pubkey: self.user_pubkey,
            boot: self.effective.clone(),
            state: Mutex::new(State {
                parent: self.parent,
                overlay: self.overlay,
                effective: self.effective,
            }),
        });
        (gate as Arc<dyn GateBackend>, rt)
    }
}

fn with_source(mut v: serde_json::Value, source: &str) -> serde_json::Value {
    v["source"] = json!(source);
    v
}

struct State {
    parent: ParentPolicy,
    overlay: Overlay,
    effective: Effective,
}

/// The running project governance.
pub struct OverlayRuntime {
    gate: Arc<OverlayGate>,
    cell: Arc<RuleHashCell>,
    chain: Arc<ChainManager>,
    paths: RuntimePaths,
    user_pubkey: [u8; 32],
    boot: Effective,
    state: Mutex<State>,
}

impl OverlayRuntime {
    /// The rules this kernel is running.
    pub fn applied(&self) -> Applied {
        let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        Applied::of(&s.effective, &self.boot)
    }

    /// The `rule_hash` the chain is stamping now.
    pub fn rule_hash(&self) -> Option<[u8; 32]> {
        self.cell.get()
    }

    /// Re-read the overlay (and a newer parent policy, if the file holds one)
    /// and apply. The only path by which a disk edit takes effect.
    pub fn reload(&self) -> Result<Applied, OverlayError> {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let result = (|| {
            let overlay = match self.paths.overlay() {
                Some(p) => load_overlay(&p)?,
                None => Overlay::empty(),
            };
            let mut parent = s.parent.clone();
            let disk = load_parent_policy(&self.paths.parent_policy())?;
            verify_parent_policy(&disk, &self.user_pubkey)?;
            // An older (validly signed) file on disk is ignored, not applied.
            if disk.version >= parent.version {
                parent = disk;
            }
            self.apply_locked(&mut s, parent, overlay)
        })();
        self.record(&result, "reload");
        result
    }

    /// Accept a pushed policy: valid user signature, version no older than
    /// the newest accepted, merged with the overlay as last loaded (the file
    /// on disk is not consulted).
    pub fn apply_parent_update(&self, policy: ParentPolicy) -> Result<Applied, OverlayError> {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let result = (|| {
            verify_parent_policy(&policy, &self.user_pubkey)?;
            let overlay = s.overlay.clone();
            self.apply_locked(&mut s, policy, overlay)
        })();
        self.record(&result, "parent.update");
        result
    }

    fn apply_locked(
        &self,
        s: &mut State,
        parent: ParentPolicy,
        overlay: Overlay,
    ) -> Result<Applied, OverlayError> {
        check_version(&parent, Some(s.parent.version))?;
        let effective = merge(&parent, &overlay)?;
        let (threshold, human) = (
            effective.risk_threshold(DEFAULT_RISK_THRESHOLD),
            effective.human_approval(false),
        );
        let mut gate = GovernanceGate::new(threshold, human).with_chain(Arc::clone(&self.chain));
        for r in effective.rules.iter().cloned() {
            gate = gate.add_rule(r);
        }
        if parent.version > s.parent.version {
            write_pin(&self.paths, parent.version)?;
        }
        self.gate.swap(gate, effective.effective_hash, &self.cell);
        let applied = Applied::of(&effective, &self.boot);
        s.parent = parent;
        s.overlay = overlay;
        s.effective = effective;
        Ok(applied)
    }

    fn record(&self, result: &Result<Applied, OverlayError>, source: &str) {
        match result {
            Ok(a) => {
                self.chain.append(
                    "governance",
                    "governance.overlay.applied",
                    Some(with_source(a.to_json(), source)),
                );
            }
            Err(e) => {
                self.chain.append(
                    "governance",
                    "governance.overlay.rejected",
                    Some(json!({"source": source, "key": e.key(), "reason": e.to_string()})),
                );
            }
        }
    }

    /// Path of the overlay file this kernel reads on reload.
    pub fn overlay_path(&self) -> Option<PathBuf> {
        self.paths.overlay()
    }
}
