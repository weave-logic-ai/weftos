//! QSR synthetic corpus generator CLI.

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use clawft_casestudy_gen_qsr::{
    chaos::{self, ChaosConfig},
    coherence,
    config::{GeneratorConfig, ScaleTier},
    dashboard,
    eml::EmlModel,
    engine::{self, ScenarioEngine},
    gaps, graph,
    ingest::IngestDriver,
    output, privacy, scenarios, scoring,
    scoring::ScenarioPrediction,
    truth::{self, CounterfactualTruth},
};
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "clawft-casestudy-gen-qsr",
    about = "QSR synthetic corpus generator (Phase 0 test harness)",
    version
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Generate a corpus at the given scale tier.
    Generate {
        #[arg(long, default_value = "tiny")]
        tier: String,
        #[arg(long, default_value_t = 42)]
        seed: u64,
        #[arg(long, default_value = "./corpus")]
        out: PathBuf,
    },
    /// Compute the ground-truth counterfactual for a scenario against a corpus.
    Counterfactual {
        #[arg(long)]
        corpus: PathBuf,
        #[arg(long)]
        scenario: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Score a prediction file against a counterfactual-truth file.
    Score {
        #[arg(long)]
        prediction: PathBuf,
        #[arg(long)]
        truth: PathBuf,
    },
    /// Phase 1: stream the corpus through the DEMOCRITUS ingest driver and
    /// report the resulting shard / graph stats.
    Ingest {
        #[arg(long)]
        corpus: PathBuf,
        #[arg(long, default_value_t = 64)]
        max_per_tick: usize,
    },
    /// Phase 2: run the scenario engine against a corpus and write a
    /// ScenarioPrediction JSON.
    Predict {
        #[arg(long)]
        corpus: PathBuf,
        #[arg(long)]
        scenario: PathBuf,
        #[arg(long, default_value_t = 256)]
        mc_samples: usize,
        #[arg(long, default_value_t = 7)]
        mc_seed: u64,
        #[arg(long)]
        eml: Option<PathBuf>,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Phase 2: train an EML residual model from the truth manifest + a
    /// directory of scenario YAMLs.
    TrainEml {
        #[arg(long)]
        corpus: PathBuf,
        #[arg(long)]
        scenarios_dir: PathBuf,
        #[arg(long, default_value_t = 1e-3)]
        ridge: f64,
        #[arg(long)]
        out: PathBuf,
    },
    /// Phase 3: run all 8 gap patterns against the corpus and emit a JSON report.
    GapSweep {
        #[arg(long)]
        corpus: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Phase 3: render the ops dashboard (text + JSON) for the corpus.
    Dashboard {
        #[arg(long)]
        corpus: PathBuf,
        #[arg(long, default_value_t = 10)]
        top_n: usize,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Phase 4: run ingest through chaos injection (drop/dup/skew/reorder)
    /// and verify the audit chain + ingest recovery.
    ChaosRun {
        #[arg(long)]
        corpus: PathBuf,
        #[arg(long, default_value_t = 0.05)]
        drop_rate: f64,
        #[arg(long, default_value_t = 0.02)]
        duplicate_rate: f64,
        #[arg(long, default_value_t = 4)]
        reorder_window: usize,
        #[arg(long, default_value_t = 0.0)]
        clock_skew_store_prob: f64,
        #[arg(long, default_value_t = 0)]
        clock_skew_days: i32,
        #[arg(long, default_value_t = 13)]
        seed: u64,
    },
    /// Phase 4: verify a prior run's audit chain (written via ChaosRun).
    AuditVerify {
        #[arg(long)]
        audit: PathBuf,
    },
    /// Phase 4: scan a corpus on disk for PII-pattern violations.
    PrivacyScan {
        #[arg(long)]
        corpus: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Roll the daily rollups in a corpus up to weekly grain.
    /// Used for historic compaction (previous years → weekly) per §4.1 of the analysis.
    RollupWeekly {
        #[arg(long)]
        corpus: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
    /// Roll the daily rollups up to calendar-month grain (coarser historic archive).
    RollupMonthly {
        #[arg(long)]
        corpus: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
    /// HNSW recall survey on financial / operational feature vectors.
    /// Compares recall@k for daily / weekly / monthly grains at several
    /// `ef_search` settings. Exact NN is the oracle.
    RecallBench {
        #[arg(long)]
        corpus: PathBuf,
        #[arg(long, default_value_t = 10)]
        k: usize,
        #[arg(long, default_value_t = 200)]
        queries: usize,
        #[arg(long, default_value_t = 200)]
        ef_construction: usize,
        #[arg(long, default_value_t = 7)]
        seed: u64,
        /// Comma-separated grains to benchmark: daily,weekly,monthly.
        #[arg(long, default_value = "daily,weekly,monthly")]
        grains: String,
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Generate { tier, seed, out } => cmd_generate(&tier, seed, &out),
        Cmd::Counterfactual {
            corpus,
            scenario,
            out,
        } => cmd_counterfactual(&corpus, &scenario, out.as_deref()),
        Cmd::Score { prediction, truth } => cmd_score(&prediction, &truth),
        Cmd::Ingest {
            corpus,
            max_per_tick,
        } => cmd_ingest(&corpus, max_per_tick),
        Cmd::Predict {
            corpus,
            scenario,
            mc_samples,
            mc_seed,
            eml,
            out,
        } => cmd_predict(
            &corpus,
            &scenario,
            mc_samples,
            mc_seed,
            eml.as_deref(),
            out.as_deref(),
        ),
        Cmd::TrainEml {
            corpus,
            scenarios_dir,
            ridge,
            out,
        } => cmd_train_eml(&corpus, &scenarios_dir, ridge, &out),
        Cmd::GapSweep { corpus, out } => cmd_gap_sweep(&corpus, out.as_deref()),
        Cmd::Dashboard { corpus, top_n, out } => cmd_dashboard(&corpus, top_n, out.as_deref()),
        Cmd::ChaosRun {
            corpus,
            drop_rate,
            duplicate_rate,
            reorder_window,
            clock_skew_store_prob,
            clock_skew_days,
            seed,
        } => cmd_chaos_run(
            &corpus,
            ChaosConfig {
                seed,
                drop_rate,
                drop_window: None,
                duplicate_rate,
                reorder_window,
                clock_skew_store_prob,
                clock_skew_days,
            },
        ),
        Cmd::AuditVerify { audit } => cmd_audit_verify(&audit),
        Cmd::PrivacyScan { corpus, out } => cmd_privacy_scan(&corpus, out.as_deref()),
        Cmd::RollupWeekly { corpus, out } => cmd_rollup_weekly(&corpus, &out),
        Cmd::RollupMonthly { corpus, out } => cmd_rollup_monthly(&corpus, &out),
        Cmd::RecallBench {
            corpus,
            k,
            queries,
            ef_construction,
            seed,
            grains,
            out,
        } => cmd_recall_bench(
            &corpus,
            k,
            queries,
            ef_construction,
            seed,
            &grains,
            out.as_deref(),
        ),
    }
}

fn cmd_generate(tier: &str, seed: u64, out: &std::path::Path) -> Result<()> {
    let tier = ScaleTier::parse(tier).ok_or_else(|| anyhow::anyhow!("unknown tier: {}", tier))?;
    let config = GeneratorConfig::default_for_tier(seed, tier);
    println!("generating tier={:?} seed={} out={:?}", tier, seed, out);
    let t0 = std::time::Instant::now();
    let corpus = clawft_casestudy_gen_qsr::generate(&config, out)?;
    let elapsed = t0.elapsed();
    println!(
        "  stores={} people={} positions={} promos={} events={} elapsed={:.2}s",
        corpus.dims.stores.len(),
        corpus.dims.people.len(),
        corpus.dims.positions.len(),
        corpus.dims.promotions.len(),
        corpus.events.len(),
        elapsed.as_secs_f64(),
    );
    println!(
        "  truth: causal_edges={} org_gaps={}",
        corpus.truth.causal_edges.len(),
        corpus.truth.org_gaps.len(),
    );
    Ok(())
}

fn cmd_counterfactual(
    corpus_dir: &std::path::Path,
    scenario_path: &std::path::Path,
    out_path: Option<&std::path::Path>,
) -> Result<()> {
    let dims = output::load_dimensions(corpus_dir)?;
    let events = output::load_events(corpus_dir)?;
    let spec = scenarios::load_from_file(scenario_path)?;
    let cf = truth::compute_counterfactual(&spec, &events, &dims);
    let json = serde_json::to_string_pretty(&cf)?;
    if let Some(p) = out_path {
        std::fs::write(p, &json)?;
        println!("wrote counterfactual truth to {:?}", p);
    }
    println!("{json}");
    Ok(())
}

fn cmd_score(prediction_path: &std::path::Path, truth_path: &std::path::Path) -> Result<()> {
    let prediction: ScenarioPrediction =
        serde_json::from_reader(BufReader::new(File::open(prediction_path)?))?;
    let truth: CounterfactualTruth =
        serde_json::from_reader(BufReader::new(File::open(truth_path)?))?;
    if prediction.scenario_id != truth.scenario_id {
        bail!(
            "scenario_id mismatch: prediction={} truth={}",
            prediction.scenario_id,
            truth.scenario_id
        );
    }
    let score = scoring::score(&prediction, &truth);
    println!("{}", serde_json::to_string_pretty(&score)?);
    if !score.passes_tier_gate {
        std::process::exit(2);
    }
    Ok(())
}

fn cmd_ingest(corpus_dir: &std::path::Path, max_per_tick: usize) -> Result<()> {
    let dims = output::load_dimensions(corpus_dir)?;
    let events = output::load_events(corpus_dir)?;
    let t0 = std::time::Instant::now();
    let mut driver = IngestDriver::new(&dims);
    driver.max_per_tick = max_per_tick;
    driver.emit_stream(&events);
    driver.run_to_completion();
    let elapsed = t0.elapsed().as_secs_f64();
    println!(
        "ingested {} events across {} shards in {:.3}s ({:.0}/sec)",
        driver.stats.impulses_applied,
        driver.shards.shards.len(),
        elapsed,
        driver.stats.impulses_applied as f64 / elapsed.max(1e-9),
    );
    println!("{}", serde_json::to_string_pretty(&driver.stats)?);
    let shard_summary: Vec<_> = driver
        .shards
        .shards
        .iter()
        .map(|(k, v)| serde_json::json!({ "path": k.path(), "impulses": v.impulses, "rollups": v.daily_rollups.len() }))
        .collect();
    println!("{}", serde_json::to_string_pretty(&shard_summary)?);
    Ok(())
}

fn cmd_predict(
    corpus_dir: &std::path::Path,
    scenario_path: &std::path::Path,
    mc_samples: usize,
    mc_seed: u64,
    eml_path: Option<&std::path::Path>,
    out_path: Option<&std::path::Path>,
) -> Result<()> {
    let dims = output::load_dimensions(corpus_dir)?;
    let events = output::load_events(corpus_dir)?;
    let spec = scenarios::load_from_file(scenario_path)?;
    let graph = graph::build(&dims, &events);
    let eml_model = if let Some(p) = eml_path {
        Some(serde_json::from_reader::<_, EmlModel>(BufReader::new(
            File::open(p)?,
        ))?)
    } else {
        None
    };

    let engine = match eml_model.as_ref() {
        Some(m) => ScenarioEngine::new(&graph, &dims).with_eml(m),
        None => ScenarioEngine::new(&graph, &dims),
    };
    let t0 = std::time::Instant::now();
    let prediction = engine.predict(&spec, mc_samples, mc_seed);
    let elapsed = t0.elapsed().as_secs_f64();

    let json = serde_json::to_string_pretty(&prediction)?;
    if let Some(p) = out_path {
        std::fs::write(p, &json)?;
    }
    println!("{json}");
    eprintln!("predicted in {:.3}s", elapsed);
    Ok(())
}

fn cmd_train_eml(
    corpus_dir: &std::path::Path,
    scenarios_dir: &std::path::Path,
    ridge: f64,
    out_path: &std::path::Path,
) -> Result<()> {
    let dims = output::load_dimensions(corpus_dir)?;
    let events = output::load_events(corpus_dir)?;
    let graph = graph::build(&dims, &events);
    let engine = ScenarioEngine::new(&graph, &dims);

    let mut specs = Vec::new();
    for entry in std::fs::read_dir(scenarios_dir)? {
        let entry = entry?;
        if entry.path().extension().and_then(|s| s.to_str()) == Some("yaml") {
            specs.push(scenarios::load_from_file(&entry.path())?);
        }
    }
    if specs.is_empty() {
        anyhow::bail!("no .yaml scenarios found in {:?}", scenarios_dir);
    }
    let samples = engine::synth_training_set(&engine, &specs, &events, &dims);
    let model = EmlModel::fit(&samples, ridge);
    std::fs::write(out_path, serde_json::to_string_pretty(&model)?)?;
    println!(
        "trained EML on {} samples, RMSE={:.2}, weights={:?}",
        model.training_samples, model.training_rmse, model.weights
    );
    Ok(())
}

fn cmd_gap_sweep(corpus_dir: &std::path::Path, out_path: Option<&std::path::Path>) -> Result<()> {
    // Reconstruct the exact generator config used at generation time from
    // the truth manifest so cadence knobs match.
    let dims = output::load_dimensions(corpus_dir)?;
    let ops = output::load_ops_events(corpus_dir)?;
    let manifest = output::load_truth(corpus_dir)?;
    let tier = ScaleTier::parse(&manifest.scale_tier).ok_or_else(|| {
        anyhow::anyhow!(
            "truth manifest has unknown scale_tier: {}",
            manifest.scale_tier
        )
    })?;
    let config = GeneratorConfig::default_for_tier(manifest.seed, tier);

    let t0 = std::time::Instant::now();
    let report = gaps::sweep(&config, &dims, &ops);
    let elapsed = t0.elapsed().as_secs_f64();

    let summary = serde_json::json!({
        "total_gaps": report.total(),
        "critical": report.count_severity(gaps::GapSeverity::Critical),
        "high":     report.count_severity(gaps::GapSeverity::High),
        "medium":   report.count_severity(gaps::GapSeverity::Medium),
        "low":      report.count_severity(gaps::GapSeverity::Low),
        "by_pattern": report.by_pattern.iter().map(|(p, c)| (p.as_str(), c)).collect::<std::collections::BTreeMap<_, _>>(),
        "elapsed_sec": elapsed,
    });
    println!("{}", serde_json::to_string_pretty(&summary)?);
    if let Some(p) = out_path {
        std::fs::write(p, serde_json::to_string_pretty(&report.gaps)?)?;
        eprintln!("wrote {} gaps to {:?}", report.total(), p);
    }
    Ok(())
}

fn cmd_chaos_run(corpus_dir: &std::path::Path, chaos_config: ChaosConfig) -> Result<()> {
    let dims = output::load_dimensions(corpus_dir)?;
    let events = output::load_events(corpus_dir)?;

    let t0 = std::time::Instant::now();
    let (mutated, report) = chaos::apply(&chaos_config, &events);
    let chaos_elapsed = t0.elapsed().as_secs_f64();

    let mut driver = IngestDriver::new(&dims);
    let t1 = std::time::Instant::now();
    driver.emit_stream(&mutated);
    driver.run_to_completion();
    let ingest_elapsed = t1.elapsed().as_secs_f64();

    let audit_status = driver.auditor.verify();
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "chaos": {
                "config": chaos_config,
                "report": report,
                "elapsed_sec": chaos_elapsed,
            },
            "ingest": {
                "impulses_emitted": driver.stats.impulses_emitted,
                "impulses_applied": driver.stats.impulses_applied,
                "impulses_blocked": driver.stats.impulses_blocked,
                "dropped_duplicates": driver.stats.dropped_duplicates,
                "late_arrivals": driver.stats.late_arrivals,
                "shard_count": driver.shards.shards.len(),
                "elapsed_sec": ingest_elapsed,
            },
            "audit": {
                "entries": driver.auditor.len(),
                "chain_ok": audit_status.is_ok(),
                "applied_entries": driver.auditor.count_by_kind(clawft_casestudy_gen_qsr::audit::AuditKind::ImpulseApplied),
                "blocked_entries": driver.auditor.count_by_kind(clawft_casestudy_gen_qsr::audit::AuditKind::ImpulseBlocked),
            },
        }))?
    );
    if let Err(e) = audit_status {
        anyhow::bail!("audit chain invalid: {}", e);
    }
    // Write the audit log next to the corpus for offline verification.
    let audit_path = corpus_dir.join("audit.json");
    std::fs::write(&audit_path, serde_json::to_string_pretty(&driver.auditor)?)?;
    eprintln!("wrote audit to {:?}", audit_path);
    Ok(())
}

fn cmd_audit_verify(audit_path: &std::path::Path) -> Result<()> {
    let auditor: clawft_casestudy_gen_qsr::audit::HashChainAuditor =
        serde_json::from_reader(BufReader::new(File::open(audit_path)?))?;
    match auditor.verify() {
        Ok(()) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "chain_ok": true,
                    "entries": auditor.len(),
                }))?
            );
            Ok(())
        }
        Err(e) => {
            eprintln!("audit chain INVALID: {}", e);
            std::process::exit(3);
        }
    }
}

fn cmd_rollup_weekly(corpus_dir: &std::path::Path, out_path: &std::path::Path) -> Result<()> {
    use clawft_casestudy_gen_qsr::rollup;
    use std::io::{BufWriter, Write};

    let events = output::load_events(corpus_dir)?;
    let t0 = std::time::Instant::now();
    let weeklies = rollup::roll_up_to_week(&events);
    let elapsed = t0.elapsed().as_secs_f64();

    let mut w = BufWriter::new(File::create(out_path)?);
    for wk in &weeklies {
        serde_json::to_writer(&mut w, wk)?;
        w.write_all(b"\n")?;
    }
    w.flush()?;

    let bytes = std::fs::metadata(out_path)?.len();
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "daily_events":   events.len(),
            "weekly_events":  weeklies.len(),
            "compression":    events.len() as f64 / weeklies.len().max(1) as f64,
            "output_bytes":   bytes,
            "bytes_per_week": bytes / weeklies.len().max(1) as u64,
            "elapsed_sec":    elapsed,
        }))?
    );
    Ok(())
}

fn cmd_rollup_monthly(corpus_dir: &std::path::Path, out_path: &std::path::Path) -> Result<()> {
    use clawft_casestudy_gen_qsr::rollup;
    use std::io::{BufWriter, Write};

    let events = output::load_events(corpus_dir)?;
    let t0 = std::time::Instant::now();
    let monthlies = rollup::roll_up_to_month(&events);
    let elapsed = t0.elapsed().as_secs_f64();

    let mut w = BufWriter::new(File::create(out_path)?);
    for m in &monthlies {
        serde_json::to_writer(&mut w, m)?;
        w.write_all(b"\n")?;
    }
    w.flush()?;
    let bytes = std::fs::metadata(out_path)?.len();
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "daily_events":    events.len(),
            "monthly_events":  monthlies.len(),
            "compression":     events.len() as f64 / monthlies.len().max(1) as f64,
            "output_bytes":    bytes,
            "bytes_per_month": bytes / monthlies.len().max(1) as u64,
            "elapsed_sec":     elapsed,
        }))?
    );
    Ok(())
}

fn cmd_recall_bench(
    corpus_dir: &std::path::Path,
    k: usize,
    queries: usize,
    ef_construction: usize,
    seed: u64,
    grains: &str,
    out_path: Option<&std::path::Path>,
) -> Result<()> {
    use clawft_casestudy_gen_qsr::recall;
    use clawft_casestudy_gen_qsr::rollup;

    let selected: std::collections::HashSet<&str> = grains
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();

    let dims = output::load_dimensions(corpus_dir)?;
    let events = output::load_events(corpus_dir)?;

    let want_daily = selected.contains("daily");
    let want_weekly = selected.contains("weekly");
    let want_monthly = selected.contains("monthly");

    let daily_feats = if want_daily {
        eprintln!("featurizing {} daily events …", events.len());
        recall::featurize_daily_corpus(&events, &dims)
    } else {
        Vec::new()
    };

    let weekly_feats = if want_weekly {
        let weekly_rolls = rollup::roll_up_to_week(&events);
        let weekly_baseline_per_store: std::collections::HashMap<&str, f64> = dims
            .stores
            .iter()
            .map(|s| (s.label.as_str(), s.baseline_daily_sales * 7.0))
            .collect();
        weekly_rolls
            .iter()
            .map(|r| {
                let b = weekly_baseline_per_store
                    .get(r.store_ref.as_str())
                    .copied()
                    .unwrap_or(35_000.0);
                recall::featurize_weekly(r, b)
            })
            .collect()
    } else {
        Vec::new()
    };

    let monthly_feats = if want_monthly {
        let monthly_rolls = rollup::roll_up_to_month(&events);
        let monthly_baseline_per_store: std::collections::HashMap<&str, f64> = dims
            .stores
            .iter()
            .map(|s| (s.label.as_str(), s.baseline_daily_sales * 30.0))
            .collect();
        monthly_rolls
            .iter()
            .map(|r| {
                let b = monthly_baseline_per_store
                    .get(r.store_ref.as_str())
                    .copied()
                    .unwrap_or(150_000.0);
                recall::featurize_monthly(r, b)
            })
            .collect()
    } else {
        Vec::new()
    };

    eprintln!(
        "corpus sizes: daily={} weekly={} monthly={}",
        daily_feats.len(),
        weekly_feats.len(),
        monthly_feats.len()
    );

    let ef_search_sweep = [16usize, 32, 64, 128, 256];
    let mut rows = Vec::new();
    for (grain, feats) in [
        ("daily", daily_feats.as_slice()),
        ("weekly", weekly_feats.as_slice()),
        ("monthly", monthly_feats.as_slice()),
    ] {
        if feats.is_empty() {
            continue;
        }
        for &ef_search in &ef_search_sweep {
            eprintln!(
                "bench grain={} n={} ef_construction={} ef_search={} …",
                grain,
                feats.len(),
                ef_construction,
                ef_search,
            );
            let row = recall::benchmark(grain, feats, k, queries, ef_construction, ef_search, seed);
            eprintln!(
                "  → recall@{}={:.4} build={}ms hnsw={:.1}µs brute={:.1}µs",
                row.k,
                row.recall_at_k,
                row.build_ms,
                row.avg_hnsw_query_us,
                row.avg_brute_force_query_us,
            );
            rows.push(row);
        }
    }

    let json = serde_json::to_string_pretty(&rows)?;
    if let Some(p) = out_path {
        std::fs::write(p, &json)?;
    }
    println!("{json}");
    Ok(())
}

fn cmd_privacy_scan(
    corpus_dir: &std::path::Path,
    out_path: Option<&std::path::Path>,
) -> Result<()> {
    let report = privacy::scan_corpus(corpus_dir)?;
    let json = serde_json::to_string_pretty(&report)?;
    if let Some(p) = out_path {
        std::fs::write(p, &json)?;
    }
    println!("{json}");
    if !report.is_clean() {
        eprintln!("privacy scan found {} violations", report.violations.len());
        std::process::exit(4);
    }
    Ok(())
}

fn cmd_dashboard(
    corpus_dir: &std::path::Path,
    top_n: usize,
    out_path: Option<&std::path::Path>,
) -> Result<()> {
    let dims = output::load_dimensions(corpus_dir)?;
    let events = output::load_events(corpus_dir)?;
    let ops = output::load_ops_events(corpus_dir)?;
    let manifest = output::load_truth(corpus_dir)?;
    let tier = ScaleTier::parse(&manifest.scale_tier).ok_or_else(|| {
        anyhow::anyhow!(
            "truth manifest has unknown scale_tier: {}",
            manifest.scale_tier
        )
    })?;
    let config = GeneratorConfig::default_for_tier(manifest.seed, tier);

    let gap_report = gaps::sweep(&config, &dims, &ops);
    let scores = coherence::score_all_stores(&dims, &events, &ops, &gap_report);
    let dash = dashboard::build(&gap_report, scores, top_n);

    let text = dashboard::render_text(&dash);
    println!("{text}");
    if let Some(p) = out_path {
        std::fs::write(p, serde_json::to_string_pretty(&dash)?)?;
        eprintln!("wrote dashboard JSON to {:?}", p);
    }
    Ok(())
}
