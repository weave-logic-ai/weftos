//! ADR-106 phase 3: in service mode only the daemon that holds the service's
//! reserved topics (the cluster owner's) runs the licence path. Another
//! tenant's daemon, linked to a real service through the daemon's own wiring,
//! boots placement but no licence runtime, exchange or binder: a bind is
//! refused and the status says it is not the holder.
//!
//! One process per test file, so the module-wide state is this test's alone.
#![cfg(all(unix, feature = "placement"))]

mod mesh_e2e;

use std::sync::Arc;
use std::sync::atomic::Ordering;

use clawft_kernel::Kernel;
use clawft_kernel::mesh_delivery::LocalDelivery;
use clawft_platform::NativePlatform;
use clawft_types::config::{ChainConfig, Config, KernelConfig, MeshServicePolicy};
use clawft_weave::commands::workload_node_cmd::render_status;
use clawft_weave::mesh_local_glue::build_endpoint;
use clawft_weave::node_identity::DaemonIdentity;
use clawft_weave::{cog_swarm, licence_boot, licence_rpc, placement_boot, workload_place_rpc};
use mesh_e2e::*;
use serde_json::json;
use tokio::sync::RwLock;

#[tokio::test]
async fn a_non_owner_tenant_daemon_refuses_bind_and_starts_no_exchange() {
    // The owner is another uid; this daemon is the current user's.
    let svc = Svc::with_config(Default::default(), |c| c.cluster_owner_uid = Some(OTHER_UID)).await;
    let home = tempfile::tempdir().unwrap();
    svc.next_uid.store(REAL, Ordering::SeqCst);
    let cfg = svc.mesh_cfg(MeshServicePolicy::Required);
    let ep = build_endpoint(&cfg, home.path(), "e2e-tenant").unwrap().unwrap();
    let wrap: Box<dyn FnOnce(Arc<Inbox>) -> Arc<dyn LocalDelivery>> = Box::new(cog_swarm::wrap_inbox);
    let tenant = link_via(&cfg, ep, fast(), Some(wrap)).await;
    cog_swarm::set_forwarder(tenant.handle.as_ref().unwrap().forwarder.clone());
    assert_eq!(cog_swarm::reserved_holder().await, Ok(false));

    let tmp = tempfile::tempdir().unwrap();
    let runtime = tmp.path().join("runtime");
    let rec = svc.record();
    let identity = DaemonIdentity::for_service(rec.node_id.clone(), rec.machine_pubkey).unwrap();
    let kcfg = KernelConfig { chain: Some(ChainConfig::isolated_in(&tmp.path().join("chain"))), ..KernelConfig::default() };
    let kernel = Kernel::boot_in_service_mode(Config::default(), kcfg, Arc::new(NativePlatform::new()), rec.node_id.clone())
        .await
        .expect("kernel boots in service mode");
    let kernel = Arc::new(RwLock::new(kernel));

    placement_boot::start(&kernel, &identity, &runtime).await;

    assert_eq!(licence_boot::reserved_holder(), Some(false));
    assert!(licence_boot::runtime().is_none(), "no licence runtime or binder on a non-owner daemon");
    assert!(workload_place_rpc::licence_exchange().is_none(), "no licence exchange on a non-owner daemon");
    assert!(workload_place_rpc::runtime_dir().is_some(), "placement itself is still initialised");

    let bind = licence_rpc::dispatch("workload.node.bind", json!({}), kernel.clone()).await;
    assert!(!bind.ok && bind.error.as_deref().unwrap_or("").contains("reserved licence topics"), "{:?}", bind.error);
    let st = licence_rpc::dispatch("workload.node.binding", json!({}), kernel.clone()).await;
    assert!(st.ok);
    let st = st.result.unwrap();
    assert_eq!(st["reserved_holder"], false);
    assert!(render_status(&st).contains("NOT the holder"), "{}", render_status(&st));
}
