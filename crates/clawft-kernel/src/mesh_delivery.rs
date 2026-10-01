//! How the mesh hands inbound messages to the local side (P3-K0).
//!
//! The in-kernel mesh delivers into the [`A2ARouter`]. A machine mesh
//! service has no kernel router; it implements [`LocalDelivery`] to route
//! to tenant registrations instead. [`MeshRuntime`](crate::mesh_runtime::MeshRuntime)
//! only sees this trait.

use async_trait::async_trait;

use crate::a2a::A2ARouter;
use crate::error::KernelResult;
use crate::ipc::KernelMessage;
use crate::mesh_ipc::Scope;

/// Sink for messages that arrived from a mesh peer and are bound for a
/// local recipient.
#[async_trait]
pub trait LocalDelivery: Send + Sync + 'static {
    /// Deliver `msg`. `scope` is the envelope's `dest_scope`, when the
    /// sender supplied one. Implementations that have no tenants ignore it.
    async fn deliver(&self, scope: Option<&Scope>, msg: KernelMessage) -> KernelResult<()>;
}

/// The collapsed (in-kernel) mode: scopes do not exist, the router decides.
#[async_trait]
impl LocalDelivery for A2ARouter {
    async fn deliver(&self, _scope: Option<&Scope>, msg: KernelMessage) -> KernelResult<()> {
        self.send(msg).await
    }
}
