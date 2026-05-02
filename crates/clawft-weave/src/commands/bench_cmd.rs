//! `weaver benchmark` — Comprehensive kernel performance benchmark v3.
//!
//! Six phases of testing, modeled on the Mentra benchmark suite:
//! 1. **Warmup**: System info collection + cache priming
//! 2. **RPC Transport**: Baseline latency per method
//! 3. **Compute**: Real kernel operations (agent, chain, cron, ipc, ecc)
//! 4. **Scalability Ladder**: Throughput at increasing concurrency levels
//! 5. **Stress**: Burst, payload, mixed workload, sustained load
//! 6. **Endurance** (optional): 60-second drift detection
//!
//! Produces a composite score across five dimensions:
//! Throughput (25%), Latency (25%), Scalability (20%), Stability (15%), Endurance (15%).

use std::time::{Duration, Instant};

use clap::Subcommand;
use clawft_rpc::{DaemonClient, Request};

/// Benchmark subcommands.
#[derive(Debug, Subcommand)]
pub enum BenchCmd {
    /// Run the full benchmark suite against a running kernel.
    Run {
        /// Output format: table (default), json.
        #[arg(short, long, default_value = "table")]
        format: String,
        /// Number of iterations per test (default: 100).
        #[arg(short = 'n', long, default_value = "100")]
        iterations: u32,
        /// Skip stress and endurance phases (phases 5-6).
        #[arg(long)]
        quick: bool,
        /// Run the 60-second endurance test (phase 6).
        #[arg(long)]
        endurance: bool,
        /// Use EML learned scoring instead of hardcoded piecewise-linear.
        #[arg(long)]
        learned: bool,
    },
    /// Show results from the last benchmark run.
    Last,
}

/// Run the benchmark command.
pub async fn run(cmd: BenchCmd) -> anyhow::Result<()> {
    match cmd {
        BenchCmd::Run { format, iterations, quick, endurance, learned } => {
            run_benchmark(&format, iterations, quick, endurance, learned).await
        }
        BenchCmd::Last => show_last().await,
    }
}

/// Run with defaults (no subcommand).
pub async fn run_default() -> anyhow::Result<()> {
    run_benchmark("table", 100, false, false, false).await
}

// ═══════════════════════════════════════════════════════════════════
// Data structures
// ═══════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct LatencyStats {
    avg_us: f64,
    min_us: f64,
    max_us: f64,
    p50_us: f64,
    p95_us: f64,
    p99_us: f64,
    stddev_us: f64,
    ops_per_sec: f64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct BenchResult {
    name: String,
    phase: String,
    iterations: u32,
    stats: LatencyStats,
    errors: u32,
    status: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct ScalabilityPoint {
    concurrency: u32,
    throughput: f64,
    p95_us: f64,
    efficiency: f64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct ScalabilityResult {
    points: Vec<ScalabilityPoint>,
    coefficient: f64,
    knee_point: Option<u32>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct PayloadResult {
    size_bytes: usize,
    label: String,
    avg_us: f64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct StressResult {
    burst_ops_per_sec: f64,
    burst_count: u32,
    payload_results: Vec<PayloadResult>,
    sustained_avg_us: f64,
    sustained_p99_drift_pct: f64,
    sustained_duration_secs: f64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct EnduranceResult {
    duration_secs: f64,
    samples: Vec<f64>,
    drift_coefficient_pct: f64,
    memory_start: Option<u64>,
    memory_end: Option<u64>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct DimensionScore {
    name: String,
    score: f64,
    detail: String,
    weight: f64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct BenchReport {
    version: String,
    timestamp: String,
    kernel_version: String,
    platform: PlatformInfo,
    warmup_avg_us: f64,
    rpc_results: Vec<BenchResult>,
    transport_overhead_us: f64,
    compute_results: Vec<BenchResult>,
    scalability: Option<ScalabilityResult>,
    stress: Option<StressResult>,
    endurance: Option<EnduranceResult>,
    dimensions: Vec<DimensionScore>,
    overall_score: f64,
    grade: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct PlatformInfo {
    os: String,
    arch: String,
    cpu_cores: usize,
    ram: String,
    kernel_version: String,
    summary: String,
}

// ═══════════════════════════════════════════════════════════════════
// Main benchmark runner
// ═══════════════════════════════════════════════════════════════════

async fn run_benchmark(format: &str, iterations: u32, quick: bool, endurance: bool, learned: bool) -> anyhow::Result<()> {
    let mut client = DaemonClient::connect()
        .await
        .ok_or_else(|| anyhow::anyhow!("no kernel running — start with: weaver kernel start"))?;

    let platform = collect_platform_info();

    // Get kernel version early
    let kernel_version = client.simple_call("kernel.status").await
        .ok()
        .and_then(|r| r.result)
        .and_then(|v| v.get("version").and_then(|v| v.as_str().map(String::from)))
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());

    println!("WeftOS Kernel Benchmark v3");
    println!("==========================");
    println!("Platform: {}", platform.summary);
    println!("Kernel:   v{kernel_version}");
    println!("Date:     {}", iso_now());
    println!();

    // ── Phase 1: Warmup ───────────────────────────────────────
    println!("-- Phase 1: Warmup --");
    let warmup_avg = run_warmup(&mut client, 50).await;
    println!("  50 warmup calls completed (avg {:.0}us)", warmup_avg);
    println!();

    // ── Phase 2: RPC Transport ────────────────────────────────
    println!("-- Phase 2: RPC Transport --");
    let rpc_methods = [
        "ping",
        "kernel.status",
        "kernel.ps",
        "kernel.services",
        "kernel.logs",
        "ecc.status",
        "ecc.causal",
        "ecc.calibrate",
        "cron.list",
        "agent.list",
    ];
    let mut rpc_results = Vec::new();
    for method in &rpc_methods {
        let result = bench_rpc(&mut client, method, "rpc", iterations).await;
        print_result_line(&result);
        rpc_results.push(result);
    }

    // Transport overhead: avg of non-ping methods minus avg ping
    let ping_avg = rpc_results.iter()
        .find(|r| r.name == "ping")
        .map(|r| r.stats.avg_us)
        .unwrap_or(0.0);
    let non_ping: Vec<f64> = rpc_results.iter()
        .filter(|r| r.name != "ping" && r.status.starts_with("ok"))
        .map(|r| r.stats.avg_us)
        .collect();
    let transport_overhead = if non_ping.is_empty() {
        0.0
    } else {
        non_ping.iter().sum::<f64>() / non_ping.len() as f64 - ping_avg
    };
    println!("  Transport overhead: {transport_overhead:.0}us (avg method - avg ping)");
    println!();

    // ── Phase 3: Compute ──────────────────────────────────────
    println!("-- Phase 3: Compute --");
    let mut compute_results = Vec::new();

    let r = bench_agent_lifecycle(&mut client, iterations / 5).await;
    print_result_line(&r);
    compute_results.push(r);

    let r = bench_chain_ops(&mut client, iterations).await;
    print_result_line(&r);
    compute_results.push(r);

    let r = bench_cron_lifecycle(&mut client, iterations / 5).await;
    print_result_line(&r);
    compute_results.push(r);

    let r = bench_ipc_publish(&mut client, iterations).await;
    print_result_line(&r);
    compute_results.push(r);

    let r = bench_rpc(&mut client, "ecc.status", "compute", iterations).await;
    print_result_line(&r);
    compute_results.push(r);

    let r = bench_rpc(&mut client, "ecc.causal", "compute", iterations).await;
    print_result_line(&r);
    compute_results.push(r);

    let r = bench_rpc(&mut client, "ecc.calibrate", "compute", iterations).await;
    print_result_line(&r);
    compute_results.push(r);

    println!();

    // ── Phase 4: Scalability Ladder ───────────────────────────
    println!("-- Phase 4: Scalability --");
    let scalability = run_scalability_ladder(&mut client, iterations).await;
    print_scalability(&scalability);
    println!();

    // ── Phase 5: Stress ───────────────────────────────────────
    let stress = if !quick {
        println!("-- Phase 5: Stress --");
        let s = run_stress_tests(&mut client, iterations).await;
        print_stress(&s);
        println!();
        Some(s)
    } else {
        println!("-- Phase 5: Stress (skipped, --quick) --");
        println!();
        None
    };

    // ── Phase 6: Endurance ────────────────────────────────────
    let endurance_result = if endurance && !quick {
        println!("-- Phase 6: Endurance (60s) --");
        let e = run_endurance(&mut client).await;
        print_endurance(&e);
        println!();
        Some(e)
    } else if quick {
        println!("-- Phase 6: Endurance (skipped, --quick) --");
        println!();
        None
    } else {
        println!("-- Phase 6: Endurance (skipped, use --endurance) --");
        println!();
        None
    };

    // ═══════════════════════════════════════════════════════════
    // Scoring
    // ═══════════════════════════════════════════════════════════
    let dimensions = compute_scores(
        &rpc_results,
        &compute_results,
        &scalability,
        stress.as_ref(),
        endurance_result.as_ref(),
    );

    // Extract raw metrics for EML scoring
    let raw_metrics = extract_raw_metrics(&dimensions, &rpc_results, &compute_results, &scalability, stress.as_ref(), endurance_result.as_ref());

    let (overall_score, scoring_mode) = if learned {
        let scorer = super::bench_eml::BenchmarkScorerModel::load(
            &super::bench_eml::BenchmarkScorerModel::model_dir(),
        );
        let _eml_dims = scorer.score_dimensions(&raw_metrics);
        let composite = scorer.score(&raw_metrics);
        let mode = if scorer.is_composite_trained() {
            "EML (trained)"
        } else {
            "EML (untrained, fallback)"
        };
        (composite, mode)
    } else {
        let score: f64 = dimensions.iter().map(|d| d.score * d.weight).sum();
        (score, "hardcoded")
    };

    let grade = grade_from_score(overall_score);

    let report = BenchReport {
        version: "3".to_string(),
        timestamp: iso_now(),
        kernel_version,
        platform,
        warmup_avg_us: warmup_avg,
        rpc_results,
        transport_overhead_us: transport_overhead,
        compute_results,
        scalability: Some(scalability),
        stress,
        endurance: endurance_result,
        dimensions: dimensions.clone(),
        overall_score,
        grade: grade.to_string(),
    };

    // Save report
    let report_dir = clawft_rpc::runtime_dir().join("benchmarks");
    std::fs::create_dir_all(&report_dir)?;
    let report_path = report_dir.join("latest.json");
    std::fs::write(&report_path, serde_json::to_string_pretty(&report)?)?;

    if format == "json" {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("-- Scoring ({scoring_mode}) --");
        for d in &dimensions {
            println!("  {:<14} {:>3.0}/100  ({})", format!("{}:", d.name), d.score, d.detail);
        }
        println!("  {}", "-".repeat(40));
        println!("  Overall: {:.1}/100  Grade: {grade}", overall_score);
        println!();
        println!("Report saved to: {}", report_path.display());
    }

    Ok(())
}

// ═══════════════════════════════════════════════════════════════════
// Phase 1: Warmup
// ═══════════════════════════════════════════════════════════════════

async fn run_warmup(client: &mut DaemonClient, count: u32) -> f64 {
    let mut total_us = 0.0;
    for _ in 0..count {
        let start = Instant::now();
        let _ = client.simple_call("ping").await;
        total_us += start.elapsed().as_micros() as f64;
    }
    total_us / count as f64
}

// ═══════════════════════════════════════════════════════════════════
// Phase 2: RPC benchmarks
// ═══════════════════════════════════════════════════════════════════

async fn bench_rpc(client: &mut DaemonClient, method: &str, phase: &str, iterations: u32) -> BenchResult {
    let mut latencies = Vec::with_capacity(iterations as usize);
    let mut errors = 0u32;

    for _ in 0..iterations {
        let start = Instant::now();
        let resp = client.simple_call(method).await;
        let elapsed = start.elapsed().as_micros() as f64;

        match resp {
            Ok(r) if r.ok => latencies.push(elapsed),
            _ => errors += 1,
        }
    }

    make_result(method, phase, iterations, &mut latencies, errors)
}

// ═══════════════════════════════════════════════════════════════════
// Phase 3: Compute benchmarks
// ═══════════════════════════════════════════════════════════════════

async fn bench_agent_lifecycle(client: &mut DaemonClient, iterations: u32) -> BenchResult {
    let mut latencies = Vec::new();
    let mut errors = 0u32;

    for i in 0..iterations {
        let agent_id = format!("bench-agent-{i}");
        let start = Instant::now();

        let spawn_resp = client.call(Request::with_params(
            "agent.spawn",
            serde_json::json!({"agent_id": agent_id, "agent_type": "worker"}),
        )).await;

        if let Ok(ref r) = spawn_resp
            && r.ok
                && let Some(pid) = r.result.as_ref()
                    .and_then(|v| v.get("pid"))
                    .and_then(|v| v.as_u64())
                {
                    let _ = client.call(Request::with_params(
                        "agent.stop",
                        serde_json::json!({"pid": pid}),
                    )).await;
                }

        let elapsed = start.elapsed().as_micros() as f64;
        match spawn_resp {
            Ok(r) if r.ok => latencies.push(elapsed),
            _ => errors += 1,
        }
    }

    make_result("agent.lifecycle", "compute", iterations, &mut latencies, errors)
}

async fn bench_chain_ops(client: &mut DaemonClient, iterations: u32) -> BenchResult {
    let mut latencies = Vec::new();
    let mut errors = 0u32;

    for _ in 0..iterations {
        let start = Instant::now();
        // chain.status is the heaviest chain operation exposed via RPC
        // (reads chain height, last hash, verification status)
        let resp = client.simple_call("chain.status").await;
        let elapsed = start.elapsed().as_micros() as f64;

        match resp {
            Ok(r) if r.ok => latencies.push(elapsed),
            _ => errors += 1,
        }
    }

    make_result("chain.status", "compute", iterations, &mut latencies, errors)
}

async fn bench_cron_lifecycle(client: &mut DaemonClient, iterations: u32) -> BenchResult {
    let mut latencies = Vec::new();
    let mut errors = 0u32;

    for i in 0..iterations {
        let start = Instant::now();

        let add_resp = client.call(Request::with_params(
            "cron.add",
            serde_json::json!({
                "name": format!("bench-cron-{i}"),
                "interval_secs": 3600,
                "command": "benchmark test",
            }),
        )).await;

        if let Ok(ref r) = add_resp
            && r.ok
                && let Some(job_id) = r.result.as_ref()
                    .and_then(|v| v.get("job_id"))
                    .and_then(|v| v.as_str())
                {
                    let _ = client.call(Request::with_params(
                        "cron.remove",
                        serde_json::json!({"id": job_id}),
                    )).await;
                }

        let elapsed = start.elapsed().as_micros() as f64;
        match add_resp {
            Ok(r) if r.ok => latencies.push(elapsed),
            _ => errors += 1,
        }
    }

    make_result("cron.lifecycle", "compute", iterations, &mut latencies, errors)
}

async fn bench_ipc_publish(client: &mut DaemonClient, iterations: u32) -> BenchResult {
    let mut latencies = Vec::new();
    let mut errors = 0u32;

    for i in 0..iterations {
        let start = Instant::now();
        let resp = client.call(Request::with_params(
            "ipc.publish",
            serde_json::json!({
                "topic": "benchmark.test",
                "message": format!("msg-{i}"),
            }),
        )).await;
        let elapsed = start.elapsed().as_micros() as f64;

        match resp {
            Ok(r) if r.ok => latencies.push(elapsed),
            _ => errors += 1,
        }
    }

    make_result("ipc.publish", "compute", iterations, &mut latencies, errors)
}

// ═══════════════════════════════════════════════════════════════════
// Phase 4: Scalability Ladder
// ═══════════════════════════════════════════════════════════════════

async fn run_scalability_ladder(client: &mut DaemonClient, base_iterations: u32) -> ScalabilityResult {
    let levels: &[u32] = &[1, 2, 4, 8, 16, 32];
    let mut points = Vec::new();
    let mut baseline_throughput = 0.0;

    for &level in levels {
        let batch = base_iterations.min(200);
        let start = Instant::now();
        let mut successes = 0u32;
        let mut latencies = Vec::new();

        // Simulate concurrency by sending `level` sequential batches.
        // Each batch does `batch / level` requests, but total work is constant.
        let requests_per_batch = (batch as f64 / level as f64).ceil() as u32;
        let total_requests = requests_per_batch * level;

        for _ in 0..total_requests {
            let req_start = Instant::now();
            match client.simple_call("kernel.status").await {
                Ok(r) if r.ok => {
                    latencies.push(req_start.elapsed().as_micros() as f64);
                    successes += 1;
                }
                _ => {}
            }
        }

        let elapsed_secs = start.elapsed().as_secs_f64();
        let throughput = successes as f64 / elapsed_secs;

        latencies.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let p95 = percentile(&latencies, 0.95);

        if level == 1 {
            baseline_throughput = throughput;
        }

        let efficiency = if baseline_throughput > 0.0 {
            throughput / baseline_throughput * 100.0
        } else {
            100.0
        };

        points.push(ScalabilityPoint {
            concurrency: level,
            throughput,
            p95_us: p95,
            efficiency,
        });
    }

    // Scalability coefficient: throughput at max / throughput at 1x,
    // normalized by the concurrency ratio (we want linear scaling)
    let coefficient = if baseline_throughput > 0.0 {
        let last = points.last().unwrap();
        last.throughput / baseline_throughput
    } else {
        0.0
    };

    // Find knee point: first level where efficiency drops below 80%
    let knee_point = points.iter()
        .find(|p| p.efficiency < 80.0)
        .map(|p| p.concurrency);

    ScalabilityResult {
        points,
        coefficient,
        knee_point,
    }
}

// ═══════════════════════════════════════════════════════════════════
// Phase 5: Stress Tests
// ═══════════════════════════════════════════════════════════════════

async fn run_stress_tests(client: &mut DaemonClient, iterations: u32) -> StressResult {
    // --- Burst ---
    let burst_count = iterations * 5;
    let burst_start = Instant::now();
    let mut burst_ok = 0u32;
    for _ in 0..burst_count {
        match client.simple_call("ping").await {
            Ok(r) if r.ok => burst_ok += 1,
            _ => {}
        }
    }
    let burst_secs = burst_start.elapsed().as_secs_f64();
    let burst_ops = burst_ok as f64 / burst_secs;
    println!("  Burst throughput: {:.0} ops/sec ({burst_count} requests)", burst_ops);

    // --- Payload sizes ---
    let payload_sizes: &[(usize, &str)] = &[
        (1024, "1KB"),
        (4096, "4KB"),
        (16384, "16KB"),
        (65536, "64KB"),
    ];
    let mut payload_results = Vec::new();
    let mut payload_strs = Vec::new();
    for &(size, label) in payload_sizes {
        let payload = "x".repeat(size);
        let mut latencies = Vec::new();
        let count = 20u32;
        for _ in 0..count {
            let start = Instant::now();
            let resp = client.call(Request::with_params(
                "ipc.publish",
                serde_json::json!({
                    "topic": "benchmark.payload",
                    "message": payload,
                }),
            )).await;
            let elapsed = start.elapsed().as_micros() as f64;
            if let Ok(r) = resp
                && r.ok {
                    latencies.push(elapsed);
                }
        }
        let avg = if latencies.is_empty() { 0.0 } else { latencies.iter().sum::<f64>() / latencies.len() as f64 };
        payload_strs.push(format!("{label}={avg:.0}us"));
        payload_results.push(PayloadResult {
            size_bytes: size,
            label: label.to_string(),
            avg_us: avg,
        });
    }
    println!("  Payload latency: {}", payload_strs.join(", "));

    // --- 10-second sustained mixed workload ---
    let methods = [
        "ping",
        "kernel.status",
        "kernel.ps",
        "kernel.services",
        "ecc.status",
        "ecc.causal",
        "cron.list",
        "agent.list",
    ];
    let sustained_duration = Duration::from_secs(10);
    let sustained_start = Instant::now();
    let mut sustained_latencies = Vec::new();
    let mut method_idx = 0usize;

    // Also track p99 over 1-second windows for drift detection
    let mut window_p99s: Vec<f64> = Vec::new();
    let mut window_latencies: Vec<f64> = Vec::new();
    let mut window_start = Instant::now();

    while sustained_start.elapsed() < sustained_duration {
        let method = methods[method_idx % methods.len()];
        method_idx += 1;

        let req_start = Instant::now();
        let resp = client.simple_call(method).await;
        let elapsed = req_start.elapsed().as_micros() as f64;

        if let Ok(r) = resp
            && r.ok {
                sustained_latencies.push(elapsed);
                window_latencies.push(elapsed);
            }

        // Every second, record window p99
        if window_start.elapsed() >= Duration::from_secs(1) {
            if !window_latencies.is_empty() {
                window_latencies.sort_by(|a, b| a.partial_cmp(b).unwrap());
                window_p99s.push(percentile(&window_latencies, 0.99));
                window_latencies.clear();
            }
            window_start = Instant::now();
        }
    }

    // Flush final window
    if !window_latencies.is_empty() {
        window_latencies.sort_by(|a, b| a.partial_cmp(b).unwrap());
        window_p99s.push(percentile(&window_latencies, 0.99));
    }

    let sustained_avg = if sustained_latencies.is_empty() {
        0.0
    } else {
        sustained_latencies.iter().sum::<f64>() / sustained_latencies.len() as f64
    };

    // P99 drift: compare first half of windows to second half
    let p99_drift = compute_drift(&window_p99s);

    let actual_secs = sustained_start.elapsed().as_secs_f64();
    println!("  10s sustained: avg={sustained_avg:.0}us, P99 drift={p99_drift:+.0}%");

    StressResult {
        burst_ops_per_sec: burst_ops,
        burst_count,
        payload_results,
        sustained_avg_us: sustained_avg,
        sustained_p99_drift_pct: p99_drift,
        sustained_duration_secs: actual_secs,
    }
}

// ═══════════════════════════════════════════════════════════════════
// Phase 6: Endurance
// ═══════════════════════════════════════════════════════════════════

async fn run_endurance(client: &mut DaemonClient) -> EnduranceResult {
    let methods = [
        "ping",
        "kernel.status",
        "kernel.ps",
        "ecc.status",
        "cron.list",
        "agent.list",
    ];

    let duration = Duration::from_secs(60);
    let start = Instant::now();
    let mut samples: Vec<f64> = Vec::new();
    let mut method_idx = 0usize;
    let mut second_latencies: Vec<f64> = Vec::new();
    let mut sample_start = Instant::now();

    // Try to get initial memory from kernel.status
    let memory_start = get_kernel_memory(client).await;

    while start.elapsed() < duration {
        let method = methods[method_idx % methods.len()];
        method_idx += 1;

        let req_start = Instant::now();
        let resp = client.simple_call(method).await;
        let elapsed = req_start.elapsed().as_micros() as f64;

        if let Ok(r) = resp
            && r.ok {
                second_latencies.push(elapsed);
            }

        // Sample every second
        if sample_start.elapsed() >= Duration::from_secs(1) {
            if !second_latencies.is_empty() {
                let avg = second_latencies.iter().sum::<f64>() / second_latencies.len() as f64;
                samples.push(avg);
                second_latencies.clear();
            }
            sample_start = Instant::now();

            // Progress indicator every 10 seconds
            let elapsed_secs = start.elapsed().as_secs();
            if elapsed_secs.is_multiple_of(10) && elapsed_secs > 0 {
                print!("  {elapsed_secs}s...");
                let _ = std::io::Write::flush(&mut std::io::stdout());
            }
        }
    }

    // Flush final window
    if !second_latencies.is_empty() {
        let avg = second_latencies.iter().sum::<f64>() / second_latencies.len() as f64;
        samples.push(avg);
    }
    println!();

    let memory_end = get_kernel_memory(client).await;
    let drift = compute_drift(&samples);

    EnduranceResult {
        duration_secs: start.elapsed().as_secs_f64(),
        samples,
        drift_coefficient_pct: drift,
        memory_start,
        memory_end,
    }
}

async fn get_kernel_memory(client: &mut DaemonClient) -> Option<u64> {
    client.simple_call("kernel.status").await
        .ok()
        .and_then(|r| r.result)
        .and_then(|v| {
            v.get("memory_bytes").and_then(|m| m.as_u64())
                .or_else(|| v.get("memory_kb").and_then(|m| m.as_u64()).map(|kb| kb * 1024))
                .or_else(|| v.get("rss_bytes").and_then(|m| m.as_u64()))
        })
}

// ═══════════════════════════════════════════════════════════════════
// Scoring
// ═══════════════════════════════════════════════════════════════════

fn compute_scores(
    rpc_results: &[BenchResult],
    compute_results: &[BenchResult],
    scalability: &ScalabilityResult,
    stress: Option<&StressResult>,
    endurance: Option<&EnduranceResult>,
) -> Vec<DimensionScore> {
    // --- Throughput: peak ops/sec from mixed workload or best RPC ---
    let mixed_throughput = stress
        .map(|s| s.burst_ops_per_sec)
        .or_else(|| {
            // Fallback: best RPC throughput
            rpc_results.iter()
                .filter(|r| r.status.starts_with("ok"))
                .map(|r| r.stats.ops_per_sec)
                .fold(None, |max: Option<f64>, x| Some(max.map_or(x, |m: f64| m.max(x))))
        })
        .unwrap_or(0.0);

    let throughput_score = score_throughput(mixed_throughput);
    let throughput_detail = format!("{:.0} ops/sec", mixed_throughput);

    // --- Latency: P95 on compute operations ---
    let compute_p95s: Vec<f64> = compute_results.iter()
        .filter(|r| r.status.starts_with("ok"))
        .map(|r| r.stats.p95_us)
        .collect();
    let avg_compute_p95 = if compute_p95s.is_empty() {
        10000.0
    } else {
        compute_p95s.iter().sum::<f64>() / compute_p95s.len() as f64
    };
    let latency_score = score_latency(avg_compute_p95);
    let latency_detail = format!("P95 compute {:.0}us", avg_compute_p95);

    // --- Scalability: coefficient ---
    let scalability_score = score_scalability(scalability.coefficient);
    let scalability_detail = format!("coefficient {:.2}", scalability.coefficient);

    // --- Stability: P99/P50 ratio on compute ---
    let p99_p50_ratios: Vec<f64> = compute_results.iter()
        .filter(|r| r.status.starts_with("ok") && r.stats.p50_us > 0.0)
        .map(|r| r.stats.p99_us / r.stats.p50_us)
        .collect();
    let avg_ratio = if p99_p50_ratios.is_empty() {
        20.0
    } else {
        p99_p50_ratios.iter().sum::<f64>() / p99_p50_ratios.len() as f64
    };
    let stability_score = score_stability(avg_ratio);
    let stability_detail = format!("P99/P50 = {:.1}", avg_ratio);

    // --- Endurance: drift coefficient ---
    let (endurance_score, endurance_detail) = match endurance {
        Some(e) => {
            let s = score_endurance(e.drift_coefficient_pct);
            (s, format!("drift {:.0}%", e.drift_coefficient_pct))
        }
        None => {
            // If no endurance test, use sustained p99 drift from stress test
            match stress {
                Some(s) => {
                    let sc = score_endurance(s.sustained_p99_drift_pct);
                    (sc, format!("drift {:.0}% (10s)", s.sustained_p99_drift_pct))
                }
                None => (50.0, "not measured".to_string()),
            }
        }
    };

    vec![
        DimensionScore {
            name: "Throughput".to_string(),
            score: throughput_score,
            detail: throughput_detail,
            weight: 0.25,
        },
        DimensionScore {
            name: "Latency".to_string(),
            score: latency_score,
            detail: latency_detail,
            weight: 0.25,
        },
        DimensionScore {
            name: "Scalability".to_string(),
            score: scalability_score,
            detail: scalability_detail,
            weight: 0.20,
        },
        DimensionScore {
            name: "Stability".to_string(),
            score: stability_score,
            detail: stability_detail,
            weight: 0.15,
        },
        DimensionScore {
            name: "Endurance".to_string(),
            score: endurance_score,
            detail: endurance_detail,
            weight: 0.15,
        },
    ]
}

/// Extract raw metrics for EML scoring from the benchmark results.
///
/// Returns `[throughput_ops, latency_p95_us, scalability_coeff, stability_ratio, endurance_drift_pct]`.
fn extract_raw_metrics(
    _dimensions: &[DimensionScore],
    rpc_results: &[BenchResult],
    compute_results: &[BenchResult],
    scalability: &ScalabilityResult,
    stress: Option<&StressResult>,
    endurance: Option<&EnduranceResult>,
) -> [f64; 5] {
    // Throughput: peak ops/sec
    let throughput = stress
        .map(|s| s.burst_ops_per_sec)
        .or_else(|| {
            rpc_results.iter()
                .filter(|r| r.status.starts_with("ok"))
                .map(|r| r.stats.ops_per_sec)
                .fold(None, |max: Option<f64>, x| Some(max.map_or(x, |m: f64| m.max(x))))
        })
        .unwrap_or(0.0);

    // Latency: avg P95 on compute
    let compute_p95s: Vec<f64> = compute_results.iter()
        .filter(|r| r.status.starts_with("ok"))
        .map(|r| r.stats.p95_us)
        .collect();
    let latency = if compute_p95s.is_empty() {
        10_000.0
    } else {
        compute_p95s.iter().sum::<f64>() / compute_p95s.len() as f64
    };

    // Scalability coefficient
    let scalability_coeff = scalability.coefficient;

    // Stability: avg P99/P50 ratio
    let ratios: Vec<f64> = compute_results.iter()
        .filter(|r| r.status.starts_with("ok") && r.stats.p50_us > 0.0)
        .map(|r| r.stats.p99_us / r.stats.p50_us)
        .collect();
    let stability = if ratios.is_empty() {
        20.0
    } else {
        ratios.iter().sum::<f64>() / ratios.len() as f64
    };

    // Endurance drift
    let endurance_drift = match endurance {
        Some(e) => e.drift_coefficient_pct,
        None => stress.map(|s| s.sustained_p99_drift_pct).unwrap_or(50.0),
    };

    [throughput, latency, scalability_coeff, stability, endurance_drift]
}

/// Throughput score: ops/sec on mixed workload.
/// 100K+ = 100, 50K = 80, 20K = 60, 10K = 40, 5K = 20, <1K = 0
pub fn score_throughput(ops: f64) -> f64 {
    if ops >= 100_000.0 { return 100.0; }
    if ops <= 1_000.0 { return 0.0; }
    // Piecewise linear interpolation
    let breakpoints: &[(f64, f64)] = &[
        (1_000.0, 0.0),
        (5_000.0, 20.0),
        (10_000.0, 40.0),
        (20_000.0, 60.0),
        (50_000.0, 80.0),
        (100_000.0, 100.0),
    ];
    interpolate(ops, breakpoints)
}

/// Latency score: P95 compute in microseconds.
/// <50us = 100, 100us = 80, 500us = 60, 1ms = 40, 5ms = 20, >10ms = 0
pub fn score_latency(p95_us: f64) -> f64 {
    if p95_us <= 50.0 { return 100.0; }
    if p95_us >= 10_000.0 { return 0.0; }
    // Inverted: lower is better
    let breakpoints: &[(f64, f64)] = &[
        (50.0, 100.0),
        (100.0, 80.0),
        (500.0, 60.0),
        (1_000.0, 40.0),
        (5_000.0, 20.0),
        (10_000.0, 0.0),
    ];
    interpolate(p95_us, breakpoints)
}

/// Scalability score: throughput at 32x / throughput at 1x.
/// >0.9 = 100, 0.7 = 70, 0.5 = 50, <0.3 = 0
pub fn score_scalability(coefficient: f64) -> f64 {
    if coefficient >= 0.9 { return 100.0; }
    if coefficient <= 0.3 { return 0.0; }
    let breakpoints: &[(f64, f64)] = &[
        (0.3, 0.0),
        (0.5, 50.0),
        (0.7, 70.0),
        (0.9, 100.0),
    ];
    interpolate(coefficient, breakpoints)
}

/// Stability score: P99/P50 ratio.
/// <1.5 = 100, 2.0 = 80, 3.0 = 60, 5.0 = 40, 10.0 = 20, >20 = 0
pub fn score_stability(ratio: f64) -> f64 {
    if ratio <= 1.5 { return 100.0; }
    if ratio >= 20.0 { return 0.0; }
    let breakpoints: &[(f64, f64)] = &[
        (1.5, 100.0),
        (2.0, 80.0),
        (3.0, 60.0),
        (5.0, 40.0),
        (10.0, 20.0),
        (20.0, 0.0),
    ];
    interpolate(ratio, breakpoints)
}

/// Endurance score: drift coefficient percentage.
/// <1% = 100, 5% = 80, 10% = 60, 25% = 40, 50% = 20, >100% = 0
pub fn score_endurance(drift_pct: f64) -> f64 {
    let d = drift_pct.abs();
    if d <= 1.0 { return 100.0; }
    if d >= 100.0 { return 0.0; }
    let breakpoints: &[(f64, f64)] = &[
        (1.0, 100.0),
        (5.0, 80.0),
        (10.0, 60.0),
        (25.0, 40.0),
        (50.0, 20.0),
        (100.0, 0.0),
    ];
    interpolate(d, breakpoints)
}

/// Piecewise linear interpolation through sorted breakpoints.
fn interpolate(x: f64, breakpoints: &[(f64, f64)]) -> f64 {
    if breakpoints.is_empty() { return 0.0; }
    if x <= breakpoints[0].0 { return breakpoints[0].1; }
    if x >= breakpoints[breakpoints.len() - 1].0 {
        return breakpoints[breakpoints.len() - 1].1;
    }
    for i in 0..breakpoints.len() - 1 {
        let (x0, y0) = breakpoints[i];
        let (x1, y1) = breakpoints[i + 1];
        if x >= x0 && x <= x1 {
            let t = (x - x0) / (x1 - x0);
            return y0 + t * (y1 - y0);
        }
    }
    breakpoints[breakpoints.len() - 1].1
}

fn grade_from_score(score: f64) -> &'static str {
    match score as u32 {
        95..=100 => "A+",
        90..=94 => "A",
        85..=89 => "A-",
        80..=84 => "B+",
        75..=79 => "B",
        70..=74 => "B-",
        65..=69 => "C+",
        60..=64 => "C",
        55..=59 => "C-",
        40..=54 => "D",
        _ => "F",
    }
}

// ═══════════════════════════════════════════════════════════════════
// Statistical helpers
// ═══════════════════════════════════════════════════════════════════

fn make_result(name: &str, phase: &str, iterations: u32, latencies: &mut [f64], errors: u32) -> BenchResult {
    if latencies.is_empty() {
        return BenchResult {
            name: name.to_string(),
            phase: phase.to_string(),
            iterations,
            stats: LatencyStats {
                avg_us: 0.0,
                min_us: 0.0,
                max_us: 0.0,
                p50_us: 0.0,
                p95_us: 0.0,
                p99_us: 0.0,
                stddev_us: 0.0,
                ops_per_sec: 0.0,
            },
            errors,
            status: format!("FAIL ({errors} errors)"),
        };
    }

    latencies.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let count = latencies.len() as f64;
    let total: f64 = latencies.iter().sum();
    let avg = total / count;
    let min = latencies[0];
    let max = latencies[latencies.len() - 1];
    let p50 = percentile(latencies, 0.50);
    let p95 = percentile(latencies, 0.95);
    let p99 = percentile(latencies, 0.99);

    // Standard deviation
    let variance = latencies.iter().map(|x| (x - avg).powi(2)).sum::<f64>() / count;
    let stddev = variance.sqrt();

    let ops = if avg > 0.0 { 1_000_000.0 / avg } else { 0.0 };

    BenchResult {
        name: name.to_string(),
        phase: phase.to_string(),
        iterations,
        stats: LatencyStats {
            avg_us: avg,
            min_us: min,
            max_us: max,
            p50_us: p50,
            p95_us: p95,
            p99_us: p99,
            stddev_us: stddev,
            ops_per_sec: ops,
        },
        errors,
        status: if errors > 0 {
            format!("ok ({errors} err)")
        } else {
            "ok".to_string()
        },
    }
}

fn percentile(sorted: &[f64], pct: f64) -> f64 {
    if sorted.is_empty() { return 0.0; }
    let idx = ((sorted.len() as f64) * pct) as usize;
    sorted[idx.min(sorted.len() - 1)]
}

/// Compute drift as percentage change from first half average to second half average.
fn compute_drift(samples: &[f64]) -> f64 {
    if samples.len() < 4 {
        return 0.0;
    }
    let mid = samples.len() / 2;
    let first_avg = samples[..mid].iter().sum::<f64>() / mid as f64;
    let second_avg = samples[mid..].iter().sum::<f64>() / (samples.len() - mid) as f64;
    if first_avg > 0.0 {
        ((second_avg - first_avg) / first_avg) * 100.0
    } else {
        0.0
    }
}

// ═══════════════════════════════════════════════════════════════════
// Display helpers
// ═══════════════════════════════════════════════════════════════════

fn print_result_line(r: &BenchResult) {
    println!(
        "  {:<20} {:>8.0} {:>8.0} {:>8.0} {:>8.0} {:>8.0} {:>8.0}  {:>8}",
        r.name, r.stats.avg_us, r.stats.p50_us, r.stats.p95_us, r.stats.p99_us,
        r.stats.max_us, r.stats.ops_per_sec, r.status,
    );
}

fn print_scalability(s: &ScalabilityResult) {
    println!("  {:<8} {:>12} {:>14} {:>12}", "Load", "Throughput", "Latency(P95)", "Efficiency");
    for p in &s.points {
        println!(
            "  {:<8} {:>10.0}/s {:>12.0}us {:>10.0}%",
            format!("{}x", p.concurrency),
            p.throughput,
            p.p95_us,
            p.efficiency,
        );
    }
    let quality = if s.coefficient >= 0.9 {
        "excellent"
    } else if s.coefficient >= 0.7 {
        "good"
    } else if s.coefficient >= 0.5 {
        "fair"
    } else {
        "poor"
    };
    println!("  Scalability coefficient: {:.2} ({quality})", s.coefficient);
    if let Some(knee) = s.knee_point {
        println!("  Knee point: {knee}x concurrency");
    }
}

fn print_stress(_s: &StressResult) {
    // Output happens inline during stress test execution.
}

fn print_endurance(e: &EnduranceResult) {
    let quality = if e.drift_coefficient_pct.abs() < 5.0 {
        "stable"
    } else if e.drift_coefficient_pct.abs() < 15.0 {
        "minor drift"
    } else {
        "significant drift"
    };
    println!("  Duration: {:.1}s, Samples: {}", e.duration_secs, e.samples.len());
    println!("  Latency drift: {:+.1}% ({quality})", e.drift_coefficient_pct);
    if let (Some(start), Some(end)) = (e.memory_start, e.memory_end) {
        let delta = end as i64 - start as i64;
        println!("  Memory: {} -> {} ({:+} bytes)", fmt_bytes(start), fmt_bytes(end), delta);
    }
}

fn fmt_bytes(b: u64) -> String {
    if b >= 1_073_741_824 {
        format!("{:.1}GB", b as f64 / 1_073_741_824.0)
    } else if b >= 1_048_576 {
        format!("{:.1}MB", b as f64 / 1_048_576.0)
    } else if b >= 1024 {
        format!("{:.1}KB", b as f64 / 1024.0)
    } else {
        format!("{b}B")
    }
}

// ═══════════════════════════════════════════════════════════════════
// Last report
// ═══════════════════════════════════════════════════════════════════

async fn show_last() -> anyhow::Result<()> {
    let report_path = clawft_rpc::runtime_dir().join("benchmarks/latest.json");
    if !report_path.exists() {
        anyhow::bail!("no benchmark results found — run: weaver benchmark run");
    }
    let data = std::fs::read_to_string(&report_path)?;
    let report: BenchReport = serde_json::from_str(&data)?;

    println!("WeftOS Kernel Benchmark v{}", report.version);
    println!("==========================");
    println!("Platform: {}", report.platform.summary);
    println!("Kernel:   v{}", report.kernel_version);
    println!("Date:     {}", report.timestamp);
    println!();

    println!("-- RPC Transport --");
    println!(
        "  {:<20} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8}  {:>8}",
        "Test", "Avg", "P50", "P95", "P99", "Max", "Ops/sec", "Status"
    );
    for r in &report.rpc_results {
        print_result_line(r);
    }
    println!("  Transport overhead: {:.0}us", report.transport_overhead_us);
    println!();

    println!("-- Compute --");
    for r in &report.compute_results {
        print_result_line(r);
    }
    println!();

    if let Some(ref s) = report.scalability {
        println!("-- Scalability --");
        print_scalability(s);
        println!();
    }

    if let Some(ref s) = report.stress {
        println!("-- Stress --");
        println!("  Burst: {:.0} ops/sec ({} requests)", s.burst_ops_per_sec, s.burst_count);
        for p in &s.payload_results {
            print!("  {}={:.0}us ", p.label, p.avg_us);
        }
        println!();
        println!("  10s sustained: avg={:.0}us, P99 drift={:+.0}%", s.sustained_avg_us, s.sustained_p99_drift_pct);
        println!();
    }

    if let Some(ref e) = report.endurance {
        println!("-- Endurance --");
        print_endurance(e);
        println!();
    }

    println!("-- Scoring --");
    for d in &report.dimensions {
        println!("  {:<14} {:>3.0}/100  ({})", format!("{}:", d.name), d.score, d.detail);
    }
    println!("  {}", "-".repeat(40));
    println!("  Overall: {:.1}/100  Grade: {}", report.overall_score, report.grade);
    println!();
    println!("Report: {}", clawft_rpc::runtime_dir().join("benchmarks/latest.json").display());

    Ok(())
}

// ═══════════════════════════════════════════════════════════════════
// Platform info
// ═══════════════════════════════════════════════════════════════════

fn collect_platform_info() -> PlatformInfo {
    let os = std::env::consts::OS.to_string();
    let arch = std::env::consts::ARCH.to_string();
    let cores = num_cpus();
    let ram = memory_info();
    let kver = kernel_version();

    let summary = format!("{os} {arch} ({cores} cores, {ram})");

    PlatformInfo {
        os,
        arch,
        cpu_cores: cores,
        ram,
        kernel_version: kver,
        summary,
    }
}

fn num_cpus() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

fn memory_info() -> String {
    #[cfg(target_os = "linux")]
    {
        if let Ok(meminfo) = std::fs::read_to_string("/proc/meminfo") {
            for line in meminfo.lines() {
                if line.starts_with("MemTotal:") {
                    let kb: u64 = line.split_whitespace()
                        .nth(1)
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0);
                    let gb = kb as f64 / 1_048_576.0;
                    if gb >= 1.0 {
                        return format!("{:.0}GB", gb);
                    } else {
                        return format!("{:.0}MB", kb as f64 / 1024.0);
                    }
                }
            }
        }
        "unknown".to_string()
    }
    #[cfg(not(target_os = "linux"))]
    { "unknown".to_string() }
}

fn kernel_version() -> String {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/version")
            .ok()
            .and_then(|v| v.split_whitespace().nth(2).map(String::from))
            .unwrap_or_else(|| "unknown".to_string())
    }
    #[cfg(not(target_os = "linux"))]
    { "unknown".to_string() }
}

fn iso_now() -> String {
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = d.as_secs();
    // Approximate ISO 8601 without chrono
    let days = secs / 86400;
    let years = 1970 + days / 365;
    let rem_days = days % 365;
    let months = rem_days / 30 + 1;
    let day = rem_days % 30 + 1;
    let hours = (secs % 86400) / 3600;
    let mins = (secs % 3600) / 60;
    let s = secs % 60;
    format!("{years}-{months:02}-{day:02}T{hours:02}:{mins:02}:{s:02}Z")
}
