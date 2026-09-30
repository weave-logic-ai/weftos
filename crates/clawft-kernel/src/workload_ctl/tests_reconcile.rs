//! No orphaned or duplicate instances: a start that fails after load is
//! rolled back on the target, and a `place` whose answer is lost is
//! reconciled before any other candidate is tried (or placement stops).

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use async_trait::async_trait;
use clawft_types::placement::TrustTier;
use ed25519_dalek::SigningKey;

use super::host_service::CtlConfig;
use super::msg::method;
use super::plane_place::PlaceOrder;
use super::session::{ctl_payload, is_envelope};
use super::test_support::*;
use super::transport::{CtlConnector, MeshConnector};
use crate::chain::{EVENT_KIND_WORKLOAD_REFUSE, EVENT_KIND_WORKLOAD_UNLOAD};
use crate::mesh::{MeshError, MeshStream};
use crate::mesh_ipc::MeshIpcEnvelope;
use crate::workload_governance::{NetworkPolicy, WorkloadGate, WorkloadPermitRule};
use crate::workload_runtime::RunMode;

const SCRIPT: &str = "#!/bin/sh\necho placed-ok\nexec sleep 30\n";

fn order(pkg: &std::path::Path, prefer: &str) -> PlaceOrder {
    PlaceOrder {
        package_dir: pkg.to_path_buf(),
        config: CtlConfig {
            mode: RunMode::Listener,
            args: vec![],
            csi_port: 15006,
        },
        pin: None,
        prefer: vec![prefer.to_string()],
        avoid: vec![],
        allow_emulated: false,
        start: true,
        dry_run: false,
    }
}

fn key() -> SigningKey {
    SigningKey::from_bytes(&[10; 32])
}

async fn instance_count(n: &HostNode) -> usize {
    n.svc.instances.lock().await.len()
}

/// Governance that allows everything but `workload.start`.
fn no_start(chain: &Arc<crate::chain::ChainManager>) -> Arc<WorkloadGate> {
    let mut permit = WorkloadPermitRule::new(
        "no-start",
        ["workload.load", "workload.unload", "workload.stop"],
        ["cog"],
    );
    permit.max_network = NetworkPolicy::Egress;
    Arc::new(
        WorkloadGate::new(0.95, false)
            .with_permit(permit)
            .unwrap()
            .with_chain(chain.clone()),
    )
}

#[tokio::test]
async fn a_start_failure_after_load_unloads_and_the_next_candidate_places() {
    let tmp = tempfile::tempdir().unwrap();
    let pkg = package(tmp.path(), "probe-cog", SCRIPT, &[arch()]);
    let k = key();
    let bad = host_node_with(41, board_caps("pi5"), true, &k, no_start);
    let good = host_node(42, board_caps("pi5"), true, &k);
    let conn = Arc::new(MeshConnector::new(false));
    let a = conn.register_local("bad", bad.svc.clone());
    let b = conn.register_local("good", good.svc.clone());
    let (plane, _chain) = controller(&k, conn);
    plane.add_target(&a, TrustTier::Paired).await.unwrap();
    plane.add_target(&b, TrustTier::Paired).await.unwrap();

    let r = plane.place(&order(&pkg, &bad.id)).await.unwrap();
    assert_eq!(r.attempts.len(), 2, "{}", r.explain);
    assert_eq!(r.attempts[0].outcome, "refused");
    assert_eq!(r.attempts[0].code.as_deref(), Some("governance"));
    let why = r.attempts[0].reason.as_deref().unwrap();
    assert!(why.contains("start failed") && why.contains("unloaded"), "{why}");
    assert_eq!(instance_count(&bad).await, 0, "no instance left on the refusing node");
    assert!(
        events(&bad.chain, EVENT_KIND_WORKLOAD_UNLOAD)
            .iter()
            .any(|(s, _)| s == "workload.runtime"),
        "the rollback unload is chained on the target"
    );
    assert_eq!(r.placed.as_ref().unwrap().node_id, good.id);
    assert_eq!(instance_count(&good).await, 1);
    plane
        .instance(method::UNLOAD, &r.placed.unwrap().instance_id)
        .await
        .unwrap();
}

/// Drops the answer to the next `place` sent to `victim` (after the
/// target carried it out); with `partition`, `victim` is unreachable from
/// then on.
struct LossyConnector {
    inner: MeshConnector,
    victim: String,
    lose: Arc<AtomicUsize>,
    partition: bool,
    cut: Arc<AtomicBool>,
}

struct LossyStream {
    inner: Box<dyn MeshStream>,
    lose: Arc<AtomicUsize>,
    cut: Option<Arc<AtomicBool>>,
}

#[async_trait]
impl MeshStream for LossyStream {
    async fn send(&mut self, data: &[u8]) -> Result<(), MeshError> {
        self.inner.send(data).await
    }
    async fn recv(&mut self) -> Result<Vec<u8>, MeshError> {
        let raw = self.inner.recv().await?;
        let is_place_answer = is_envelope(&raw)
            && MeshIpcEnvelope::from_bytes(&raw)
                .ok()
                .and_then(|e| ctl_payload(&e).ok())
                .is_some_and(|(m, _)| m == method::PLACE);
        if is_place_answer
            && self
                .lose
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                .is_ok()
        {
            if let Some(c) = &self.cut {
                c.store(true, Ordering::SeqCst);
            }
            return Err(MeshError::ConnectionClosed);
        }
        Ok(raw)
    }
    async fn close(&mut self) -> Result<(), MeshError> {
        self.inner.close().await
    }
    fn remote_addr(&self) -> Option<SocketAddr> {
        None
    }
}

#[async_trait]
impl CtlConnector for LossyConnector {
    async fn connect(&self, addr: &str) -> Result<Box<dyn MeshStream>, MeshError> {
        if addr == self.victim && self.cut.load(Ordering::SeqCst) {
            return Err(MeshError::PeerNotConnected(addr.to_string()));
        }
        let s = self.inner.connect(addr).await?;
        if addr != self.victim {
            return Ok(s);
        }
        Ok(Box::new(LossyStream {
            inner: s,
            lose: self.lose.clone(),
            cut: self.partition.then(|| self.cut.clone()),
        }))
    }
}

struct Pair {
    first: HostNode,
    second: HostNode,
    plane: super::plane::PlacementControlPlane,
    chain: Arc<crate::chain::ChainManager>,
}

async fn lossy_pair(partition: bool) -> Pair {
    let k = key();
    let first = host_node(51, board_caps("pi5"), true, &k);
    let second = host_node(52, board_caps("pi5"), true, &k);
    let inner = MeshConnector::new(false);
    let a = inner.register_local("first", first.svc.clone());
    let b = inner.register_local("second", second.svc.clone());
    let conn = Arc::new(LossyConnector {
        inner,
        victim: a.clone(),
        lose: Arc::new(AtomicUsize::new(0)),
        partition,
        cut: Arc::new(AtomicBool::new(false)),
    });
    let (plane, chain) = controller(&k, conn.clone());
    plane.add_target(&a, TrustTier::Paired).await.unwrap();
    plane.add_target(&b, TrustTier::Paired).await.unwrap();
    conn.lose.store(1, Ordering::SeqCst);
    Pair {
        first,
        second,
        plane,
        chain,
    }
}

#[tokio::test]
async fn a_lost_place_answer_is_reconciled_before_the_next_candidate() {
    let tmp = tempfile::tempdir().unwrap();
    let pkg = package(tmp.path(), "probe-cog", SCRIPT, &[arch()]);
    let p = lossy_pair(false).await;

    let r = p.plane.place(&order(&pkg, &p.first.id)).await.unwrap();
    assert_eq!(r.attempts.len(), 2, "{}", r.explain);
    assert_eq!(r.attempts[0].outcome, "indeterminate");
    let why = r.attempts[0].reason.as_deref().unwrap();
    assert!(why.contains("reconciled: unloaded 1 instance"), "{why}");
    assert_eq!(
        instance_count(&p.first).await,
        0,
        "the orphan on the first node was unloaded"
    );
    assert_eq!(r.placed.as_ref().unwrap().node_id, p.second.id);
    assert_eq!(instance_count(&p.second).await, 1, "exactly one instance");
    assert!(
        events(&p.chain, EVENT_KIND_WORKLOAD_REFUSE)
            .iter()
            .any(|(_, e)| e["outcome"] == "indeterminate"
                && e["next"] == "trying the next candidate")
    );
    p.plane
        .instance(method::UNLOAD, &r.placed.unwrap().instance_id)
        .await
        .unwrap();
}

#[tokio::test]
async fn an_unreconcilable_lost_answer_stops_placement_instead_of_duplicating() {
    let tmp = tempfile::tempdir().unwrap();
    let pkg = package(tmp.path(), "probe-cog", SCRIPT, &[arch()]);
    let p = lossy_pair(true).await;

    let r = p.plane.place(&order(&pkg, &p.first.id)).await.unwrap();
    assert_eq!(r.attempts.len(), 1, "no second candidate: {}", r.explain);
    assert_eq!(r.attempts[0].outcome, "indeterminate");
    assert!(r.placed.is_none());
    assert!(
        r.attempts[0]
            .reason
            .as_deref()
            .unwrap()
            .contains("not reconciled"),
        "{:?}",
        r.attempts[0].reason
    );
    assert_eq!(instance_count(&p.second).await, 0, "no duplicate placed");
    assert_eq!(instance_count(&p.first).await, 1, "the only instance");
    assert!(
        events(&p.chain, EVENT_KIND_WORKLOAD_REFUSE)
            .iter()
            .any(|(_, e)| e["outcome"] == "indeterminate"
                && e["next"] == "stopped: the target may hold the instance")
    );
    let left: Vec<_> = p.first.svc.instances.lock().await.drain().collect();
    for (_, placed) in left {
        let _ = p.first.svc.routes[&placed.route].unload(placed.handle).await;
    }
}
