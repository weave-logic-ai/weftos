//! Phase 4 — hardening: chaos, audit chain, privacy, full-tier smoke.

use clawft_casestudy_gen_qsr::{
    audit::{AuditKind, HashChainAuditor},
    chaos::{self, ChaosConfig},
    config::GeneratorConfig,
    generate,
    ingest::IngestDriver,
    privacy,
};

fn tiny_corpus() -> (tempfile::TempDir, clawft_casestudy_gen_qsr::Corpus) {
    let tmp = tempfile::tempdir().unwrap();
    let config = GeneratorConfig::tiny(42);
    let corpus = generate(&config, tmp.path()).unwrap();
    (tmp, corpus)
}

// -----------------------------------------------------------------------
// Audit chain
// -----------------------------------------------------------------------

#[test]
fn audit_chain_verifies_on_clean_run() {
    let (_tmp, corpus) = tiny_corpus();
    let mut driver = IngestDriver::new(&corpus.dims);
    driver.emit_stream(&corpus.events);
    driver.run_to_completion();

    assert!(!driver.auditor.is_empty());
    driver.auditor.verify().expect("audit chain must verify");
    assert_eq!(
        driver.auditor.count_by_kind(AuditKind::ImpulseApplied),
        corpus.events.len(),
        "every applied impulse should have one audit entry",
    );
}

#[test]
fn audit_chain_detects_payload_tampering() {
    let (_tmp, corpus) = tiny_corpus();
    let mut driver = IngestDriver::new(&corpus.dims);
    driver.emit_stream(&corpus.events);
    driver.run_to_completion();

    // Tamper with one entry's payload_hash post-hoc.
    driver.auditor.entries[5].payload_hash = "deadbeef".repeat(8);
    assert!(driver.auditor.verify().is_err());
}

#[test]
fn audit_chain_detects_reordering() {
    let (_tmp, corpus) = tiny_corpus();
    let mut driver = IngestDriver::new(&corpus.dims);
    driver.emit_stream(&corpus.events);
    driver.run_to_completion();

    // Swap two mid-chain entries.
    let a = 5;
    let b = 8;
    driver.auditor.entries.swap(a, b);
    assert!(driver.auditor.verify().is_err());
}

#[test]
fn audit_chain_records_blocked_impulses() {
    let (_tmp, corpus) = tiny_corpus();
    let mut driver = IngestDriver::new(&corpus.dims);
    driver.governance.denylist_franchisee("metro-alpha_0000");

    driver.emit_stream(&corpus.events);
    driver.run_to_completion();

    let blocked = driver.auditor.count_by_kind(AuditKind::ImpulseBlocked);
    assert!(blocked > 0);
    driver.auditor.verify().unwrap();
}

#[test]
fn audit_standalone_round_trip_through_json() {
    let mut auditor = HashChainAuditor::new();
    for i in 0..20u64 {
        auditor.record(
            AuditKind::ImpulseApplied,
            format!("entry-{}", i),
            &serde_json::json!({"i": i}),
        );
    }
    auditor.verify().unwrap();
    let json = serde_json::to_string(&auditor).unwrap();
    let loaded: HashChainAuditor = serde_json::from_str(&json).unwrap();
    loaded.verify().unwrap();
    assert_eq!(loaded.len(), 20);
}

// -----------------------------------------------------------------------
// Chaos
// -----------------------------------------------------------------------

#[test]
fn chaos_drop_removes_expected_fraction() {
    let (_tmp, corpus) = tiny_corpus();
    let config = ChaosConfig {
        seed: 7,
        drop_rate: 0.30,
        drop_window: None,
        duplicate_rate: 0.0,
        reorder_window: 0,
        clock_skew_store_prob: 0.0,
        clock_skew_days: 0,
    };
    let (mutated, report) = chaos::apply(&config, &corpus.events);
    assert_eq!(report.total_input, corpus.events.len());
    assert!(report.dropped > 0);
    assert_eq!(mutated.len() + report.dropped, corpus.events.len());
    // Observed drop rate should be within ±10 percentage points of the target
    // at N=300.
    let observed = report.dropped as f64 / corpus.events.len() as f64;
    assert!(
        (observed - 0.30).abs() < 0.10,
        "observed drop rate {} not within tolerance",
        observed
    );
}

#[test]
fn chaos_dedupe_preserved_after_duplicate_storm() {
    let (_tmp, corpus) = tiny_corpus();
    let config = ChaosConfig {
        seed: 11,
        drop_rate: 0.0,
        drop_window: None,
        duplicate_rate: 0.50, // double half the stream
        reorder_window: 0,
        clock_skew_store_prob: 0.0,
        clock_skew_days: 0,
    };
    let (mutated, report) = chaos::apply(&config, &corpus.events);
    assert!(mutated.len() > corpus.events.len());
    assert!(report.duplicated > 0);

    let mut driver = IngestDriver::new(&corpus.dims);
    driver.emit_stream(&mutated);
    driver.run_to_completion();

    // The impulse queue must dedupe — final applied count == original.
    assert_eq!(driver.stats.impulses_applied, corpus.events.len() as u64);
    assert!(driver.stats.dropped_duplicates >= report.duplicated as u64);
    driver.auditor.verify().unwrap();
}

#[test]
fn chaos_clock_skew_does_not_break_audit_or_graph() {
    // The specific late-arrival → BeliefUpdate rewrite is covered in
    // `phase1_e2e::late_arrival_is_rewritten_to_belief_update`. The chaos
    // requirement is different: clock-skew must compose with the pipeline
    // without breaking audit integrity or graph consistency.
    let (_tmp, corpus) = tiny_corpus();
    let config = ChaosConfig {
        seed: 17,
        drop_rate: 0.0,
        drop_window: None,
        duplicate_rate: 0.0,
        reorder_window: 0,
        clock_skew_store_prob: 0.40,
        clock_skew_days: 3,
    };
    let (skewed, report) = chaos::apply(&config, &corpus.events);
    assert!(report.skewed_stores > 0);

    let mut driver = IngestDriver::new(&corpus.dims);
    driver.emit_stream(&skewed);
    driver.run_to_completion();

    driver.auditor.verify().unwrap();
    // Skewed events still create distinct daily rollups (skew changed the
    // business_date, not the day_index, so idempotency_keys remain unique).
    let daily_count = driver
        .graph
        .nodes
        .iter()
        .filter(|n| n.node_type == clawft_casestudy_gen_qsr::graph::NodeType::DailyRollup)
        .count();
    assert_eq!(daily_count, corpus.events.len());
}

#[test]
fn chaos_reorder_is_rescued_by_hlc_ordering() {
    let (_tmp, corpus) = tiny_corpus();
    let config = ChaosConfig {
        seed: 23,
        drop_rate: 0.0,
        drop_window: None,
        duplicate_rate: 0.0,
        reorder_window: 20,
        clock_skew_store_prob: 0.0,
        clock_skew_days: 0,
    };
    let (mutated, report) = chaos::apply(&config, &corpus.events);
    assert!(report.reordered_pairs > 0);

    let mut driver = IngestDriver::new(&corpus.dims);
    driver.emit_stream(&mutated);
    driver.run_to_completion();

    // Graph correctness invariant: every day:* node still unique and count
    // equals the number of distinct (store, day) pairs in the stream.
    let distinct_expected: std::collections::HashSet<_> = corpus
        .events
        .iter()
        .map(|e| (e.store_ref.clone(), e.day_index))
        .collect();
    let daily_count = driver
        .graph
        .nodes
        .iter()
        .filter(|n| n.node_type == clawft_casestudy_gen_qsr::graph::NodeType::DailyRollup)
        .count();
    assert_eq!(daily_count, distinct_expected.len());
    driver.auditor.verify().unwrap();
}

// -----------------------------------------------------------------------
// Privacy
// -----------------------------------------------------------------------

#[test]
fn privacy_scan_clean_on_generator_output() {
    let (tmp, _corpus) = tiny_corpus();
    let report = privacy::scan_corpus(tmp.path()).unwrap();
    assert_eq!(
        report.violations.len(),
        0,
        "unexpected violations: {:?}",
        report.violations
    );
    assert!(report.hashed_id_count > 0);
    assert_eq!(report.hashed_id_count, report.people_count);
    assert!(!report.scanned_files.is_empty());
}

#[test]
fn privacy_scan_detects_raw_email_injected_into_corpus() {
    let (tmp, _corpus) = tiny_corpus();
    // Simulate a bad-actor payload leaking an email into a dimensions file.
    let leaky = tmp.path().join("dimensions").join("leak.json");
    std::fs::write(
        &leaky,
        r#"[{"note":"contact jane.doe@acme.com for details"}]"#,
    )
    .unwrap();
    let report = privacy::scan_corpus(tmp.path()).unwrap();
    assert!(
        report
            .violations
            .iter()
            .any(|v| matches!(v.kind, privacy::ViolationKind::EmailPattern))
    );
}

#[test]
fn privacy_scan_detects_malformed_hash_prefix() {
    let (_tmp, mut corpus) = tiny_corpus();
    corpus.dims.people[0].employee_id_hashed = "raw-employee-id-0000".into();
    let violations = privacy::check_dimensions(&corpus.dims);
    assert!(
        violations
            .iter()
            .any(|v| matches!(v.kind, privacy::ViolationKind::MalformedHashPrefix))
    );
}

// -----------------------------------------------------------------------
// Full-tier smoke (ignored)
// -----------------------------------------------------------------------

/// Medium-tier full-stack smoke: generate → chaos → ingest → gap-sweep →
/// audit verify. Ignored by default so CI stays fast. Run with:
///
/// `cargo test -p clawft-casestudy-gen-qsr --test phase4_hardening full_stack_smoke_medium -- --ignored --nocapture --release`
#[ignore]
#[test]
fn full_stack_smoke_medium() {
    use clawft_casestudy_gen_qsr::config::ScaleTier;
    use clawft_casestudy_gen_qsr::{coherence, dashboard, gaps};

    let tmp = tempfile::tempdir().unwrap();
    let t_gen = std::time::Instant::now();
    let corpus = generate(
        &GeneratorConfig::default_for_tier(42, ScaleTier::Small),
        tmp.path(),
    )
    .unwrap();
    eprintln!(
        "generated {} events across {} stores in {:.2}s",
        corpus.events.len(),
        corpus.dims.stores.len(),
        t_gen.elapsed().as_secs_f64()
    );

    // Apply mild chaos
    let (mutated, chaos_report) = chaos::apply(
        &ChaosConfig {
            seed: 7,
            drop_rate: 0.02,
            drop_window: None,
            duplicate_rate: 0.01,
            reorder_window: 4,
            clock_skew_store_prob: 0.05,
            clock_skew_days: -60,
        },
        &corpus.events,
    );
    eprintln!("chaos: {:?}", chaos_report);

    let t_ingest = std::time::Instant::now();
    let mut driver = IngestDriver::new(&corpus.dims);
    driver.emit_stream(&mutated);
    driver.run_to_completion();
    eprintln!(
        "ingested in {:.2}s — {} applied, {} dup-dropped, {} late",
        t_ingest.elapsed().as_secs_f64(),
        driver.stats.impulses_applied,
        driver.stats.dropped_duplicates,
        driver.stats.late_arrivals
    );

    driver.auditor.verify().unwrap();

    // Gap sweep
    let t_gap = std::time::Instant::now();
    let config = GeneratorConfig::default_for_tier(42, ScaleTier::Small);
    let gap_report = gaps::sweep(&config, &corpus.dims, &corpus.ops);
    eprintln!(
        "gap sweep: {} gaps in {:.3}s",
        gap_report.total(),
        t_gap.elapsed().as_secs_f64()
    );

    // Dashboard
    let scores =
        coherence::score_all_stores(&corpus.dims, &corpus.events, &corpus.ops, &gap_report);
    let dash = dashboard::build(&gap_report, scores, 10);
    eprintln!(
        "dashboard: {} stores, avg_health={:.3}",
        dash.summary.total_stores, dash.summary.avg_ops_health
    );

    // Privacy
    let report = privacy::scan_corpus(tmp.path()).unwrap();
    assert!(
        report.is_clean(),
        "privacy scan failed: {:?}",
        report.violations
    );
}
