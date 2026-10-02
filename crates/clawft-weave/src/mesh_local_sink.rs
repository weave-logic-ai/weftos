//! The daemon's two seams to the mesh service (ADR-103 P3-U): inbound
//! `deliver` frames into the A2A router ([`MeshSink`]) and outbound
//! remote-node messages out as `send` ([`ServiceForwarder`]).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use clawft_kernel::a2a::RemoteForwarder;
use clawft_kernel::error::{KernelError, KernelResult};
use clawft_kernel::ipc::KernelMessage;
use clawft_kernel::mesh_delivery::{LocalDelivery, PeerCtx};
use clawft_kernel::mesh_ipc::Scope;
use clawft_mesh_local::proto::Deliver;
use clawft_mesh_local::{Node, WeftAddr};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};

/// How long a forwarded message waits for the service's acknowledgement.
pub const FORWARD_TIMEOUT: Duration = Duration::from_secs(10);
/// Queue between the router and the link task.
pub const OUTBOUND_QUEUE: usize = 256;

/// Delivers what the service hands us into the local router.
pub struct MeshSink {
    delivery: Arc<dyn LocalDelivery>,
    user_id: String,
}

impl MeshSink {
    /// Sink for the user `user_id`, delivering through `delivery`
    /// (the daemon's `A2ARouter`).
    pub fn new(delivery: Arc<dyn LocalDelivery>, user_id: impl Into<String>) -> Self {
        Self { delivery, user_id: user_id.into() }
    }

    /// Deliver one `deliver` frame. A frame addressed to another user is
    /// refused: the service routes by registration, so this only happens on
    /// a service bug, and it must not reach this daemon's agents.
    pub async fn deliver(&self, d: Deliver) -> Result<(), String> {
        if d.scope.user_id != self.user_id {
            return Err(format!("deliver for user {} refused (this daemon is {})", d.scope.user_id, self.user_id));
        }
        let msg: KernelMessage =
            serde_json::from_value(d.message).map_err(|e| format!("malformed delivered message: {e}"))?;
        let scope = Scope { user_id: d.scope.user_id, project_id: d.scope.project_id };
        // The service vouches for the source node, not this process: treat the
        // peer as unverified here (no `node_verified`).
        let ctx = PeerCtx::unauthenticated(d.source_node);
        self.delivery.deliver(&ctx, Some(&scope), msg).await.map_err(|e| e.to_string())
    }
}

/// One outbound message for the link task.
pub struct OutCmd {
    /// Destination (`weft://<node>/_/_/`).
    pub dest: WeftAddr,
    /// The kernel message as JSON.
    pub message: Value,
    /// Result of the `send` request.
    pub reply: oneshot::Sender<Result<(), String>>,
}

/// Sends the router's remote-node messages to the service as `send`.
pub struct ServiceForwarder {
    tx: mpsc::Sender<OutCmd>,
    connected: Arc<AtomicBool>,
}

impl ServiceForwarder {
    /// A forwarder and the receiving end the link task drains.
    pub fn new(connected: Arc<AtomicBool>) -> (Arc<Self>, mpsc::Receiver<OutCmd>) {
        let (tx, rx) = mpsc::channel(OUTBOUND_QUEUE);
        (Arc::new(Self { tx, connected }), rx)
    }
}

#[async_trait::async_trait]
impl RemoteForwarder for ServiceForwarder {
    async fn forward(&self, node_id: &str, msg: KernelMessage) -> KernelResult<()> {
        // Refuse at once while the link is down: queueing remote traffic
        // behind a dead socket would only delay the error.
        if !self.connected.load(Ordering::Acquire) {
            return Err(KernelError::Mesh(format!(
                "cannot reach node '{node_id}': the mesh service link is reconnecting"
            )));
        }
        let dest = WeftAddr::new(Node::Id(node_id.to_owned()), None, None, "")
            .map_err(|e| KernelError::Mesh(format!("bad remote node id '{node_id}': {e}")))?;
        let message = serde_json::to_value(&msg)
            .map_err(|e| KernelError::Mesh(format!("cannot encode message: {e}")))?;
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(OutCmd { dest, message, reply })
            .await
            .map_err(|_| KernelError::Mesh("the mesh service link has shut down".into()))?;
        match tokio::time::timeout(FORWARD_TIMEOUT, rx).await {
            Ok(Ok(Ok(()))) => Ok(()),
            Ok(Ok(Err(why))) => Err(KernelError::Mesh(format!("send to '{node_id}' failed: {why}"))),
            Ok(Err(_)) => Err(KernelError::Mesh("the mesh service link dropped the message".into())),
            Err(_) => Err(KernelError::Mesh(format!("send to '{node_id}' timed out"))),
        }
    }
}
