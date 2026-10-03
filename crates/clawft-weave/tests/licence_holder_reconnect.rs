//! ADR-106 phase 3: a service-mode daemon whose service drops before it boots
//! runs no licence path (holder unknown); when the link reconnects the
//! service answers and the licence runtime and exchange come up then.
//!
//! One process per test file: the module-wide licence state is this test's.
#![cfg(all(unix, feature = "placement"))]

mod mesh_e2e;

use std::sync::Arc;
use std::sync::atomic::Ordering;

use clawft_kernel::Kernel;
use clawft_kernel::mesh_delivery::LocalDelivery;
use clawft_platform::NativePlatform;
use clawft_types::config::{ChainConfig, Config, KernelConfig, MeshServicePolicy};
use clawft_weave::licence_boot::{self, HolderState};
use clawft_weave::node_identity::DaemonIdentity;
use clawft_weave::{cog_swarm, placement_boot, workload_place_rpc};
use mesh_e2e::*;
use tokio::sync::RwLock;

#[tokio::test]
async fn the_licence_path_comes_up_when_the_link_reconnects() {
    let mut svc = Svc::with_config(Default::default(), |c| c.cluster_owner_uid = Some(OTHER_UID)).await;
    svc.next_uid.store(OTHER_UID, Ordering::SeqCst);
    let cfg = svc.mesh_cfg(MeshServicePolicy::Required);
    let wrap: Box<dyn FnOnce(Arc<Inbox>) -> Arc<dyn LocalDelivery>> = Box::new(cog_swarm::wrap_inbox);
    let owner = link_via(&cfg, other_endpoint(&svc, key(2), vec![]), fast(), Some(wrap)).await;
    let rec = svc.record();

    // The service drops before the daemon's boot step runs.
    svc.stop().await;
    wait_until("the link notices", || owner.link_state().as_deref() == Some("reconnecting")).await;
    cog_swarm::set_forwarder(owner.handle.as_ref().unwrap().forwarder.clone());

    let tmp = tempfile::tempdir().unwrap();
    let identity = DaemonIdentity::for_service(rec.node_id.clone(), rec.machine_pubkey).unwrap();
    let kcfg = KernelConfig { chain: Some(ChainConfig::isolated_in(&tmp.path().join("chain"))), ..KernelConfig::default() };
    let kernel = Kernel::boot_in_service_mode(Config::default(), kcfg, Arc::new(NativePlatform::new()), rec.node_id.clone())
        .await
        .expect("kernel boots in service mode");
    let kernel = Arc::new(RwLock::new(kernel));
    placement_boot::start(&kernel, &identity, &tmp.path().join("runtime")).await;
    assert_eq!(licence_boot::holder_state(), HolderState::Unknown);
    // The local licence runtime exists from boot; the exchange waits for the role.
    assert!(licence_boot::runtime().is_some() && workload_place_rpc::licence_exchange().is_none());

    // The service is back; the link reconnects and the next refresh answers.
    svc.next_uid.store(OTHER_UID, Ordering::SeqCst);
    svc.begin().await;
    wait_until("the link reconnects", || owner.link_state().as_deref() == Some("connected")).await;
    let links = cog_swarm::licence_links();
    for _ in 0..200 {
        let _ = links.refresh().await;
        if workload_place_rpc::licence_exchange().is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert_eq!(licence_boot::holder_state(), HolderState::Holder);
    assert!(licence_boot::runtime().is_some(), "the licence runtime is there");
    assert!(workload_place_rpc::licence_exchange().is_some(), "and the exchange");
}
