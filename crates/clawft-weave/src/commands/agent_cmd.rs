//! `weaver agent` — agent lifecycle management commands.

use clap::{Args, Subcommand};
use comfy_table::{Cell, Table};

use crate::client::DaemonClient;
use crate::protocol::{
    AgentInspectResult, AgentRestartParams, AgentSendParams, AgentSpawnParams, AgentSpawnResult,
    AgentStopParams, ProcessInfo, Request,
};

#[derive(Args)]
pub struct AgentArgs {
    #[command(subcommand)]
    pub command: AgentCommand,
}

#[derive(Subcommand)]
pub enum AgentCommand {
    /// Spawn a new supervised agent.
    Spawn {
        /// Agent identifier.
        agent_id: String,
        /// Parent PID (for spawn lineage).
        #[arg(long)]
        parent: Option<u64>,
    },
    /// Stop a running agent.
    Stop {
        /// PID of the agent to stop.
        pid: u64,
        /// Force immediate stop (skip graceful shutdown).
        #[arg(short, long)]
        force: bool,
    },
    /// Restart an agent (stop + respawn with same config).
    Restart {
        /// PID of the agent to restart.
        pid: u64,
    },
    /// Inspect a specific agent process.
    Inspect {
        /// PID of the agent to inspect.
        pid: u64,
    },
    /// List all agent processes.
    List,
    /// Send a text message to an agent.
    Send {
        /// Target agent PID.
        pid: u64,
        /// Message text.
        message: String,
    },
    /// Attach to an agent's output stream (planned).
    Attach {
        /// PID of the agent to attach to.
        pid: u64,
    },
}

pub async fn run(args: AgentArgs) -> anyhow::Result<()> {
    let mut client = DaemonClient::connect()
        .await
        .ok_or_else(|| anyhow::anyhow!("no daemon running — start with 'weaver kernel start'"))?;

    match args.command {
        AgentCommand::Spawn { agent_id, parent } => {
            let params = serde_json::to_value(AgentSpawnParams {
                agent_id,
                parent_pid: parent,
            })?;
            let resp = client
                .call(Request::with_params("agent.spawn", params))
                .await?;
            if !resp.ok {
                anyhow::bail!("{}", resp.error.unwrap_or_default());
            }
            let result: AgentSpawnResult = serde_json::from_value(resp.result.unwrap_or_default())?;
            println!("Agent spawned");
            println!("  PID:      {}", result.pid);
            println!("  Agent ID: {}", result.agent_id);
        }
        AgentCommand::Stop { pid, force } => {
            let params = serde_json::to_value(AgentStopParams {
                pid,
                graceful: !force,
            })?;
            let resp = client
                .call(Request::with_params("agent.stop", params))
                .await?;
            if !resp.ok {
                anyhow::bail!("{}", resp.error.unwrap_or_default());
            }
            println!(
                "Agent {} stopped ({})",
                pid,
                if force { "force" } else { "graceful" }
            );
        }
        AgentCommand::Restart { pid } => {
            let params = serde_json::to_value(AgentRestartParams { pid })?;
            let resp = client
                .call(Request::with_params("agent.restart", params))
                .await?;
            if !resp.ok {
                anyhow::bail!("{}", resp.error.unwrap_or_default());
            }
            let result: AgentSpawnResult = serde_json::from_value(resp.result.unwrap_or_default())?;
            println!("Agent restarted");
            println!("  Old PID:  {pid}");
            println!("  New PID:  {}", result.pid);
            println!("  Agent ID: {}", result.agent_id);
        }
        AgentCommand::Inspect { pid } => {
            let params = serde_json::json!({"pid": pid});
            let resp = client
                .call(Request::with_params("agent.inspect", params))
                .await?;
            if !resp.ok {
                anyhow::bail!("{}", resp.error.unwrap_or_default());
            }
            let info: AgentInspectResult = serde_json::from_value(resp.result.unwrap_or_default())?;

            println!("Agent {}", info.pid);
            println!("  Agent ID:      {}", info.agent_id);
            println!("  State:         {}", info.state);
            println!(
                "  Parent PID:    {}",
                info.parent_pid.map_or("none".into(), |p| p.to_string())
            );
            println!("  Resource Usage:");
            println!("    Memory:        {} bytes", info.memory_bytes);
            println!("    CPU time:      {} ms", info.cpu_time_ms);
            println!("    Messages sent: {}", info.messages_sent);
            println!("    Tool calls:    {}", info.tool_calls);
            if info.topics.is_empty() {
                println!("  Topics:        (none)");
            } else {
                println!("  Topics:        {}", info.topics.join(", "));
            }
            println!("  Capabilities:");
            println!(
                "    spawn: {}  ipc: {}  tools: {}  network: {}",
                info.can_spawn, info.can_ipc, info.can_exec_tools, info.can_network
            );
        }
        AgentCommand::List => {
            let resp = client.simple_call("agent.list").await?;
            if !resp.ok {
                anyhow::bail!("{}", resp.error.unwrap_or_default());
            }
            let agents: Vec<ProcessInfo> = serde_json::from_value(resp.result.unwrap_or_default())?;

            if agents.is_empty() {
                println!("No agents running.");
                return Ok(());
            }

            let mut table = Table::new();
            table.set_header(vec![
                "PID", "Agent ID", "State", "Parent", "Memory", "CPU (ms)",
            ]);
            for a in &agents {
                table.add_row(vec![
                    Cell::new(a.pid),
                    Cell::new(&a.agent_id),
                    Cell::new(&a.state),
                    Cell::new(a.parent_pid.map_or("-".into(), |p| p.to_string())),
                    Cell::new(a.memory_bytes),
                    Cell::new(a.cpu_time_ms),
                ]);
            }
            println!("{table}");
        }
        AgentCommand::Send { pid, message } => {
            let params = serde_json::to_value(AgentSendParams { pid, message })?;
            let resp = client
                .call(Request::with_params("agent.send", params))
                .await?;
            if !resp.ok {
                anyhow::bail!("{}", resp.error.unwrap_or_default());
            }
            // Display the agent's reply if one was received within the timeout
            match resp.result {
                Some(ref v) if v.is_object() || v.is_array() => {
                    println!("{}", serde_json::to_string_pretty(v)?);
                }
                Some(ref v) if v.as_str() == Some("sent") => {
                    println!("Message sent to PID {pid} (no reply within timeout)");
                }
                Some(ref v) => {
                    println!("{v}");
                }
                None => {
                    println!("Message sent to PID {pid}");
                }
            }
        }
        AgentCommand::Attach { pid } => {
            println!("attach to PID {pid} — not yet implemented (planned for K2)");
        }
    }

    Ok(())
}
