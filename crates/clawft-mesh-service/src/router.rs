//! `TenantRouter`: how mesh traffic reaches tenants and how tenants reach the
//! mesh (plan 1.6, 2 S).
//!
//! Inbound (the kernel's `serve_listener` calls [`LocalDelivery::deliver`]):
//!
//! - `dest_scope` in an envelope is **untrusted**: it is a peer's request for
//!   an address. It selects a tenant only when the connection was *admitted*
//!   (`PeerCtx::node_verified`). An unadmitted peer (legacy, leaf, anything
//!   under `observe`) can reach only the default tenant: the sole registered
//!   user, or the cluster owner's registration when several are registered. A
//!   scope naming anyone else is dropped and counted, never rerouted.
//! - Admitted peers: scope present delivers to that registration (and project)
//!   or drops as `unknown_scope`; scope absent matches the longest registered
//!   topic prefix, then the single registered user, else drops as
//!   `scope_required`.
//!
//! Outbound (`send` from a registration): the envelope carries
//! `source_node` = the machine node id and a `src_scope` stamped from the
//! sending registration. Whatever a daemon puts in the message about who it
//! is cannot change that. A destination on this machine is delivered locally
//! with the sender's certificate attached, but only when the recipient's
//! registration opted in (`accept_from`: user ids or `*`; default none).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, Weak};

use async_trait::async_trait;
use clawft_kernel::error::{KernelError, KernelResult};
use clawft_kernel::ipc::{KernelMessage, MessageTarget};
use clawft_kernel::mesh_admit::PeerClass;
use clawft_kernel::mesh_delivery::{LocalDelivery, PeerCtx};
use clawft_kernel::mesh_ipc::{MeshIpcEnvelope, Scope as WireScope};
use clawft_kernel::mesh_runtime::MeshRuntime;
use clawft_mesh_local::proto::{Deliver, DeliverOrigin, Frame, Message, OriginClass, Scope, PROTO_ORIGIN};
use clawft_mesh_local::{Node, WeftAddr};

use crate::registry::{QueueError, Registration, Registry, ScopeMiss};
use crate::state::PolicyCell;

/// Router counters, all monotonic.
#[derive(Debug, Default)]
pub struct RouterCounters {
    pub delivered: AtomicU64,
    /// Admitted peer, no scope and no prefix match, several users registered.
    pub scope_required: AtomicU64,
    /// Scope named a user or project nobody has registered.
    pub unknown_scope: AtomicU64,
    /// Unadmitted peer asked for a tenant other than the default.
    pub denied_scope: AtomicU64,
    /// Nobody registered at all.
    pub no_tenant: AtomicU64,
    pub dropped_full: AtomicU64,
    pub sent_remote: AtomicU64,
    pub sent_local: AtomicU64,
    /// Reserved-topic sends refused, and reserved deliveries with no holder.
    pub reserved_refused: AtomicU64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendError {
    /// Destination node is not connected.
    Unreachable(String),
    /// Local destination tenant is not registered.
    UnknownScope(String),
    /// The recipient's registration has not opted in to receive from this tenant.
    Forbidden(String),
    /// The message could not be placed on the wire or in a queue.
    Failed(String),
}

pub struct TenantRouter {
    registry: Arc<Registry>,
    policy: Arc<PolicyCell>,
    node_id: String,
    runtime: OnceLock<Weak<MeshRuntime>>,
    /// The uid that holds the reserved topics when no `cluster_owner_uid` is
    /// set: the service's own. Never "whoever registered first".
    fallback_uid: OnceLock<u32>,
    warned_no_holder: std::sync::atomic::AtomicBool,
    pub counters: RouterCounters,
}

fn topic_of(msg: &KernelMessage) -> Option<&str> {
    match &msg.target {
        MessageTarget::Topic(t) => Some(t.as_str()),
        _ => None,
    }
}

impl TenantRouter {
    pub fn new(registry: Arc<Registry>, policy: Arc<PolicyCell>, node_id: String) -> Arc<Self> {
        Arc::new(Self {
            registry,
            policy,
            node_id,
            runtime: OnceLock::new(),
            fallback_uid: OnceLock::new(),
            warned_no_holder: std::sync::atomic::AtomicBool::new(false),
            counters: RouterCounters::default(),
        })
    }

    /// Attach the mesh runtime (it holds this router, so the link is weak).
    pub fn set_runtime(&self, rt: &Arc<MeshRuntime>) {
        let _ = self.runtime.set(Arc::downgrade(rt));
    }

    pub fn runtime(&self) -> Option<Arc<MeshRuntime>> {
        self.runtime.get().and_then(Weak::upgrade)
    }

    /// The tenant an unadmitted peer may reach: the sole registered user, or
    /// the cluster owner's registration when several are registered.
    pub fn default_tenant(&self) -> Option<Arc<Registration>> {
        if let Some(r) = self.registry.sole() {
            return Some(r);
        }
        let uid = self.policy.owner_uid()?;
        self.registry.by_principal(&clawft_mesh_local::Principal::Uid(uid))
    }

    fn miss(&self, why: ScopeMiss) {
        let _ = why;
        self.counters.unknown_scope.fetch_add(1, Ordering::Relaxed);
    }

    /// Set the uid that holds the reserved topics when no cluster owner is configured.
    pub fn set_fallback_uid(&self, uid: u32) {
        let _ = self.fallback_uid.set(uid);
    }

    /// The uid allowed to send and receive the reserved topics: the cluster
    /// owner, else the service's own uid, else nobody. It is never inferred
    /// from who registered (a squatter at boot, or a second registration,
    /// must not move it).
    pub fn reserved_holder_uid(&self) -> Option<u32> {
        self.policy.owner_uid().or_else(|| self.fallback_uid.get().copied())
    }

    /// Where [`Self::reserved_holder_uid`] came from, for status and the doctor.
    pub fn reserved_holder_source(&self) -> &'static str {
        if self.policy.owner_uid().is_some() {
            "cluster_owner_uid"
        } else if self.fallback_uid.get().is_some() {
            "service_uid"
        } else {
            "none"
        }
    }

    /// The registration of [`Self::reserved_holder_uid`], if it is registered.
    fn reserved_holder(&self) -> Option<Arc<Registration>> {
        let Some(uid) = self.reserved_holder_uid() else {
            if !self.warned_no_holder.swap(true, Ordering::Relaxed) {
                tracing::warn!(
                    "cluster_owner_uid required for the licence/artifact mesh: no uid holds the \
                     reserved mesh.cog./mesh.artifact. topics, so they are refused"
                );
            }
            return None;
        };
        self.registry.by_principal(&clawft_mesh_local::Principal::Uid(uid))
    }

    /// Choose the registration for an inbound message and the scope to report.
    fn resolve(
        &self,
        from: &PeerCtx,
        dest: Option<&WireScope>,
        topic: Option<&str>,
    ) -> Option<(Arc<Registration>, Scope)> {
        if from.node_verified {
            return match dest {
                Some(s) => match self.registry.lookup_scope(&s.user_id, s.project_id.as_deref()) {
                    Ok(r) => Some((r, Scope { user_id: s.user_id.clone(), project_id: s.project_id.clone() })),
                    Err(m) => {
                        self.miss(m);
                        None
                    }
                },
                None => {
                    let reg = topic
                        .and_then(|t| self.registry.longest_prefix(t))
                        .or_else(|| self.registry.sole());
                    match reg {
                        Some(r) => {
                            let scope = Scope { user_id: r.user_id.clone(), project_id: None };
                            Some((r, scope))
                        }
                        None if self.registry.is_empty() => {
                            self.counters.no_tenant.fetch_add(1, Ordering::Relaxed);
                            None
                        }
                        None => {
                            self.counters.scope_required.fetch_add(1, Ordering::Relaxed);
                            None
                        }
                    }
                }
            };
        }
        // Unadmitted: the envelope's scope is only a claim and can never move
        // traffic to another tenant.
        let Some(default) = self.default_tenant() else {
            self.counters.no_tenant.fetch_add(1, Ordering::Relaxed);
            return None;
        };
        let mut project = None;
        if let Some(s) = dest {
            if s.user_id != default.user_id {
                self.counters.denied_scope.fetch_add(1, Ordering::Relaxed);
                tracing::warn!(peer = %from.peer_id, claimed = %s.user_id,
                    "dropping message: unadmitted peer named a non-default tenant");
                return None;
            }
            if let Some(p) = &s.project_id {
                match self.registry.lookup_scope(&default.user_id, Some(p)) {
                    Ok(_) => project = Some(p.clone()),
                    Err(m) => {
                        self.miss(m);
                        return None;
                    }
                }
            }
        }
        let scope = Scope { user_id: default.user_id.clone(), project_id: project };
        Some((default, scope))
    }

    /// The origin the service vouches for, from the connection (never from
    /// anything in the envelope). `node_verified` is set only by admission.
    fn origin_of(from: &PeerCtx) -> DeliverOrigin {
        if !from.node_verified {
            return DeliverOrigin::Unadmitted;
        }
        let class = match from.class {
            PeerClass::Node => OriginClass::Node,
            PeerClass::Leaf => OriginClass::Leaf,
            PeerClass::Legacy => OriginClass::Other,
        };
        DeliverOrigin::AdmittedPeer { node_id: from.peer_id.clone(), class }
    }

    /// Queue a `deliver` for `reg`. The origin stamp is written only when the
    /// connection negotiated a protocol that has the field; an older daemon
    /// never sees it, and reads every delivery as unadmitted.
    fn queue(
        &self,
        reg: &Registration,
        from_node: &str,
        scope: Scope,
        source_cert: Option<clawft_mesh_local::UserCert>,
        origin: DeliverOrigin,
        msg: &KernelMessage,
    ) -> Result<(), QueueError> {
        let message = serde_json::to_value(msg).map_err(|_| QueueError::Closed)?;
        let origin = (reg.proto() >= PROTO_ORIGIN).then_some(origin);
        let frame = Frame::new(Message::Deliver(Deliver {
            source_node: from_node.to_string(),
            source_cert,
            scope,
            envelope_id: msg.id.clone(),
            message,
            origin,
        }));
        match reg.try_queue(frame) {
            Ok(()) => {
                reg.counters.delivered.fetch_add(1, Ordering::Relaxed);
                self.counters.delivered.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            Err(QueueError::Full) => {
                self.counters.dropped_full.fetch_add(1, Ordering::Relaxed);
                Err(QueueError::Full)
            }
            Err(e) => Err(e),
        }
    }

    /// A registered tenant sends `msg` to `dest`.
    pub async fn route_outbound(
        &self,
        from: &Arc<Registration>,
        dest: &WeftAddr,
        mut msg: KernelMessage,
    ) -> Result<(), SendError> {
        if !dest.topic.is_empty() {
            msg.target = MessageTarget::Topic(dest.topic.clone());
        }
        // Reserved topics speak for the machine: only the owner's daemon may send them.
        if topic_of(&msg).is_some_and(clawft_mesh_local::proto::is_reserved_topic)
            && !self.reserved_holder().is_some_and(|h| Arc::ptr_eq(&h, from))
        {
            self.counters.reserved_refused.fetch_add(1, Ordering::Relaxed);
            return Err(SendError::Forbidden(
                "this topic is reserved for the cluster owner's daemon".into(),
            ));
        }
        let node = match &dest.node {
            Node::Local => self.node_id.clone(),
            Node::Id(n) => n.clone(),
        };
        let dest_scope = dest.user.as_ref().map(|u| WireScope {
            user_id: u.clone(),
            project_id: dest.project.clone(),
        });
        if node == self.node_id {
            return self.deliver_local(from, dest_scope, msg);
        }
        let rt = self.runtime().ok_or_else(|| SendError::Failed("mesh runtime is not running".into()))?;
        let mut env = MeshIpcEnvelope::new(self.node_id.clone(), node.clone(), msg);
        env.dest_scope = dest_scope;
        env.src_scope = Some(WireScope { user_id: from.user_id.clone(), project_id: None });
        rt.send_to_peer(&node, env).await.map_err(|e| SendError::Unreachable(e.to_string()))?;
        from.counters.sent.fetch_add(1, Ordering::Relaxed);
        self.counters.sent_remote.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Delivery between tenants on this machine. The sender is a registered
    /// tenant, so the destination scope is honoured as for an admitted peer.
    fn deliver_local(
        &self,
        from: &Arc<Registration>,
        dest: Option<WireScope>,
        msg: KernelMessage,
    ) -> Result<(), SendError> {
        let ctx = PeerCtx {
            peer_id: self.node_id.clone(),
            node_verified: true,
            class: PeerClass::Node,
            remote_static: None,
            src_scope: None,
        };
        let Some((reg, scope)) = self.resolve(&ctx, dest.as_ref(), topic_of(&msg)) else {
            return Err(SendError::UnknownScope(
                dest.map_or_else(|| "no matching tenant".into(), |s| s.user_id),
            ));
        };
        // Cross-tenant delivery on one machine is opt-in by the recipient.
        if !reg.accepts(&from.user_id) {
            return Err(SendError::Forbidden(format!(
                "{} does not accept messages from other tenants (accept_from)",
                reg.user_id
            )));
        }
        self.queue(&reg, &self.node_id, scope, from.cert(), DeliverOrigin::LocalTenant, &msg)
            .map_err(|e| SendError::Failed(format!("{e:?}")))?;
        from.counters.sent.fetch_add(1, Ordering::Relaxed);
        self.counters.sent_local.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

#[async_trait]
impl LocalDelivery for TenantRouter {
    async fn deliver(
        &self,
        from: &PeerCtx,
        dest_scope: Option<&WireScope>,
        msg: KernelMessage,
    ) -> KernelResult<()> {
        // Reserved topics go only to the owner's registration, whatever the
        // envelope's scope claims and whoever holds a prefix.
        if topic_of(&msg).is_some_and(clawft_mesh_local::proto::is_reserved_topic) {
            let Some(reg) = self.reserved_holder() else {
                self.counters.reserved_refused.fetch_add(1, Ordering::Relaxed);
                return Ok(());
            };
            let scope = Scope { user_id: reg.user_id.clone(), project_id: None };
            return self
                .queue(&reg, &from.peer_id, scope, None, Self::origin_of(from), &msg)
                .map_err(|e| KernelError::Mesh(format!("tenant {} queue: {e:?}", reg.user_id)));
        }
        let Some((reg, scope)) = self.resolve(from, dest_scope, topic_of(&msg)) else {
            return Ok(());
        };
        self.queue(&reg, &from.peer_id, scope, None, Self::origin_of(from), &msg)
            .map_err(|e| KernelError::Mesh(format!("tenant {} queue: {e:?}", reg.user_id)))
    }

    async fn authorize_subscribe(
        &self,
        from: &PeerCtx,
        _topic: &str,
        dest_scope: Option<&WireScope>,
    ) -> bool {
        if from.node_verified {
            return true;
        }
        match (dest_scope, self.default_tenant()) {
            (Some(s), Some(d)) => s.user_id == d.user_id,
            (Some(_), None) => false,
            (None, _) => true,
        }
    }
}

#[cfg(test)]
#[path = "router_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "router_origin_tests.rs"]
mod origin_tests;
