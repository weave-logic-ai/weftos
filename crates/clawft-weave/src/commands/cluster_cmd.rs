//! `weaver cluster` subcommand implementation.
//!
//! Provides cluster management commands:
//! - `weaver cluster status`  -- cluster node/shard summary
//! - `weaver cluster nodes`   -- list all cluster nodes
//! - `weaver cluster join`    -- add a node to the cluster
//! - `weaver cluster leave`   -- remove a node from the cluster
//! - `weaver cluster health`  -- per-node health check
//! - `weaver cluster shards`  -- shard distribution table

use clap::{Parser, Subcommand};
use comfy_table::{Table, presets};

use crate::client::DaemonClient;
use crate::protocol;

/// Cluster management subcommand.
#[derive(Parser)]
#[command(about = "WeftOS cluster management (nodes, shards, health)")]
pub struct ClusterArgs {
    #[command(subcommand)]
    pub action: ClusterAction,
}

/// Cluster subcommands.
#[derive(Subcommand)]
pub enum ClusterAction {
    /// Show cluster summary (node count, shard count, consensus).
    Status,

    /// List all nodes in the cluster.
    Nodes,

    /// Add a node to the cluster.
    Join {
        /// Address of the node to join (e.g. "10.0.0.1:8080").
        #[arg(required = false)]
        address: Option<String>,

        /// Platform type: native, browser, edge, wasi.
        #[arg(short, long, default_value = "native")]
        platform: String,

        /// Node display name.
        #[arg(short, long)]
        name: Option<String>,
    },

    /// Remove a node from the cluster.
    Leave {
        /// Node ID to remove.
        node_id: String,
    },

    /// Show per-node health status.
    Health,

    /// Show shard distribution.
    Shards,
}

/// Run the cluster subcommand.
pub async fn run(args: ClusterArgs) -> anyhow::Result<()> {
    let mut client = DaemonClient::connect()
        .await
        .ok_or_else(|| anyhow::anyhow!("no daemon running (use 'weaver kernel start' first)"))?;

    match args.action {
        ClusterAction::Status => {
            let resp = client.simple_call("cluster.status").await?;
            if resp.ok {
                let result: protocol::ClusterStatusResult =
                    serde_json::from_value(resp.result.unwrap())?;
                println!("WeftOS Cluster Status");
                println!("---------------------");
                println!(
                    "Nodes:     {} total, {} healthy",
                    result.total_nodes, result.healthy_nodes
                );
                println!(
                    "Shards:    {} total, {} active",
                    result.total_shards, result.active_shards
                );
                println!(
                    "Consensus: {}",
                    if result.consensus_enabled {
                        "enabled"
                    } else {
                        "disabled"
                    }
                );
            } else {
                let msg = resp.error.unwrap_or_else(|| "unknown error".into());
                eprintln!("error: {msg}");
            }
        }
        ClusterAction::Nodes => {
            let resp = client.simple_call("cluster.nodes").await?;
            if resp.ok {
                let nodes: Vec<protocol::ClusterNodeInfo> =
                    serde_json::from_value(resp.result.unwrap())?;
                if nodes.is_empty() {
                    println!("No cluster nodes.");
                } else {
                    let mut table = Table::new();
                    table.load_preset(presets::UTF8_FULL_CONDENSED);
                    table.set_header(vec!["Node ID", "Name", "Platform", "State", "Address"]);
                    for node in &nodes {
                        table.add_row(vec![
                            &node.node_id,
                            &node.name,
                            &node.platform,
                            &node.state,
                            node.address.as_deref().unwrap_or("-"),
                        ]);
                    }
                    println!("{table}");
                }
            } else {
                let msg = resp.error.unwrap_or_else(|| "unknown error".into());
                eprintln!("error: {msg}");
            }
        }
        ClusterAction::Join {
            address,
            platform,
            name,
        } => {
            let params = protocol::ClusterJoinParams {
                address,
                platform,
                name,
            };
            let req = protocol::Request::with_params("cluster.join", serde_json::to_value(params)?);
            let resp = client.call(req).await?;
            if resp.ok {
                let result = resp.result.unwrap();
                let node_id = result["node_id"].as_str().unwrap_or("unknown");
                println!("Node joined: {node_id}");
            } else {
                let msg = resp.error.unwrap_or_else(|| "unknown error".into());
                eprintln!("join failed: {msg}");
            }
        }
        ClusterAction::Leave { node_id } => {
            let params = protocol::ClusterLeaveParams {
                node_id: node_id.clone(),
            };
            let req =
                protocol::Request::with_params("cluster.leave", serde_json::to_value(params)?);
            let resp = client.call(req).await?;
            if resp.ok {
                println!("Node removed: {node_id}");
            } else {
                let msg = resp.error.unwrap_or_else(|| "unknown error".into());
                eprintln!("leave failed: {msg}");
            }
        }
        ClusterAction::Health => {
            let resp = client.simple_call("cluster.health").await?;
            if resp.ok {
                let health: Vec<serde_json::Value> = serde_json::from_value(resp.result.unwrap())?;
                if health.is_empty() {
                    println!("No cluster nodes.");
                } else {
                    let mut table = Table::new();
                    table.load_preset(presets::UTF8_FULL_CONDENSED);
                    table.set_header(vec!["Node ID", "Healthy", "State"]);
                    for entry in &health {
                        let healthy = if entry["healthy"].as_bool().unwrap_or(false) {
                            "yes"
                        } else {
                            "no"
                        };
                        table.add_row(vec![
                            entry["node_id"].as_str().unwrap_or("?"),
                            healthy,
                            entry["state"].as_str().unwrap_or("?"),
                        ]);
                    }
                    println!("{table}");
                }
            } else {
                let msg = resp.error.unwrap_or_else(|| "unknown error".into());
                eprintln!("error: {msg}");
            }
        }
        ClusterAction::Shards => {
            let resp = client.simple_call("cluster.shards").await?;
            if resp.ok {
                let shards: Vec<protocol::ClusterShardInfo> =
                    serde_json::from_value(resp.result.unwrap())?;
                if shards.is_empty() {
                    println!("No shards configured.");
                } else {
                    let mut table = Table::new();
                    table.load_preset(presets::UTF8_FULL_CONDENSED);
                    table.set_header(vec!["Shard", "Primary", "Replicas", "Vectors", "Status"]);
                    for shard in &shards {
                        table.add_row(vec![
                            &shard.shard_id.to_string(),
                            &shard.primary_node,
                            &shard.replica_nodes.join(", "),
                            &shard.vector_count.to_string(),
                            &shard.status,
                        ]);
                    }
                    println!("{table}");
                }
            } else {
                let msg = resp.error.unwrap_or_else(|| "unknown error".into());
                eprintln!("error: {msg}");
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cluster_args_parses() {
        ClusterArgs::command().debug_assert();
    }
}
