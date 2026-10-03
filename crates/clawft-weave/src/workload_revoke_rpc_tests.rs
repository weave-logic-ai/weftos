//! `workload.revoke` over a real control plane, host, exchange and chain:
//! revoking stops and unloads what runs from the subject, records it, and
//! says what it did and did not reach.

use std::path::PathBuf;
use std::sync::Arc;

use clawft_kernel::chain::ChainManager;
use clawft_kernel::mesh_runtime::MeshRuntime;
use clawft_kernel::mesh_swarm_revoke::RevocationExchange;
use clawft_kernel::node_facts_advert::sign_node_facts;
use clawft_kernel::revocation::{RevocationKind, RevocationList};
use clawft_kernel::workload_ctl::{MeshConnector, PlacementControlPlane, WorkloadHostService};
use clawft_kernel::workload_governance::{
    NetworkPolicy, NodeTrustTier, WorkloadGate, WorkloadPermitRule, chain_revocations,
};
use clawft_kernel::workload_pkg::codec::hex_encode;
use clawft_kernel::workload_pkg::{KeyOrigin, TrustAnchors};
use clawft_kernel::workload_runtime::native::host_arch;
use clawft_kernel::workload_runtime::{NativeConfig, NativeRuntime, WorkloadHost};
use clawft_types::placement::{AttrValue, NodeFacts, TrustTier};
use ed25519_dalek::SigningKey;
use serde_json::{Value, json};

use super::*;
use crate::workload_place_rpc::tests::{anchors, cap, exchange, package};

struct Rig {
    revoker: Revoker,
    plane: Arc<PlacementControlPlane>,
    svc: Arc<WorkloadHostService>,
    chain: Arc<ChainManager>,
    list: Arc<RevocationList>,
    ctl: SigningKey,
    iid: String,
    _tmp: tempfile::TempDir,
}

fn kinds(chain: &ChainManager, kind: &str) -> Vec<Value> {
    chain
        .tail(chain.len())
        .into_iter()
        .filter(|e| e.kind == kind)
        .map(|e| e.payload.unwrap_or(Value::Null))
        .collect()
}

/// A placed, running instance; `list_path` is where the list persists.
async fn rig(list_at: impl FnOnce(&std::path::Path) -> PathBuf) -> Rig {
    let tmp = tempfile::tempdir().unwrap();
    let ctl = SigningKey::from_bytes(&[3; 32]);
    let board_key = SigningKey::from_bytes(&[4; 32]);
    let board_id = clawft_kernel::node_id_from_pubkey(&board_key.verifying_key().to_bytes());
    let chain = Arc::new(ChainManager::new(0, 4000));
    let list = Arc::new(RevocationList::new(list_at(tmp.path())));
    chain_revocations(&list, chain.clone());
    let mut permit = WorkloadPermitRule::new("t", ["workload.*"], ["cog"]);
    permit.max_network = NetworkPolicy::Egress;
    let gate = Arc::new(
        WorkloadGate::new(0.95, false)
            .with_permit(permit)
            .unwrap()
            .with_chain(chain.clone())
            .with_revocations(list.clone()),
    );
    let rt = NativeRuntime::new(NativeConfig {
        root: tmp.path().join("inst"),
        run_as: None,
        allow_interpreted: true,
    });
    let host = WorkloadHost::new(Arc::new(rt), gate.clone(), board_id.clone(), NodeTrustTier::Paired)
        .with_chain(chain.clone());
    let arch = host_arch().unwrap();
    let mut facts = NodeFacts::new(board_id.clone(), chrono::Utc::now().timestamp() as u64, 600, 1);
    facts.capabilities = vec![
        cap(&format!("cpu.arch.{arch}")),
        cap("os.linux"),
        cap("runtime.native").with_attr("arches_native", AttrValue::List(vec![AttrValue::from(arch)])),
        cap("mem.system").with_attr("free", 1i64 << 32),
    ];
    let ex = exchange(&chain);
    ex.set_revocations(list.clone());
    let svc = WorkloadHostService::new(board_key.clone(), exchange(&chain), anchors(&ctl), gate.clone())
        .with_route("native", Arc::new(host))
        .with_controllers(vec![ctl.verifying_key().to_bytes()])
        .with_chain(chain.clone());
    svc.set_facts(sign_node_facts(&facts, &board_key).unwrap());
    let svc = Arc::new(svc);
    let conn = Arc::new(MeshConnector::new(false));
    let addr = conn.register_local("board", svc.clone());
    let plane = Arc::new(PlacementControlPlane::new(
        ctl.clone(),
        gate,
        chain.clone(),
        ex.clone(),
        anchors(&ctl),
        conn,
    ));
    plane.add_target(&addr, TrustTier::Paired).await.unwrap();
    let pkg = package(&tmp.path().join("pkgsrc"), &ctl);
    let placed = route(
        &plane,
        "workload.place",
        json!({ "package_dir": pkg, "mode": "listener", "csi_port": 15041 }),
    )
    .await
    .result
    .expect("placed");
    let iid = placed["placed"]["instance_id"].as_str().unwrap().to_string();
    let revoker = Revoker {
        list: list.clone(),
        exchange: ex,
        host: Some(svc.clone()),
        plane: Some(plane.clone()),
        notices: None,
        notice_key: None,
    };
    Rig { revoker, plane, svc, chain, list, ctl, iid, _tmp: tmp }
}

use crate::workload_place_rpc::route;

fn plain(dir: &std::path::Path) -> PathBuf {
    dir.join("revoked_hosts.json")
}

#[tokio::test]
async fn revoking_stops_unloads_records_and_chains() {
    let signer = |r: &Rig| hex_encode(&r.ctl.verifying_key().to_bytes());
    for by in ["package", "signer", "hash"] {
        let r = rig(plain).await;
        let rec = r.plane.placements().pop().unwrap();
        let id = match by {
            "package" => rec.package_id.clone().unwrap(),
            "signer" => signer(&r),
            _ => rec.artifact_hashes[0].clone(),
        };
        let out = r
            .revoker
            .revoke(json!({ by: id, "reason": "leaked" }))
            .await
            .unwrap();
        assert_eq!(out["newly_revoked"], true, "{by}");
        assert_eq!(out["persisted"], true);
        let forced = out["forced"].as_array().unwrap();
        assert_eq!(forced.len(), 1, "{by}: {out}");
        assert_eq!(forced[0]["instance_id"], r.iid.as_str());
        assert_eq!(forced[0]["unloaded"], true);
        assert!(out["notice"].as_str().unwrap().contains("no mesh"), "{out}");

        // Gone from the host and from the controller's records.
        assert!(r.plane.placements().is_empty(), "{by}");
        assert!(r.svc.enforce_revocations(&r.list).await.is_empty(), "{by}: nothing left");
        // The revocation is chained once, by the operator, with the reason,
        // and the forced stop and unload are chained as forced.
        let rev = kinds(&r.chain, "workload.revoke");
        assert_eq!(rev.len(), 1, "{by}");
        assert_eq!(rev[0]["revoked_by"], "operator");
        assert_eq!(rev[0]["reason"], "leaked");
        for k in ["workload.stop", "workload.unload"] {
            assert!(
                kinds(&r.chain, k)
                    .iter()
                    .any(|p| p["forced_by_revocation"]["id"] == id.as_str()),
                "{by}: {k} chained as forced"
            );
        }
        // Repeating it is reported as already revoked and chains nothing new.
        let again = r.revoker.revoke(json!({ by: id })).await.unwrap();
        assert_eq!(again["newly_revoked"], false);
        assert_eq!(kinds(&r.chain, "workload.revoke").len(), 1);
    }
}

#[tokio::test]
async fn exactly_one_well_formed_subject_is_required() {
    let r = rig(plain).await;
    let key = "ab".repeat(32);
    for bad in [
        json!({}),
        json!({ "package": "cog.x", "signer": key }),
        json!({ "signer": "not-hex" }),
        json!({ "hash": "ab" }),
        json!({ "package": "bad id with spaces" }),
        json!({ "package": "cog.x", "colour": "red" }),
    ] {
        let e = r.revoker.revoke(bad.clone()).await.unwrap_err();
        assert!(!e.is_empty(), "{bad}");
    }
    assert!(r.list.list_subjects(None).is_empty(), "nothing was revoked");
    assert_eq!(r.plane.placements().len(), 1, "and nothing was torn down");
}

#[tokio::test]
async fn a_failed_write_is_reported_but_the_revocation_still_bites() {
    // A file where the list's directory should be: the write fails.
    let r = rig(|d| {
        std::fs::write(d.join("blocker"), "x").unwrap();
        d.join("blocker").join("revoked_hosts.json")
    })
    .await;
    let pid = r.plane.placements().pop().unwrap().package_id.unwrap();
    let out = r.revoker.revoke(json!({ "package": pid })).await.unwrap();
    assert_eq!(out["persisted"], false);
    assert!(out["persist_error"].is_string());
    assert_eq!(out["forced"].as_array().unwrap().len(), 1, "still torn down");
    let rev = kinds(&r.chain, "workload.revoke");
    assert_eq!(rev.len(), 1, "and chained");
    assert_eq!(rev[0]["persisted"], false);
    assert!(r.list.is_subject_revoked(RevocationKind::Package, &pid));
}

fn mesh_revoker(r: &Rig, anchors: TrustAnchors, key: Option<SigningKey>) -> Revoker {
    let rt = Arc::new(MeshRuntime::new("n".into()));
    let notices = RevocationExchange::start(
        r.revoker.exchange.clone(),
        r.list.clone(),
        anchors,
        rt,
    );
    Revoker {
        list: r.list.clone(),
        exchange: r.revoker.exchange.clone(),
        host: Some(r.svc.clone()),
        plane: Some(r.plane.clone()),
        notices: Some(notices),
        notice_key: key,
    }
}

#[tokio::test]
async fn a_notice_is_issued_only_with_a_pinned_operator_key() {
    let op = SigningKey::from_bytes(&[8; 32]);
    let mut pinned = TrustAnchors::default();
    pinned
        .push_signer("op", &hex_encode(&op.verifying_key().to_bytes()), KeyOrigin::Operator)
        .unwrap();

    // Key pinned as an operator key: the notice is signed and issued.
    let r = rig(plain).await;
    let m = mesh_revoker(&r, pinned.clone(), Some(op.clone()));
    let out = m.revoke(json!({ "hash": "cd".repeat(32) })).await.unwrap();
    assert_eq!(out["notice"], "issued to every connected peer", "{out}");
    assert_eq!(kinds(&r.chain, "workload.revoke").len(), 1, "one event, not one per path");

    // Mesh but no operator key: said so, and the revocation still holds here.
    let r = rig(plain).await;
    let m = mesh_revoker(&r, pinned, None);
    let out = m.revoke(json!({ "hash": "ef".repeat(32) })).await.unwrap();
    assert!(
        out["notice"].as_str().unwrap().contains("not a pinned operator key"),
        "{out}"
    );
    assert!(r.list.is_subject_revoked(RevocationKind::ArtifactHash, &"ef".repeat(32)));
}
