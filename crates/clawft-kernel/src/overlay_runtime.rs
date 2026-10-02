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

use clawft_types::config::KernelConfig;
use clawft_types::project::canon::hex_encode;
use clawft_types::runtime_paths::RuntimePaths;
use serde_json::json;

use crate::chain::ChainManager;
use crate::chain_rule_hash::RuleHashCell;
use crate::gate::{GateBackend, GateDecision, GovernanceGate, GovernanceSnapshot};
use crate::governance_overlay::{
    Effective, Overlay, OverlayError, load_overlay, merge,
};
pub use crate::overlay_trust::{REVOKED_FILE, USER_PIN_FILE, VERSION_PIN_FILE, write_user_pin};
use crate::overlay_trust::{
    check_version, last_applied_overlay_hash, load_user_pubkey, read_pin, write_pin,
};
use crate::parent_policy::{
    ParentPolicy, load_parent_policy, verify_parent_policy,
};

/// Fallback engine threshold when neither parent nor overlay set one.
pub const DEFAULT_RISK_THRESHOLD: f64 = 0.7;
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
    overlay_present: bool,
    pinned: Option<u64>,
    effective: Effective,
    cell: Arc<RuleHashCell>,
}

/// Read, verify and merge the certificate, parent policy and overlay of the
/// child at `paths`. Fails closed.
pub fn prepare(paths: &RuntimePaths) -> Result<Prepared, OverlayError> {
    let paths = child_paths(paths)?;
    let user_pubkey = load_user_pubkey(&paths)?;
    let parent = load_parent_policy(&paths.parent_policy())?;
    verify_parent_policy(&parent, &user_pubkey)?;
    let pinned = read_pin(&paths)?;
    check_version(&parent, pinned)?;
    let overlay_present = paths.overlay().is_some_and(|p| p.exists());
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
        overlay_present,
        pinned,
        effective,
        cell,
    })
}

impl Prepared {
    /// Boot-time history checks against the project chain, then persist the
    /// rollback pin. Call once the chain is restored and before anything is
    /// appended to it.
    ///
    /// * the pin is missing but the chain records an applied overlay: someone
    ///   removed it to roll the parent policy back, so boot is refused;
    /// * `overlay.toml` is missing but the last applied overlay was not empty:
    ///   deleting the file must not clear the overlay (an empty file does).
    pub fn commit(&self, chain: &ChainManager) -> Result<(), OverlayError> {
        let last = last_applied_overlay_hash(chain);
        if self.pinned.is_none() && last.is_some() {
            return Err(OverlayError::PinMissing);
        }
        let empty = hex_encode(&Overlay::empty().hash);
        if let Some(h) = &last
            && !self.overlay_present
            && *h != empty
        {
            return Err(OverlayError::OverlayMissing);
        }
        if self.pinned.is_none_or(|p| self.parent.version > p) {
            write_pin(&self.paths, self.parent.version)?;
        }
        Ok(())
    }

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

    /// Re-check that the trust root is unchanged: certificate, expiry,
    /// revocation and the user-key pin (see [`load_user_pubkey`]).
    fn recheck_trust(&self) -> Result<(), OverlayError> {
        let pk = load_user_pubkey(&self.paths)?;
        if pk != self.user_pubkey {
            return Err(OverlayError::Cert(
                "the trusted user key changed since boot; restart the kernel".into(),
            ));
        }
        Ok(())
    }

    /// Re-read the overlay (and a newer parent policy, if the file holds one)
    /// and apply. The only path by which a disk edit takes effect. A deleted
    /// overlay file is refused once a non-empty overlay is in force; an empty
    /// file is the deliberate way to clear it.
    pub fn reload(&self) -> Result<Applied, OverlayError> {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let result = (|| {
            self.recheck_trust()?;
            let overlay = match self.paths.overlay() {
                Some(p) if p.exists() => load_overlay(&p)?,
                _ if s.overlay != Overlay::empty() => return Err(OverlayError::OverlayMissing),
                _ => Overlay::empty(),
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
    /// on disk is not consulted). The project must still be unrevoked and its
    /// certificate unexpired.
    pub fn apply_parent_update(&self, policy: ParentPolicy) -> Result<Applied, OverlayError> {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let result = (|| {
            self.recheck_trust()?;
            verify_parent_policy(&policy, &self.user_pubkey)?;
            let overlay = s.overlay.clone();
            self.apply_locked(&mut s, policy, overlay)
        })();
        self.record(&result, "parent.update");
        result
    }

    /// The effective rules and engine parameters now in force.
    pub fn effective_rules(&self) -> (Vec<crate::governance::GovernanceRule>, f64, bool) {
        let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        (
            s.effective.rules.clone(),
            s.effective.risk_threshold(DEFAULT_RISK_THRESHOLD),
            s.effective.human_approval(false),
        )
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
