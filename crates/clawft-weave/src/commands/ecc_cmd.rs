//! `weaver ecc` — ECC cognitive substrate commands.
//!
//! Includes Phase E spatial BVH CLI (WEFT-720 / ADR-056):
//! `weaver ecc spatial insert|query|branch|diff|status`.

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
    /// Show active vector backend config (WEFT-364).
    #[command(name = "vector-config")]
    VectorConfig,
    /// BVH spatial index (ADR-056 / WEFT-720).
    Spatial {
        #[command(subcommand)]
        command: SpatialCommand,
    },
}

/// `weaver ecc spatial …` subcommands.
#[derive(Subcommand)]
pub enum SpatialCommand {
    /// Insert a leaf: `--tag` + `--aabb minx,miny,minz,maxx,maxy,maxz`.
    Insert {
        /// Registry tag name (`wm_object`, `sphere`, …) or numeric / `0x…`.
        #[arg(long)]
        tag: String,
        /// Axis-aligned bound: minx,miny,minz,maxx,maxy,maxz.
        #[arg(long)]
        aabb: String,
        /// Optional hex payload bytes.
        #[arg(long)]
        payload: Option<String>,
        /// Identity kind: `object` (default) or `event`.
        #[arg(long, default_value = "object")]
        identity: String,
        /// Branch id (default main = 0).
        #[arg(long, default_value_t = 0)]
        branch: u64,
    },
    /// Broad-phase query.
    Query {
        #[command(subcommand)]
        kind: SpatialQueryKind,
    },
    /// Branch lifecycle.
    Branch {
        #[command(subcommand)]
        op: SpatialBranchOp,
    },
    /// Diff two branches inside a region AABB.
    Diff {
        /// Branch A id.
        #[arg(long)]
        a: u64,
        /// Branch B id.
        #[arg(long)]
        b: u64,
        /// Region AABB: minx,miny,minz,maxx,maxy,maxz.
        #[arg(long)]
        region: String,
    },
    /// Service health, leaf counts, epoch, chain event count.
    Status,
    /// Dump recorded chain events (scaffold replay source).
    Events {
        /// Limit number of events printed (0 = all).
        #[arg(long, default_value_t = 0)]
        limit: usize,
    },
    /// Replay chain events into a fresh store and report status (E2E helper).
    Replay,
}

#[derive(Subcommand)]
pub enum SpatialQueryKind {
    /// Leaves whose AABB contains the point.
    Point {
        /// Point x,y,z.
        #[arg(long)]
        at: String,
        #[arg(long, default_value_t = 0)]
        branch: u64,
        /// Optional tag allow-list (comma-separated) — OQ4 membership wrapper.
        #[arg(long)]
        filter_tags: Option<String>,
    },
    /// Leaves intersecting an AABB.
    Aabb {
        /// AABB minx,miny,minz,maxx,maxy,maxz.
        #[arg(long)]
        region: String,
        #[arg(long, default_value_t = 0)]
        branch: u64,
        #[arg(long)]
        filter_tags: Option<String>,
        #[arg(long, default_value_t = 0)]
        limit: usize,
    },
    /// Leaves intersecting a sphere.
    Sphere {
        /// Center x,y,z.
        #[arg(long)]
        center: String,
        /// Radius.
        #[arg(long)]
        radius: f32,
        #[arg(long, default_value_t = 0)]
        branch: u64,
        #[arg(long)]
        filter_tags: Option<String>,
    },
    /// Ray cast (broad-phase).
    Ray {
        /// Origin x,y,z.
        #[arg(long)]
        origin: String,
        /// Direction x,y,z.
        #[arg(long)]
        dir: String,
        #[arg(long, default_value_t = 1.0e6)]
        max_t: f32,
        #[arg(long, default_value_t = 0)]
        branch: u64,
    },
    /// k nearest by AABB center distance.
    Knn {
        /// Point x,y,z.
        #[arg(long)]
        at: String,
        #[arg(short, long, default_value_t = 5)]
        k: usize,
        #[arg(long, default_value_t = 0)]
        branch: u64,
    },
}

#[derive(Subcommand)]
pub enum SpatialBranchOp {
    /// Derive a COW child branch from parent.
    Derive {
        /// Parent branch id.
        #[arg(long, default_value_t = 0)]
        parent: u64,
        /// Child branch name.
        #[arg(long)]
        name: String,
        /// Optional note.
        #[arg(long, default_value = "")]
        note: String,
    },
    /// List branches.
    List,
}

pub async fn run(args: EccArgs) -> anyhow::Result<()> {
    let mut client = clawft_rpc::connect_or_bail().await?;

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
            if let Some(spatial) = result.get("spatial") {
                println!("  Spatial (BVH):");
                println!(
                    "    Backend:         {}",
                    spatial
                        .get("backend")
                        .and_then(|v| v.as_str())
                        .unwrap_or("?")
                );
                println!(
                    "    Leaves:          {}",
                    spatial.get("len").and_then(|v| v.as_u64()).unwrap_or(0)
                );
                println!(
                    "    Epoch:           {}",
                    spatial.get("epoch").and_then(|v| v.as_u64()).unwrap_or(0)
                );
                println!(
                    "    Chain events:    {}",
                    spatial
                        .get("chain_events")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0)
                );
            }
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
        EccCommand::VectorConfig => {
            let resp = client.simple_call("ecc.vector-config").await?;
            if !resp.ok {
                anyhow::bail!("{}", resp.error.unwrap_or_default());
            }
            let result = resp.result.unwrap_or_default();
            println!("ECC Vector Backend Config");
            println!(
                "  Active:              {}",
                result
                    .get("active")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
            );
            println!(
                "  Live backend:        {}",
                result
                    .get("backend")
                    .and_then(|v| v.as_str())
                    .unwrap_or("(none)")
            );
            println!(
                "  Configured backend:  {}",
                result
                    .get("configured_backend")
                    .and_then(|v| v.as_str())
                    .unwrap_or("hnsw")
            );
            println!(
                "  Strict:              {}",
                result
                    .get("strict")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
            );
            println!(
                "  Vector count:        {}",
                result
                    .get("vector_count")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0)
            );
            println!(
                "  Epoch:               {}",
                result.get("epoch").and_then(|v| v.as_u64()).unwrap_or(0)
            );
            if let Some(max) = result.get("max_vectors").filter(|v| !v.is_null()) {
                println!("  Max vectors:         {max}");
            } else {
                println!("  Max vectors:         (unbounded)");
            }
            if let Some(params) = result.get("parameters") {
                println!("  Parameters:");
                println!("{}", serde_json::to_string_pretty(params)?);
            }
        }
        EccCommand::Spatial { command } => {
            run_spatial(&mut client, command).await?;
        }
    }

    Ok(())
}

async fn run_spatial(client: &mut DaemonClient, command: SpatialCommand) -> anyhow::Result<()> {
    match command {
        SpatialCommand::Insert {
            tag,
            aabb,
            payload,
            identity,
            branch,
        } => {
            let params = serde_json::json!({
                "tag": tag,
                "aabb": aabb,
                "payload": payload,
                "identity": identity,
                "branch": branch,
            });
            let resp = client
                .call(Request::with_params("ecc.spatial.insert", params))
                .await?;
            if !resp.ok {
                anyhow::bail!("{}", resp.error.unwrap_or_default());
            }
            let r = resp.result.unwrap_or_default();
            println!(
                "inserted leaf_id={} branch={} epoch={}",
                r.get("leaf_id").and_then(|v| v.as_u64()).unwrap_or(0),
                r.get("branch").and_then(|v| v.as_u64()).unwrap_or(0),
                r.get("epoch").and_then(|v| v.as_u64()).unwrap_or(0),
            );
        }
        SpatialCommand::Query { kind } => {
            let (method, params) = match kind {
                SpatialQueryKind::Point {
                    at,
                    branch,
                    filter_tags,
                } => (
                    "ecc.spatial.query",
                    serde_json::json!({
                        "kind": "point",
                        "at": at,
                        "branch": branch,
                        "filter_tags": filter_tags,
                    }),
                ),
                SpatialQueryKind::Aabb {
                    region,
                    branch,
                    filter_tags,
                    limit,
                } => (
                    "ecc.spatial.query",
                    serde_json::json!({
                        "kind": "aabb",
                        "region": region,
                        "branch": branch,
                        "filter_tags": filter_tags,
                        "limit": limit,
                    }),
                ),
                SpatialQueryKind::Sphere {
                    center,
                    radius,
                    branch,
                    filter_tags,
                } => (
                    "ecc.spatial.query",
                    serde_json::json!({
                        "kind": "sphere",
                        "center": center,
                        "radius": radius,
                        "branch": branch,
                        "filter_tags": filter_tags,
                    }),
                ),
                SpatialQueryKind::Ray {
                    origin,
                    dir,
                    max_t,
                    branch,
                } => (
                    "ecc.spatial.query",
                    serde_json::json!({
                        "kind": "ray",
                        "origin": origin,
                        "dir": dir,
                        "max_t": max_t,
                        "branch": branch,
                    }),
                ),
                SpatialQueryKind::Knn { at, k, branch } => (
                    "ecc.spatial.query",
                    serde_json::json!({
                        "kind": "knn",
                        "at": at,
                        "k": k,
                        "branch": branch,
                    }),
                ),
            };
            let resp = client
                .call(Request::with_params(method, params))
                .await?;
            if !resp.ok {
                anyhow::bail!("{}", resp.error.unwrap_or_default());
            }
            let r = resp.result.unwrap_or_default();
            if let Some(hits) = r.get("hits").and_then(|v| v.as_array()) {
                if hits.is_empty() {
                    println!("No hits");
                } else {
                    for h in hits {
                        if let Some(obj) = h.as_object() {
                            if obj.contains_key("t") {
                                println!(
                                    "  leaf_id={} t={}",
                                    obj.get("leaf_id")
                                        .and_then(|v| v.as_u64())
                                        .unwrap_or(0),
                                    obj.get("t").and_then(|v| v.as_f64()).unwrap_or(0.0),
                                );
                            } else {
                                println!(
                                    "  leaf_id={}",
                                    obj.get("leaf_id")
                                        .and_then(|v| v.as_u64())
                                        .unwrap_or(0)
                                );
                            }
                        } else if let Some(id) = h.as_u64() {
                            println!("  leaf_id={id}");
                        }
                    }
                }
            } else {
                println!("{}", serde_json::to_string_pretty(&r)?);
            }
        }
        SpatialCommand::Branch { op } => match op {
            SpatialBranchOp::Derive { parent, name, note } => {
                let params = serde_json::json!({
                    "parent": parent,
                    "name": name,
                    "note": note,
                });
                let resp = client
                    .call(Request::with_params("ecc.spatial.branch.derive", params))
                    .await?;
                if !resp.ok {
                    anyhow::bail!("{}", resp.error.unwrap_or_default());
                }
                let r = resp.result.unwrap_or_default();
                println!(
                    "derived branch_id={} parent={} name={}",
                    r.get("branch_id").and_then(|v| v.as_u64()).unwrap_or(0),
                    r.get("parent").and_then(|v| v.as_u64()).unwrap_or(0),
                    r.get("name").and_then(|v| v.as_str()).unwrap_or(""),
                );
            }
            SpatialBranchOp::List => {
                let resp = client.simple_call("ecc.spatial.branch.list").await?;
                if !resp.ok {
                    anyhow::bail!("{}", resp.error.unwrap_or_default());
                }
                let r = resp.result.unwrap_or_default();
                if let Some(branches) = r.get("branches").and_then(|v| v.as_array()) {
                    for b in branches {
                        println!(
                            "  branch_id={} name={} leaves={}",
                            b.get("id").and_then(|v| v.as_u64()).unwrap_or(0),
                            b.get("name").and_then(|v| v.as_str()).unwrap_or("?"),
                            b.get("leaves").and_then(|v| v.as_u64()).unwrap_or(0),
                        );
                    }
                } else {
                    println!("{}", serde_json::to_string_pretty(&r)?);
                }
            }
        },
        SpatialCommand::Diff { a, b, region } => {
            let params = serde_json::json!({"a": a, "b": b, "region": region});
            let resp = client
                .call(Request::with_params("ecc.spatial.diff", params))
                .await?;
            if !resp.ok {
                anyhow::bail!("{}", resp.error.unwrap_or_default());
            }
            let r = resp.result.unwrap_or_default();
            if let Some(entries) = r.get("entries").and_then(|v| v.as_array()) {
                if entries.is_empty() {
                    println!("No differences in region");
                } else {
                    for e in entries {
                        println!(
                            "  leaf_id={} kind={}",
                            e.get("leaf_id").and_then(|v| v.as_u64()).unwrap_or(0),
                            e.get("kind").and_then(|v| v.as_str()).unwrap_or("?"),
                        );
                    }
                }
            } else {
                println!("{}", serde_json::to_string_pretty(&r)?);
            }
        }
        SpatialCommand::Status => {
            let resp = client.simple_call("ecc.spatial.status").await?;
            if !resp.ok {
                anyhow::bail!("{}", resp.error.unwrap_or_default());
            }
            let r = resp.result.unwrap_or_default();
            println!("ECC Spatial (BVH) Status");
            println!(
                "  Backend:        {}",
                r.get("backend").and_then(|v| v.as_str()).unwrap_or("?")
            );
            println!(
                "  Epoch:          {}",
                r.get("epoch").and_then(|v| v.as_u64()).unwrap_or(0)
            );
            println!(
                "  Total leaves:   {}",
                r.get("len").and_then(|v| v.as_u64()).unwrap_or(0)
            );
            println!(
                "  Chain events:   {}",
                r.get("chain_events")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0)
            );
            if let Some(branches) = r.get("branches").and_then(|v| v.as_array()) {
                println!("  Branches:");
                for b in branches {
                    println!(
                        "    id={} name={} leaves={}",
                        b.get("id").and_then(|v| v.as_u64()).unwrap_or(0),
                        b.get("name").and_then(|v| v.as_str()).unwrap_or("?"),
                        b.get("leaves").and_then(|v| v.as_u64()).unwrap_or(0),
                    );
                }
            }
            println!(
                "  OQ4 filter:     post-query wrapper (MembershipFilter); see ADR-056 / PLAN.md"
            );
        }
        SpatialCommand::Events { limit } => {
            let params = serde_json::json!({"limit": limit});
            let resp = client
                .call(Request::with_params("ecc.spatial.events", params))
                .await?;
            if !resp.ok {
                anyhow::bail!("{}", resp.error.unwrap_or_default());
            }
            let r = resp.result.unwrap_or_default();
            println!("{}", serde_json::to_string_pretty(&r)?);
        }
        SpatialCommand::Replay => {
            let resp = client.simple_call("ecc.spatial.replay").await?;
            if !resp.ok {
                anyhow::bail!("{}", resp.error.unwrap_or_default());
            }
            let r = resp.result.unwrap_or_default();
            println!(
                "replay ok={} events={} epoch={} leaves={}",
                r.get("ok").and_then(|v| v.as_bool()).unwrap_or(false),
                r.get("events_replayed")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                r.get("epoch").and_then(|v| v.as_u64()).unwrap_or(0),
                r.get("len").and_then(|v| v.as_u64()).unwrap_or(0),
            );
        }
    }
    Ok(())
}
