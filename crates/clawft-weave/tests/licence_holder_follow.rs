//! ADR-106 phase 3: the licence path follows the service's holder answer.
//! A daemon that boots before its service link is bound is "unknown" and runs
//! no licence path; once the link is bound the path comes up; when the
//! service's `cluster_owner_uid` moves to another uid the path idles (the
//! licence RPCs refuse) and comes back when it returns; a dropped service
//! makes the holder unknown again, with its own refusal reason, and the
//! reconnect restores it.
//!
//! One process per test file: the module-wide licence state is this test's.
#![cfg(all(unix, feature = "placement"))]

mod mesh_e2e;

use std::sync::Arc;
use std::sync::atomic::Ordering;

use clawft_kernel::Kernel;
use clawft_kernel::mesh_delivery::LocalDelivery;
use clawft_mesh_local::proto::Message;
use clawft_platform::NativePlatform;
use clawft_types::config::{ChainConfig, Config, KernelConfig, MeshServicePolicy};
use clawft_weave::licence_boot::{self, HolderState};
use clawft_weave::node_identity::DaemonIdentity;
use clawft_weave::{cog_swarm, licence_rpc, placement_boot, workload_place_rpc};
use mesh_e2e::*;
use serde_json::json;
use tokio::sync::RwLock;

async fn bind_error(kernel: &Arc<RwLock<Kernel<NativePlatform>>>) -> String {
    let r = licence_rpc::dispatch("workload.node.bind", json!({}), kernel.clone()).await;
    assert!(!r.ok);
    r.error.unwrap_or_default()
}

async fn until_state(want: HolderState) {
    let links = cog_swarm::licence_links();
    for _ in 0..200 {
        let _ = links.refresh().await;
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        if licence_boot::holder_state() == want {
            return;
        }
    }
    panic!("holder state never became {want:?} (is {:?})", licence_boot::holder_state());
}

#[tokio::test]
async fn boot_before_link_then_owner_flips_and_a_service_restart_are_followed() {
    let mut svc = Svc::with_config(Default::default(), |c| c.cluster_owner_uid = Some(OTHER_UID)).await;
    svc.next_uid.store(OTHER_UID, Ordering::SeqCst);
    let cfg = svc.mesh_cfg(MeshServicePolicy::Required);
    let wrap: Box<dyn FnOnce(Arc<Inbox>) -> Arc<dyn LocalDelivery>> = Box::new(cog_swarm::wrap_inbox);
    let owner = link_via(&cfg, other_endpoint(&svc, key(2), vec![]), fast(), Some(wrap)).await;

    let tmp = tempfile::tempdir().unwrap();
    let runtime = tmp.path().join("runtime");
    let rec = svc.record();
    let identity = DaemonIdentity::for_service(rec.node_id.clone(), rec.machine_pubkey).unwrap();
    let kcfg = KernelConfig { chain: Some(ChainConfig::isolated_in(&tmp.path().join("chain"))), ..KernelConfig::default() };
    let kernel = Kernel::boot_in_service_mode(Config::default(), kcfg, Arc::new(NativePlatform::new()), rec.node_id.clone())
        .await
        .expect("kernel boots in service mode");
    let kernel = Arc::new(RwLock::new(kernel));

    // Boot before the link is bound to the daemon's wiring: unknown, nothing runs.
    placement_boot::start(&kernel, &identity, &runtime).await;
    assert_eq!(licence_boot::holder_state(), HolderState::Unknown);
    assert!(licence_boot::runtime().is_none() && workload_place_rpc::licence_exchange().is_none());
    assert!(bind_error(&kernel).await.contains("holder status unknown"));

    // The link is bound: its first refresh says holder, and the path comes up.
    cog_swarm::set_forwarder(owner.handle.as_ref().unwrap().forwarder.clone());
    wait_until("the licence path comes up", || {
        licence_boot::holder_state() == HolderState::Holder
            && licence_boot::runtime().is_some()
            && workload_place_rpc::licence_exchange().is_some()
    })
    .await;
    assert!(!bind_error(&kernel).await.contains("reserved licence topics"), "a holder does not refuse for that");

    // The owner moves to another uid: the path idles.
    svc.admin(Message::PolicySet { admission: None, cluster_owner_uid: Some(9002) }).await;
    until_state(HolderState::NotHolder).await;
    assert!(bind_error(&kernel).await.contains("reserved licence topics"));
    let st = licence_rpc::dispatch("workload.node.binding", json!({}), kernel.clone()).await.result.unwrap();
    assert_eq!((st["reserved_holder"].clone(), st["installed"].clone()), (json!(false), json!(true)));
    // ...and back.
    svc.admin(Message::PolicySet { admission: None, cluster_owner_uid: Some(OTHER_UID) }).await;
    until_state(HolderState::Holder).await;

    // The service goes away: unknown, with its own reason; back on reconnect.
    svc.next_uid.store(OTHER_UID, Ordering::SeqCst);
    svc.stop().await;
    wait_until("the link notices", || owner.link_state().as_deref() == Some("reconnecting")).await;
    until_state(HolderState::Unknown).await;
    assert!(bind_error(&kernel).await.contains("holder status unknown"));
    svc.begin().await;
    wait_until("the link reconnects", || owner.link_state().as_deref() == Some("connected")).await;
    until_state(HolderState::Holder).await;
    assert!(workload_place_rpc::licence_exchange().is_some());
}
