//! `weaver ipc` — inter-process communication commands.

use clap::{Args, Subcommand};
use comfy_table::{Cell, Table};

use crate::client::DaemonClient;
use crate::protocol::{IpcPublishParams, IpcSubscribeParams, IpcTopicInfo, Request};

#[derive(Args)]
pub struct IpcArgs {
    #[command(subcommand)]
    pub command: IpcCommand,
}

#[derive(Subcommand)]
pub enum IpcCommand {
    /// List all active topics and their subscribers.
    Topics,
    /// Subscribe a PID to a topic.
    Subscribe {
        /// PID to subscribe.
        pid: u64,
        /// Topic name.
        topic: String,
    },
    /// Publish a message to a topic.
    Publish {
        /// Topic name.
        topic: String,
        /// Message (text or JSON).
        message: String,
    },
}

pub async fn run(args: IpcArgs) -> anyhow::Result<()> {
    let mut client = DaemonClient::connect()
        .await
        .ok_or_else(|| anyhow::anyhow!("no daemon running — start with 'weaver kernel start'"))?;

    match args.command {
        IpcCommand::Topics => {
            let resp = client.simple_call("ipc.topics").await?;
            if !resp.ok {
                anyhow::bail!("{}", resp.error.unwrap_or_default());
            }
            let topics: Vec<IpcTopicInfo> =
                serde_json::from_value(resp.result.unwrap_or_default())?;

            if topics.is_empty() {
                println!("No active topics.");
                return Ok(());
            }

            let mut table = Table::new();
            table.set_header(vec!["Topic", "Subscribers", "PIDs"]);
            for t in &topics {
                let pids_str: Vec<String> = t.subscribers.iter().map(|p| p.to_string()).collect();
                table.add_row(vec![
                    Cell::new(&t.topic),
                    Cell::new(t.subscriber_count),
                    Cell::new(pids_str.join(", ")),
                ]);
            }
            println!("{table}");
        }
        IpcCommand::Subscribe { pid, topic } => {
            let params = serde_json::to_value(IpcSubscribeParams {
                pid,
                topic: topic.clone(),
            })?;
            let resp = client
                .call(Request::with_params("ipc.subscribe", params))
                .await?;
            if !resp.ok {
                anyhow::bail!("{}", resp.error.unwrap_or_default());
            }
            println!("PID {pid} subscribed to '{topic}'");
        }
        IpcCommand::Publish { topic, message } => {
            let params = serde_json::to_value(IpcPublishParams {
                topic: topic.clone(),
                message,
                actor_id: None,
                signature: None,
                ts: None,
            })?;
            let resp = client
                .call(Request::with_params("ipc.publish", params))
                .await?;
            if !resp.ok {
                anyhow::bail!("{}", resp.error.unwrap_or_default());
            }
            let subs = resp
                .result
                .as_ref()
                .and_then(|v| v.get("subscribers"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            println!(
                "Published to '{topic}' ({subs} subscriber{})",
                if subs == 1 { "" } else { "s" },
            );
        }
    }

    Ok(())
}
