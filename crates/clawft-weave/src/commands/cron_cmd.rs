//! `weaver cron` — cron job management commands.

use clap::{Args, Subcommand};
use comfy_table::{Cell, Table};

use crate::client::DaemonClient;
use crate::protocol::{CronAddParams, CronJobInfo, CronRemoveParams, Request};

#[derive(Args)]
pub struct CronArgs {
    #[command(subcommand)]
    pub command: CronCommand,
}

#[derive(Subcommand)]
pub enum CronCommand {
    /// Add a new cron job.
    Add {
        /// Human-readable name for the job.
        #[arg(long)]
        name: String,
        /// Fire every N seconds.
        #[arg(long)]
        interval: u64,
        /// Command payload to send.
        #[arg(long)]
        command: String,
        /// Target agent PID (sends cron fire messages to this agent).
        #[arg(long)]
        target: Option<u64>,
    },
    /// List all cron jobs.
    List,
    /// Remove a cron job by ID.
    Remove {
        /// Job ID to remove.
        id: String,
    },
}

pub async fn run(args: CronArgs) -> anyhow::Result<()> {
    let mut client = DaemonClient::connect()
        .await
        .ok_or_else(|| anyhow::anyhow!("no daemon running — start with 'weaver kernel start'"))?;

    match args.command {
        CronCommand::Add {
            name,
            interval,
            command,
            target,
        } => {
            let params = serde_json::to_value(CronAddParams {
                name,
                interval_secs: interval,
                command,
                target_pid: target,
            })?;
            let resp = client
                .call(Request::with_params("cron.add", params))
                .await?;
            if !resp.ok {
                anyhow::bail!("{}", resp.error.unwrap_or_default());
            }
            let job: CronJobInfo = serde_json::from_value(resp.result.unwrap_or_default())?;
            println!("Cron job created");
            println!("  ID:       {}", job.id);
            println!("  Name:     {}", job.name);
            println!("  Interval: {}s", job.interval_secs);
            println!("  Command:  {}", job.command);
            if let Some(pid) = job.target_pid {
                println!("  Target:   PID {pid}");
            }
        }
        CronCommand::List => {
            let resp = client.simple_call("cron.list").await?;
            if !resp.ok {
                anyhow::bail!("{}", resp.error.unwrap_or_default());
            }
            let jobs: Vec<CronJobInfo> = serde_json::from_value(resp.result.unwrap_or_default())?;

            if jobs.is_empty() {
                println!("No cron jobs.");
                return Ok(());
            }

            let mut table = Table::new();
            table.set_header(vec![
                "ID (short)",
                "Name",
                "Interval",
                "Command",
                "Target",
                "Fires",
                "Enabled",
            ]);
            for j in &jobs {
                let short_id = if j.id.len() > 8 { &j.id[..8] } else { &j.id };
                table.add_row(vec![
                    Cell::new(short_id),
                    Cell::new(&j.name),
                    Cell::new(format!("{}s", j.interval_secs)),
                    Cell::new(&j.command),
                    Cell::new(j.target_pid.map_or("-".into(), |p| format!("PID {p}"))),
                    Cell::new(j.fire_count),
                    Cell::new(if j.enabled { "yes" } else { "no" }),
                ]);
            }
            println!("{table}");
        }
        CronCommand::Remove { id } => {
            let params = serde_json::to_value(CronRemoveParams { id: id.clone() })?;
            let resp = client
                .call(Request::with_params("cron.remove", params))
                .await?;
            if !resp.ok {
                anyhow::bail!("{}", resp.error.unwrap_or_default());
            }
            println!("Cron job {id} removed");
        }
    }

    Ok(())
}
