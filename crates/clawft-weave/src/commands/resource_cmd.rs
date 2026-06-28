//! `weaver resource` — resource tree management commands.

use clap::{Args, Subcommand};
use comfy_table::{Cell, Table};

use crate::client::DaemonClient;
use crate::protocol::{
    Request, ResourceInspectParams, ResourceNodeInfo, ResourceRankEntry, ResourceRankParams,
    ResourceScoreParams, ResourceScoreResult, ResourceStatsResult,
};

#[derive(Args)]
pub struct ResourceArgs {
    #[command(subcommand)]
    pub command: ResourceCommand,
}

#[derive(Subcommand)]
pub enum ResourceCommand {
    /// Display the full resource tree.
    Tree,
    /// Inspect a specific resource node.
    Inspect {
        /// Resource path (e.g. "/kernel/services/cron").
        path: String,
    },
    /// Show resource tree statistics.
    Stats,
    /// Show scoring for a resource node.
    Score {
        /// Resource path (e.g. "/kernel/agents/worker-1").
        path: String,
    },
    /// Rank resource nodes by composite score.
    Rank {
        /// Number of top-ranked nodes to show.
        #[arg(short = 'n', long, default_value = "10")]
        count: usize,
    },
}

pub async fn run(args: ResourceArgs) -> anyhow::Result<()> {
    let mut client = DaemonClient::connect()
        .await
        .ok_or_else(|| anyhow::anyhow!("no daemon running — start with 'weaver kernel start'"))?;

    match args.command {
        ResourceCommand::Tree => {
            let resp = client.simple_call("resource.tree").await?;
            if !resp.ok {
                anyhow::bail!("{}", resp.error.unwrap_or_default());
            }
            let nodes: Vec<ResourceNodeInfo> =
                serde_json::from_value(resp.result.unwrap_or_default())?;

            let mut table = Table::new();
            table.set_header(vec!["Path", "Kind", "Children", "Hash"]);
            for n in &nodes {
                let hash_short = if n.merkle_hash.len() >= 12 {
                    format!("{}...", &n.merkle_hash[..12])
                } else {
                    n.merkle_hash.clone()
                };
                table.add_row(vec![
                    Cell::new(&n.id),
                    Cell::new(&n.kind),
                    Cell::new(n.children.len()),
                    Cell::new(hash_short),
                ]);
            }
            println!("{table}");
        }
        ResourceCommand::Inspect { path } => {
            let params = serde_json::to_value(ResourceInspectParams { path })?;
            let resp = client
                .call(Request::with_params("resource.inspect", params))
                .await?;
            if !resp.ok {
                anyhow::bail!("{}", resp.error.unwrap_or_default());
            }
            let node: ResourceNodeInfo = serde_json::from_value(resp.result.unwrap_or_default())?;

            println!("Resource: {}", node.id);
            println!("  Kind:       {}", node.kind);
            println!(
                "  Parent:     {}",
                node.parent.as_deref().unwrap_or("(root)")
            );
            println!("  Children:   {}", node.children.join(", "));
            println!("  Hash:       {}", node.merkle_hash);
            if node.metadata != serde_json::json!({}) {
                println!(
                    "  Metadata:   {}",
                    serde_json::to_string_pretty(&node.metadata)?
                );
            }
            if let Some(ref score) = node.scoring {
                println!("  Scoring:");
                println!("    Trust:       {:.3}", score.trust);
                println!("    Performance: {:.3}", score.performance);
                println!("    Difficulty:  {:.3}", score.difficulty);
                println!("    Reward:      {:.3}", score.reward);
                println!("    Reliability: {:.3}", score.reliability);
                println!("    Velocity:    {:.3}", score.velocity);
                println!("    Composite:   {:.3}", score.composite);
            }
        }
        ResourceCommand::Stats => {
            let resp = client.simple_call("resource.stats").await?;
            if !resp.ok {
                anyhow::bail!("{}", resp.error.unwrap_or_default());
            }
            let stats: ResourceStatsResult =
                serde_json::from_value(resp.result.unwrap_or_default())?;

            println!("Resource Tree Statistics");
            println!("  Total nodes:   {}", stats.total_nodes);
            println!("  Namespaces:    {}", stats.namespaces);
            println!("  Services:      {}", stats.services);
            println!("  Agents:        {}", stats.agents);
            println!("  Devices:       {}", stats.devices);
            println!("  Root hash:     {}...", &stats.root_hash[..16]);
        }
        ResourceCommand::Score { path } => {
            let params = serde_json::to_value(ResourceScoreParams { path })?;
            let resp = client
                .call(Request::with_params("resource.score", params))
                .await?;
            if !resp.ok {
                anyhow::bail!("{}", resp.error.unwrap_or_default());
            }
            let score: ResourceScoreResult =
                serde_json::from_value(resp.result.unwrap_or_default())?;

            println!("Scoring: {}", score.path);
            println!("  Trust:       {:.3}", score.trust);
            println!("  Performance: {:.3}", score.performance);
            println!("  Difficulty:  {:.3}", score.difficulty);
            println!("  Reward:      {:.3}", score.reward);
            println!("  Reliability: {:.3}", score.reliability);
            println!("  Velocity:    {:.3}", score.velocity);
            println!("  Composite:   {:.3}", score.composite);
        }
        ResourceCommand::Rank { count } => {
            let params = serde_json::to_value(ResourceRankParams { count })?;
            let resp = client
                .call(Request::with_params("resource.rank", params))
                .await?;
            if !resp.ok {
                anyhow::bail!("{}", resp.error.unwrap_or_default());
            }
            let entries: Vec<ResourceRankEntry> =
                serde_json::from_value(resp.result.unwrap_or_default())?;

            if entries.is_empty() {
                println!("No scored resources.");
                return Ok(());
            }

            let mut table = Table::new();
            table.set_header(vec!["Rank", "Path", "Score"]);
            for (i, e) in entries.iter().enumerate() {
                table.add_row(vec![
                    Cell::new(i + 1),
                    Cell::new(&e.path),
                    Cell::new(format!("{:.4}", e.score)),
                ]);
            }
            println!("{table}");
        }
    }

    Ok(())
}
