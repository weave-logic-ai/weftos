//! Cog checkout and artifact transfer over machine-mesh deliveries (ADR-106
//! sections 4 and 5, phase 1c).
//!
//! [`CogMeshDelivery`] sits in front of the daemon's router. It takes the
//! three topics of this feature, whatever their sender, and hands everything
//! else on untouched:
//!
//! - `mesh.artifact.tunnel`: artifact piece frames ([`ArtifactTunnel`]);
//! - `mesh.cog.checkout`: a member's request to the steward;
//! - `mesh.cog.checkout.reply`: the steward's answer.
//!
//! Every one of them is honoured only from a verified `node` peer. The
//! [`PeerCtx`] is built by the daemon from the service-stamped origin, so
//! `LocalTenant` and `Unadmitted` deliveries never reach a handler, and are
//! dropped here with a count instead of an error the sender could observe.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use crate::error::KernelResult;
use crate::ipc::{KernelMessage, MessagePayload, MessageTarget};
use crate::licence::{
    CheckoutCaller, CheckoutGrantStore, CheckoutRefusal, CheckoutRelay, CheckoutWire, SignedGrant,
    install_grant,
};
use crate::mesh_artifact::ArtifactExchange;
use crate::mesh_artifact_tunnel::{ArtifactTunnel, PeerSender, TOPIC_TUNNEL, licensed_peer};
use crate::mesh_delivery::{LocalDelivery, PeerCtx};
use crate::mesh_ipc::Scope;

/// Topic of checkout requests to the steward.
pub const TOPIC_CHECKOUT: &str = "mesh.cog.checkout";
/// Topic of the steward's replies.
pub const TOPIC_CHECKOUT_REPLY: &str = "mesh.cog.checkout.reply";
/// Checkout requests this node waits on at once.
const MAX_PENDING: usize = 64;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplyWire {
    request_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    grant: Option<SignedGrant>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

struct Pending {
    steward: String,
    tx: oneshot::Sender<ReplyWire>,
}

/// Counters, all monotonic.
#[derive(Debug, Default)]
pub struct CogMeshCounters {
    /// Deliveries on a feature topic from anything but a verified node.
    pub refused_unverified: AtomicU64,
    /// Requests answered `no_steward` (this node runs no relay).
    pub no_steward: AtomicU64,
    /// Replies that matched no pending request or the wrong steward.
    pub stray_replies: AtomicU64,
}

/// One node's cog mesh: tunnel, optional steward relay, requester side.
pub struct CogMesh {
    exchange: Arc<ArtifactExchange>,
    store: Arc<CheckoutGrantStore>,
    sender: Arc<dyn PeerSender>,
    relay: Option<Arc<CheckoutRelay>>,
    tunnel: Arc<ArtifactTunnel>,
    pending: DashMap<String, Pending>,
    /// Counters.
    pub counters: CogMeshCounters,
}

impl CogMesh {
    /// A node's cog mesh. `relay` is `Some` only on the steward.
    pub fn new(
        exchange: Arc<ArtifactExchange>,
        store: Arc<CheckoutGrantStore>,
        sender: Arc<dyn PeerSender>,
        relay: Option<Arc<CheckoutRelay>>,
    ) -> Arc<Self> {
        let tunnel = ArtifactTunnel::new(exchange.clone(), sender.clone());
        Arc::new(Self {
            exchange,
            store,
            sender,
            relay,
            tunnel,
            pending: DashMap::new(),
            counters: CogMeshCounters::default(),
        })
    }

    /// The artifact tunnel (its [`ArtifactTunnel::dialer`] is the production
    /// [`crate::mesh_swarm_fetch::PeerDialer`]).
    pub fn tunnel(&self) -> &Arc<ArtifactTunnel> {
        &self.tunnel
    }

    /// The steward's own kernel asks for a checkout.
    pub async fn checkout_local(&self, req: &CheckoutWire) -> Result<SignedGrant, CheckoutRefusal> {
        match &self.relay {
            Some(r) => r.handle(CheckoutCaller::Kernel, req).await,
            None => Err(CheckoutRefusal::Licence("no_steward".into())),
        }
    }

    /// Ask `steward` (a verified node) for a checkout. The returned grant is
    /// verified and registered here, so this node can fetch and share bytes.
    pub async fn request_checkout(
        &self,
        steward: &str,
        req: CheckoutWire,
        timeout: Duration,
    ) -> Result<SignedGrant, CheckoutRefusal> {
        req.validate().map_err(CheckoutRefusal::BadRequest)?;
        if self.pending.len() >= MAX_PENDING {
            return Err(CheckoutRefusal::BadRequest("too many checkouts in flight".into()));
        }
        let (tx, rx) = oneshot::channel();
        self.pending.insert(req.request_id.clone(), Pending { steward: steward.to_owned(), tx });
        let sent = self.send_json(steward, TOPIC_CHECKOUT, serde_json::to_value(&req)).await;
        if let Err(e) = sent {
            self.pending.remove(&req.request_id);
            return Err(CheckoutRefusal::LicenceLink(e));
        }
        let reply = match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(r)) => r,
            _ => {
                self.pending.remove(&req.request_id);
                return Err(CheckoutRefusal::LicenceLink("no reply from the steward".into()));
            }
        };
        match (reply.grant, reply.code) {
            (Some(g), _) => {
                install_grant(&self.store, &self.exchange, &g)
                    .map_err(|e| CheckoutRefusal::BadGrant(e.to_string()))?;
                Ok(g)
            }
            (None, code) => Err(CheckoutRefusal::Licence(code.unwrap_or_else(|| "refused".into()))),
        }
    }

    async fn send_json(
        &self,
        node: &str,
        topic: &str,
        v: Result<serde_json::Value, serde_json::Error>,
    ) -> Result<(), String> {
        let v = v.map_err(|e| e.to_string())?;
        let msg = KernelMessage::new(0, MessageTarget::Topic(topic.into()), MessagePayload::Json(v));
        self.sender.send_to_node(node, msg).await.map_err(|e| e.to_string())
    }

    async fn on_checkout(self: &Arc<Self>, from: &PeerCtx, msg: KernelMessage) {
        let (Some(relay), MessagePayload::Json(v)) = (self.relay.clone(), msg.payload.clone()) else {
            self.counters.no_steward.fetch_add(1, Ordering::Relaxed);
            let _ = self.reply_refused(&from.peer_id, "", "no_steward").await;
            return;
        };
        let Ok(req) = serde_json::from_value::<CheckoutWire>(v) else { return };
        let (me, peer, ctx) = (self.clone(), from.peer_id.clone(), from.clone());
        // The relay call can take a registry fetch: never hold the delivery path.
        tokio::spawn(async move {
            let reply = match relay.handle(CheckoutCaller::Peer(&ctx), &req).await {
                Ok(g) => ReplyWire {
                    request_id: req.request_id.clone(),
                    grant: Some(g),
                    code: None,
                    message: None,
                },
                Err(e) => ReplyWire {
                    request_id: req.request_id.clone(),
                    grant: None,
                    code: Some(e.code()),
                    message: Some(e.to_string()),
                },
            };
            let _ = me.send_json(&peer, TOPIC_CHECKOUT_REPLY, serde_json::to_value(&reply)).await;
        });
    }

    async fn reply_refused(&self, peer: &str, request_id: &str, code: &str) -> Result<(), String> {
        let r = ReplyWire {
            request_id: request_id.to_owned(),
            grant: None,
            code: Some(code.to_owned()),
            message: None,
        };
        self.send_json(peer, TOPIC_CHECKOUT_REPLY, serde_json::to_value(&r)).await
    }

    fn on_reply(&self, from: &PeerCtx, msg: KernelMessage) {
        let MessagePayload::Json(v) = msg.payload else { return };
        let Ok(r) = serde_json::from_value::<ReplyWire>(v) else { return };
        let matched = self
            .pending
            .remove_if(&r.request_id, |_, p| p.steward == from.peer_id)
            .map(|(_, p)| p);
        match matched {
            Some(p) => {
                let _ = p.tx.send(r);
            }
            None => {
                self.counters.stray_replies.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Route one delivery on a feature topic. Returns false when the topic
    /// is not one of ours (the caller passes it on).
    pub async fn on_delivery(self: &Arc<Self>, from: &PeerCtx, msg: KernelMessage) -> bool {
        let topic = match &msg.target {
            MessageTarget::Topic(t) => t.as_str(),
            _ => return false,
        };
        let known = matches!(topic, TOPIC_TUNNEL | TOPIC_CHECKOUT | TOPIC_CHECKOUT_REPLY);
        if !known {
            return false;
        }
        if !licensed_peer(from) {
            self.counters.refused_unverified.fetch_add(1, Ordering::Relaxed);
            return true;
        }
        match topic {
            TOPIC_TUNNEL => self.tunnel.on_message(from, msg).await,
            TOPIC_CHECKOUT => self.on_checkout(from, msg).await,
            _ => self.on_reply(from, msg),
        }
        true
    }
}

/// A slot the daemon fills once its exchange exists (the mesh link starts
/// before placement builds the exchange). Until then feature topics are
/// dropped.
#[derive(Default)]
pub struct CogMeshSlot(OnceLock<Arc<CogMesh>>);

impl CogMeshSlot {
    /// Install the node's cog mesh; false when one was already installed.
    pub fn install(&self, m: Arc<CogMesh>) -> bool {
        self.0.set(m).is_ok()
    }

    /// The installed cog mesh.
    pub fn get(&self) -> Option<&Arc<CogMesh>> {
        self.0.get()
    }
}

/// The router in front of `inner` that takes the cog mesh topics.
pub struct CogMeshDelivery {
    inner: Arc<dyn LocalDelivery>,
    slot: Arc<CogMeshSlot>,
}

impl CogMeshDelivery {
    /// Wrap `inner`; feature topics go to whatever `slot` holds.
    pub fn new(inner: Arc<dyn LocalDelivery>, slot: Arc<CogMeshSlot>) -> Self {
        Self { inner, slot }
    }
}

fn is_feature_topic(msg: &KernelMessage) -> bool {
    matches!(&msg.target, MessageTarget::Topic(t)
        if matches!(t.as_str(), TOPIC_TUNNEL | TOPIC_CHECKOUT | TOPIC_CHECKOUT_REPLY))
}

#[async_trait]
impl LocalDelivery for CogMeshDelivery {
    async fn deliver(
        &self,
        from: &PeerCtx,
        dest_scope: Option<&Scope>,
        msg: KernelMessage,
    ) -> KernelResult<()> {
        if !is_feature_topic(&msg) {
            return self.inner.deliver(from, dest_scope, msg).await;
        }
        // Never passed on to the router, even when no cog mesh is installed.
        if let Some(m) = self.slot.get() {
            m.on_delivery(from, msg).await;
        }
        Ok(())
    }

    async fn authorize_subscribe(&self, from: &PeerCtx, topic: &str, dest_scope: Option<&Scope>) -> bool {
        self.inner.authorize_subscribe(from, topic, dest_scope).await
    }
}
