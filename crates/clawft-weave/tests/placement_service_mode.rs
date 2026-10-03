//! ADR-106 phase 3: placement in service mode. The daemon's node key belongs
//! to the machine mesh service, so placement signs with the daemon-local
//! control key (`placement_boot`). A real mesh service with this daemon
//! linked as the cluster owner through the daemon's own `cog_swarm` wiring
//! (`wrap`, `set_forwarder`), a kernel booted in service mode under the
//! service's node id, the daemon's boot step (`placement_boot::start`: the
//! reserved-holder check, the licence runtime and the eager licence exchange
//! over the service links, whose peer view refresh starts with the link), the
//! operator's policy files in a temp runtime dir and
//! a board serving `workload-host` over Noise TCP to that control key: a
//! `workload.place` through `dispatch` runs the cog on the board, and the
//! licence runtime names the machine as steward with the control key as its
//! request key. A board that trusts only the machine key is never placed on.
//!
//! One process per test file, so the module-wide placement state is this
//! test's alone.
#![cfg(all(unix, feature = "placement"))]

mod mesh_e2e;

use std::path::Path;
use std::sync::Arc;

use clawft_kernel::Kernel;
use clawft_kernel::artifact_store::ArtifactStore;
use clawft_kernel::chain::ChainManager;
use clawft_kernel::mesh_artifact::{ArtifactExchange, ExchangeConfig};
use clawft_kernel::node_facts_advert::sign_node_facts;
use clawft_kernel::workload_ctl::{WorkloadHostService, listen_tcp, serve_listener};
use clawft_kernel::workload_governance::{NetworkPolicy, NodeTrustTier, WorkloadGate, WorkloadPermitRule};
use clawft_kernel::workload_pkg::codec::hex_encode;
use clawft_kernel::workload_pkg::{
    CogPackInput, KeyOrigin, PackageSource, TrustAnchors, key_id_for, pack_cog, sign_envelope, write_manifest,
};
use clawft_kernel::workload_runtime::native::host_arch;
use clawft_kernel::workload_runtime::{NativeConfig, NativeRuntime, WorkloadHost};
use clawft_platform::NativePlatform;
use clawft_types::config::{ChainConfig, Config, KernelConfig};
use clawft_types::placement::{AttrValue, Capability, CapabilityId, NodeFacts, Provenance};
use clawft_weave::node_identity::DaemonIdentity;
use clawft_types::config::MeshServicePolicy;
use clawft_weave::{cog_swarm, licence_boot, placement_boot, workload_place_rpc};
use mesh_e2e::*;
use std::sync::atomic::Ordering;
use ed25519_dalek::SigningKey;
use serde_json::{Value, json};
use tokio::sync::RwLock;

fn cap(id: &str) -> Capability {
    Capability::new(CapabilityId::new(id).unwrap(), Provenance::Probed)
}

fn anchors(k: &SigningKey) -> TrustAnchors {
    let pk = k.verifying_key().to_bytes();
    let mut a = TrustAnchors::default();
    a.push_signer(&key_id_for(&pk), &hex_encode(&pk), KeyOrigin::Operator).unwrap();
    a
}

fn gate(chain: &Arc<ChainManager>) -> Arc<WorkloadGate> {
    let mut p = WorkloadPermitRule::new("t", ["workload.*"], ["cog"]);
    p.max_network = NetworkPolicy::Egress;
    Arc::new(WorkloadGate::exempt(0.95, false, "test").with_permit(p).unwrap().with_chain(chain.clone()))
}

fn package(dir: &Path, k: &SigningKey) -> std::path::PathBuf {
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("cog.toml"), "[cog]\nid = \"svc-probe\"\nname = \"P\"\nversion = \"0.1.0\"\n").unwrap();
    std::fs::write(dir.join("bin"), "#!/bin/sh\necho svc-ok\nexec sleep 30\n").unwrap();
    clawft_kernel::workload_pkg::write_source_build_provenance(&src, &dir.join("bin")).unwrap();
    let input = CogPackInput {
        cog_dir: src,
        binaries: vec![(host_arch().unwrap().into(), dir.join("bin"))],
        source: PackageSource { repo: None, commit: Some("0000000".into()), release_url: None },
        cognitum_record: None,
        redistributable: true,
        provenance: None,
        allow_no_provenance: false,
    };
    let pkg = dir.join("pkg");
    let mut env = pack_cog(&input, &pkg).unwrap();
    sign_envelope(&mut env, k, &key_id_for(&k.verifying_key().to_bytes())).unwrap();
    write_manifest(&pkg, &env).unwrap();
    pkg
}

/// A Linux board serving `workload-host` to `controller`, trusting packages
/// from `signer`. Returns (node id, address, its chain).
async fn board(root: &Path, seed: u8, controller: [u8; 32], signer: &SigningKey) -> (String, String, Arc<ChainManager>) {
    let key = SigningKey::from_bytes(&[seed; 32]);
    let id = clawft_kernel::node_id_from_pubkey(&key.verifying_key().to_bytes());
    let chain = Arc::new(ChainManager::new(0, 1000));
    let g = gate(&chain);
    let rt = NativeRuntime::new(NativeConfig { root: root.join(format!("board-{seed}")), run_as: None, allow_interpreted: true });
    let host = WorkloadHost::new(Arc::new(rt), g.clone(), id.clone(), NodeTrustTier::Paired).with_chain(chain.clone());
    let mut ex = ArtifactExchange::new("b", Arc::new(ArtifactStore::new_memory()), ExchangeConfig::default()).unwrap();
    ex.set_chain_manager(chain.clone());
    let svc = WorkloadHostService::new(key.clone(), Arc::new(ex), anchors(signer), g)
        .with_route("native", Arc::new(host))
        .with_controllers(vec![controller])
        .with_chain(chain.clone());
    let arch = host_arch().unwrap();
    let mut f = NodeFacts::new(id.clone(), chrono::Utc::now().timestamp() as u64, 600, 1);
    f.capabilities = vec![
        cap(&format!("cpu.arch.{arch}")),
        cap("os.linux"),
        cap("node.class.pi5"),
        cap("runtime.native").with_attr("arches_native", AttrValue::List(vec![AttrValue::from(arch)])),
        cap("mem.system").with_attr("free", 1i64 << 32),
    ];
    svc.set_facts(sign_node_facts(&f, &key).unwrap());
    let listener = listen_tcp("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    tokio::spawn(serve_listener(listener, Arc::new(svc), true));
    (id, addr, chain)
}

fn write(dir: &Path, name: &str, v: &Value) {
    std::fs::write(dir.join(name), serde_json::to_vec(v).unwrap()).unwrap();
}

async fn call(kernel: &Arc<RwLock<Kernel<NativePlatform>>>, m: &str, p: Value) -> Result<Value, String> {
    let r = workload_place_rpc::dispatch(m, p, kernel.clone()).await;
    if r.ok { Ok(r.result.unwrap_or(Value::Null)) } else { Err(r.error.unwrap_or_default()) }
}

#[tokio::test]
async fn placement_runs_in_service_mode_signed_by_the_control_key() {
    let tmp = tempfile::tempdir().unwrap();
    let runtime = tmp.path().join("runtime");
    std::fs::create_dir_all(&runtime).unwrap();
    // A real service; this daemon links as its cluster owner with the
    // daemon's own cog mesh wiring. The machine key is the service's.
    let svc = Svc::with_config(Default::default(), |c| c.cluster_owner_uid = Some(OTHER_UID)).await;
    svc.next_uid.store(OTHER_UID, Ordering::SeqCst);
    let cfg = svc.mesh_cfg(MeshServicePolicy::Required);
    let wrap: Box<dyn FnOnce(Arc<Inbox>) -> Arc<dyn clawft_kernel::mesh_delivery::LocalDelivery>> =
        Box::new(cog_swarm::wrap_inbox);
    let owner = link_via(&cfg, other_endpoint(&svc, key(2), vec![]), fast(), Some(wrap)).await;
    svc.next_uid.store(REAL, Ordering::SeqCst);
    cog_swarm::set_forwarder(owner.handle.as_ref().unwrap().forwarder.clone());
    let rec = svc.record();
    let (machine, machine_id) = (rec.machine_pubkey, rec.node_id.clone());
    let identity = DaemonIdentity::for_service(machine_id.clone(), machine).unwrap();
    let kcfg = KernelConfig { chain: Some(ChainConfig::isolated_in(&tmp.path().join("chain"))), ..KernelConfig::default() };
    let kernel = Kernel::boot_in_service_mode(Config::default(), kcfg, Arc::new(NativePlatform::new()), machine_id.clone())
        .await
        .expect("kernel boots in service mode");
    let kernel = Arc::new(RwLock::new(kernel));

    // The operator lists the control key on the board (what `weaver` shows).
    let control = placement_boot::signer(&identity, &runtime).unwrap();
    assert!(control.control_key);
    let control_pk = control.key.verifying_key().to_bytes();
    let signer = SigningKey::from_bytes(&[6; 32]);
    let spk = signer.verifying_key().to_bytes();
    let mut permit = WorkloadPermitRule::new("operator-cog", ["workload.*"], ["cog"]);
    permit.max_network = NetworkPolicy::Egress;
    write(&runtime, workload_place_rpc::PERMITS_FILE, &json!([permit]));
    write(
        &runtime,
        workload_place_rpc::TRUST_FILE,
        &json!({ "schema": "weftos.workload-trust.v1",
                 "operator_keys": [{ "key_id": key_id_for(&spk), "public_key": hex_encode(&spk) }] }),
    );
    let (pi, pi_addr, pi_chain) = board(tmp.path(), 7, control_pk, &signer).await;
    // A second board that trusts only the machine key: this daemon cannot sign as it.
    let (other, other_addr, _) = board(tmp.path(), 8, machine, &signer).await;
    write(
        &runtime,
        workload_place_rpc::PEERS_FILE,
        &json!([{ "addr": pi_addr, "tier": "paired" }, { "addr": other_addr, "tier": "paired" }]),
    );

    placement_boot::start(&kernel, &identity, &runtime).await;

    // The boot step: this daemon holds the reserved topics, the exchange is
    // up from boot over the service links, and their peer view is refreshed.
    assert_eq!(licence_boot::reserved_holder(), Some(true));
    assert!(workload_place_rpc::licence_exchange().is_some(), "the exchange starts at boot, before any placement");
    let links = cog_swarm::service_licence_links().expect("service links once the forwarder is set");
    wait_until("the peer view is refreshed", || links.counters.refreshed.load(Ordering::SeqCst) >= 1).await;

    // The licence runtime: the machine is the steward, the control key signs.
    let lic = licence_boot::runtime().expect("licence runtime installed in service mode");
    assert_eq!(lic.steward_node_id, machine_id);
    assert_eq!(lic.steward_pubkey, hex_encode(&control_pk));
    assert!(!runtime.join(clawft_kernel::NODE_KEY_FILE).exists(), "node.key is never read or made");

    let pkg = package(&tmp.path().join("pkgsrc"), &signer);
    let params = json!({ "package_dir": pkg, "mode": "listener", "csi_port": 15037 });
    let st = call(&kernel, "workload.status", json!({})).await.unwrap();
    let control_id = clawft_kernel::node_id_from_pubkey(&control_pk);
    assert_eq!(st["controller"], control_id.as_str(), "the controller is the control key");
    let targets: Vec<String> =
        st["targets"].as_array().unwrap().iter().filter_map(|t| t["node_id"].as_str().map(str::to_owned)).collect();
    assert!(targets.contains(&pi), "the board that trusts the control key answered: {st}");
    assert!(!targets.contains(&other), "a board that trusts only the machine key refuses this daemon: {st}");

    let v = call(&kernel, "workload.place", params).await.unwrap();
    assert_eq!(v["placed"]["node_id"], pi.as_str(), "{}", v["explain"]);
    let iid = v["placed"]["instance_id"].as_str().unwrap().to_string();
    let s = call(&kernel, "workload.status", json!({ "instance_id": iid })).await.unwrap();
    assert_eq!(s["status"]["state"], "running");
    assert!(pi_chain.tail(pi_chain.len()).iter().any(|e| e.kind == "workload.place"), "placed on the board's chain");
    call(&kernel, "workload.stop", json!({ "instance_id": iid })).await.unwrap();
    call(&kernel, "workload.unload", json!({ "instance_id": iid })).await.unwrap();
}
