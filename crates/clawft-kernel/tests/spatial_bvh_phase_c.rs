//! WEFT-718 Phase C integration: insert leaves → snapshot → restore → match.
//!
//! Boots a standalone `SpatialService` with a recording (and, when
//! `exochain` is on, real) chain sink, inserts 100 leaves, seals, then
//! restores into a fresh service and asserts byte-identical leaf sets.

#![cfg(feature = "ecc")]

use std::sync::Arc;

use clawft_bvh::{
    Aabb, BranchId, IdentityKind, Leaf, RecordingChainSink, Vec3,
};
use clawft_kernel::spatial_backend::SpatialBackend;
use clawft_kernel::spatial_bvh::BvhBackend;
use clawft_kernel::spatial_service::{SpatialService, SpatialServiceConfig};
use clawft_kernel::SystemService;
use clawft_types::config::SpatialConfig;

#[test]
fn insert_100_snapshot_restore_identical() {
    let cfg = SpatialServiceConfig {
        enabled: true,
        max_leaves: 10_000,
        phase_commit_threshold: 64,
    };
    let sink = Arc::new(RecordingChainSink::new());
    let backend = Arc::new(BvhBackend::with_sink(cfg.to_store_config(), sink.clone()));
    let svc = SpatialService::new(cfg, backend.clone());

    let mut ids = Vec::with_capacity(100);
    for i in 0..100u32 {
        let x = (i % 10) as f32;
        let y = (i / 10) as f32;
        let leaf = Leaf::new(
            Aabb::from_min_max(Vec3::new(x, y, 0.0), Vec3::new(x + 0.4, y + 0.4, 0.4)),
            if i % 2 == 0 {
                IdentityKind::Object
            } else {
                IdentityKind::Event
            },
            i,
            vec![i as u8, (i >> 8) as u8],
        );
        ids.push(svc.insert(leaf).expect("insert"));
    }

    assert_eq!(svc.status().leaf_count, 100);
    assert_eq!(sink.len(), 100, "each insert must emit a chain event");

    // Seal for phase boundary witness.
    backend.rebalance_seal().expect("seal");
    assert_eq!(sink.len(), 101);

    let snap = backend.snapshot_main();
    assert_eq!(snap.len(), 100);

    // Fresh backend + restore (simulates restart + replay of sealed state).
    let backend2 = Arc::new(BvhBackend::with_defaults());
    backend2.restore_main(&snap).expect("restore");
    assert_eq!(backend.snapshot_main(), backend2.snapshot_main());

    // Query parity on both backends.
    let region = Aabb::from_min_max(Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 1.0, 1.0));
    let mut a = backend.query_aabb(region);
    let mut b = backend2.query_aabb(region);
    a.sort_by_key(|id| id.0);
    b.sort_by_key(|id| id.0);
    assert_eq!(a, b);
    assert!(!a.is_empty());

    // Spot-check first leaf id still resolvable with same tag.
    let first = ids[0];
    assert_eq!(
        backend.get(first).map(|l| l.tag),
        backend2.get(first).map(|l| l.tag)
    );
    let _ = BranchId::MAIN;
}

#[test]
fn spatial_config_opt_in_defaults_disabled() {
    let cfg = SpatialConfig::default();
    assert!(!cfg.enabled);
    let svc = SpatialService::from_spatial_config(&cfg);
    assert!(!svc.status().enabled);
    assert_eq!(svc.backend().backend_name(), "bvh");
}

#[cfg(feature = "exochain")]
#[test]
fn chain_manager_receives_bvh_events() {
    use clawft_kernel::chain::{
        ChainManager, EVENT_KIND_BVH_INSERT, EVENT_KIND_BVH_REBALANCE_SEAL,
    };
    use clawft_kernel::spatial_bvh::KernelBvhChainSink;

    let cm = Arc::new(ChainManager::default_local());
    let before = cm.len();
    let sink = Arc::new(KernelBvhChainSink::new(cm.clone()));
    let backend = BvhBackend::with_sink(clawft_bvh::BvhStoreConfig::default(), sink);

    backend
        .insert(Leaf::empty_payload(Aabb::unit(), IdentityKind::Object, 42))
        .unwrap();
    backend.rebalance_seal().unwrap();

    let events = cm.tail(0);
    let kinds: Vec<&str> = events.iter().map(|e| e.kind.as_str()).collect();
    assert!(
        kinds.contains(&EVENT_KIND_BVH_INSERT),
        "expected bvh.insert in {kinds:?}"
    );
    assert!(
        kinds.contains(&EVENT_KIND_BVH_REBALANCE_SEAL),
        "expected bvh.rebalance_seal in {kinds:?}"
    );
    assert!(cm.len() > before);
}

#[test]
fn service_status_path() {
    let svc = SpatialService::from_config(SpatialServiceConfig {
        enabled: true,
        max_leaves: 50,
        phase_commit_threshold: 8,
    });
    let st = svc.status();
    assert_eq!(st.backend, "bvh");
    assert_eq!(st.leaf_count, 0);
    assert!(st.enabled);
    // health_detail is what CLI / weft status will surface until Phase E.
    assert!(svc.health_detail().unwrap().contains("backend=bvh"));
}
