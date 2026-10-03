//! What the licence exchange needs from a mesh (ADR-106 phase 3, service
//! mode): a way to receive the three control topics, the peers to sync with,
//! which of them may be sent floods, a send, and peer-join events.
//!
//! Two implementations:
//!
//! - [`MeshRuntime`], the kernel's own mesh (collapsed mode): the control
//!   topics arrive as runtime control sinks, and the licensed predicate is the
//!   admitted class of the route.
//! - [`super::ServiceLicenceLinks`], the machine mesh service (service mode):
//!   the control topics arrive as service-stamped deliveries, sends go out
//!   through the service link, and the peer view is the service's.
//!
//! An exchange is started over exactly one of them, so a node never takes the
//! same record from two paths.

use std::sync::Arc;

use async_trait::async_trait;

use crate::error::KernelResult;
use crate::ipc::KernelMessage;
use crate::mesh_discovery::MeshPeerEvent;
use crate::mesh_runtime::{MeshRuntime, PeerControlSink};

/// The mesh a [`super::LicenceExchange`] floods and syncs over.
#[async_trait]
pub trait LicenceLinks: Send + Sync + 'static {
    /// Receive `topic` with `sink` (first registration wins).
    fn set_control_sink(&self, topic: &str, sink: Arc<dyn PeerControlSink>);

    /// Peers to ask for a catch-up sync (the answering side checks admission).
    fn peer_ids(&self) -> Vec<String>;

    /// True when `peer` may be sent bindings and grants: verified by
    /// admission and of class `node`.
    fn peer_licensed(&self, peer: &str) -> bool;

    /// Send `msg` to `peer`.
    async fn route_to_remote(&self, peer: &str, msg: KernelMessage) -> KernelResult<()>;

    /// Peer-join events, for the sync on connect (`None`: this mesh has none
    /// and only the periodic sync runs).
    fn subscribe_peer_events(&self) -> Option<tokio::sync::broadcast::Receiver<MeshPeerEvent>>;
}

#[async_trait]
impl LicenceLinks for MeshRuntime {
    fn set_control_sink(&self, topic: &str, sink: Arc<dyn PeerControlSink>) {
        MeshRuntime::set_control_sink(self, topic, sink);
    }

    fn peer_ids(&self) -> Vec<String> {
        MeshRuntime::peer_ids(self)
    }

    fn peer_licensed(&self, peer: &str) -> bool {
        MeshRuntime::peer_licensed(self, peer)
    }

    async fn route_to_remote(&self, peer: &str, msg: KernelMessage) -> KernelResult<()> {
        MeshRuntime::route_to_remote(self, peer, msg).await
    }

    fn subscribe_peer_events(&self) -> Option<tokio::sync::broadcast::Receiver<MeshPeerEvent>> {
        Some(MeshRuntime::subscribe_peer_events(self))
    }
}
