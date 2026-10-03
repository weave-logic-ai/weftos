//! Licence control topics to the owner's daemon (ADR-106 phase 3).
//!
//! The mesh runtime consumes `mesh.cog.binding`, `mesh.cog.grant` and
//! `mesh.cog.sync` as control topics: they are never routed to a tenant on
//! their own. The service holds no licence state, so it installs a sink for
//! each that hands the record to the reserved-topic holder's registration (the
//! cluster owner's daemon), stamped with the connection's origin, where the
//! daemon's licence exchange takes it.
//!
//! Only a licensed peer's records are passed on: admission verified the id
//! and classed it `node` (the daemon checks the stamp again). Anything else
//! (an unadmitted connection, a verified leaf) is dropped and counted, so an
//! unlicensed peer cannot make the daemon spend work on these topics. Replies
//! (sync pages) are sent by the daemon through its own registration.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Weak};

use clawft_kernel::ipc::{KernelMessage, MessagePayload, MessageTarget};
use clawft_kernel::mesh_admit::PeerClass;
use clawft_kernel::mesh_delivery::PeerCtx;
use clawft_kernel::mesh_runtime::{
    COG_BINDING_TOPIC, COG_GRANT_TOPIC, COG_SYNC_TOPIC, MeshRuntime, PeerControlSink,
};

use crate::router::TenantRouter;

/// The control topics forwarded to the owner's daemon.
pub const FORWARDED_TOPICS: [&str; 3] = [COG_BINDING_TOPIC, COG_GRANT_TOPIC, COG_SYNC_TOPIC];

struct OwnerForward {
    topic: &'static str,
    router: Weak<TenantRouter>,
}

impl PeerControlSink for OwnerForward {
    fn on_peer_control(&self, ctx: &PeerCtx, _conn: u64, payload: &serde_json::Value) -> Vec<serde_json::Value> {
        let Some(router) = self.router.upgrade() else { return Vec::new() };
        if !(ctx.node_verified && ctx.class == PeerClass::Node) {
            router.counters.licence_unlicensed.fetch_add(1, Ordering::Relaxed);
            return Vec::new();
        }
        let msg = KernelMessage::new(0, MessageTarget::Topic(self.topic.into()), MessagePayload::Json(payload.clone()));
        match router.deliver_reserved(ctx, &msg) {
            Ok(()) => {
                router.counters.licence_forwarded.fetch_add(1, Ordering::Relaxed);
            }
            Err(e) => tracing::debug!(peer = %ctx.peer_id, topic = self.topic, error = %e, "licence record not queued"),
        }
        Vec::new()
    }
}

/// Install the forwarding sinks on the service's runtime.
pub fn install(rt: &MeshRuntime, router: &Arc<TenantRouter>) {
    for topic in FORWARDED_TOPICS {
        rt.set_control_sink(topic, Arc::new(OwnerForward { topic, router: Arc::downgrade(router) }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{Registration, Registry};
    use crate::state::PolicyCell;
    use clawft_kernel::mesh_ipc::MeshIpcEnvelope;
    use clawft_mesh_local::proto::{DeliverOrigin, Frame, Message, OriginClass};
    use clawft_mesh_local::{Principal, node_id_from_pubkey};
    use clawft_types::config::MeshAdmissionMode;
    use tokio::sync::mpsc;

    fn tenant(registry: &Registry, uid: u32) -> (Arc<Registration>, mpsc::Receiver<Frame>) {
        let id = node_id_from_pubkey(&[uid as u8; 32]);
        let (reg, rx, _) =
            Registration::new(u64::from(uid), Principal::Uid(uid), id, [uid as u8; 32], uid, String::new(), vec![], 0);
        reg.set_proto(clawft_mesh_local::proto::PROTO_ORIGIN);
        registry.register(&reg, &[], &[]).unwrap();
        (reg, rx)
    }

    async fn from_peer(rt: &MeshRuntime, peer: &str, verified: bool, class: PeerClass, topic: &str) {
        let msg = KernelMessage::new(0, MessageTarget::Topic(topic.into()), MessagePayload::Json(serde_json::json!({"x": 1})));
        let bytes = MeshIpcEnvelope::new(peer.into(), "node-local".into(), msg).to_bytes().unwrap();
        let (tx, _rx) = mpsc::channel(4);
        let ctx = PeerCtx { peer_id: peer.into(), node_verified: verified, class, remote_static: None, src_scope: None };
        rt.handle_incoming_peer(&bytes, tx, Some(&ctx)).await.unwrap();
    }

    #[tokio::test]
    async fn licence_records_from_a_licensed_peer_reach_only_the_owner_stamped() {
        let registry = Arc::new(Registry::new());
        let policy = PolicyCell::new(Some(501), MeshAdmissionMode::Enforce);
        let router = TenantRouter::new(registry.clone(), policy, "node-local".into());
        let (_owner, mut owner_rx) = tenant(&registry, 501);
        let (_other, mut other_rx) = tenant(&registry, 502);
        let rt = MeshRuntime::new("node-local".into());
        install(&rt, &router);
        for topic in FORWARDED_TOPICS {
            from_peer(&rt, "node-p", true, PeerClass::Node, topic).await;
        }
        for topic in FORWARDED_TOPICS {
            match owner_rx.try_recv() {
                Ok(Frame { msg: Message::Deliver(d), .. }) => {
                    let m: KernelMessage = serde_json::from_value(d.message.clone()).unwrap();
                    assert!(matches!(&m.target, MessageTarget::Topic(t) if t == topic), "{:?}", d.message);
                    assert_eq!(
                        d.origin,
                        Some(DeliverOrigin::AdmittedPeer { node_id: "node-p".into(), class: OriginClass::Node })
                    );
                }
                other => panic!("expected a deliver on {topic}, got {other:?}"),
            }
        }
        assert!(other_rx.try_recv().is_err(), "a non-owner tenant gets nothing");
        assert_eq!(router.counters.licence_forwarded.load(Ordering::Relaxed), 3);

        // A verified leaf and an unadmitted connection are dropped here.
        from_peer(&rt, "leaf-l", true, PeerClass::Leaf, COG_BINDING_TOPIC).await;
        from_peer(&rt, "node-u", false, PeerClass::Legacy, COG_GRANT_TOPIC).await;
        assert!(owner_rx.try_recv().is_err() && other_rx.try_recv().is_err());
        assert_eq!(router.counters.licence_unlicensed.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn only_the_owner_holds_the_topics_and_sends_go_only_to_licensed_nodes() {
        use clawft_mesh_local::{Node, WeftAddr};
        let registry = Arc::new(Registry::new());
        let policy = PolicyCell::new(Some(501), MeshAdmissionMode::Enforce);
        let router = TenantRouter::new(registry.clone(), policy, "node-local".into());
        let (owner, _orx) = tenant(&registry, 501);
        let (other, _xrx) = tenant(&registry, 502);
        assert!(router.holds_reserved(&owner) && !router.holds_reserved(&other));
        let rt = Arc::new(MeshRuntime::new("node-local".into()));
        let (node_p, leaf_l) = (node_id_from_pubkey(&[0x70; 32]), node_id_from_pubkey(&[0x71; 32]));
        router.set_runtime(&rt);
        let tally = clawft_kernel::mesh_runtime::RouteTally::default();
        let (ntx, mut nrx) = mpsc::channel(8);
        let (ltx, mut lrx) = mpsc::channel(8);
        assert!(rt.register_authenticated_as(node_p.clone(), ntx, true, PeerClass::Node, &tally));
        assert!(rt.register_authenticated_as(leaf_l.clone(), ltx, true, PeerClass::Leaf, &tally));
        let msg = || KernelMessage::new(0, MessageTarget::Topic(COG_BINDING_TOPIC.into()), MessagePayload::Json(serde_json::json!({})));
        let to = |n: &str| WeftAddr::new(Node::Id(n.into()), None, None, "").unwrap();
        assert!(router.route_outbound(&owner, &to(&leaf_l), msg()).await.is_err(), "a leaf is refused");
        assert_eq!(router.counters.licence_out_refused.load(Ordering::Relaxed), 1);
        assert!(lrx.try_recv().is_err());
        router.route_outbound(&owner, &to(&node_p), msg()).await.unwrap();
        assert!(nrx.try_recv().is_ok(), "a licensed node gets it");
        assert!(router.route_outbound(&other, &to(&node_p), msg()).await.is_err(), "a non-owner may not send it");
    }
}
