//! Node loss: instances become `Lost`, are rescheduled through the same gate
//! and engine, pins and non-migratable instances raise an alert instead, and
//! a node that returns has its replaced copy unloaded.

use std::sync::Arc;
use std::time::{Duration, Instant};

use clawft_types::placement::TrustTier;
use ed25519_dalek::SigningKey;

use super::lifecycle::{LifecyclePolicy, LifecycleState};
use super::plane::{PlacementControlPlane, PlaneConfig};
use super::plane_lifecycle::LifecycleEvent;
use super::test_support::*;
use crate::chain::{
    ChainManager, EVENT_KIND_WORKLOAD_LIFECYCLE, EVENT_KIND_WORKLOAD_MIGRATE,
    EVENT_KIND_WORKLOAD_REFUSE,
};
use crate::workload_runtime::RunMode;

const SCRIPT: &str = "#!/bin/sh\nexec sleep 60\n";
/// How long a node must be unreachable to count as dead in these tests.
const DEAD_AFTER: Duration = Duration::from_millis(300);

struct Rig {
    a: HostNode,
    b: HostNode,
    addr_a: String,
    addr_b: String,
    net: Arc<Killable>,
    plane: PlacementControlPlane,
    chain: Arc<ChainManager>,
    pkg: std::path::PathBuf,
    _tmp: tempfile::TempDir,
}

async fn rig() -> Rig {
    let tmp = tempfile::tempdir().unwrap();
    let pkg = package(tmp.path(), "life-cog", SCRIPT, &[arch()]);
    let key = SigningKey::from_bytes(&[10; 32]);
    let a = host_node(31, board_caps("pi5"), true, &key);
    let b = host_node(32, board_caps("pi5"), true, &key);
    let net = Killable::new();
    let addr_a = net.serve("a", a.svc.clone());
    let addr_b = net.serve("b", b.svc.clone());
    let (plane, chain) = controller(&key, net.clone());
    let plane = plane.with_config(PlaneConfig {
        dead_after: DEAD_AFTER,
        ..PlaneConfig::default()
    });
    plane.add_target(&addr_a, TrustTier::Paired).await.unwrap();
    plane.add_target(&addr_b, TrustTier::Paired).await.unwrap();
    Rig { a, b, addr_a, addr_b, net, plane, chain, pkg, _tmp: tmp }
}

async fn count(n: &HostNode) -> usize {
    n.svc.instances.lock().await.len()
}

async fn past_dead_after() {
    tokio::time::sleep(DEAD_AFTER + Duration::from_millis(80)).await;
}

fn actions(ev: &[LifecycleEvent]) -> Vec<&str> {
    ev.iter().map(|e| e.action.as_str()).collect()
}

fn chained(chain: &ChainManager, kind: &str) -> Vec<serde_json::Value> {
    events(chain, kind).into_iter().map(|(_, p)| p).collect()
}

#[tokio::test]
async fn killing_a_node_reschedules_to_the_fallback_within_the_window() {
    let r = rig().await;
    let placed = r.plane.place(&mode_order(&r.pkg, RunMode::Listener, Some(&r.a.id))).await.unwrap().placed.unwrap();
    assert_eq!(placed.node_id, r.a.id);
    assert_eq!(r.plane.lifecycle_of(&placed.instance_id), Some(LifecycleState::Running));

    let killed = Instant::now();
    r.net.kill(&r.addr_a);
    // Unreachable is not yet dead: a blip must not move anything.
    assert!(r.plane.lifecycle_tick().await.is_empty());
    assert_eq!(count(&r.b).await, 0);

    past_dead_after().await;
    let ev = r.plane.lifecycle_tick().await;
    let window = killed.elapsed();
    assert_eq!(actions(&ev), ["lost", "rescheduled"], "{ev:#?}");
    assert!(window < DEAD_AFTER + Duration::from_secs(2), "took {window:?}");

    assert_eq!(count(&r.b).await, 1, "running on the fallback");
    let now: Vec<_> = r.plane.placements();
    let new = now.iter().find(|p| p.node_id == r.b.id).expect("replacement record");
    assert_eq!(r.plane.lifecycle_of(&new.instance_id), Some(LifecycleState::Running));
    assert_eq!(r.plane.lifecycle_of(&placed.instance_id), Some(LifecycleState::Rescheduled));

    // Chained: the loss, the reschedule decision and the move.
    let life = chained(&r.chain, EVENT_KIND_WORKLOAD_LIFECYCLE);
    assert!(life.iter().any(|p| p["to"] == "lost" && p["reason"] == "node is dead"));
    assert!(life.iter().any(|p| p["to"] == "rescheduled"));
    let mig = chained(&r.chain, EVENT_KIND_WORKLOAD_MIGRATE);
    assert_eq!(mig.len(), 1);
    assert_eq!(mig[0]["from_node"], r.a.id);
    assert_eq!(mig[0]["to_node"], r.b.id);
    assert_eq!(mig[0]["reason"], "node_dead");
    assert!(mig[0]["decision_id"].as_str().is_some_and(|d| d.len() == 64));

    // Nothing more happens while the node stays down.
    assert!(r.plane.lifecycle_tick().await.is_empty());
    assert_eq!(count(&r.b).await, 1);
}

#[tokio::test]
async fn a_node_that_returns_has_the_replaced_copy_unloaded() {
    let r = rig().await;
    let old = r.plane.place(&mode_order(&r.pkg, RunMode::Listener, Some(&r.a.id))).await.unwrap().placed.unwrap();
    r.net.kill(&r.addr_a);
    r.plane.lifecycle_tick().await;
    past_dead_after().await;
    r.plane.lifecycle_tick().await;
    assert_eq!((count(&r.a).await, count(&r.b).await), (1, 1), "the old copy still runs on the dead node");

    r.net.revive(&r.addr_a);
    let ev = r.plane.lifecycle_tick().await;
    assert_eq!(actions(&ev), ["orphan_unloaded"], "{ev:#?}");
    assert_eq!((count(&r.a).await, count(&r.b).await), (0, 1), "the workload runs once");
    assert!(r.plane.placements().iter().all(|p| p.instance_id != old.instance_id));
    assert!(r.plane.lifecycle_of(&old.instance_id).is_none());
}

#[tokio::test]
async fn a_pinned_instance_is_not_moved_and_raises_an_alert() {
    let r = rig().await;
    let mut o = mode_order(&r.pkg, RunMode::Listener, None);
    o.pin = Some(r.a.id.clone());
    let placed = r.plane.place(&o).await.unwrap().placed.unwrap();
    r.net.kill(&r.addr_a);
    r.plane.lifecycle_tick().await;
    past_dead_after().await;
    let ev = r.plane.lifecycle_tick().await;
    assert_eq!(actions(&ev), ["lost", "alert"], "{ev:#?}");
    assert!(ev[1].detail.contains("pinned"), "{}", ev[1].detail);
    assert_eq!(count(&r.b).await, 0);
    assert_eq!(r.plane.lifecycle_of(&placed.instance_id), Some(LifecycleState::Lost));
    assert!(chained(&r.chain, EVENT_KIND_WORKLOAD_LIFECYCLE)
        .iter()
        .any(|p| p["phase"] == "alert" && p["reason"].as_str().unwrap().contains("pinned")));
    // The alert is raised once, not every tick.
    assert!(r.plane.lifecycle_tick().await.is_empty());

    // When the node returns with the instance, it is adopted back.
    r.net.revive(&r.addr_a);
    let ev = r.plane.lifecycle_tick().await;
    assert_eq!(actions(&ev), ["recovered"], "{ev:#?}");
    assert_eq!(r.plane.lifecycle_of(&placed.instance_id), Some(LifecycleState::Running));
}

#[tokio::test]
async fn an_operator_non_migratable_instance_is_not_moved() {
    let r = rig().await;
    let o = mode_order(&r.pkg, RunMode::Listener, Some(&r.a.id));
    let policy = LifecyclePolicy { migratable: false, ..LifecyclePolicy::default() };
    r.plane.place_with(&o, policy).await.unwrap();
    r.net.kill(&r.addr_a);
    r.plane.lifecycle_tick().await;
    past_dead_after().await;
    let ev = r.plane.lifecycle_tick().await;
    assert_eq!(actions(&ev), ["lost", "alert"], "{ev:#?}");
    assert!(ev[1].detail.contains("non-migratable"));
    assert_eq!(count(&r.b).await, 0);
}

#[tokio::test]
async fn a_stopped_instance_is_not_started_elsewhere_when_its_node_dies() {
    let r = rig().await;
    let placed = r.plane.place(&mode_order(&r.pkg, RunMode::Listener, Some(&r.a.id))).await.unwrap().placed.unwrap();
    r.plane.instance(super::msg::method::STOP, &placed.instance_id).await.unwrap();
    r.plane.lifecycle_tick().await; // mirrors "stopped"
    r.net.kill(&r.addr_a);
    r.plane.lifecycle_tick().await;
    past_dead_after().await;
    let ev = r.plane.lifecycle_tick().await;
    assert_eq!(actions(&ev), ["lost", "alert"], "{ev:#?}");
    assert!(ev[1].detail.contains("not running"), "{}", ev[1].detail);
    assert_eq!(count(&r.b).await, 0);
}

#[tokio::test]
async fn a_lost_instance_is_not_placed_on_an_unverified_node_until_it_is_verified() {
    use crate::cluster::{ClusterConfig, ClusterMembership, NodePlatform, NodeState, PeerNode};
    let mut r = rig().await;
    let membership = Arc::new(ClusterMembership::new(ClusterConfig::default()));
    r.plane = r.plane.with_membership(membership.clone());
    let peer = |id: &str, state: NodeState| PeerNode {
        id: id.to_string(),
        name: id.to_string(),
        platform: NodePlatform::CloudNative,
        state,
        address: None,
        first_seen: chrono::Utc::now(),
        last_heartbeat: chrono::Utc::now(),
        capabilities: vec![],
        labels: Default::default(),
    };
    membership.add_peer(peer(&r.a.id, NodeState::Active)).unwrap();
    membership.add_peer(peer(&r.b.id, NodeState::Active)).unwrap();
    r.plane.place(&mode_order(&r.pkg, RunMode::Listener, Some(&r.a.id))).await.unwrap();

    // Membership declares `a` dead at once; `b` is only an unverified claim.
    membership.update_state(&r.a.id, NodeState::Unreachable).unwrap();
    membership.update_state(&r.b.id, NodeState::Unverified).unwrap();
    let ev = r.plane.lifecycle_tick().await;
    assert_eq!(actions(&ev), ["lost", "reschedule_failed"], "{ev:#?}");
    assert_eq!(count(&r.b).await, 0, "never placed on an unverified node");
    assert!(!chained(&r.chain, EVENT_KIND_WORKLOAD_REFUSE).is_empty());
    // The same refusal is not chained again every tick.
    let n = chained(&r.chain, EVENT_KIND_WORKLOAD_REFUSE).len();
    assert!(r.plane.lifecycle_tick().await.is_empty());
    assert_eq!(chained(&r.chain, EVENT_KIND_WORKLOAD_REFUSE).len(), n);

    membership.update_state(&r.b.id, NodeState::Active).unwrap();
    let ev = r.plane.lifecycle_tick().await;
    assert_eq!(actions(&ev), ["rescheduled"], "{ev:#?}");
    assert_eq!(count(&r.b).await, 1);
}

#[tokio::test]
async fn a_lost_instance_is_not_placed_on_a_demoted_node() {
    let r = rig().await;
    r.plane.place(&mode_order(&r.pkg, RunMode::Listener, Some(&r.a.id))).await.unwrap();
    assert!(r.plane.set_tier(&r.b.id, TrustTier::Discovered), "operator demotes b");
    r.net.kill(&r.addr_a);
    r.plane.lifecycle_tick().await;
    past_dead_after().await;
    let ev = r.plane.lifecycle_tick().await;
    assert_eq!(actions(&ev), ["lost", "reschedule_failed"], "{ev:#?}");
    assert_eq!(count(&r.b).await, 0, "a demoted node gets nothing");

    assert!(r.plane.set_tier(&r.b.id, TrustTier::Paired));
    let ev = r.plane.lifecycle_tick().await;
    assert_eq!(actions(&ev), ["rescheduled"], "{ev:#?}");
    assert_eq!(count(&r.b).await, 1);
}

#[tokio::test]
async fn lifecycle_and_orders_survive_a_controller_restart() {
    let r = rig().await;
    let state = r._tmp.path().join("placements.json");
    let key = SigningKey::from_bytes(&[10; 32]);
    let (plane, _) = controller(&key, r.net.clone());
    let plane = plane.with_state_file(&state).unwrap().with_config(PlaneConfig {
        dead_after: DEAD_AFTER,
        ..PlaneConfig::default()
    });
    plane.add_target(&r.addr_a, TrustTier::Paired).await.unwrap();
    plane.add_target(&r.addr_b, TrustTier::Paired).await.unwrap();
    let placed = plane.place(&mode_order(&r.pkg, RunMode::Listener, Some(&r.a.id))).await.unwrap().placed.unwrap();

    // A new controller process reads the file; the order came with it.
    let (again, _) = controller(&key, r.net.clone());
    let again = again.with_state_file(&state).unwrap().with_config(PlaneConfig {
        dead_after: DEAD_AFTER,
        ..PlaneConfig::default()
    });
    assert_eq!(again.lifecycle_of(&placed.instance_id), Some(LifecycleState::Running));
    r.net.kill(&r.addr_a);
    again.lifecycle_tick().await;
    past_dead_after().await;
    let ev = again.lifecycle_tick().await;
    assert_eq!(actions(&ev), ["lost", "rescheduled"], "{ev:#?}");
    assert_eq!(count(&r.b).await, 1);
}
