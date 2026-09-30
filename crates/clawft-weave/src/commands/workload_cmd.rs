//! `weaver workload` — governed workloads (ADR-099).
//!
//! This file owns the subcommand group plus the daemon-backed `list` and
//! `inspect` verbs (card mesh-placement-06). Other cards add their verbs
//! in their own modules and hook one variant each into
//! [`WorkloadCommand`].

use clap::{Args, Subcommand};
use serde_json::Value;

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
    /// Package commands (pack, verify, keygen); these run locally, without the daemon.
    #[cfg(all(feature = "ecc", feature = "exochain"))]
    #[command(flatten)]
    Package(super::workload_pack::WorkloadPackCmd),
    /// Placement commands (place, explain, status, stop, logs, unload).
    #[cfg(all(feature = "placement", unix))]
    #[command(flatten)]
    Placement(super::workload_place_cmd::WorkloadPlaceCmd),
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

/// `weaver workload list`: the node-local catalog, then the instances this
/// node's control plane placed (anywhere in the mesh), when it has one.
pub fn render_list(catalog: &[Value], placed: Option<&Value>) -> String {
    let mut out = render_table(catalog);
    #[cfg(all(feature = "placement", unix))]
    if let Some(p) = placed {
        out.push('\n');
        out.push_str(&super::workload_place_cmd::render_placements(p));
    }
    #[cfg(not(all(feature = "placement", unix)))]
    let _ = placed;
    out
}

/// Run a `weaver workload` subcommand against the daemon.
pub async fn run(args: WorkloadArgs) -> anyhow::Result<()> {
    #[cfg(all(feature = "ecc", feature = "exochain"))]
    if let WorkloadCommand::Package(cmd) = args.command {
        return super::workload_pack::run(cmd);
    }
    let mut client = clawft_rpc::connect_or_bail().await?;
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
                // Placed instances (mesh-placement-12), when the daemon has a
                // placement control plane.
                #[allow(unused_mut)]
                let mut placed: Option<Value> = None;
                #[cfg(all(feature = "placement", unix))]
                if let Ok(r) = client
                    .call(Request::with_params("workload.status", serde_json::json!({})))
                    .await
                    && r.ok
                {
                    placed = r.result;
                }
                print!(
                    "{}",
                    render_list(result.as_array().map(Vec::as_slice).unwrap_or(&[]), placed.as_ref())
                );
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
        #[cfg(all(feature = "ecc", feature = "exochain"))]
        WorkloadCommand::Package(_) => unreachable!("package commands are handled before connecting"),
        #[cfg(all(feature = "placement", unix))]
        WorkloadCommand::Placement(cmd) => {
            let cwd = std::env::current_dir()?;
            let (method, params) =
                super::workload_place_cmd::request(&cmd, &cwd).map_err(anyhow::Error::msg)?;
            let resp = client.call(Request::with_params(method, params)).await?;
            if !resp.ok {
                anyhow::bail!("{}", resp.error.unwrap_or_default());
            }
            let result = resp.result.unwrap_or_default();
            print!("{}", super::workload_place_cmd::render(&cmd, &result));
            // A place that placed nothing exits non-zero.
            if let Some(why) = super::workload_place_cmd::failure(&cmd, &result) {
                anyhow::bail!("{why}");
            }
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

    /// Review round 3: `list` must show instances placed on other nodes.
    #[cfg(all(feature = "placement", unix))]
    #[test]
    fn list_shows_placed_instances_after_the_catalog() {
        let placed = serde_json::json!({"instances": [
            {"placement": {"instance_id": "cog-a1", "node_id": "n-pi5", "variant": "aarch64-native"},
             "status": {"Ok": {"status": {"state": "running"}}}}]});
        let out = render_list(&[], Some(&placed));
        assert!(out.starts_with("No workloads installed\n"));
        let row = out.lines().find(|l| l.starts_with("cog-a1")).expect("placed row");
        assert!(row.contains("n-pi5") && row.contains("running"), "{out}");
        assert!(!render_list(&[], None).contains("cog-a1"));
    }
}
