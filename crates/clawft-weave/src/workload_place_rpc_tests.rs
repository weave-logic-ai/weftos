//! `workload.*` placement RPCs over a real control plane (in-process
//! `workload-host`, real package, native adapter, in-memory chain).

use std::sync::Arc;

use clawft_kernel::artifact_store::ArtifactStore;
use clawft_kernel::chain::ChainManager;
use clawft_kernel::mesh_artifact::{ArtifactExchange, ExchangeConfig};
use clawft_kernel::node_facts_advert::sign_node_facts;
use clawft_kernel::workload_ctl::{MeshConnector, PlacementControlPlane, WorkloadHostService};
use clawft_kernel::workload_governance::{
    NetworkPolicy, NodeTrustTier, WorkloadGate, WorkloadPermitRule,
};
use clawft_kernel::workload_pkg::codec::hex_encode;
use clawft_kernel::workload_pkg::{
    CogPackInput, KeyOrigin, PackageSource, TrustAnchors, key_id_for, pack_cog, sign_envelope,
    write_manifest,
};
use clawft_kernel::workload_runtime::native::host_arch;
use clawft_kernel::workload_runtime::{NativeConfig, NativeRuntime, WorkloadHost};
use clawft_types::placement::{
    AttrValue, Capability, CapabilityId, NodeFacts, Provenance, TrustTier,
};
use ed25519_dalek::SigningKey;
use serde_json::json;

use super::*;

pub(crate) fn cap(id: &str) -> Capability {
    Capability::new(CapabilityId::new(id).unwrap(), Provenance::Probed)
}

pub(crate) fn anchors(k: &SigningKey) -> TrustAnchors {
    let pk = k.verifying_key().to_bytes();
    let mut a = TrustAnchors::default();
    a.push_signer(&key_id_for(&pk), &hex_encode(&pk), KeyOrigin::Operator)
        .unwrap();
    a
}

pub(crate) fn gate(chain: &Arc<ChainManager>) -> Arc<WorkloadGate> {
    let mut p = WorkloadPermitRule::new("t", ["workload.*"], ["cog"]);
    p.max_network = NetworkPolicy::Egress;
    Arc::new(
        WorkloadGate::exempt(0.95, false, "test")
            .with_permit(p)
            .unwrap()
            .with_chain(chain.clone()),
    )
}

pub(crate) fn exchange(chain: &Arc<ChainManager>) -> Arc<ArtifactExchange> {
    let mut ex = ArtifactExchange::new(
        "t",
        Arc::new(ArtifactStore::new_memory()),
        ExchangeConfig::default(),
    )
    .unwrap();
    ex.set_chain_manager(chain.clone());
    Arc::new(ex)
}

pub(crate) fn package(dir: &std::path::Path, k: &SigningKey) -> std::path::PathBuf {
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        src.join("cog.toml"),
        "[cog]\nid = \"rpc-probe\"\nname = \"P\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    std::fs::write(dir.join("bin"), "#!/bin/sh\necho rpc-ok\nexec sleep 30\n").unwrap();
    let input = CogPackInput {
        cog_dir: src,
        binaries: vec![(host_arch().unwrap().into(), dir.join("bin"))],
        source: PackageSource {
            repo: None,
            commit: Some("0000000".into()),
            release_url: None,
        },
        cognitum_record: None,
        redistributable: true,
    };
    let pkg = dir.join("pkg");
    let mut env = pack_cog(&input, &pkg).unwrap();
    sign_envelope(&mut env, k, &key_id_for(&k.verifying_key().to_bytes())).unwrap();
    write_manifest(&pkg, &env).unwrap();
    pkg
}

/// Controller plane plus one Linux board `workload-host` that runs scripts.
async fn plane(tmp: &std::path::Path) -> (PlacementControlPlane, Arc<ChainManager>, SigningKey) {
    let ctl = SigningKey::from_bytes(&[3; 32]);
    let board_key = SigningKey::from_bytes(&[4; 32]);
    let board_id = clawft_kernel::node_id_from_pubkey(&board_key.verifying_key().to_bytes());
    let chain = Arc::new(ChainManager::new(0, 1000));
    let g = gate(&chain);
    let rt = NativeRuntime::new(NativeConfig {
        root: tmp.join("inst"),
        run_as: None,
        allow_interpreted: true,
    });
    let host = WorkloadHost::new(
        Arc::new(rt),
        g.clone(),
        board_id.clone(),
        NodeTrustTier::Paired,
    )
    .with_chain(chain.clone());
    let arch = host_arch().unwrap();
    let mut facts = NodeFacts::new(
        board_id.clone(),
        chrono::Utc::now().timestamp() as u64,
        600,
        1,
    );
    facts.capabilities = vec![
        cap(&format!("cpu.arch.{arch}")),
        cap("os.linux"),
        cap("runtime.native").with_attr(
            "arches_native",
            AttrValue::List(vec![AttrValue::from(arch)]),
        ),
        cap("mem.system").with_attr("free", 1i64 << 32),
    ];
    let svc = WorkloadHostService::new(
        board_key.clone(),
        exchange(&chain),
        anchors(&ctl),
        g.clone(),
    )
    .with_route("native", Arc::new(host))
    .with_controllers(vec![ctl.verifying_key().to_bytes()])
    .with_chain(chain.clone());
    svc.set_facts(sign_node_facts(&facts, &board_key).unwrap());
    let conn = Arc::new(MeshConnector::new(false));
    let addr = conn.register_local("board", Arc::new(svc));
    let p = PlacementControlPlane::new(
        ctl.clone(),
        g,
        chain.clone(),
        exchange(&chain),
        anchors(&ctl),
        conn,
    );
    p.add_target(&addr, TrustTier::Paired).await.unwrap();
    (p, chain, ctl)
}

#[test]
fn policy_files_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    assert!(
        load_permits(dir.path()).unwrap().is_empty(),
        "no file: no permits (default deny)"
    );
    assert!(
        load_anchors(dir.path()).unwrap().signers.is_empty(),
        "no operator signers"
    );
    std::fs::write(dir.path().join(PERMITS_FILE), "{not json").unwrap();
    assert!(load_permits(dir.path()).is_err());
    std::fs::write(dir.path().join(TRUST_FILE), "[]").unwrap();
    assert!(load_anchors(dir.path()).is_err());
}

#[test]
fn a_policy_file_others_could_have_written_is_refused() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join(PERMITS_FILE);
    std::fs::write(&f, "[]").unwrap();
    for (mode, ok) in [(0o600, true), (0o644, true), (0o664, false), (0o666, false), (0o620, false)] {
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(mode)).unwrap();
        let r = load_permits(dir.path());
        assert_eq!(r.is_ok(), ok, "{mode:o}: {r:?}");
        if !ok {
            assert!(r.unwrap_err().contains("writable"));
        }
    }
    // The trust file is held to the same rule.
    std::fs::write(dir.path().join(TRUST_FILE), "{}").unwrap();
    std::fs::set_permissions(dir.path().join(TRUST_FILE), std::fs::Permissions::from_mode(0o666)).unwrap();
    assert!(load_anchors(dir.path()).unwrap_err().contains("writable"));
}

#[test]
fn only_placement_methods_are_routed_here() {
    for m in METHODS {
        assert!(handles(m, &json!({})), "{m}");
    }
    assert!(handles("workload.unload", &json!({"instance_id": "i"})));
    assert!(!handles(
        "workload.unload",
        &json!({"name": "catalog-entry"})
    ));
    assert!(!handles("workload.install", &json!({})));
    assert!(!handles("workload.bogus", &json!({})));
}

#[tokio::test]
async fn explain_decides_without_dispatch_and_place_runs_the_instance() {
    let tmp = tempfile::tempdir().unwrap();
    let (p, chain, ctl) = plane(tmp.path()).await;
    let pkg = package(&tmp.path().join("pkgsrc"), &ctl);
    let params = json!({ "package_dir": pkg, "mode": "listener", "csi_port": 15007 });

    let r = route(&p, "workload.explain", params.clone()).await;
    assert!(r.ok, "{:?}", r.error);
    let v = r.result.unwrap();
    assert!(v["explain"].as_str().unwrap().contains("PLACED on"));
    assert!(
        v["attempts"].as_array().unwrap().is_empty(),
        "explain dispatches nothing"
    );
    assert!(v["placed"].is_null());

    let r = route(&p, "workload.place", params).await;
    let v = r.result.expect("placed");
    let iid = v["placed"]["instance_id"].as_str().unwrap().to_string();
    let st = route(&p, "workload.status", json!({}))
        .await
        .result
        .unwrap();
    assert_eq!(
        st["instances"][0]["status"]["Ok"]["status"]["state"],
        "running"
    );
    assert!(
        route(&p, "workload.stop", json!({}))
            .await
            .error
            .unwrap()
            .contains("instance_id")
    );
    assert!(
        route(&p, "workload.stop", json!({ "instance_id": iid }))
            .await
            .ok
    );
    assert!(
        route(&p, "workload.unload", json!({ "instance_id": iid }))
            .await
            .ok
    );
    assert!(p.placements().is_empty());
    let kinds: Vec<String> = chain
        .tail(chain.len())
        .into_iter()
        .map(|e| e.kind)
        .collect();
    assert!(
        kinds.iter().filter(|k| *k == "workload.place").count() >= 3,
        "{kinds:?}"
    );

    let bad = route(
        &p,
        "workload.place",
        json!({ "package_dir": pkg_missing(), "mode": "sometimes" }),
    )
    .await;
    assert!(bad.error.unwrap().contains("unknown mode"));
}

fn pkg_missing() -> String {
    std::env::temp_dir().to_string_lossy().into_owned()
}

#[tokio::test]
async fn a_caller_cannot_assign_trust_or_use_a_relative_package_path() {
    let tmp = tempfile::tempdir().unwrap();
    let (p, _chain, _ctl) = plane(tmp.path()).await;
    let board = p.targets()[0].clone();
    assert_eq!(board.tier, TrustTier::Paired);

    // Relative paths are refused (the daemon's cwd is not the caller's).
    let rel = route(&p, "workload.explain", json!({ "package_dir": "pkg" })).await;
    assert!(rel.error.unwrap().contains("must be absolute"));

    // A known peer named again keeps its operator-assigned tier ...
    let pkg = package(&tmp.path().join("pkgsrc"), &SigningKey::from_bytes(&[3; 32]));
    let r = route(
        &p,
        "workload.explain",
        json!({ "package_dir": pkg, "peers": [board.addr], "mode": "listener" }),
    )
    .await;
    assert!(r.ok, "{:?}", r.error);
    assert_eq!(p.targets()[0].tier, TrustTier::Paired);

    // ... and an unknown one can never be more than discovered: here it is
    // not reachable, so it is not added at all.
    let r = route(
        &p,
        "workload.explain",
        json!({ "package_dir": pkg, "peers": ["127.0.0.1:9"], "mode": "listener" }),
    )
    .await;
    assert!(r.error.unwrap().contains("peer 127.0.0.1:9"));
    assert_eq!(p.targets().len(), 1);
}
