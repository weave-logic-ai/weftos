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
//! (from a reply, flood or sync) makes shareable. The relay's [`GrantFlood`]
//! is [`grant_flood`]: the node's `LicenceExchange` once placement has started
//! it (phase 1b), `NoFlood` before.
//!
//! In service mode the licence exchange runs here too (ADR-106 phase 3): the
//! wrap also takes `mesh.cog.binding`, `mesh.cog.grant` and `mesh.cog.sync`
//! for the node's [`ServiceLicenceLinks`] ([`licence_links`]), which send
//! through the service link and read the service's peer view. The exchange is
//! started over them only when the kernel runs no mesh of its own, so a
//! record never arrives by two paths.
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
use clawft_kernel::licence::{
    CheckoutGrantStore, GrantFlood, NoFlood, PEER_REFRESH, PeerDirectory, PeerSnapshot,
    ServiceLicenceLinks,
};
use clawft_kernel::mesh_artifact::ArtifactExchange;
use clawft_kernel::mesh_artifact_tunnel::PeerSender;
use clawft_kernel::mesh_cog::{CogMesh, CogMeshDelivery, CogMeshSlot};
use clawft_kernel::mesh_delivery::LocalDelivery;

use crate::mesh_local_sink::ServiceForwarder;

static SLOT: OnceLock<Arc<CogMeshSlot>> = OnceLock::new();
static LINK: OnceLock<Arc<ForwarderLink>> = OnceLock::new();
static LICENCE: OnceLock<Arc<ServiceLicenceLinks>> = OnceLock::new();
static FLOOD: OnceLock<Arc<dyn GrantFlood>> = OnceLock::new();

fn slot() -> &'static Arc<CogMeshSlot> {
    SLOT.get_or_init(Arc::default)
}

fn link() -> &'static Arc<ForwarderLink> {
    LINK.get_or_init(Arc::default)
}

/// The service link as the cog mesh sees it: sends, and the service's peer
/// view. Late-bound: the link starts after the delivery wrap is built.
#[derive(Default)]
pub struct ForwarderLink(OnceLock<Arc<ServiceForwarder>>);

impl ForwarderLink {
    /// Bind to the link's forwarder (first call wins).
    pub fn bind(&self, f: Arc<ServiceForwarder>) {
        let _ = self.0.set(f);
    }

    fn forwarder(&self) -> Result<&Arc<ServiceForwarder>, String> {
        self.0.get().ok_or_else(|| "the mesh service link is not up".to_string())
    }
}

#[async_trait]
impl PeerSender for ForwarderLink {
    async fn send_to_node(&self, node_id: &str, msg: KernelMessage) -> KernelResult<()> {
        let f = self.forwarder().map_err(KernelError::Mesh)?;
        f.forward(node_id, msg).await
    }
}

#[async_trait]
impl PeerDirectory for ForwarderLink {
    async fn peers(&self) -> Result<PeerSnapshot, String> {
        let v = self.forwarder()?.peers().await?;
        Ok(snapshot_of(&v))
    }
}

/// `peers.list` data as a snapshot. A service without the `licensed` list
/// (older than ADR-106 phase 3) licenses nobody: no floods, sync only.
pub fn snapshot_of(v: &serde_json::Value) -> PeerSnapshot {
    let ids = |k: &str| -> Vec<String> {
        v.get(k)
            .and_then(serde_json::Value::as_array)
            .map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_owned)).collect())
            .unwrap_or_default()
    };
    PeerSnapshot {
        connected: ids("connected"),
        licensed: ids("licensed"),
        reserved_holder: v.get("reserved_holder").and_then(serde_json::Value::as_bool),
    }
}

/// True when this daemon's registration holds the service's reserved topics
/// (the cluster owner's daemon): only it runs the licence path. A service
/// that does not say (older than ADR-106 phase 3) forwards no licence
/// records either, so its answer counts as no.
pub async fn reserved_holder() -> Result<bool, String> {
    let v = link().forwarder()?.peers().await?;
    Ok(v.get("reserved_holder").and_then(serde_json::Value::as_bool).unwrap_or(false))
}

/// The node's licence links over the service link (service mode).
pub fn licence_links() -> Arc<ServiceLicenceLinks> {
    LICENCE
        .get_or_init(|| ServiceLicenceLinks::new(link().clone(), link().clone()))
        .clone()
}

/// The licence links, once the service link is up (`None` in collapsed mode).
pub fn service_licence_links() -> Option<Arc<ServiceLicenceLinks>> {
    link().0.get()?;
    Some(licence_links())
}

/// Put the cog mesh router in front of `delivery` (the daemon's router),
/// with the node's licence links.
pub fn wrap(delivery: Arc<dyn LocalDelivery>) -> Arc<dyn LocalDelivery> {
    Arc::new(CogMeshDelivery::new(delivery, slot().clone()).with_licence(licence_links()))
}

/// Remember the service link's forwarder for outbound frames and start
/// refreshing the licence links' peer view.
pub fn set_forwarder(f: Arc<ServiceForwarder>) {
    if link().0.set(f).is_ok() {
        let _ = licence_links().spawn_refresh(PEER_REFRESH);
    }
}

/// Set the flood a steward relay hands its grants to (the node's licence
/// exchange). First call wins.
pub fn set_grant_flood(f: Arc<dyn GrantFlood>) {
    let _ = FLOOD.set(f);
}

/// The flood for a steward relay on this node: the licence exchange once it
/// is started, [`NoFlood`] before.
pub fn grant_flood() -> Arc<dyn GrantFlood> {
    FLOOD.get().cloned().unwrap_or_else(|| Arc::new(NoFlood))
}

/// Install this node's cog mesh once its exchange and grant store exist, and
/// make the bytes of the grants already held shareable again (a restart).
/// Returns the cog mesh, whose tunnel dialer is the production
/// [`clawft_kernel::mesh_swarm_fetch::PeerDialer`].
pub fn install(exchange: &Arc<ArtifactExchange>, store: &Arc<CheckoutGrantStore>) -> Arc<CogMesh> {
    for v in store.verified_grants() {
        exchange.grant_checkout(&v);
    }
    let mesh = CogMesh::new(exchange.clone(), store.clone(), link().clone(), None);
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
