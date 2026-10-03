//! Restart and loss handling: a node that comes back without its instances,
//! retry with backoff, the caps, and unload failures on a returned node.

use std::sync::Arc;
use std::time::Duration;

use clawft_types::placement::TrustTier;
use ed25519_dalek::SigningKey;

use super::lifecycle::{LifecyclePolicy, LifecycleState};
use super::plane::PlaneConfig;
use super::test_support::*;
use super::tests_life_plane::{actions, chained, count, cfg, past_backoff, past_dead_after, rig};
use crate::chain::{EVENT_KIND_WORKLOAD_LIFECYCLE, EVENT_KIND_WORKLOAD_MIGRATE, EVENT_KIND_WORKLOAD_UNLOAD};
use crate::workload_governance::{NetworkPolicy, WorkloadGate, WorkloadPermitRule};
use crate::workload_runtime::RunMode;

fn listener(r: &super::tests_life_plane::Rig) -> super::plane_place::PlaceOrder {
    mode_order(&r.pkg, RunMode::Listener, Some(&r.a.id))
}

/// A reboot: everything the node held, in the host and in its adapter, is gone.
async fn reboot(n: &HostNode) {
    let held: Vec<_> = n.svc.instances.lock().await.drain().collect();
    for (_, p) in held {
        let _ = n.svc.routes[&p.route].unload(p.handle).await;
    }
}

#[tokio::test]
async fn a_node_that_reboots_inside_the_grace_period_gets_its_instance_placed_again() {
    let r = rig().await;
    let old = r.plane.place(&listener(&r)).await.unwrap().placed.unwrap();
    // The node restarts: it answers, but its in-memory instances are gone.
    reboot(&r.a).await;
    let ev = r.plane.lifecycle_tick().await;
    assert_eq!(actions(&ev), ["lost", "replaced"], "{ev:#?}");
    assert_eq!(count(&r.a).await + count(&r.b).await, 1, "one copy, placed again");
    let new = r.plane.placements().into_iter().next().expect("one record");
    assert_eq!(r.plane.lifecycle_of(&new.instance_id), Some(LifecycleState::Running));
    assert_eq!(r.plane.life_of(&new.instance_id).unwrap().reschedules, 1);
    if new.instance_id != old.instance_id {
        assert!(r.plane.lifecycle_of(&old.instance_id).is_none());
    }
    let mig = chained(&r.chain, EVENT_KIND_WORKLOAD_MIGRATE);
    assert_eq!(mig[0]["phase"], "replaced_after_loss");
    assert_eq!(mig[0]["reason"], "instance_lost");
}

#[tokio::test]
async fn an_instance_the_node_took_down_on_purpose_is_not_placed_again() {
    let r = rig().await;
    let old = r.plane.place(&listener(&r)).await.unwrap().placed.unwrap();
    // Unloaded behind the controller's back (another controller, say).
    let decision = "ab".repeat(32);
    r.plane
        .call(&r.a.id, super::msg::method::UNLOAD, Some(decision), serde_json::json!({ "instance_id": old.instance_id }))
        .await
        .unwrap();
    let ev = r.plane.lifecycle_tick().await;
    assert_eq!(actions(&ev), ["gone"], "{ev:#?}");
    assert!(ev[0].detail.contains("unloaded"), "{}", ev[0].detail);
    assert_eq!(count(&r.a).await + count(&r.b).await, 0);
    assert!(r.plane.placements().is_empty());
}

#[tokio::test]
async fn a_pinned_node_that_reboots_after_the_grace_period_gets_it_back_on_the_same_node() {
    let r = rig().await;
    let mut o = listener(&r);
    o.pin = Some(r.a.id.clone());
    r.plane.place(&o).await.unwrap();
    r.net.kill(&r.addr_a);
    r.plane.lifecycle_tick().await;
    past_dead_after().await;
    let ev = r.plane.lifecycle_tick().await;
    assert_eq!(actions(&ev), ["lost", "alert"], "{ev:#?}");

    // It comes back from a reboot with nothing loaded.
    reboot(&r.a).await;
    r.net.revive(&r.addr_a);
    let ev = r.plane.lifecycle_tick().await;
    assert_eq!(actions(&ev), ["replaced"], "{ev:#?}");
    assert_eq!((count(&r.a).await, count(&r.b).await), (1, 0), "pinned: back on the same node");
}

#[tokio::test]
async fn a_reschedule_that_finds_no_node_is_retried_with_backoff_up_to_the_cap() {
    let r = rig().await;
    let policy = LifecyclePolicy { max_reschedules: 2, ..LifecyclePolicy::default() };
    let placed = r.plane.place_with(&listener(&r), policy).await.unwrap().placed.unwrap();
    assert!(r.plane.set_tier(&r.b.id, TrustTier::Discovered));
    r.net.kill(&r.addr_a);
    r.plane.lifecycle_tick().await;
    past_dead_after().await;
    let ev = r.plane.lifecycle_tick().await;
    assert_eq!(actions(&ev), ["lost", "reschedule_failed"], "{ev:#?}");
    let attempts = |r: &super::tests_life_plane::Rig| r.plane.life_of(&placed.instance_id).unwrap().attempts;
    assert_eq!(attempts(&r), 1);
    // Inside the backoff nothing is attempted.
    assert!(r.plane.lifecycle_tick().await.is_empty());
    assert_eq!(attempts(&r), 1);
    // The second failure reaches the cap and says so.
    past_backoff().await;
    let ev = r.plane.lifecycle_tick().await;
    assert_eq!(actions(&ev), ["alert"], "{ev:#?}");
    assert!(ev[0].detail.contains("gave up"), "{}", ev[0].detail);
    assert_eq!(attempts(&r), 2);

    // Capacity returns too late: the cap holds.
    assert!(r.plane.set_tier(&r.b.id, TrustTier::Paired));
    past_backoff().await;
    r.plane.lifecycle_tick().await;
    assert_eq!(count(&r.b).await, 0);
    assert_eq!(attempts(&r), 2, "no further attempts");
}

#[tokio::test]
async fn an_instance_is_rescheduled_at_most_max_reschedules_times() {
    let r = rig().await;
    let policy = LifecyclePolicy { max_reschedules: 1, ..LifecyclePolicy::default() };
    r.plane.place_with(&listener(&r), policy).await.unwrap();
    r.net.kill(&r.addr_a);
    r.plane.lifecycle_tick().await;
    past_dead_after().await;
    let ev = r.plane.lifecycle_tick().await;
    assert_eq!(actions(&ev), ["lost", "rescheduled"], "{ev:#?}");
    // The replacement's node dies too: the cap is spent.
    r.net.kill(&r.addr_b);
    r.plane.lifecycle_tick().await;
    past_dead_after().await;
    let ev = r.plane.lifecycle_tick().await;
    assert_eq!(actions(&ev), ["lost", "alert"], "{ev:#?}");
    assert!(ev[1].detail.contains("already rescheduled 1 times"), "{}", ev[1].detail);
}

#[tokio::test]
async fn an_adopted_instance_without_an_order_raises_an_alert_instead_of_moving() {
    let r = rig().await;
    let placed = r.plane.place(&listener(&r)).await.unwrap().placed.unwrap();
    // As after adoption of a lost `place` answer: a record with no order.
    r.plane.lives.lock().unwrap().remove(&placed.instance_id);
    r.plane.lifecycle_tick().await;
    assert_eq!(r.plane.lifecycle_of(&placed.instance_id), Some(LifecycleState::Running));
    r.net.kill(&r.addr_a);
    r.plane.lifecycle_tick().await;
    past_dead_after().await;
    let ev = r.plane.lifecycle_tick().await;
    assert_eq!(actions(&ev), ["lost", "alert"], "{ev:#?}");
    assert!(ev[1].detail.contains("no stored placement order"), "{}", ev[1].detail);
    assert_eq!(count(&r.b).await, 0);
}

#[tokio::test]
async fn a_refused_orphan_unload_backs_off_and_is_chained_once_per_reason() {
    // Node `a` has no unload permit, so it refuses to unload the old copy.
    let tmp = tempfile::tempdir().unwrap();
    let pkg = package(tmp.path(), "orphan-cog", "#!/bin/sh\nexec sleep 60\n", &[arch()]);
    let key = SigningKey::from_bytes(&[10; 32]);
    let a = host_node_with(33, board_caps("pi5"), true, &key, |c| {
        let mut p = WorkloadPermitRule::new("no-unload", ["workload.place", "workload.load", "workload.start", "workload.stop"], ["cog"]);
        p.max_network = NetworkPolicy::Egress;
        Arc::new(WorkloadGate::exempt(0.95, false, "test").with_permit(p).unwrap().with_chain(c.clone()))
    });
    let b = host_node(34, board_caps("pi5"), true, &key);
    let net = Killable::new();
    let (addr_a, addr_b) = (net.serve("a", a.svc.clone()), net.serve("b", b.svc.clone()));
    let (plane, chain) = controller(&key, net.clone());
    let plane = plane.with_config(PlaneConfig { retry_backoff: Duration::from_secs(3600), ..cfg() });
    plane.add_target(&addr_a, TrustTier::Paired).await.unwrap();
    plane.add_target(&addr_b, TrustTier::Paired).await.unwrap();
    let old = plane.place(&mode_order(&pkg, RunMode::Listener, Some(&a.id))).await.unwrap().placed.unwrap();
    net.kill(&addr_a);
    plane.lifecycle_tick().await;
    past_dead_after().await;
    plane.lifecycle_tick().await;
    net.revive(&addr_a);

    let ev = plane.lifecycle_tick().await;
    assert_eq!(actions(&ev), ["orphan_unload_failed"], "{ev:#?}");
    assert_eq!(count(&a).await, 1, "the node refused");
    let requests = |c: &crate::chain::ChainManager| {
        events(c, EVENT_KIND_WORKLOAD_UNLOAD).iter().filter(|(_, p)| p["phase"] == "request").count()
    };
    let n = requests(&chain);
    // Backing off: no new attempt, no new chain events.
    for _ in 0..3 {
        assert!(plane.lifecycle_tick().await.is_empty());
    }
    assert_eq!(requests(&chain), n);
    assert_eq!(plane.life_of(&old.instance_id).unwrap().orphan_attempts, 1);
    // Retried when the backoff ends; the same failure is not chained again.
    plane.update_life(&old.instance_id, |l| l.orphan_next_ms = 0);
    assert!(plane.lifecycle_tick().await.is_empty());
    assert_eq!(plane.life_of(&old.instance_id).unwrap().orphan_attempts, 2);
    let alerts = chained(&chain, EVENT_KIND_WORKLOAD_LIFECYCLE)
        .into_iter()
        .filter(|p| p["phase"] == "alert" && p["reason"].as_str().unwrap().contains("not unloaded"))
        .count();
    assert_eq!(alerts, 1);
}
