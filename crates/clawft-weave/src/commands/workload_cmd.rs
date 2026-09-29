//! `weaver workload` — governed workloads (ADR-099).
//!
//! This file owns the subcommand group plus the daemon-backed `list` and
//! `inspect` verbs (card mesh-placement-06). Other cards add their verbs
//! in their own modules and hook one variant each into
//! [`WorkloadCommand`].

use clap::{Args, Subcommand};
use serde_json::Value;

use crate::client::DaemonClient;
use crate::protocol::Request;

/// `weaver workload` arguments.
#[derive(Args)]
pub struct WorkloadArgs {
    /// Subcommand.
    #[command(subcommand)]
    pub command: WorkloadCommand,
}

/// `weaver workload` subcommands.
#[derive(Subcommand)]
pub enum WorkloadCommand {
    /// List workloads catalogued on this node.
    List {
        /// Print raw JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Show one workload record.
    Inspect {
        /// Workload name.
        name: String,
    },
}

fn short_hash(h: &str) -> String {
    match h.split_once(':') {
        Some((algo, hex)) if hex.len() > 12 => format!("{algo}:{}", &hex[..12]),
        _ => h.to_owned(),
    }
}

/// Render `workload.list` rows as a fixed-width table.
pub fn render_table(rows: &[Value]) -> String {
    if rows.is_empty() {
        return "No workloads installed\n".to_owned();
    }
    let s = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or("?").to_owned();
    let mut out = format!("{:<24} {:<12} {:<10} {:<12} MANIFEST\n", "NAME", "KIND", "STATE", "NODE");
    for r in rows {
        out.push_str(&format!(
            "{:<24} {:<12} {:<10} {:<12} {}\n",
            s(r, "name"),
            s(r, "kind"),
            s(r, "state"),
            s(r, "node_id"),
            short_hash(&s(r, "manifest_hash")),
        ));
    }
    out
}

/// Run a `weaver workload` subcommand against the daemon.
pub async fn run(args: WorkloadArgs) -> anyhow::Result<()> {
    let mut client = DaemonClient::connect()
        .await
        .ok_or_else(|| anyhow::anyhow!("no daemon running — start with 'weaver kernel start'"))?;
    match args.command {
        WorkloadCommand::List { json } => {
            let resp = client.simple_call("workload.list").await?;
            if !resp.ok {
                anyhow::bail!("{}", resp.error.unwrap_or_default());
            }
            let result = resp.result.unwrap_or(Value::Array(Vec::new()));
            if json {
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else {
                print!("{}", render_table(result.as_array().map(Vec::as_slice).unwrap_or(&[])));
            }
        }
        WorkloadCommand::Inspect { name } => {
            let params = serde_json::json!({ "name": name });
            let resp = client
                .call(Request::with_params("workload.inspect", params))
                .await?;
            if !resp.ok {
                anyhow::bail!("{}", resp.error.unwrap_or_default());
            }
            println!("{}", serde_json::to_string_pretty(&resp.result.unwrap_or_default())?);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_renders_rows_and_empty_state() {
        assert_eq!(render_table(&[]), "No workloads installed\n");
        let row = serde_json::json!({
            "name": "anomaly-detect", "kind": "cog", "state": "installed", "node_id": "n-1",
            "manifest_hash": "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        });
        let t = render_table(&[row]);
        assert!(t.starts_with("NAME"));
        assert!(t.contains("anomaly-detect"));
        assert!(t.contains("sha256:0123456789ab\n"));
    }
}
