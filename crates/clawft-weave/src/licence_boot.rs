//! Daemon boot for the Seed licence path (ADR-106 phase 1d): the mesh id from
//! `kernel.mesh.genesis_hash` and `kernel.mesh.mesh_nonce`, the checkout
//! policy (and the store it reads), the chain sink for licence events, and
//! the steward's [`SeedBinder`] over `dir/workload-seed-binds.json`.
//!
//! A node with no `mesh_nonce` has no local mesh id: the policy then equals
//! `ManifestPolicy` and no binding is accepted (the path is inert). A changed
//! nonce or pin makes the stored binding orphaned: the store chains
//! `binding_orphaned` (once, at boot below), turns checkout off, and
//! `weaver doctor` reports it.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use clawft_kernel::chain::ChainManager;
use clawft_kernel::licence::{
    AdmissionPosture, LicenceEvent, LicenceEventSink, LocalMeshId, MeshCheckoutPolicy,
    mesh_id_from_config,
};
use clawft_kernel::revocation::RevocationList;
use clawft_kernel::workload_pkg::TrustAnchors;
use clawft_kernel::workload_runtime::seed_bind::{BIND_STATE_FILE, SeedBinder};
use clawft_types::config::{MeshAdmissionMode, MeshConfig};
use serde_json::{Value, json};

/// Chain source of licence events.
pub const LICENCE_CHAIN_SOURCE: &str = "licence";

/// What the licence RPCs and the policy share, built once at daemon boot.
pub struct LicenceRuntime {
    /// The live local mesh id (unset without a nonce).
    pub local: LocalMeshId,
    /// The policy the artifact exchange uses; owns the grant store.
    pub policy: Arc<MeshCheckoutPolicy>,
    /// The steward's binder, or why it could not be built (an unreadable
    /// replay file: binding then refuses rather than overwrite it).
    pub binder: Result<SeedBinder, String>,
    /// The runtime dir (`workload-seeds.json`, the bind state file).
    pub dir: PathBuf,
    /// `kernel.mesh.genesis_hash` was set.
    pub genesis_pinned: bool,
    /// `kernel.mesh.mesh_nonce` was set.
    pub nonce_set: bool,
    /// The configured pin or nonce is malformed (checkout stays off).
    pub config_error: Option<String>,
    /// This node's id, the steward a binding names.
    pub steward_node_id: String,
    /// This node's request-signing key, 64 hex chars.
    pub steward_pubkey: String,
}

static RUNTIME: OnceLock<Arc<LicenceRuntime>> = OnceLock::new();

/// The daemon's licence runtime (`None` before boot or without placement).
pub fn runtime() -> Option<Arc<LicenceRuntime>> {
    RUNTIME.get().cloned()
}

/// Maps store events to chain events (`binding_orphaned`, `binding_refused`,
/// ...). The payload carries only identifiers.
pub struct ChainEvents(pub Arc<ChainManager>);

impl LicenceEventSink for ChainEvents {
    fn emit(&self, ev: LicenceEvent) {
        let payload = match &ev {
            LicenceEvent::BindingRefused(why) => json!({ "reason": why }),
            LicenceEvent::BindingConflict(seq) => json!({ "seq": seq }),
            LicenceEvent::BindingOrphaned { stored, local } => {
                json!({ "stored_mesh_id": stored, "local_mesh_id": local })
            }
            LicenceEvent::GrantConflict { cog_id, version, seq } => {
                json!({ "cog_id": cog_id, "version": version, "seq": seq })
            }
            LicenceEvent::FloorReset(to) => json!({ "to": to }),
        };
        self.0.append(LICENCE_CHAIN_SOURCE, ev.name(), Some(payload));
    }
}

/// Everything [`init`] reads.
pub struct InitArgs<'a> {
    /// The runtime dir.
    pub dir: &'a Path,
    /// Pinned operator keys (a binding is accepted only from one of them).
    pub anchors: TrustAnchors,
    /// The node's revocation list.
    pub revocations: Arc<RevocationList>,
    /// The kernel chain.
    pub chain: Arc<ChainManager>,
    /// `kernel.mesh`, when configured.
    pub mesh: Option<&'a MeshConfig>,
    /// This node's id.
    pub steward_node_id: String,
    /// This node's request-signing key, 64 hex chars.
    pub steward_pubkey: String,
}

/// Build the licence runtime: derive the mesh id, set it on the shared
/// handle, open the store (poisoned, not fatal, on a bad file), route its
/// events to the chain, and look at the binding once so an orphaned one is
/// chained at boot, not at first use.
pub fn build(a: InitArgs<'_>) -> LicenceRuntime {
    let genesis = a.mesh.and_then(|m| m.genesis_hash.as_deref());
    let nonce = a.mesh.and_then(|m| m.mesh_nonce.as_deref());
    let local = LocalMeshId::unset();
    let mut config_error = None;
    match mesh_id_from_config(genesis, nonce) {
        Ok(id) => local.set(id),
        Err(e) => {
            tracing::warn!(error = %e, "mesh id not derived; Seed checkout stays off");
            config_error = Some(e.to_string());
        }
    }
    let policy = MeshCheckoutPolicy::open(a.dir, a.anchors, a.revocations, local.clone());
    policy.store().set_sink(Arc::new(ChainEvents(a.chain.clone())));
    let _ = policy.store().binding_status(); // chains binding_orphaned now
    let binder = SeedBinder::new(Vec::new(), a.chain)
        .with_state_file(a.dir.join(BIND_STATE_FILE))
        .map_err(|e| e.to_string());
    if let Err(why) = &binder {
        tracing::warn!(%why, "Seed bind state unreadable; workload.node.bind refuses");
    }
    LicenceRuntime {
        local,
        policy,
        binder,
        dir: a.dir.to_owned(),
        genesis_pinned: genesis.is_some(),
        nonce_set: nonce.is_some(),
        config_error,
        steward_node_id: a.steward_node_id,
        steward_pubkey: a.steward_pubkey,
    }
}

/// Install the daemon's runtime (first call wins).
pub fn install(rt: LicenceRuntime) -> Arc<LicenceRuntime> {
    let rt = Arc::new(rt);
    let _ = RUNTIME.set(rt.clone());
    RUNTIME.get().cloned().unwrap_or(rt)
}

/// The node's admission state, as a binding sees it: enforce, a governance
/// verdict source bound, open membership off.
pub fn posture(mesh: Option<&MeshConfig>, gate_bound: bool) -> AdmissionPosture {
    AdmissionPosture {
        enforce: mesh.is_some_and(|m| m.admission == MeshAdmissionMode::Enforce),
        verdict_source_bound: gate_bound,
        open_membership: mesh.is_some_and(|m| m.admission_open_membership),
    }
}

/// `{mesh_id, genesis_pinned, nonce_set, ...}` for `workload.node.binding`.
pub fn status(rt: &LicenceRuntime) -> Value {
    let store = rt.policy.store();
    let _ = store.binding_status(); // chains an orphan once, if not yet
    let local = rt.local.get().map(|m| m.to_hex());
    let held = store.held_binding();
    let binding = held.as_ref().map(|h| {
        let orphaned = local.as_deref().is_some_and(|l| l != h.mesh_id);
        json!({
            "state": h.state,
            "mesh_id": h.mesh_id,
            "seq": h.seq,
            "device_id": h.device_id,
            "grant_pubkey": h.grant_pubkey,
            "grant_fingerprint": clawft_kernel::workload_runtime::grant_fingerprint(&h.grant_pubkey),
            "steward_node_id": h.steward_node_id,
            "orphaned": orphaned,
            "record": h,
        })
    });
    json!({
        "mesh_id": local,
        "genesis_pinned": rt.genesis_pinned,
        "nonce_set": rt.nonce_set,
        "config_error": rt.config_error,
        "poisoned": store.poisoned(),
        "binding": binding,
        "next_seq": held.as_ref().map_or(1, |h| h.seq + 1),
        "steward": { "node_id": rt.steward_node_id, "pubkey": rt.steward_pubkey },
    })
}
