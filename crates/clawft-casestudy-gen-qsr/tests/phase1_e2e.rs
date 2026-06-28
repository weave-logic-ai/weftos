//! Phase 1 — streaming ingest spine.
//!
//! Verifies: impulse HLC ordering, idempotent dedupe, late-arrival rewrite,
//! shard routing, governance blocking, missing-window detection, and that the
//! stream-built case graph is equivalent to the batch-built graph.

use clawft_casestudy_gen_qsr::{
    config::GeneratorConfig,
    events::DailyRollup,
    generate,
    governance::Governance,
    graph,
    impulse::{Hlc, Impulse, ImpulseQueue, ImpulseType},
    ingest::IngestDriver,
    shard::ShardRouter,
};

fn tiny_corpus() -> (tempfile::TempDir, clawft_casestudy_gen_qsr::Corpus) {
    let tmp = tempfile::tempdir().unwrap();
    let config = GeneratorConfig::tiny(42);
    let corpus = generate(&config, tmp.path()).unwrap();
    (tmp, corpus)
}

#[test]
fn impulse_queue_dedupes_identical_emissions() {
    let mut q = ImpulseQueue::new();
    let imp = Impulse {
        hlc: Hlc {
            physical_ms: 1000,
            logical: 0,
        },
        kind: ImpulseType::NoveltyDetected,
        store_ref: "store:brand-a:metro-alpha_0000".into(),
        brand: "brand-a".into(),
        region: "region-1".into(),
        metro: "metro-alpha".into(),
        day_index: 0,
        business_date: "2026-01-01".into(),
        revenue: 5000.0,
        budget_revenue: 5100.0,
        budget_variance_pct: -0.02,
        promo_codes_active: vec![],
        reconstructed_from_lake: false,
        idempotency_key: "k0".into(),
    };
    assert!(q.emit(imp.clone()));
    assert!(!q.emit(imp.clone()));
    assert!(!q.emit(imp.clone()));
    assert_eq!(q.pending(), 1);
    assert_eq!(q.stats().dropped_duplicates, 2);
}

#[test]
fn stream_built_graph_matches_batch_built_graph() {
    let (_tmp, corpus) = tiny_corpus();

    // Batch: build graph directly from events.
    let batch_graph = graph::build(&corpus.dims, &corpus.events);

    // Stream: build graph via ingest driver.
    let mut driver = IngestDriver::new(&corpus.dims);
    driver.emit_stream(&corpus.events);
    driver.run_to_completion();

    // Equivalent node/edge counts for DailyRollup and Causes edges.
    let batch_dailies = batch_graph
        .nodes
        .iter()
        .filter(|n| matches!(n.node_type, graph::NodeType::DailyRollup))
        .count();
    let stream_dailies = driver
        .graph
        .nodes
        .iter()
        .filter(|n| matches!(n.node_type, graph::NodeType::DailyRollup))
        .count();
    assert_eq!(batch_dailies, stream_dailies);

    let batch_causes = batch_graph
        .edges
        .iter()
        .filter(|e| matches!(e.edge_type, graph::EdgeType::Causes))
        .count();
    let stream_causes = driver
        .graph
        .edges
        .iter()
        .filter(|e| matches!(e.edge_type, graph::EdgeType::Causes))
        .count();
    assert_eq!(batch_causes, stream_causes);
}

#[test]
fn late_arrival_is_rewritten_to_belief_update() {
    // watermark = 1h so anything older than 1h of the store's high-water mark
    // is a late arrival.
    let mut q = ImpulseQueue::with_watermark_ms(3_600_000);
    let fresh = Impulse {
        hlc: Hlc {
            physical_ms: 10_000_000,
            logical: 0,
        },
        kind: ImpulseType::NoveltyDetected,
        store_ref: "store:x".into(),
        brand: "brand-a".into(),
        region: "region-1".into(),
        metro: "metro-alpha".into(),
        day_index: 100,
        business_date: "2026-04-20".into(),
        revenue: 5000.0,
        budget_revenue: 5100.0,
        budget_variance_pct: 0.0,
        promo_codes_active: vec![],
        reconstructed_from_lake: false,
        idempotency_key: "store:x::100".into(),
    };
    q.emit(fresh.clone());

    let mut late = fresh.clone();
    late.hlc.physical_ms = 100_000; // 1e7 − 1e5 > 3.6e6, so late
    late.day_index = 99;
    late.idempotency_key = "store:x::99".into();
    q.emit(late);

    let drained = q.drain_ready(16);
    let late_one = drained.iter().find(|i| i.day_index == 99).unwrap();
    assert_eq!(late_one.kind, ImpulseType::BeliefUpdate);
    assert!(late_one.reconstructed_from_lake);
    assert_eq!(q.stats().late_arrivals, 1);
}

#[test]
fn shard_router_keys_by_brand_region_quarter() {
    let router = ShardRouter::new();
    let imp = |date: &str, brand: &str, region: &str| Impulse {
        hlc: Hlc {
            physical_ms: 0,
            logical: 0,
        },
        kind: ImpulseType::NoveltyDetected,
        store_ref: format!("store:{}:{}_0001", brand, region),
        brand: brand.into(),
        region: region.into(),
        metro: "metro-alpha".into(),
        day_index: 0,
        business_date: date.into(),
        revenue: 0.0,
        budget_revenue: 0.0,
        budget_variance_pct: 0.0,
        promo_codes_active: vec![],
        reconstructed_from_lake: false,
        idempotency_key: date.into(),
    };
    let q1 = router.route(&imp("2026-02-15", "brand-a", "region-1"));
    let q2 = router.route(&imp("2026-05-15", "brand-a", "region-1"));
    let r2 = router.route(&imp("2026-02-15", "brand-a", "region-2"));
    assert_eq!(q1.quarter, 1);
    assert_eq!(q2.quarter, 2);
    assert_ne!(q1, q2);
    assert_ne!(q1, r2);
    assert!(q1.path().ends_with("2026-Q1.rvf"));
}

#[test]
fn governance_blocks_denylisted_franchisee() {
    let mut g = Governance::new();
    g.denylist_franchisee("metro-alpha_0001");
    let imp = Impulse {
        hlc: Hlc {
            physical_ms: 0,
            logical: 0,
        },
        kind: ImpulseType::NoveltyDetected,
        store_ref: "store:brand-a:metro-alpha_0001".into(),
        brand: "brand-a".into(),
        region: "region-1".into(),
        metro: "metro-alpha".into(),
        day_index: 0,
        business_date: "2026-02-15".into(),
        revenue: 5000.0,
        budget_revenue: 5000.0,
        budget_variance_pct: 0.0,
        promo_codes_active: vec![],
        reconstructed_from_lake: false,
        idempotency_key: "k".into(),
    };
    let decision = g.evaluate(&imp);
    assert!(!decision.permitted);
    assert_eq!(decision.rule, "franchisee_boundary");
}

#[test]
fn governance_blocks_novelty_in_sealed_quarter() {
    let mut g = Governance::new();
    g.seal_quarter(2026, 1);
    let imp = Impulse {
        hlc: Hlc {
            physical_ms: 0,
            logical: 0,
        },
        kind: ImpulseType::NoveltyDetected,
        store_ref: "store:brand-a:metro-alpha_0000".into(),
        brand: "brand-a".into(),
        region: "region-1".into(),
        metro: "metro-alpha".into(),
        day_index: 0,
        business_date: "2026-02-15".into(), // Q1
        revenue: 5000.0,
        budget_revenue: 5000.0,
        budget_variance_pct: 0.0,
        promo_codes_active: vec![],
        reconstructed_from_lake: false,
        idempotency_key: "k".into(),
    };
    let decision = g.evaluate(&imp);
    assert!(!decision.permitted);
    assert_eq!(decision.rule, "sox_sealed_quarter");
}

#[test]
fn governance_permits_belief_update_in_sealed_quarter() {
    let mut g = Governance::new();
    g.seal_quarter(2026, 1);
    let imp = Impulse {
        hlc: Hlc {
            physical_ms: 0,
            logical: 0,
        },
        kind: ImpulseType::BeliefUpdate,
        store_ref: "store:brand-a:metro-alpha_0000".into(),
        brand: "brand-a".into(),
        region: "region-1".into(),
        metro: "metro-alpha".into(),
        day_index: 0,
        business_date: "2026-02-15".into(),
        revenue: 5000.0,
        budget_revenue: 5000.0,
        budget_variance_pct: 0.0,
        promo_codes_active: vec![],
        reconstructed_from_lake: true,
        idempotency_key: "k".into(),
    };
    assert!(g.evaluate(&imp).permitted);
}

#[test]
fn ingest_driver_throughput_smoke() {
    let (_tmp, corpus) = tiny_corpus();
    let mut driver = IngestDriver::new(&corpus.dims);
    let t0 = std::time::Instant::now();
    driver.emit_stream(&corpus.events);
    driver.run_to_completion();
    let elapsed = t0.elapsed().as_secs_f64();
    let per_sec = driver.stats.impulses_applied as f64 / elapsed.max(1e-9);
    assert!(
        per_sec > 1_000.0,
        "expected >1K/sec, got {:.0}/sec",
        per_sec
    );
    assert_eq!(driver.stats.impulses_applied, corpus.events.len() as u64);
    assert!(!driver.shards.shards.is_empty());
}

#[test]
fn missing_windows_are_detected() {
    let mut q = ImpulseQueue::new();
    let base_event = |day: u32| DailyRollup {
        label: format!("day:brand-a:0000:day-{}", day),
        store_ref: "store:brand-a:metro-alpha_0000".into(),
        business_date: format!("2026-01-{:02}", day + 1),
        day_index: day,
        tickets: 300,
        revenue: 5000.0,
        cogs: 1500.0,
        labor: 1200.0,
        labor_hours: 60.0,
        avg_ticket: 14.0,
        budget_revenue: 5100.0,
        budget_variance_pct: -0.02,
        promo_codes_active: vec![],
        verified: true,
    };
    // Emit days 0, 1, 3, 4 (day 2 missing).
    for day in [0, 1, 3, 4] {
        let ev = base_event(day);
        q.emit(Impulse::from_rollup_with_context(
            &ev,
            "brand-a",
            "region-1",
            "metro-alpha",
        ));
    }
    let gaps = q.detect_missing_windows(&["store:brand-a:metro-alpha_0000".into()]);
    assert_eq!(gaps.len(), 1);
    assert_eq!(gaps[0].day_index, 2);
}
