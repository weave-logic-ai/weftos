//! A node's health heartbeat and bounded restarts, on real native adapters.

use std::sync::Arc;
use std::time::Duration;

use clawft_types::placement::TrustTier;
use ed25519_dalek::SigningKey;

use super::lifecycle::LifecycleState;
use super::msg::method;
use super::test_support::*;
use super::transport::MeshConnector;
use crate::chain::{EVENT_KIND_WORKLOAD_LIFECYCLE, EVENT_KIND_WORKLOAD_START};
use crate::workload_runtime::RunMode;

const T0: u64 = 1_000_000;
/// Longer than any kind's poll interval, so every call polls.
const STEP: u64 = 11_000;

struct Rig {
    node: HostNode,
    plane: super::plane::PlacementControlPlane,
    iid: String,
    _tmp: tempfile::TempDir,
}

async fn rig(script: &str, mode: RunMode) -> Rig {
    let tmp = tempfile::tempdir().unwrap();
    let pkg = package(tmp.path(), "life-cog", script, &[arch()]);
    let key = SigningKey::from_bytes(&[10; 32]);
    let node = host_node(21, board_caps("pi5"), true, &key);
    let conn = Arc::new(MeshConnector::new(false));
    let addr = conn.register_local("n", node.svc.clone());
    let (plane, _) = controller(&key, conn);
    plane.add_target(&addr, TrustTier::Paired).await.unwrap();
    let r = plane.place(&mode_order(&pkg, mode, None)).await.unwrap();
    let iid = r.placed.expect("placed").instance_id;
    Rig { node, plane, iid, _tmp: tmp }
}

/// One supervision pass at synthetic time `k`, after letting a short-lived
/// payload exit.
async fn pass(r: &Rig, k: u64) -> Vec<super::host_supervise::Supervised> {
    tokio::time::sleep(Duration::from_millis(120)).await;
    r.node.svc.supervise(T0 + k * STEP).await
}

fn life_events(r: &Rig) -> Vec<serde_json::Value> {
    events(&r.node.chain, EVENT_KIND_WORKLOAD_LIFECYCLE)
        .into_iter()
        .map(|(_, p)| p)
        .collect()
}

#[tokio::test]
async fn a_healthy_instance_is_polled_and_left_alone() {
    let r = rig("#!/bin/sh\nexec sleep 30\n", RunMode::Listener).await;
    for k in 0..5 {
        assert!(pass(&r, k).await.is_empty());
    }
    assert_eq!(r.node.svc.lifecycle_of(&r.iid).await, Some(LifecycleState::Running));
    assert!(life_events(&r).is_empty());
    r.plane.instance(method::STOP, &r.iid).await.unwrap();
}

#[tokio::test]
async fn a_crashing_instance_is_restarted_a_bounded_number_of_times_then_fails() {
    let r = rig("#!/bin/sh\nexit 3\n", RunMode::Listener).await;
    let mut seen = Vec::new();
    for k in 0..30 {
        seen.extend(pass(&r, k).await);
        if r.node.svc.lifecycle_of(&r.iid).await == Some(LifecycleState::Failed) {
            break;
        }
    }
    assert_eq!(r.node.svc.lifecycle_of(&r.iid).await, Some(LifecycleState::Failed));
    let restarts: Vec<_> = seen
        .iter()
        .filter(|s| s.to == LifecycleState::Restarting)
        .collect();
    assert_eq!(restarts.len(), 3, "exactly the budget: {seen:#?}");
    assert!(restarts[0].reason.contains("restart 1/3") && restarts[0].reason.contains("code 3"));
    let failed = seen.last().unwrap();
    assert_eq!(failed.to, LifecycleState::Failed);
    assert!(failed.reason.contains("budget spent"), "{}", failed.reason);

    // Every step is chained on the node, and each restart ran a gated start.
    let chained = life_events(&r);
    assert_eq!(chained.len(), seen.len());
    assert!(chained.iter().all(|p| p["phase"] == "supervise" && p["node"] == r.node.id));
    assert_eq!(
        chained.iter().filter(|p| p["to"] == "restarting").count(),
        3
    );
    assert!(
        events(&r.node.chain, EVENT_KIND_WORKLOAD_START).len() >= 4,
        "the placement start plus one start per restart"
    );

    // Held for the operator: no further restarts, and the controller sees why.
    assert!(pass(&r, 99).await.is_empty());
    let rows = r.plane.call(&r.node.id, method::STATUS, None, serde_json::json!({})).await.unwrap();
    assert_eq!(rows[0]["lifecycle"], "failed");
    assert_eq!(rows[0]["restarts"], 3);
    r.plane.instance(method::UNLOAD, &r.iid).await.unwrap();
}

#[tokio::test]
async fn an_instance_the_operator_stopped_is_never_restarted() {
    let r = rig("#!/bin/sh\nexec sleep 30\n", RunMode::Listener).await;
    r.plane.instance(method::STOP, &r.iid).await.unwrap();
    assert_eq!(r.node.svc.lifecycle_of(&r.iid).await, Some(LifecycleState::Stopped));
    for k in 0..8 {
        assert!(pass(&r, k).await.is_empty());
    }
    assert_eq!(r.node.svc.lifecycle_of(&r.iid).await, Some(LifecycleState::Stopped));
    // Starting it again puts it back under supervision.
    r.plane.instance(method::START, &r.iid).await.unwrap();
    assert_eq!(r.node.svc.lifecycle_of(&r.iid).await, Some(LifecycleState::Running));
    r.plane.instance(method::STOP, &r.iid).await.unwrap();
}

#[tokio::test]
async fn a_one_shot_run_that_ends_cleanly_is_finished_not_restarted() {
    let r = rig("#!/bin/sh\nexit 0\n", RunMode::Once).await;
    let starts = events(&r.node.chain, EVENT_KIND_WORKLOAD_START).len();
    let seen = pass(&r, 0).await;
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(seen[0].to, LifecycleState::Finished);
    for k in 1..6 {
        assert!(pass(&r, k).await.is_empty());
    }
    assert_eq!(r.node.svc.lifecycle_of(&r.iid).await, Some(LifecycleState::Finished));
    assert_eq!(
        events(&r.node.chain, EVENT_KIND_WORKLOAD_START).len(),
        starts,
        "never restarted"
    );
}

#[tokio::test]
async fn a_slow_poll_does_not_count_before_the_kinds_interval() {
    let r = rig("#!/bin/sh\nexit 3\n", RunMode::Listener).await;
    tokio::time::sleep(Duration::from_millis(120)).await;
    // First poll is due (never polled); the next one a second later is not.
    r.node.svc.supervise(T0).await;
    r.node.svc.supervise(T0 + 1_000).await;
    let rows = r.plane.call(&r.node.id, method::STATUS, None, serde_json::json!({})).await.unwrap();
    assert_eq!(rows[0]["lifecycle"], "running", "one miss of three so far");
    r.plane.instance(method::UNLOAD, &r.iid).await.unwrap();
}
