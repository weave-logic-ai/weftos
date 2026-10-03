//! ADR-106 phase 3 integration: placement built while the licence role is
//! still unknown holds the node's real grant store (the licence runtime is
//! installed at boot in every mode), so when the role arrives and a binding
//! comes in, the run gate and the steward relay both see it.
//!
//! One process per test file: the module-wide licence state is this test's.
#![cfg(all(unix, feature = "placement"))]

mod mesh_e2e;

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use clawft_kernel::Kernel;
use clawft_kernel::licence::*;
use clawft_kernel::mesh_delivery::LocalDelivery;
use clawft_kernel::workload_pkg::codec::hex_encode;
use clawft_kernel::workload_pkg::key_id_for;
use clawft_platform::NativePlatform;
use clawft_types::config::{ChainConfig, Config, KernelConfig, MeshAdmissionMode, MeshConfig, MeshServicePolicy};
use clawft_weave::licence_boot::{self, HolderState};
use clawft_weave::node_identity::DaemonIdentity;
use clawft_weave::{cog_swarm, licence_steward, placement_boot, workload_place_rpc};
use ed25519_dalek::SigningKey;
use mesh_e2e::*;
use serde_json::{Value, json};
use tokio::sync::RwLock;

fn write(dir: &std::path::Path, name: &str, v: &str) {
    let p = dir.join(name);
    std::fs::write(&p, v).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
}

#[tokio::test]
async fn placement_built_before_the_role_sees_the_binding_that_arrives_after_it() {
    let svc = Svc::with_config(Default::default(), |c| c.cluster_owner_uid = Some(OTHER_UID)).await;
    svc.next_uid.store(OTHER_UID, Ordering::SeqCst);
    let cfg = svc.mesh_cfg(MeshServicePolicy::Required);
    let wrap: Box<dyn FnOnce(Arc<Inbox>) -> Arc<dyn LocalDelivery>> = Box::new(cog_swarm::wrap_inbox);
    let owner = link_via(&cfg, other_endpoint(&svc, key(2), vec![]), fast(), Some(wrap)).await;

    let tmp = tempfile::tempdir().unwrap();
    let runtime = tmp.path().join("runtime");
    std::fs::create_dir_all(&runtime).unwrap();
    std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700)).unwrap();
    let op = SigningKey::from_bytes(&[1; 32]);
    let opk = op.verifying_key().to_bytes();
    write(&runtime, workload_place_rpc::PERMITS_FILE, "[]");
    write(
        &runtime,
        workload_place_rpc::TRUST_FILE,
        &json!({ "schema": "weftos.workload-trust.v1",
                 "operator_keys": [{ "key_id": key_id_for(&opk), "public_key": hex_encode(&opk) }] })
        .to_string(),
    );
    // A steward link to a loopback port nothing listens on.
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    write(&runtime, licence_steward::LINK_FILE, &format!(r#"{{"url":"http://127.0.0.1:{port}","allow_unpinned_lab_link":true,"timeout_secs":2}}"#));

    let rec = svc.record();
    let identity = DaemonIdentity::for_service(rec.node_id.clone(), rec.machine_pubkey).unwrap();
    let mesh = MeshConfig {
        genesis_hash: Some("47".repeat(32)),
        mesh_nonce: Some("ab".repeat(32)),
        admission: MeshAdmissionMode::Enforce,
        ..Default::default()
    };
    let kcfg = KernelConfig {
        chain: Some(ChainConfig::isolated_in(&tmp.path().join("chain"))),
        mesh: Some(mesh),
        ..KernelConfig::default()
    };
    let kernel = Kernel::boot_in_service_mode(Config::default(), kcfg, Arc::new(NativePlatform::new()), rec.node_id.clone())
        .await
        .expect("kernel boots in service mode");
    let kernel = Arc::new(RwLock::new(kernel));

    // Boot with the role unknown (the link is not bound to the daemon yet).
    placement_boot::start(&kernel, &identity, &runtime).await;
    assert_eq!(licence_boot::holder_state(), HolderState::Unknown);
    let rt = licence_boot::runtime().expect("the licence runtime exists from boot");
    assert!(workload_place_rpc::licence_exchange().is_none(), "no exchange without the role");
    // Placement is built now, while the role is unknown.
    let r = workload_place_rpc::dispatch("workload.status", json!({}), kernel.clone()).await;
    assert!(r.ok, "{:?}", r.error);

    // The role arrives, then a binding naming this machine and its control key.
    cog_swarm::set_forwarder(owner.handle.as_ref().unwrap().forwarder.clone());
    wait_until("the exchange starts with the role", || workload_place_rpc::licence_exchange().is_some()).await;
    let mesh_id = rt.local.get().expect("mesh id from the configured nonce");
    let b = BindingRecord {
        v: 2,
        device_id: "seed-x".into(),
        device_pubkey: hex_encode(&[5; 32]),
        mesh_id: mesh_id.to_hex(),
        grant_pubkey: hex_encode(&SigningKey::from_bytes(&[2; 32]).verifying_key().to_bytes()),
        steward_node_id: rec.node_id.clone(),
        steward_pubkey: rt.steward_pubkey.clone(),
        state: BindState::Bound,
        seq: 1,
        bound_at: clawft_mesh_local::client::now_unix(),
    };
    let x = workload_place_rpc::licence_exchange().unwrap();
    assert_eq!(x.issue_binding(sign_binding(&b, &op).unwrap()).await, Ok(Receipt::New));

    // The run gate placement built sees the binding: a Cognitum run is now
    // checked against grants (none held), not waved through as not Seed-bound.
    let host = workload_place_rpc::in_process_host().expect("placement built the workload-host");
    let gate = host.licence_gate().expect("the run gate is set");
    let (sha, b3) = (sha256_hex(b"cog"), hex_encode(blake3::hash(b"cog").as_bytes()));
    let req = RunRequest { cog_id: "fall-detect", version: "1.2.0", sha256: &sha, blake3: &b3 };
    assert_eq!(gate.check(&req).unwrap_err().code(), "no_grant");

    // And so does the relay: its client signs for the binding and tries the link
    // (refused at the closed port), instead of `seed_not_bound`.
    let relay = cog_swarm::get().and_then(|m| m.relay()).expect("the relay is installed");
    let wire = CheckoutWire { request_id: "r-1".into(), cog_id: "fall-detect".into(), version: "1.2.0".into(), arch: "aarch64".into() };
    let e = relay.handle(CheckoutCaller::Kernel, &wire).await.unwrap_err();
    assert_eq!(e.code(), "licence_unreachable", "{e}");
    let _: Option<Value> = None;
}
