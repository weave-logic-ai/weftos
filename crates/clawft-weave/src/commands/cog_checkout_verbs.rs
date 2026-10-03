//! `weaver cog checkout release | renew | list | reset-floor` (ADR-106).

use clawft_rpc::DaemonClient;
use serde_json::{Value, json};

use super::cog_checkout_cmd::{CheckoutCmd, call, parse_ref};

/// True for the verbs this module runs.
pub fn handles(cmd: &CheckoutCmd) -> bool {
    matches!(cmd, CheckoutCmd::Release { .. } | CheckoutCmd::Renew { .. } | CheckoutCmd::List { .. } | CheckoutCmd::ResetFloor { .. })
}

fn exact(reference: &str) -> anyhow::Result<(String, String)> {
    let (cog, version) = parse_ref(reference).map_err(anyhow::Error::msg)?;
    if version == "latest" {
        anyhow::bail!("give an exact version, not latest");
    }
    Ok((cog, version))
}

fn print(v: &Value, json: bool, text: String) -> anyhow::Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(v)?);
    } else {
        print!("{text}");
    }
    Ok(())
}

/// Run one verb against the daemon.
pub async fn run(cmd: CheckoutCmd, client: &mut DaemonClient) -> anyhow::Result<()> {
    match cmd {
        CheckoutCmd::Release { reference, json } => {
            let (cog, version) = exact(&reference)?;
            let v = call(client, "workload.cog.checkout.release", json!({ "cog_id": cog, "version": version })).await?;
            let t = format!("released {cog}@{version} (withdrawal seq {}, flooded; chained as cog.checkout.release)\n", v["seq"]);
            print(&v, json, t)
        }
        CheckoutCmd::Renew { reference, json } => {
            let (cog, version) = exact(&reference)?;
            let v = call(client, "workload.cog.checkout.renew", json!({ "cog_id": cog, "version": version })).await?;
            let t = if v["withdrawn"].as_bool() == Some(true) {
                format!("{cog}@{version} was WITHDRAWN by the Seed (seq {}): the licence no longer covers it\n", v["seq"])
            } else {
                format!("renewed {cog}@{version}: seq {} -> {}, expires_at {}\n", v["seq_before"], v["seq"], v["expires_at"])
            };
            print(&v, json, t)
        }
        CheckoutCmd::List { json } => {
            let v = call(client, "workload.cog.checkout.list", json!({})).await?;
            print(&v, json, render_list(&v))
        }
        CheckoutCmd::ResetFloor { confirm } => {
            let shown = call(client, "workload.node.reset-floor", json!({})).await?;
            let r = if confirm {
                let floor = shown["preview"]["floor"].as_u64();
                call(client, "workload.node.reset-floor", json!({ "confirm": true, "floor": floor })).await?
            } else {
                shown
            };
            print!("{}", super::workload_node_cmd::render_floor_preview(&r["preview"]));
            if r["applied"].as_bool() == Some(true) {
                println!("floor reset (chained as licence.floor_reset_requested and licence.floor_reset)");
            } else {
                println!("nothing changed; run again with --confirm to apply");
            }
            Ok(())
        }
        _ => unreachable!("handles() selects the verbs"),
    }
}

fn hours(secs: u64) -> String {
    format!("{}h{:02}m", secs / 3600, (secs % 3600) / 60)
}

/// Plain-text `list`.
pub fn render_list(v: &Value) -> String {
    let mut out = String::new();
    let grants = v["grants"].as_array().cloned().unwrap_or_default();
    if grants.is_empty() {
        out.push_str("no grants held\n");
    }
    for g in &grants {
        let state = if g["withdrawn"].as_bool() == Some(true) {
            "WITHDRAWN".to_string()
        } else if g["valid"].as_bool() == Some(true) {
            format!("valid, expires in {}", hours(g["expires_in"].as_u64().unwrap_or(0)))
        } else {
            "LAPSED".to_string()
        };
        out.push_str(&format!(
            "{}@{}  seq {}  {state}\n",
            g["cog_id"].as_str().unwrap_or("?"),
            g["version"].as_str().unwrap_or("?"),
            g["seq"]
        ));
        for a in g["artifacts"].as_array().into_iter().flatten() {
            let approved = if a["approval_id"].is_string() { "approved" } else { "NOT APPROVED" };
            out.push_str(&format!(
                "  {} {}  {approved}\n",
                a["arch"].as_str().unwrap_or("?"),
                a["sha256"].as_str().map_or("?", |s| s.get(..16).unwrap_or(s))
            ));
        }
    }
    let n = v["approvals"].as_array().map_or(0, Vec::len);
    let orphaned = v["approvals"].as_array().into_iter().flatten().filter(|a| a["active"] == false).count();
    out.push_str(&format!("approvals {n}{}\n", if orphaned > 0 { format!(" ({orphaned} orphaned)") } else { String::new() }));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::cog_checkout_cmd::CheckoutArgs;
    use clap::Parser;

    #[derive(Parser, Debug)]
    struct W {
        #[command(subcommand)]
        top: Top,
    }

    #[derive(clap::Subcommand, Debug)]
    enum Top {
        Checkout(CheckoutArgs),
    }

    fn parse(args: &[&str]) -> Result<CheckoutCmd, clap::Error> {
        let mut v = vec!["weaver", "checkout"];
        v.extend_from_slice(args);
        W::try_parse_from(v).map(|w| match w.top {
            Top::Checkout(a) => a.command.expect("a subcommand"),
        })
    }

    #[test]
    fn release_renew_list_and_reset_floor_parse() {
        assert!(matches!(parse(&["release", "fall-detect@1.2.0"]).unwrap(), CheckoutCmd::Release { ref reference, json: false } if reference == "fall-detect@1.2.0"));
        assert!(matches!(parse(&["renew", "fall-detect@1.2.0", "--json"]).unwrap(), CheckoutCmd::Renew { json: true, .. }));
        assert!(matches!(parse(&["list"]).unwrap(), CheckoutCmd::List { json: false }));
        assert!(matches!(parse(&["reset-floor", "--confirm"]).unwrap(), CheckoutCmd::ResetFloor { confirm: true }));
        assert!(parse(&["release"]).is_err(), "release needs a reference");
        assert!(handles(&parse(&["list"]).unwrap()));
        assert!(exact("fall-detect@latest").is_err());
        assert!(exact("fall-detect@1.2.0").is_ok());
    }

    #[test]
    fn list_shows_state_expiry_and_approval_per_artifact() {
        let v = json!({"grants": [
            {"cog_id": "fall-detect", "version": "1.2.0", "seq": 3, "valid": true, "withdrawn": false, "expires_in": 7260,
             "artifacts": [{"arch": "aarch64", "sha256": "ab".repeat(32), "approval_id": "k"},
                           {"arch": "armv7", "sha256": "cd".repeat(32), "approval_id": null}]},
            {"cog_id": "old", "version": "1", "seq": 2, "valid": false, "withdrawn": true, "expires_in": 0, "artifacts": []}],
            "approvals": [{"active": true}, {"active": false}]});
        let t = render_list(&v);
        assert!(t.contains("fall-detect@1.2.0  seq 3  valid, expires in 2h01m"), "{t}");
        assert!(t.contains("aarch64 abababababababab  approved") && t.contains("armv7 cdcdcdcdcdcdcdcd  NOT APPROVED"), "{t}");
        assert!(t.contains("old@1  seq 2  WITHDRAWN"), "{t}");
        assert!(t.contains("approvals 2 (1 orphaned)"), "{t}");
        assert!(render_list(&json!({"grants": [], "approvals": []})).contains("no grants held"));
    }
}
