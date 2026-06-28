//! `weaver ecc` — ECC cognitive substrate commands.

use clap::{Args, Subcommand};

use crate::client::DaemonClient;
use crate::protocol::Request;

#[derive(Args)]
pub struct EccArgs {
    #[command(subcommand)]
    pub command: EccCommand,
}

#[derive(Subcommand)]
pub enum EccCommand {
    /// Show ECC subsystem status (calibration, tick stats).
    Status,
    /// Re-run boot calibration.
    Calibrate,
    /// HNSW similarity search.
    Search {
        /// Search query text (will be hashed to a vector).
        query: String,
        /// Number of results.
        #[arg(short, long, default_value_t = 10)]
        k: usize,
    },
    /// Show causal edges for a node.
    Causal {
        /// Node ID to inspect.
        node: u64,
        /// Direction: forward or reverse.
        #[arg(short, long, default_value = "forward")]
        direction: String,
        /// Traversal depth.
        #[arg(short = 'D', long, default_value_t = 3)]
        depth: usize,
    },
    /// Show cross-references for a Universal Node ID.
    Crossrefs {
        /// Hex-encoded Universal Node ID.
        id: String,
    },
    /// Show current tick statistics.
    Tick,
}

pub async fn run(args: EccArgs) -> anyhow::Result<()> {
    let mut client = DaemonClient::connect()
        .await
        .ok_or_else(|| anyhow::anyhow!("no daemon running — start with 'weaver kernel start'"))?;

    match args.command {
        EccCommand::Status => {
            let resp = client.simple_call("ecc.status").await?;
            if !resp.ok {
                anyhow::bail!("{}", resp.error.unwrap_or_default());
            }
            let result = resp.result.unwrap_or_default();

            println!("ECC Cognitive Substrate Status");
            println!("  Calibration:");
            if let Some(cal) = result.get("calibration") {
                println!(
                    "    Compute P50:     {}μs",
                    cal.get("compute_p50_us")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0)
                );
                println!(
                    "    Compute P95:     {}μs",
                    cal.get("compute_p95_us")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0)
                );
                println!(
                    "    Tick interval:   {}ms",
                    cal.get("tick_interval_ms")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0)
                );
                println!(
                    "    Spectral:        {}",
                    cal.get("spectral_capable")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false)
                );
            } else {
                println!("    (not calibrated)");
            }
            println!("  Tick:");
            if let Some(tick) = result.get("tick") {
                println!(
                    "    Count:           {}",
                    tick.get("tick_count").and_then(|v| v.as_u64()).unwrap_or(0)
                );
                println!(
                    "    Interval:        {}ms",
                    tick.get("current_interval_ms")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0)
                );
                println!(
                    "    Avg compute:     {}μs",
                    tick.get("avg_compute_us")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0)
                );
                println!(
                    "    Drift count:     {}",
                    tick.get("drift_count")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0)
                );
                println!(
                    "    Running:         {}",
                    tick.get("running")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false)
                );
            } else {
                println!("    (not started)");
            }
            println!("  HNSW:");
            println!(
                "    Vectors:         {}",
                result
                    .get("hnsw_count")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0)
            );
            println!("  Causal:");
            println!(
                "    Nodes:           {}",
                result
                    .get("causal_nodes")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0)
            );
            println!(
                "    Edges:           {}",
                result
                    .get("causal_edges")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0)
            );
            println!(
                "  CrossRefs:         {}",
                result
                    .get("crossref_count")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0)
            );
            println!(
                "  Impulses pending:  {}",
                result
                    .get("impulse_pending")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0)
            );
        }
        EccCommand::Calibrate => {
            let resp = client.simple_call("ecc.calibrate").await?;
            if !resp.ok {
                anyhow::bail!("{}", resp.error.unwrap_or_default());
            }
            println!("Calibration complete");
            if let Some(result) = resp.result {
                println!(
                    "  P50: {}μs",
                    result
                        .get("compute_p50_us")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0)
                );
                println!(
                    "  P95: {}μs",
                    result
                        .get("compute_p95_us")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0)
                );
                println!(
                    "  Tick: {}ms",
                    result
                        .get("tick_interval_ms")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0)
                );
            }
        }
        EccCommand::Search { query, k } => {
            let params = serde_json::json!({"query": query, "k": k});
            let resp = client
                .call(Request::with_params("ecc.search", params))
                .await?;
            if !resp.ok {
                anyhow::bail!("{}", resp.error.unwrap_or_default());
            }
            let raw = resp.result.unwrap_or_default();
            let results: Vec<serde_json::Value> = if raw.is_array() {
                serde_json::from_value(raw)?
            } else {
                Vec::new()
            };
            if results.is_empty() {
                println!("No results");
            } else {
                for r in &results {
                    println!(
                        "  {} (score: {:.4})",
                        r.get("id").and_then(|v| v.as_str()).unwrap_or("?"),
                        r.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0),
                    );
                }
            }
        }
        EccCommand::Causal {
            node,
            direction,
            depth,
        } => {
            let params = serde_json::json!({"node": node, "direction": direction, "depth": depth});
            let resp = client
                .call(Request::with_params("ecc.causal", params))
                .await?;
            if !resp.ok {
                anyhow::bail!("{}", resp.error.unwrap_or_default());
            }
            let result = resp.result.unwrap_or_default();
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        EccCommand::Crossrefs { id } => {
            let params = serde_json::json!({"id": id});
            let resp = client
                .call(Request::with_params("ecc.crossrefs", params))
                .await?;
            if !resp.ok {
                anyhow::bail!("{}", resp.error.unwrap_or_default());
            }
            let result = resp.result.unwrap_or_default();
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        EccCommand::Tick => {
            let resp = client.simple_call("ecc.tick").await?;
            if !resp.ok {
                anyhow::bail!("{}", resp.error.unwrap_or_default());
            }
            let result = resp.result.unwrap_or_default();
            println!("Cognitive Tick Statistics");
            println!(
                "  Count:         {}",
                result
                    .get("tick_count")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0)
            );
            println!(
                "  Interval:      {}ms",
                result
                    .get("current_interval_ms")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0)
            );
            println!(
                "  Avg compute:   {}μs",
                result
                    .get("avg_compute_us")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0)
            );
            println!(
                "  Max compute:   {}μs",
                result
                    .get("max_compute_us")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0)
            );
            println!(
                "  Drift count:   {}",
                result
                    .get("drift_count")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0)
            );
            println!(
                "  Running:       {}",
                result
                    .get("running")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
            );
        }
    }

    Ok(())
}
