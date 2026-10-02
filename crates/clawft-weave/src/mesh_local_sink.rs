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
    machine_key: Option<[u8; 32]>,
}

impl MeshSink {
    /// Sink for the user `user_id`, delivering through `delivery`
    /// (the daemon's `A2ARouter`).
    pub fn new(delivery: Arc<dyn LocalDelivery>, user_id: impl Into<String>) -> Self {
        Self { delivery, user_id: user_id.into(), machine_key: None }
    }

    /// Trust sender certificates issued by this machine key (the service's,
    /// already verified at hello). A delivery whose `source_cert` verifies
    /// against it carries the sender's user id as `src_scope`.
    pub fn with_machine_key(mut self, key: [u8; 32]) -> Self {
        self.machine_key = Some(key);
        self
    }

    /// The sender's scope, when its certificate verifies against the machine
    /// key. Anything else (no cert, another machine's cert, expired) is no
    /// claim at all.
    fn sender_scope(&self, d: &Deliver) -> Option<Scope> {
        let (cert, key) = (d.source_cert.as_ref()?, self.machine_key.as_ref()?);
        cert.verify(key, clawft_mesh_local::client::now_unix()).ok()?;
        Some(Scope { user_id: cert.user_id.clone(), project_id: None })
    }

    /// Deliver one `deliver` frame. A frame addressed to another user is
    /// refused: the service routes by registration, so this only happens on
    /// a service bug, and it must not reach this daemon's agents.
    pub async fn deliver(&self, d: Deliver) -> Result<(), String> {
        if d.scope.user_id != self.user_id {
            return Err(format!("deliver for user {} refused (this daemon is {})", d.scope.user_id, self.user_id));
        }
        let src_scope = self.sender_scope(&d);
        let msg: KernelMessage =
            serde_json::from_value(d.message).map_err(|e| format!("malformed delivered message: {e}"))?;
        let scope = Scope { user_id: d.scope.user_id, project_id: d.scope.project_id };
        // The service vouches for the source node, not this process: treat the
        // peer as unverified here (no `node_verified`). The sender's user id
        // rests on its certificate, checked above.
        let mut ctx = PeerCtx::unauthenticated(d.source_node);
        ctx.src_scope = src_scope;
        self.delivery.deliver(&ctx, Some(&scope), msg).await.map_err(|e| e.to_string())
    }
}

/// One outbound message for the link task.
pub struct OutCmd {
    /// Destination (`weft://<node>/_/_/` from the router, any scoped
    /// address from [`ServiceForwarder::send_to`]).
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
        self.send_to(dest, &msg).await
    }
}

impl ServiceForwarder {
    /// Send `msg` to a `weft://` address (a user or project scope on this
    /// machine or another). The service stamps the sender's scope from this
    /// daemon's registration; nothing here can claim another user.
    pub async fn send_to(&self, dest: WeftAddr, msg: &KernelMessage) -> KernelResult<()> {
        if !self.connected.load(Ordering::Acquire) {
            return Err(KernelError::Mesh(format!(
                "cannot reach '{dest}': the mesh service link is reconnecting"
            )));
        }
        let message = serde_json::to_value(msg)
            .map_err(|e| KernelError::Mesh(format!("cannot encode message: {e}")))?;
        let shown = dest.to_string();
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(OutCmd { dest, message, reply })
            .await
            .map_err(|_| KernelError::Mesh("the mesh service link has shut down".into()))?;
        match tokio::time::timeout(FORWARD_TIMEOUT, rx).await {
            Ok(Ok(Ok(()))) => Ok(()),
            Ok(Ok(Err(why))) => Err(KernelError::Mesh(format!("send to '{shown}' failed: {why}"))),
            Ok(Err(_)) => Err(KernelError::Mesh("the mesh service link dropped the message".into())),
            Err(_) => Err(KernelError::Mesh(format!("send to '{shown}' timed out"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clawft_kernel::ipc::MessageTarget;
    use clawft_mesh_local::UserCert;
    use clawft_mesh_local::proto::Scope as WireScope;
    use ed25519_dalek::SigningKey;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Seen(Mutex<Vec<Option<Scope>>>);

    #[async_trait::async_trait]
    impl LocalDelivery for Seen {
        async fn deliver(&self, from: &PeerCtx, _: Option<&Scope>, _: KernelMessage) -> KernelResult<()> {
            self.0.lock().unwrap().push(from.src_scope.clone());
            Ok(())
        }
    }

    fn deliver_with(cert: Option<UserCert>) -> Deliver {
        let msg = KernelMessage::text(0, MessageTarget::Topic("t".into()), "x");
        Deliver {
            source_node: "n".into(),
            source_cert: cert,
            scope: WireScope { user_id: "me".into(), project_id: None },
            envelope_id: "e".into(),
            message: serde_json::to_value(msg).unwrap(),
        }
    }

    #[tokio::test]
    async fn only_a_cert_from_the_pinned_machine_names_the_sender() {
        let machine = SigningKey::from_bytes(&[1; 32]);
        let other = SigningKey::from_bytes(&[2; 32]);
        let sender = SigningKey::from_bytes(&[3; 32]).verifying_key().to_bytes();
        let now = clawft_mesh_local::client::now_unix();
        let seen = Arc::new(Seen::default());
        let sink = MeshSink::new(seen.clone(), "me").with_machine_key(machine.verifying_key().to_bytes());
        let good = UserCert::issue(&machine, sender, 1, now, 3600);
        let forged = UserCert::issue(&other, sender, 1, now, 3600);
        let expired = UserCert::issue(&machine, sender, 1, now - 7200, 3600);
        for c in [Some(good.clone()), Some(forged), Some(expired), None] {
            sink.deliver(deliver_with(c)).await.unwrap();
        }
        // Without a trusted machine key nothing is a claim either.
        MeshSink::new(seen.clone(), "me").deliver(deliver_with(Some(good.clone()))).await.unwrap();
        let got: Vec<Option<String>> =
            seen.0.lock().unwrap().iter().map(|s| s.as_ref().map(|s| s.user_id.clone())).collect();
        assert_eq!(got, vec![Some(good.user_id), None, None, None, None]);
    }
}
