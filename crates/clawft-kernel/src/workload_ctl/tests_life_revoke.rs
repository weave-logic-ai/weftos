//! Revocation across nodes, including nodes that rejoin later, and the
//! scripted scenario: place, kill the node, reschedule, revoke, unload.

use std::sync::Arc;
use std::time::Duration;

use clawft_types::placement::TrustTier;
use ed25519_dalek::SigningKey;

use super::lifecycle::LifecycleState;
use super::plane::{PlacementControlPlane, PlaneConfig};
use super::test_support::*;
use crate::chain::{ChainManager, EVENT_KIND_WORKLOAD_UNLOAD};
use crate::mesh_runtime::MeshRuntime;
use crate::mesh_swarm_revoke::{RevocationExchange, sign_revocation};
use crate::revocation::{RevocationKind, RevocationList};
use crate::workload_governance::{NetworkPolicy, WorkloadGate, WorkloadPermitRule, revoke_and_record};
use crate::workload_runtime::RunMode;

const SCRIPT: &str = "#!/bin/sh\nexec sleep 60\n";

fn revoking_gate(chain: &Arc<ChainManager>, list: &Arc<RevocationList>) -> Arc<WorkloadGate> {
    let mut permit = WorkloadPermitRule::new("life-revoke", ["workload.*"], ["cog"]);
    permit.max_network = NetworkPolicy::Egress;
    Arc::new(
        WorkloadGate::exempt(0.95, false, "test")
            .with_permit(permit)
            .unwrap()
            .with_chain(chain.clone())
            .with_revocations(list.clone()),
    )
}

/// One mesh member: a `workload-host`, its revocation list and the notice
/// exchange that enforces what arrives.
struct Member {
    host: HostNode,
    list: Arc<RevocationList>,
    rt: Arc<MeshRuntime>,
    rev: Arc<RevocationExchange>,
    addr: String,
}

fn member(seed: u8, net: &Killable, tmp: &std::path::Path, key: &SigningKey) -> Member {
    let list = Arc::new(RevocationList::new(tmp.join(format!("revoked-{seed}.json"))));
    let l = list.clone();
    let host = host_node_with(seed, board_caps("pi5"), true, key, move |c| revoking_gate(c, &l));
    let rt = Arc::new(MeshRuntime::new(host.id.clone()));
    let rev = RevocationExchange::start(exchange(&host.id, &host.chain), list.clone(), anchors(), rt.clone());
    // What the daemon does: a notice that was new here sweeps the node.
    let (svc, l) = (host.svc.clone(), list.clone());
    rev.set_on_applied(Arc::new(move |_| {
        let (svc, l) = (svc.clone(), l.clone());
        tokio::spawn(async move {
            svc.enforce_revocations(&l).await;
        });
    }));
    let addr = net.serve(&format!("m{seed}"), host.svc.clone());
    Member { host, list, rt, rev, addr }
}

fn link(a: &Member, b: &Member) {
    let (tx_ab, mut rx_ab) = tokio::sync::mpsc::channel::<Vec<u8>>(64);
    let (tx_ba, mut rx_ba) = tokio::sync::mpsc::channel::<Vec<u8>>(64);
    let (b_rt, back) = (b.rt.clone(), tx_ba.clone());
    tokio::spawn(async move {
        while let Some(bytes) = rx_ab.recv().await {
            let _ = b_rt.handle_incoming_peer(&bytes, back.clone(), None).await;
        }
    });
    let (a_rt, back) = (a.rt.clone(), tx_ab.clone());
    tokio::spawn(async move {
        while let Some(bytes) = rx_ba.recv().await {
            let _ = a_rt.handle_incoming_peer(&bytes, back.clone(), None).await;
        }
    });
    a.rt.add_peer(b.host.id.clone(), tx_ab);
    b.rt.add_peer(a.host.id.clone(), tx_ba);
}

async fn wait_for(what: &str, mut ok: impl FnMut() -> bool) {
    for _ in 0..600 {
        if ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("timed out waiting for {what}");
}

async fn count(n: &HostNode) -> usize {
    n.svc.instances.lock().await.len()
}

#[tokio::test]
async fn a_revocation_unloads_everywhere_including_a_node_that_rejoins_later() {
    let tmp = tempfile::tempdir().unwrap();
    let pkg = package(tmp.path(), "rev-cog", SCRIPT, &[arch()]);
    let key = SigningKey::from_bytes(&[10; 32]);
    let net = Killable::new();
    // The operator's node (no host of its own), and two hosts.
    let op = member(60, &net, tmp.path(), &key);
    let b = member(61, &net, tmp.path(), &key);
    let c = member(62, &net, tmp.path(), &key);
    let (plane, _) = controller(&key, net.clone());
    plane.add_target(&b.addr, TrustTier::Paired).await.unwrap();
    plane.add_target(&c.addr, TrustTier::Paired).await.unwrap();
    let on_b = plane.place(&mode_order(&pkg, RunMode::Listener, Some(&b.host.id))).await.unwrap().placed.unwrap();
    let on_c = plane.place(&mode_order(&pkg, RunMode::Listener, Some(&c.host.id))).await.unwrap().placed.unwrap();
    assert_eq!((on_b.node_id.as_str(), on_c.node_id.as_str()), (b.host.id.as_str(), c.host.id.as_str()));
    let package_id = on_b.package_id.clone().unwrap();

    // C is offline when the operator revokes: only B is linked.
    link(&op, &b);
    let notice = sign_revocation(RevocationKind::Package, &package_id, "compromised", 1, &signer()).unwrap();
    assert!(op.rev.issue(notice).await.unwrap());
    wait_for("B to unload", || b.list.is_subject_revoked(RevocationKind::Package, &package_id)).await;
    for _ in 0..400 {
        if count(&b.host).await == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(count(&b.host).await, 0, "B stopped and unloaded it");
    assert_eq!(count(&c.host).await, 1, "C has not heard yet");
    assert!(!c.list.is_subject_revoked(RevocationKind::Package, &package_id));

    // C rejoins through B: B replays what it holds, and C unloads.
    assert_eq!(b.rev.logged().len(), 1);
    link(&b, &c);
    wait_for("C to learn the revocation", || c.list.is_subject_revoked(RevocationKind::Package, &package_id)).await;
    for _ in 0..400 {
        if count(&c.host).await == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(count(&c.host).await, 0, "C unloaded after it rejoined");
    assert!(
        events(&c.host.chain, EVENT_KIND_WORKLOAD_UNLOAD)
            .iter()
            .any(|(_, p)| p["forced_by_revocation"]["kind"] == "package"),
        "chained on C as a forced unload"
    );
    assert_eq!(c.rev.logged().len(), 1, "C can pass it on to the next node that joins");
}

#[tokio::test]
async fn the_replay_log_survives_a_restart_and_drops_what_no_longer_verifies() {
    let tmp = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&[10; 32]);
    let net = Killable::new();
    let file = tmp.path().join("revocation-notices.json");
    let hash = crate::workload_pkg::codec::hex_encode(&[7u8; 32]);

    let first = member(70, &net, tmp.path(), &key);
    assert_eq!(first.rev.with_log_file(&file), 0);
    let notice = sign_revocation(RevocationKind::ArtifactHash, &hash, "test", 1, &signer()).unwrap();
    assert!(first.rev.issue(notice.clone()).await.unwrap());
    assert!(file.exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
    }

    // A restarted node (fresh list and runtime) reads the log back and a
    // new peer that joins it receives the notice.
    let second = member(71, &net, tmp.path(), &key);
    // The node's own list persists across a restart; the log is read after it.
    revoke_and_record(&second.list, None, RevocationKind::ArtifactHash, &hash, "test", "operator").unwrap();
    assert_eq!(second.rev.with_log_file(&file), 1);
    let joiner = member(72, &net, tmp.path(), &key);
    assert!(!joiner.list.is_subject_revoked(RevocationKind::ArtifactHash, &hash));
    link(&second, &joiner);
    wait_for("the joiner to learn it", || {
        joiner.list.is_subject_revoked(RevocationKind::ArtifactHash, &hash)
    })
    .await;

    // A notice signed by a key the node does not pin is not kept.
    let rogue = sign_revocation(RevocationKind::ArtifactHash, &hash, "x", 1, &SigningKey::from_bytes(&[99; 32])).unwrap();
    std::fs::write(&file, serde_json::to_vec(&vec![notice, rogue]).unwrap()).unwrap();
    let third = member(73, &net, tmp.path(), &key);
    revoke_and_record(&third.list, None, RevocationKind::ArtifactHash, &hash, "test", "operator").unwrap();
    assert_eq!(third.rev.with_log_file(&file), 1, "only the verifiable one");
    // Garbage is ignored, not fatal.
    std::fs::write(&file, b"not json").unwrap();
    assert_eq!(member(74, &net, tmp.path(), &key).rev.with_log_file(&file), 0);
}

#[tokio::test]
async fn scripted_scenario_place_kill_reschedule_revoke_unload() {
    let tmp = tempfile::tempdir().unwrap();
    let pkg = package(tmp.path(), "scenario-cog", SCRIPT, &[arch()]);
    let key = SigningKey::from_bytes(&[10; 32]);
    let net = Killable::new();
    let list = Arc::new(RevocationList::new(tmp.path().join("revoked-plane.json")));
    let a = member(80, &net, tmp.path(), &key);
    let b = member(81, &net, tmp.path(), &key);
    let chain = Arc::new(ChainManager::new(0, 1000));
    let id = crate::node_id_from_pubkey(&key.verifying_key().to_bytes());
    let plane = PlacementControlPlane::new(
        key.clone(),
        revoking_gate(&chain, &list),
        chain.clone(),
        exchange(&id, &chain),
        anchors(),
        net.clone(),
    )
    .with_config(PlaneConfig {
        dead_after: Duration::from_millis(200),
        ..PlaneConfig::default()
    });
    plane.add_target(&a.addr, TrustTier::Paired).await.unwrap();
    plane.add_target(&b.addr, TrustTier::Paired).await.unwrap();

    // 1. place on a. 2. kill a. 3. it is rescheduled onto b.
    let placed = plane.place(&mode_order(&pkg, RunMode::Listener, Some(&a.host.id))).await.unwrap().placed.unwrap();
    assert_eq!(placed.node_id, a.host.id);
    net.kill(&a.addr);
    plane.lifecycle_tick().await;
    tokio::time::sleep(Duration::from_millis(280)).await;
    let ev = plane.lifecycle_tick().await;
    assert_eq!(ev.iter().map(|e| e.action.as_str()).collect::<Vec<_>>(), ["lost", "rescheduled"], "{ev:#?}");
    assert_eq!(count(&b.host).await, 1);
    let package_id = placed.package_id.clone().unwrap();

    // 4. revoke the package: b unloads it and the controller drops it.
    revoke_and_record(&b.list, Some(&b.host.chain), RevocationKind::Package, &package_id, "test", "operator").unwrap();
    revoke_and_record(&list, Some(&chain), RevocationKind::Package, &package_id, "test", "operator").unwrap();
    let forced = b.host.svc.enforce_revocations(&b.list).await;
    assert_eq!(forced.len(), 1);
    assert!(forced[0].unloaded);
    assert_eq!(count(&b.host).await, 0);
    let ev = plane.lifecycle_tick().await;
    assert!(ev.iter().any(|e| e.action == "gone" && e.node_id == b.host.id), "{ev:#?}");
    let live: Vec<_> = plane
        .placements()
        .into_iter()
        .filter(|p| plane.lifecycle_of(&p.instance_id) != Some(LifecycleState::Rescheduled))
        .collect();
    assert!(live.is_empty(), "{live:#?}");

    // 5. a revoked package cannot be placed again, wherever it would go.
    net.revive(&a.addr);
    plane.refresh().await;
    let again = plane.place(&mode_order(&pkg, RunMode::Listener, None)).await;
    assert!(
        again.as_ref().map_or(true, |r| r.placed.is_none()),
        "revoked package was placed: {again:?}"
    );
    assert_eq!(count(&b.host).await, 0);
}

fn hash_of(i: u8) -> String {
    crate::workload_pkg::codec::hex_encode(&[i; 32])
}

/// Revoke `n` artifact hashes on `m` (list and notice log), as the operator would.
async fn revoke_many(m: &Member, n: u8) {
    for i in 1..=n {
        let notice = sign_revocation(RevocationKind::ArtifactHash, &hash_of(i), "t", 1, &signer()).unwrap();
        assert!(m.rev.issue(notice).await.unwrap());
    }
}

fn recovered(id: &str, verified: bool) -> crate::mesh_discovery::MeshPeerEvent {
    crate::mesh_discovery::MeshPeerEvent::Recovered {
        node_id: id.to_string(),
        address: None,
        verified,
    }
}

#[tokio::test]
async fn repeated_recoveries_of_one_peer_cause_one_replay_and_unverified_peers_get_none() {
    let tmp = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&[10; 32]);
    let net = Killable::new();
    let a = member(90, &net, tmp.path(), &key);
    let b = member(91, &net, tmp.path(), &key);
    revoke_many(&a, 2).await;
    link(&a, &b); // Joined, verified: replay number one
    wait_for("the first replay", || a.rev.replays_started() == 1).await;
    wait_for("B to learn both", || {
        (1..=2).all(|i| b.list.is_subject_revoked(RevocationKind::ArtifactHash, &hash_of(i)))
    })
    .await;

    // A link that flaps: five more recoveries inside the cooldown.
    for _ in 0..5 {
        a.rt.emit_peer_event(recovered(&b.host.id, true));
    }
    // A peer admission did not verify is never replayed to.
    a.rt.emit_peer_event(recovered("someone-else", false));
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(a.rev.replays_started(), 1, "cooldown and verification bound the replays");
}

#[tokio::test]
async fn a_replay_stops_when_its_peer_leaves() {
    let tmp = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&[10; 32]);
    let net = Killable::new();
    let a = member(92, &net, tmp.path(), &key);
    let b = member(93, &net, tmp.path(), &key);
    // More than the receiver's burst, so the replay is paced and still running.
    revoke_many(&a, 14).await;
    link(&a, &b);
    let learned = || (1..=14).filter(|i| b.list.is_subject_revoked(RevocationKind::ArtifactHash, &hash_of(*i))).count();
    wait_for("the burst to arrive", || learned() >= 10).await;
    a.rt.emit_peer_event(crate::mesh_discovery::MeshPeerEvent::Left { node_id: b.host.id.clone() });
    tokio::time::sleep(Duration::from_millis(1_800)).await;
    assert_eq!(learned(), 10, "the paced remainder was not sent after the peer left");
}

#[tokio::test]
async fn a_lifted_revocation_does_not_come_back_through_the_replay_log() {
    let tmp = tempfile::tempdir().unwrap();
    let key = SigningKey::from_bytes(&[10; 32]);
    let net = Killable::new();
    let a = member(94, &net, tmp.path(), &key);
    let b = member(95, &net, tmp.path(), &key);
    revoke_many(&a, 2).await;
    assert_eq!(a.rev.logged().len(), 2);

    // The operator lifts one of them on this node.
    assert!(
        crate::workload_governance::unrevoke_and_record(
            &a.list, None, RevocationKind::ArtifactHash, &hash_of(1), "operator"
        )
        .unwrap()
    );
    assert_eq!(a.rev.logged().len(), 1, "its notice left the log");

    link(&a, &b);
    wait_for("B to learn the other", || {
        b.list.is_subject_revoked(RevocationKind::ArtifactHash, &hash_of(2))
    })
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!b.list.is_subject_revoked(RevocationKind::ArtifactHash, &hash_of(1)), "not re-revoked");

    // Explicit purge works too, and the saved log follows.
    let c = member(96, &net, tmp.path(), &key);
    let file = tmp.path().join("log.json");
    c.rev.with_log_file(&file);
    revoke_many(&c, 1).await;
    assert_eq!(c.rev.forget_subject(RevocationKind::ArtifactHash, &hash_of(1)), 1);
    let saved: Vec<crate::mesh_swarm_revoke::SignedRevocation> =
        serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert!(saved.is_empty());
}
