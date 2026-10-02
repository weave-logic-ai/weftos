//! How the mesh hands inbound messages to the local side (P3-K0, K1).
//!
//! The in-kernel mesh delivers into the [`A2ARouter`]. A machine mesh
//! service has no kernel router; it implements [`LocalDelivery`] to route
//! to tenant registrations instead. [`MeshRuntime`](crate::mesh_runtime::MeshRuntime)
//! only sees this trait.
//!
//! # Trust
//!
//! Everything a peer puts in an envelope is untrusted input. In
//! particular `dest_scope` is only a *request* for an address and
//! `src_scope` only a claim; neither is evidence of tenancy. What the
//! implementor may rely on is [`PeerCtx`]: it is built by the kernel from
//! the connection (admission result, Noise static key), never from
//! envelope fields.

use async_trait::async_trait;

use crate::a2a::A2ARouter;
use crate::error::KernelResult;
use crate::ipc::KernelMessage;
use crate::mesh_admit::PeerClass;
use crate::mesh_ipc::Scope;

/// Who delivered a message, as established by the connection.
#[derive(Debug, Clone)]
pub struct PeerCtx {
    /// The peer's node id. When [`node_verified`](Self::node_verified)
    /// this is the id admission verified; otherwise it is merely what the
    /// peer claimed in `source_node`.
    pub peer_id: String,
    /// True when admission verified `peer_id` (signed hello bound to the
    /// Noise session). False for legacy/plaintext peers and in `off` mode.
    pub node_verified: bool,
    /// Class admission assigned.
    pub class: PeerClass,
    /// The peer's Noise static public key, when the channel had one.
    pub remote_static: Option<Vec<u8>>,
    /// The envelope's `src_scope` claim. Already stripped by the listener
    /// unless the peer is verified; still unproven until the receiver
    /// checks a certificate chain.
    pub src_scope: Option<Scope>,
}

impl PeerCtx {
    /// A peer with no authenticated identity (embedded use, tests, `off`
    /// mode): `peer_id` is only the claimed source node.
    pub fn unauthenticated(peer_id: impl Into<String>) -> Self {
        Self {
            peer_id: peer_id.into(),
            node_verified: false,
            class: PeerClass::Legacy,
            remote_static: None,
            src_scope: None,
        }
    }
}

/// Sink for messages that arrived from a mesh peer and are bound for a
/// local recipient.
#[async_trait]
pub trait LocalDelivery: Send + Sync + 'static {
    /// Deliver `msg` from `from`. `dest_scope` is the envelope's
    /// `dest_scope` as supplied by the peer: **untrusted**. Implementations
    /// that have no tenants ignore it.
    async fn deliver(
        &self,
        from: &PeerCtx,
        dest_scope: Option<&Scope>,
        msg: KernelMessage,
    ) -> KernelResult<()>;

    /// May `from` subscribe to `topic` (a `mesh.subscribe` control
    /// message, which is consumed before delivery)? Default: yes.
    async fn authorize_subscribe(
        &self,
        _from: &PeerCtx,
        _topic: &str,
        _dest_scope: Option<&Scope>,
    ) -> bool {
        true
    }
}

/// The collapsed (in-kernel) mode: scopes do not exist, the router decides.
#[async_trait]
impl LocalDelivery for A2ARouter {
    async fn deliver(
        &self,
        _from: &PeerCtx,
        _scope: Option<&Scope>,
        msg: KernelMessage,
    ) -> KernelResult<()> {
        self.send(msg).await
    }
}
