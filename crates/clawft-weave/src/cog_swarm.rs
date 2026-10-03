//! Daemon wiring of the cog mesh (ADR-106 phase 1c): the artifact tunnel and
//! the `mesh.cog.checkout` handler, fed by service-stamped deliveries.
//!
//! The mesh link starts at boot, before placement builds the artifact
//! exchange, so both ends are late-bound: [`wrap`] puts the feature router in
//! front of the daemon's `A2ARouter` at link time, and [`install`] fills its
//! slot once the exchange exists. Until then feature topics are dropped.
//!
//! The daemon runs no steward relay yet: the real `weft-licence` link arrives
//! with phase 2, so a request to this node's `mesh.cog.checkout` is answered
//! `no_steward`. Members can still fetch from peers and serve what a grant
//! (from a reply, flood or sync) makes shareable.
//!
//! The service reserves the `mesh.cog.`, `mesh.artifact.` and `mesh.licence.`
//! topics for the cluster owner's registration (the only one with no
//! configured owner): it routes them only there, whatever scope or prefix a
//! delivery or another tenant names, refuses other tenants' prefix claims on
//! them, and refuses other tenants' sends of them. A daemon needs to register
//! nothing to receive these topics, on a single-user or a shared machine.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use clawft_kernel::a2a::RemoteForwarder;
use clawft_kernel::error::{KernelError, KernelResult};
use clawft_kernel::ipc::KernelMessage;
use clawft_kernel::licence::CheckoutGrantStore;
use clawft_kernel::mesh_artifact::ArtifactExchange;
use clawft_kernel::mesh_artifact_tunnel::PeerSender;
use clawft_kernel::mesh_cog::{CogMesh, CogMeshDelivery, CogMeshSlot};
use clawft_kernel::mesh_delivery::LocalDelivery;

use crate::mesh_local_sink::ServiceForwarder;

static SLOT: OnceLock<Arc<CogMeshSlot>> = OnceLock::new();
static FORWARDER: OnceLock<Arc<ServiceForwarder>> = OnceLock::new();

fn slot() -> &'static Arc<CogMeshSlot> {
    SLOT.get_or_init(Arc::default)
}

/// Sends through the service link once it is up.
struct DaemonSender;

#[async_trait]
impl PeerSender for DaemonSender {
    async fn send_to_node(&self, node_id: &str, msg: KernelMessage) -> KernelResult<()> {
        match FORWARDER.get() {
            Some(f) => f.forward(node_id, msg).await,
            None => Err(KernelError::Mesh("the mesh service link is not up".into())),
        }
    }
}

/// Put the cog mesh router in front of `delivery` (the daemon's router).
pub fn wrap(delivery: Arc<dyn LocalDelivery>) -> Arc<dyn LocalDelivery> {
    Arc::new(CogMeshDelivery::new(delivery, slot().clone()))
}

/// Remember the service link's forwarder for outbound tunnel frames.
pub fn set_forwarder(f: Arc<ServiceForwarder>) {
    let _ = FORWARDER.set(f);
}

/// Install this node's cog mesh once its exchange and grant store exist, and
/// make the bytes of the grants already held shareable again (a restart).
/// Returns the cog mesh, whose tunnel dialer is the production
/// [`clawft_kernel::mesh_swarm_fetch::PeerDialer`].
pub fn install(exchange: &Arc<ArtifactExchange>, store: &Arc<CheckoutGrantStore>) -> Arc<CogMesh> {
    for v in store.verified_grants() {
        exchange.grant_checkout(&v);
    }
    let mesh = CogMesh::new(exchange.clone(), store.clone(), Arc::new(DaemonSender), None);
    if !slot().install(mesh.clone()) {
        tracing::debug!("cog mesh already installed; keeping the first");
        if let Some(first) = slot().get() {
            return first.clone();
        }
    }
    mesh
}

/// The installed cog mesh, if placement has built it.
pub fn get() -> Option<Arc<CogMesh>> {
    slot().get().cloned()
}

/// [`wrap`] for any concrete delivery (the daemon's router, or a test inbox).
pub fn wrap_inbox<D: LocalDelivery>(delivery: Arc<D>) -> Arc<dyn LocalDelivery> {
    wrap(delivery)
}
