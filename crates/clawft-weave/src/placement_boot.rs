//! Who signs placement and licence requests, and their start at daemon boot
//! (ADR-106 phase 3, decision "service-mode signer").
//!
//! - **Collapsed** (the daemon holds `<runtime>/node.key`): the node key, as
//!   before. The mesh id and the signer's id are the same.
//! - **Service mode** (ADR-103 P3-U: the box key belongs to the machine mesh
//!   service): a daemon-local **placement control key**,
//!   `<runtime>/control.key` (0600, generated on first use). It signs
//!   `workload.ctl` requests and the in-process workload-host's replies, the
//!   steward's `weft-licence` requests, and revocation notices only where an
//!   operator pinned it as a revoking key. It never signs as the machine:
//!   not the admission hello, not the machine's facts, not the chain, not a
//!   user certificate. Nothing trusts it unless an operator names it
//!   (`workload-host.json` `controllers`, `workload-peers.json`, the Seed
//!   binding's `steward_pubkey`, which the operator signs). The mesh id stays
//!   the machine's: a binding names it as `steward_node_id`, and peers
//!   address this node's swarm by it.
//!
//! In service mode the placement controller's id (what `workload.status`
//! shows as `controller`, and what targets see) is the control key's id, not
//! the machine's node id.
//!
//! [`start`] then installs placement, the licence runtime and the licence
//! exchange (over the kernel's mesh when it has one, over the service links
//! otherwise), which before phase 3 never ran in service mode. In service
//! mode only the daemon that holds the service's reserved topics (the cluster
//! owner's) runs the licence path; another tenant's daemon places but runs no
//! licence runtime, exchange or binder.

use std::path::Path;
use std::sync::Arc;

use clawft_kernel::boot::Kernel;
use clawft_platform::NativePlatform;
use ed25519_dalek::SigningKey;
use tokio::sync::RwLock;
use serde_json::json;
use tracing::warn;

use crate::node_identity::{DaemonIdentity, IdentityError};

/// The placement control key under the runtime dir, in service mode.
pub const CONTROL_KEY_FILE: &str = "control.key";

/// What placement and the licence steward sign with, and as whom.
pub struct PlacementSigner {
    /// The signing key.
    pub key: SigningKey,
    /// The node id peers address this node by on the mesh.
    pub mesh_node_id: String,
    /// The key is the service-mode control key, not the node key.
    pub control_key: bool,
}

impl PlacementSigner {
    /// The signing key's public half, 64 hex chars.
    pub fn pubkey_hex(&self) -> String {
        hex::encode(self.key.verifying_key().to_bytes())
    }
}

/// The signer for `identity`. Service mode loads (or creates) the control
/// key; it never reads `<runtime>/node.key`.
pub fn signer(identity: &DaemonIdentity, runtime_dir: &Path) -> Result<PlacementSigner, String> {
    match identity.signing_key() {
        Ok(k) => Ok(PlacementSigner { key: k.clone(), mesh_node_id: identity.node_id.clone(), control_key: false }),
        Err(IdentityError::KeyHeldByService) => {
            let key = clawft_kernel::load_or_generate_key_file(runtime_dir, CONTROL_KEY_FILE)
                .map_err(|e| format!("placement control key: {e}"))?;
            if key.verifying_key().to_bytes() == identity.public_key() {
                // Only a copied box key could do this; it must never sign here.
                return Err(format!("{CONTROL_KEY_FILE} holds the machine key; refusing to use it"));
            }
            Ok(PlacementSigner { key, mesh_node_id: identity.node_id.clone(), control_key: true })
        }
        Err(e) => Err(e.to_string()),
    }
}

/// Daemon boot: placement, the licence runtime and the licence exchange.
pub async fn start(
    kernel: &Arc<RwLock<Kernel<NativePlatform>>>,
    identity: &DaemonIdentity,
    runtime_dir: &Path,
) {
    let s = match signer(identity, runtime_dir) {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, "placement control plane disabled");
            crate::licence_boot::skipped(kernel.read().await.kernel_config().mesh.as_ref(), "no placement signing key");
            return;
        }
    };
    if s.control_key {
        tracing::info!(controller = %s.pubkey_hex(), "service mode: placement and the licence steward sign with the control key");
    }
    crate::workload_place_rpc::init_with_mesh_id(s.key.clone(), runtime_dir.to_path_buf(), s.mesh_node_id.clone());
    let holder = if identity.is_service() { Some(service_holder().await) } else { None };
    crate::licence_boot::set_reserved_holder(holder);
    if holder == Some(false) {
        warn!("{}", crate::licence_boot::NOT_HOLDER);
        return;
    }
    // ADR-106: the mesh id from the configured nonce, the checkout policy and
    // the steward binder. Built here (not lazily) so a changed nonce chains
    // `binding_orphaned` at boot.
    let k = kernel.read().await;
    let Some(chain) = k.chain_manager().cloned() else {
        crate::licence_boot::skipped(k.kernel_config().mesh.as_ref(), "no chain manager");
        return;
    };
    let anchors = crate::workload_place_policy::load_anchors(runtime_dir).unwrap_or_else(|e| {
        warn!(error = %e, "operator keys unreadable; no Seed binding can verify");
        Default::default()
    });
    let rt = crate::licence_boot::install(crate::licence_boot::build(crate::licence_boot::InitArgs {
        dir: runtime_dir,
        anchors: anchors.clone(),
        revocations: k.revocation_list().clone(),
        chain: chain.clone(),
        mesh: k.kernel_config().mesh.as_ref(),
        steward_node_id: s.mesh_node_id.clone(),
        steward_pubkey: s.pubkey_hex(),
    }));
    // The exchange from boot, not from the first placement call: a member
    // that never places still takes and passes on bindings and grants.
    check_steward_key(&rt, &s);
    let posture = crate::licence_boot::posture(k.kernel_config().mesh.as_ref(), k.governance_gate().is_some());
    let links = crate::workload_place_rpc::licence_links(k.a2a_router().mesh_runtime().cloned());
    drop(k);
    let started = crate::workload_place_rpc::ensure_licence(
        runtime_dir,
        Some(rt.policy()),
        &anchors,
        &chain,
        links,
        Arc::new(move || posture),
    );
    if started.is_none() {
        if identity.is_service() {
            warn!("service mode: the licence exchange did not start (no mesh service link); no bindings or grants reach this node");
        } else {
            tracing::debug!("no kernel mesh: the licence exchange is not started");
        }
    }
}

/// Ask the service whether this daemon holds the reserved topics, a few
/// times while the link settles. Fails closed (not the holder).
async fn service_holder() -> bool {
    let mut last = String::new();
    for _ in 0..5 {
        match crate::cog_swarm::reserved_holder().await {
            Ok(v) => return v,
            Err(e) => last = e,
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    warn!(error = %last, "could not ask the mesh service who holds the licence topics; the licence path stays off");
    false
}

/// A held binding that names this node as steward under another key: the
/// control key was replaced (or this is another daemon's binding). The Seed
/// refuses every request until the operator rebinds with the current key.
fn check_steward_key(rt: &crate::licence_boot::LicenceRuntime, s: &PlacementSigner) {
    let Some(h) = rt.store().held_binding() else { return };
    let ours = s.pubkey_hex();
    if h.state == clawft_kernel::licence::BindState::Bound
        && h.steward_node_id == s.mesh_node_id
        && h.steward_pubkey != ours
    {
        tracing::error!(held = %h.steward_pubkey, signer = %ours,
            "the Seed binding names this node as steward with a DIFFERENT key: steward requests will be refused; \
             rebind (`weaver workload node bind`) with the current key");
        rt.chain.append(
            crate::licence_boot::LICENCE_CHAIN_SOURCE,
            &crate::licence_boot::licence_kind("steward_key_mismatch"),
            Some(json!({ "held": h.steward_pubkey, "signer": ours, "seq": h.seq })),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapsed_signs_with_the_node_key_and_service_mode_with_a_separate_control_key() {
        let tmp = tempfile::tempdir().unwrap();
        let node = SigningKey::from_bytes(&[5; 32]);
        let local = DaemonIdentity::local(node.clone());
        let s = signer(&local, tmp.path()).unwrap();
        assert!(!s.control_key);
        assert_eq!(s.key.to_bytes(), node.to_bytes());
        assert_eq!(s.mesh_node_id, local.node_id);
        assert!(!tmp.path().join(CONTROL_KEY_FILE).exists(), "collapsed mode never makes a control key");

        let machine = SigningKey::from_bytes(&[9; 32]).verifying_key().to_bytes();
        let id = clawft_kernel::node_id_from_pubkey(&machine);
        let svc = DaemonIdentity::for_service(id.clone(), machine).unwrap();
        let a = signer(&svc, tmp.path()).unwrap();
        assert!(a.control_key);
        assert_eq!(a.mesh_node_id, id, "the mesh id stays the machine's");
        assert_ne!(a.key.verifying_key().to_bytes(), machine);
        assert!(!tmp.path().join(clawft_kernel::NODE_KEY_FILE).exists(), "service mode never reads or makes node.key");
        // Stable across restarts, private on disk.
        let b = signer(&svc, tmp.path()).unwrap();
        assert_eq!(a.key.to_bytes(), b.key.to_bytes());
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(tmp.path().join(CONTROL_KEY_FILE)).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
