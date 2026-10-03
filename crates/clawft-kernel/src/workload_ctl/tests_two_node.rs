//! Hermetic two-node placement over the real wire (card 12 acceptance, in
//! process): envelopes, signatures, fetch-before-load with the piece
//! protocol, native adapters, governance and chains are all real; only the
//! transport is in-memory. The same run on real hardware (Mac + Pi 5) is
//! `scripts/build.sh test-pi --placement`.

use std::sync::Arc;
use std::time::Duration;

use clawft_types::placement::TrustTier;
use ed25519_dalek::SigningKey;
use serde_json::json;

use super::host_service::CtlConfig;
use super::msg::{CtlOutcome, CtlRequest, RefusalCode, method, verify_response};
use super::plane_place::PlaceOrder;
use super::session::CtlConnection;
use super::test_support::*;
use super::transport::{CtlConnector, MeshConnector};
use crate::chain::{
    EVENT_KIND_ARTIFACT_FETCH, EVENT_KIND_WORKLOAD_PLACE, EVENT_KIND_WORKLOAD_REFUSE,
};
use crate::workload_runtime::RunMode;

const SCRIPT: &str = "#!/bin/sh\necho placed-ok\nexec sleep 30\n";

fn order(pkg: &std::path::Path) -> PlaceOrder {
    PlaceOrder {
        package_dir: pkg.to_path_buf(),
        config: CtlConfig {
            mode: RunMode::Listener,
            args: vec![],
            csi_port: 15006,
        },
        pin: None,
        prefer: vec![],
        avoid: vec![],
        allow_emulated: false,
        start: true,
        dry_run: false,
        project_id: None,
    }
}

/// Wait (bounded) for `path` to exist.
async fn wait_for_file(path: &std::path::Path) {
    for _ in 0..1_500 {
        if path.exists() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for {}", path.display());
}

fn ctl_key() -> SigningKey {
    SigningKey::from_bytes(&[10; 32])
}

#[tokio::test]
async fn place_prefers_the_arm_board_over_the_dev_mac_and_explains_why() {
    let tmp = tempfile::tempdir().unwrap();
    // The payload drops a marker file once it has printed: the test waits on
    // that, not on a guess of how long a loaded host takes to exec it.
    let printed = tmp.path().join("printed");
    let script = format!("#!/bin/sh\necho placed-ok\n: > '{}'\nexec sleep 30\n", printed.display());
    let pkg = package(tmp.path(), "probe-cog", &script, &[arch()]);
    let key = ctl_key();
    // The Mac's own adapter cannot run the payload (as macOS cannot run a
    // Linux ELF); the board's can.
    let mac = host_node(11, mac_caps(), false, &key);
    let pi = host_node(12, board_caps("pi5"), true, &key);
    let conn = Arc::new(MeshConnector::new(false));
    let mac_addr = conn.register_local("mac", mac.svc.clone());
    let pi_addr = conn.register_local("pi", pi.svc.clone());
    let (plane, chain) = controller(&key, conn);
    assert_eq!(
        plane
            .add_target(&mac_addr, TrustTier::Pinned)
            .await
            .unwrap(),
        mac.id
    );
    assert_eq!(
        plane.add_target(&pi_addr, TrustTier::Paired).await.unwrap(),
        pi.id
    );
    assert_eq!(plane.hosts().len(), 2, "both workload-host adverts merged");

    let r = plane.place(&order(&pkg)).await.unwrap();
    let p = r.decision.placement.as_ref().unwrap();
    assert_eq!(p.node_id, pi.id, "{}", r.explain);
    assert_eq!(p.variant, format!("{}-native", arch()));
    let mac_row = r
        .decision
        .candidates
        .iter()
        .find(|c| c.node_id == mac.id)
        .unwrap();
    assert!(mac_row.eligible(), "the Mac is a fallback, not rejected");
    assert_eq!(mac_row.tier.unwrap().as_str(), "dev_fallback");
    assert!(r.explain.contains(&format!(
        "PLACED on {} via {}-native (tier native",
        pi.id,
        arch()
    )));
    assert!(r.explain.contains("native > emulated > dev_fallback"));
    assert!(r.explain.contains(&format!(
        "{} eligible via {}-container (tier dev_fallback)",
        mac.id,
        arch()
    )));
    assert_eq!(r.attempts.len(), 1);
    assert_eq!(r.attempts[0].outcome, "placed");

    // The payload was fetched from the controller before load, and chained.
    assert_eq!(
        events(&pi.chain, EVENT_KIND_ARTIFACT_FETCH).len(),
        3,
        "manifest, cog.toml, binary"
    );
    let placed = r.placed.clone().unwrap();
    let status = plane
        .instance(method::STATUS, &placed.instance_id)
        .await
        .unwrap();
    assert_eq!(status["status"]["state"], "running");

    // Decision and placement chained on the controller, placement on the target.
    let ctl_place = events(&chain, EVENT_KIND_WORKLOAD_PLACE);
    let decision = ctl_place
        .iter()
        .find(|(_, p)| p["phase"] == "decision")
        .unwrap();
    assert_eq!(
        decision.1["decision"]["placement"]["node_id"],
        pi.id.as_str()
    );
    assert!(
        ctl_place
            .iter()
            .any(|(_, p)| p["phase"] == "placed" && p["decision_id"] == r.decision_id.as_str())
    );
    let host_place = events(&pi.chain, EVENT_KIND_WORKLOAD_PLACE);
    assert!(
        host_place
            .iter()
            .any(|(s, p)| s == "workload.host" && p["decision_id"] == r.decision_id.as_str())
    );

    // Stopping signals the process group, so it must have printed first
    // (stdout is read back from the run's evidence at stop).
    wait_for_file(&printed).await;
    plane
        .instance(method::STOP, &placed.instance_id)
        .await
        .unwrap();
    let logs = plane
        .instance(method::LOGS, &placed.instance_id)
        .await
        .unwrap();
    assert!(logs["stdout"].as_str().unwrap().contains("placed-ok"));
    plane
        .instance(method::UNLOAD, &placed.instance_id)
        .await
        .unwrap();
    assert!(plane.placements().is_empty());
}

#[tokio::test]
async fn admission_refusal_is_chained_and_the_next_candidate_is_tried() {
    let tmp = tempfile::tempdir().unwrap();
    let pkg = package(tmp.path(), "probe-cog", SCRIPT, &[arch()]);
    let key = ctl_key();
    // Both advertise the same board facts; the first's adapter disagrees
    // at admission (its runtime refuses the payload).
    let liar = host_node(21, board_caps("pi5"), false, &key);
    let good = host_node(22, board_caps("pi5"), true, &key);
    let conn = Arc::new(MeshConnector::new(false));
    let a = conn.register_local("liar", liar.svc.clone());
    let b = conn.register_local("good", good.svc.clone());
    let (plane, chain) = controller(&key, conn);
    plane.add_target(&a, TrustTier::Paired).await.unwrap();
    plane.add_target(&b, TrustTier::Paired).await.unwrap();
    let mut o = order(&pkg);
    o.prefer = vec![liar.id.clone()];

    let r = plane.place(&o).await.unwrap();
    assert_eq!(
        r.decision.placement.as_ref().unwrap().node_id,
        liar.id,
        "engine picks the preferred node"
    );
    assert_eq!(r.attempts.len(), 2, "{}", r.explain);
    assert_eq!(r.attempts[0].outcome, "refused");
    assert_eq!(r.attempts[0].code.as_deref(), Some("admission"));
    assert_eq!(r.attempts[1].outcome, "placed");
    assert_eq!(r.placed.as_ref().unwrap().node_id, good.id);
    assert!(r.explain.contains("refused (admission:"));

    // Chained on the controller (dispatch refusal) and on the refusing target.
    let ctl = events(&chain, EVENT_KIND_WORKLOAD_REFUSE);
    assert!(ctl.iter().any(|(_, p)| p["phase"] == "dispatch"
        && p["code"] == "admission"
        && p["node"] == liar.id.as_str()));
    let tgt = events(&liar.chain, EVENT_KIND_WORKLOAD_REFUSE);
    assert!(
        tgt.iter()
            .any(|(s, p)| s == "workload.host" && p["code"] == "admission")
    );
    assert!(
        tgt.iter()
            .any(|(s, p)| s == "workload.runtime" && p["error_code"] == "admission-refused")
    );
    let id = r.placed.unwrap().instance_id;
    plane.instance(method::UNLOAD, &id).await.unwrap();
}

#[tokio::test]
async fn unknown_workload_method_is_denied_by_default_and_chained() {
    let key = ctl_key();
    let host = host_node(31, board_caps("pi5"), true, &key);
    let conn = MeshConnector::new(false);
    let addr = conn.register_local("h", host.svc.clone());
    let req = CtlRequest::new(
        &key,
        "workload.frobnicate",
        &host.id,
        now(),
        60_000,
        Some("d".into()),
        json!({}),
    );
    let signed = req.sign(&key).unwrap();
    let mut c = CtlConnection::new(conn.connect(&addr).await.unwrap(), "ctl");
    let resp = c
        .call(
            &host.id,
            &req.method,
            &signed,
            None,
            Duration::from_secs(10),
        )
        .await
        .unwrap();
    let (r, _) = verify_response(&resp, &req, None).unwrap();
    match r.outcome {
        CtlOutcome::Refused { refusal } => assert_eq!(refusal.code, RefusalCode::UnknownMethod),
        other => panic!("unknown method served: {other:?}"),
    }
    let refused = events(&host.chain, EVENT_KIND_WORKLOAD_REFUSE);
    assert!(
        refused
            .iter()
            .any(|(s, p)| s == "workload" && p["action"] == "workload.frobnicate"),
        "gate chained it"
    );
    assert!(
        refused
            .iter()
            .any(|(s, p)| s == "workload.host" && p["code"] == "unknown_method")
    );
}

fn now() -> u64 {
    chrono::Utc::now().timestamp_millis() as u64
}

#[tokio::test]
async fn unauthorised_and_replayed_requests_are_refused_and_chained() {
    let key = ctl_key();
    let host = host_node(41, board_caps("pi5"), true, &key);
    let conn = Arc::new(MeshConnector::new(false));
    let addr = conn.register_local("h", host.svc.clone());
    // A controller the node does not list cannot even describe it.
    let (stranger, _) = controller(&SigningKey::from_bytes(&[99; 32]), conn.clone());
    let e = stranger
        .add_target(&addr, TrustTier::Paired)
        .await
        .unwrap_err();
    assert!(e.to_string().contains("unauthorized"), "{e}");
    assert!(
        events(&host.chain, EVENT_KIND_WORKLOAD_REFUSE)
            .iter()
            .any(|(_, p)| p["code"] == "unauthorized")
    );

    // The same signed request twice: the second is a replay.
    let req = CtlRequest::new(
        &key,
        method::STATUS,
        &host.id,
        now(),
        60_000,
        None,
        json!({}),
    );
    let signed = req.sign(&key).unwrap();
    let mut outcomes = Vec::new();
    for _ in 0..2 {
        let mut c = CtlConnection::new(conn.connect(&addr).await.unwrap(), "ctl");
        let resp = c
            .call(
                &host.id,
                method::STATUS,
                &signed,
                None,
                Duration::from_secs(10),
            )
            .await
            .unwrap();
        outcomes.push(verify_response(&resp, &req, None).unwrap().0.outcome);
    }
    assert!(matches!(outcomes[0], CtlOutcome::Ok { .. }));
    match &outcomes[1] {
        CtlOutcome::Refused { refusal } => assert_eq!(refusal.code, RefusalCode::Replay),
        other => panic!("replay served: {other:?}"),
    }
}

#[tokio::test]
async fn nothing_fits_is_unplaceable_with_reasons_and_chained() {
    let tmp = tempfile::tempdir().unwrap();
    // A binary for an arch no node has.
    let other = if arch() == "aarch64" {
        "x86_64"
    } else {
        "aarch64"
    };
    let pkg = package(tmp.path(), "probe-cog", SCRIPT, &[other]);
    let key = ctl_key();
    let pi = host_node(51, board_caps("pi5"), true, &key);
    let conn = Arc::new(MeshConnector::new(false));
    let a = conn.register_local("pi", pi.svc.clone());
    let (plane, chain) = controller(&key, conn);
    plane.add_target(&a, TrustTier::Paired).await.unwrap();
    let r = plane.place(&order(&pkg)).await.unwrap();
    assert!(r.decision.is_unplaceable());
    assert!(r.attempts.is_empty());
    assert!(r.explain.contains("UNPLACEABLE"));
    assert!(
        r.explain
            .contains(&format!("cpu.arch.{other}: not advertised")),
        "{}",
        r.explain
    );
    assert!(
        events(&chain, EVENT_KIND_WORKLOAD_PLACE)
            .iter()
            .any(|(_, p)| p["phase"] == "decision")
    );
}

#[tokio::test]
async fn noise_over_tcp_carries_a_payload_larger_than_one_noise_message() {
    let tmp = tempfile::tempdir().unwrap();
    // ~300 KB: artifact pieces exceed the 64 KiB Noise message limit.
    let big = format!("{SCRIPT}# {}\n", "x".repeat(300_000));
    let pkg = package(tmp.path(), "probe-cog", &big, &[arch()]);
    let key = ctl_key();
    let pi = host_node(61, board_caps("pi5"), true, &key);
    let listener = super::transport::listen_tcp("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let svc = pi.svc.clone();
    let server = tokio::spawn(super::transport::serve_listener(listener, svc, true));
    let (plane, _chain) = controller(&key, Arc::new(MeshConnector::new(true)));
    assert_eq!(
        plane.add_target(&addr, TrustTier::Paired).await.unwrap(),
        pi.id
    );
    let r = plane.place(&order(&pkg)).await.unwrap();
    assert_eq!(
        r.placed.as_ref().map(|p| p.node_id.as_str()),
        Some(pi.id.as_str()),
        "{}",
        r.explain
    );
    let fetched = events(&pi.chain, EVENT_KIND_ARTIFACT_FETCH);
    assert!(
        fetched
            .iter()
            .any(|(_, p)| p["total_size"].as_u64().unwrap_or(0) > 300_000)
    );
    plane
        .instance(method::UNLOAD, &r.placed.unwrap().instance_id)
        .await
        .unwrap();
    server.abort();
}

#[tokio::test]
async fn a_node_replaced_at_its_address_stops_being_a_placement_target() {
    use clawft_types::placement::engine::{Liveness, PlacementFacts};
    let key = ctl_key();
    let old = host_node(71, board_caps("pi5"), true, &key);
    let new = host_node(72, board_caps("pi5"), true, &key);
    let conn = Arc::new(MeshConnector::new(false));
    let addr = conn.register_local("board", old.svc.clone());
    let (plane, _chain) = controller(&key, conn.clone());
    plane.add_target(&addr, TrustTier::Paired).await.unwrap();
    // The board restarts with a new identity at the same address.
    conn.register_local("board", new.svc.clone());
    plane.refresh().await;
    let view = plane.view();
    let live = |id: &str| {
        view.iter()
            .find(|v| v.node_id() == id)
            .map(|v| v.liveness())
    };
    assert_eq!(live(&old.id), Some(Liveness::Suspect));
    // The new key is not learned under the old identity's tier (trust is
    // bound to the key; the operator pins the new key to trust it).
    assert_eq!(live(&new.id), None);
    assert!(plane.targets().iter().all(|t| t.node_id != new.id));
}

#[tokio::test]
async fn a_node_without_a_container_adapter_is_not_offered_a_container_variant() {
    let tmp = tempfile::tempdir().unwrap();
    let pkg = package(tmp.path(), "probe-cog", SCRIPT, &[arch()]);
    let key = ctl_key();
    // The Mac probes a container engine but serves only its native adapter
    // (review round 3: it was offered aarch64-container and always refused).
    let mac = host_node_native_only(91, mac_caps(), false, &key);
    let conn = Arc::new(MeshConnector::new(false));
    let addr = conn.register_local("mac", mac.svc.clone());
    let (plane, _chain) = controller(&key, conn);
    plane.add_target(&addr, TrustTier::Pinned).await.unwrap();
    let r = plane.place(&order(&pkg)).await.unwrap();
    assert!(r.decision.placement.is_none(), "{}", r.explain);
    let row = r
        .decision
        .candidates
        .iter()
        .find(|c| c.node_id == mac.id)
        .unwrap();
    assert!(!row.eligible(), "{}", r.explain);
    assert!(r.attempts.is_empty(), "nothing dispatched to a route miss");
}

#[tokio::test]
async fn mutations_without_a_chained_decision_id_are_refused() {
    let key = ctl_key();
    let host = host_node(81, board_caps("pi5"), true, &key);
    let conn = MeshConnector::new(false);
    let addr = conn.register_local("h", host.svc.clone());
    for decision in [None, Some("not-a-chain-hash".to_string())] {
        let req = CtlRequest::new(
            &key,
            method::STOP,
            &host.id,
            now(),
            60_000,
            decision,
            json!({"instance_id": "x"}),
        );
        let signed = req.sign(&key).unwrap();
        let mut c = CtlConnection::new(conn.connect(&addr).await.unwrap(), "ctl");
        let resp = c
            .call(
                &host.id,
                method::STOP,
                &signed,
                None,
                Duration::from_secs(10),
            )
            .await
            .unwrap();
        match verify_response(&resp, &req, None).unwrap().0.outcome {
            CtlOutcome::Refused { refusal } => {
                assert_eq!(refusal.code, RefusalCode::InvalidRequest)
            }
            other => panic!("served without a decision: {other:?}"),
        }
    }
}
