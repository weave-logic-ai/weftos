//! `weaver route` (ADR-116 §5): the tailnet router inside the user daemon.
//!
//! - `weaver route list [--json]`: routes, refused declarations, upstream
//!   health and each project's process-compose state. Read.
//! - `weaver route reload`: re-read every project's `compose/ports.yaml` now.
//! - `weaver route serve --plan|--apply`: the Tailscale Serve change that puts
//!   `:443` in front of the router (`--plan` prints it; `--apply` runs it).
//!   Refuses when Funnel is on for `:443` or an existing mapping has no route
//!   equivalent. Never enables Funnel. Runs `tailscale` without a shell.

use std::net::SocketAddr;

use clap::{Parser, Subcommand};
use serde_json::{Value, json};

use crate::protocol;
use crate::router_routes::{Route, RouteTable};
use crate::router_serve;

/// Tailnet router subcommand.
#[derive(Parser)]
#[command(about = "The tailnet router: routes, reload, and the Tailscale Serve front door")]
pub struct RouteArgs {
    #[command(subcommand)]
    pub action: RouteAction,
}

/// Route subcommands.
#[derive(Subcommand)]
pub enum RouteAction {
    /// Show routes, refused declarations, upstream health and process-compose state.
    List {
        /// Print the raw JSON (the `/_weftos/routes.json` document).
        #[arg(long)]
        json: bool,
    },
    /// Re-read every registered project's compose/ports.yaml now.
    Reload,
    /// Show or apply the Tailscale Serve change (`:443 → the router`, tailnet only).
    Serve {
        /// Print the change and stop.
        #[arg(long, conflicts_with = "apply")]
        plan: bool,
        /// Run the change (after printing it).
        #[arg(long)]
        apply: bool,
        /// The tailscale binary to run.
        #[arg(long, default_value = "tailscale")]
        tailscale: String,
    },
}

const OFF: &str = "tailnet router: off ([router] enabled = true in ~/.weftos/weave.toml to turn it on)\n";

fn cell(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or("-").to_owned()
}

fn health_text(h: &Value) -> String {
    match h["state"].as_str() {
        Some("ok") => "ok".into(),
        Some("down") => format!("down ({})", h["detail"].as_str().unwrap_or("")),
        _ => h["detail"].as_str().unwrap_or("unknown").to_owned(),
    }
}

fn pc_text(p: &Value) -> String {
    match p["state"].as_str() {
        Some("ok") => format!("{}/{} running", p["running"], p["total"]),
        _ => p["detail"].as_str().unwrap_or("unknown").to_owned(),
    }
}

/// The list document as lines.
pub fn render_list(v: &Value) -> String {
    if v["enabled"] != true {
        return OFF.into();
    }
    let mut out = format!(
        "tailnet router on {} (routes generation {}, reloaded {})\n",
        cell(v, "listen"),
        v["generation"],
        cell(v, "reloaded_at")
    );
    let routes = v["routes"].as_array().cloned().unwrap_or_default();
    if routes.is_empty() {
        out += "  no routes: add `routes:` to a registered project's compose/ports.yaml\n";
    } else {
        out += &format!("  {:<22} {:<16} {:<24} {:<28} {}\n", "PREFIX", "PROJECT", "UPSTREAM", "HEALTH", "PROCESS-COMPOSE");
    }
    let projects = v["projects"].as_array().cloned().unwrap_or_default();
    for r in &routes {
        let project = cell(r, "project");
        let pc = projects.iter().find(|p| p["slug"] == project).map(|p| pc_text(&p["process_compose"])).unwrap_or_default();
        let prefix = format!("{}/{}", cell(r, "prefix"), if r["default"] == true { " *" } else { "" });
        out += &format!("  {:<22} {:<16} {:<24} {:<28} {}\n", prefix, project, cell(r, "upstream"), health_text(&r["health"]), pc);
    }
    if routes.iter().any(|r| r["default"] == true) {
        out += "  * also serves / (transitional default route)\n";
    }
    let refused = v["refused"].as_array().cloned().unwrap_or_default();
    if !refused.is_empty() {
        out += "refused:\n";
        for x in refused {
            out += &format!("  {:<16} {:<22} :{:<6} {}\n", cell(&x, "project"), cell(&x, "prefix"), x["port"], cell(&x, "reason"));
        }
    }
    out
}

/// The route table a `route.list` document carries (routes only).
pub fn table_from_list(v: &Value) -> Result<(RouteTable, SocketAddr), String> {
    let routes: Vec<Route> = serde_json::from_value(v["routes"].clone()).map_err(|e| format!("route.list: {e}"))?;
    let listen: SocketAddr = v["listen"].as_str().unwrap_or("").parse().map_err(|e| format!("route.list listen: {e}"))?;
    Ok((RouteTable { routes, ..Default::default() }, listen))
}

/// Lines for a plan.
pub fn render_plan(p: &router_serve::ServePlan, bin: &str, router: SocketAddr) -> String {
    let mut out = String::new();
    for n in &p.notes {
        out += &format!("  {n}\n");
    }
    if p.is_noop() {
        out += &format!("  already configured: :443 → http://{router} (tailnet only)\n");
    } else {
        out += "  change:\n";
        for s in &p.steps {
            out += &format!("    {bin} {}\n", s.join(" "));
        }
    }
    out
}

async fn call(method: &str) -> anyhow::Result<Value> {
    let mut client = clawft_rpc::connect_or_bail().await?;
    let resp = client.call(protocol::Request::with_params(method, json!({}))).await?;
    if !resp.ok {
        anyhow::bail!(resp.error.unwrap_or_else(|| "unknown error".into()));
    }
    Ok(resp.result.unwrap_or_default())
}

/// Run the route subcommand.
pub async fn run(args: RouteArgs) -> anyhow::Result<()> {
    match args.action {
        RouteAction::List { json } => {
            let v = call(crate::router_rpc::LIST).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&v)?);
            } else {
                print!("{}", render_list(&v));
            }
        }
        RouteAction::Reload => {
            let v = call(crate::router_rpc::RELOAD).await?;
            println!("routes reloaded at {}: {} routes, {} refused (generation {})", cell(&v, "reloaded_at"), v["routes"], v["refused"], v["generation"]);
        }
        RouteAction::Serve { plan, apply, tailscale } => {
            if !plan && !apply {
                anyhow::bail!("pass --plan to print the Tailscale Serve change or --apply to run it");
            }
            let v = call(crate::router_rpc::LIST).await?;
            if v["enabled"] != true {
                anyhow::bail!(OFF.trim_end().to_owned());
            }
            let (table, router) = table_from_list(&v).map_err(anyhow::Error::msg)?;
            let status = router_serve::tailscale_status(&tailscale).map_err(anyhow::Error::msg)?;
            let p = router_serve::plan(&status, &table, router).map_err(|e| anyhow::anyhow!("refused: {e}"))?;
            print!("{}", render_plan(&p, &tailscale, router));
            if apply && !p.is_noop() {
                for step in &p.steps {
                    router_serve::run(&tailscale, step).map_err(anyhow::Error::msg)?;
                }
                let after = router_serve::tailscale_status(&tailscale).map_err(anyhow::Error::msg)?;
                let root = after.mounts.iter().find(|m| m.path == "/").map(|m| m.target.clone()).unwrap_or_else(|| "(none)".into());
                println!("applied: :443 / → {root} (tailnet only; Funnel {})", if after.funnel_443 { "ON" } else { "off" });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_renders_off_routes_default_marker_and_refusals() {
        assert!(render_list(&json!({"enabled": false})).contains("off"));
        let v = json!({
            "enabled": true, "listen": "127.0.0.1:18100", "generation": 2, "reloaded_at": "t",
            "routes": [{"prefix": "/shastaos", "project": "shastaos", "port": 18120, "default": true,
                        "upstream": "http://127.0.0.1:18120", "health": {"state": "ok"}}],
            "projects": [{"slug": "shastaos", "process_compose": {"state": "ok", "running": 3, "total": 4}}],
            "refused": [{"project": "other", "prefix": "/shastaos", "port": 5000, "reason": "prefix /shastaos is already routed by project shastaos"}]
        });
        let t = render_list(&v);
        assert!(t.contains("/shastaos/ *") && t.contains("3/4 running") && t.contains("refused:") && t.contains("already routed"), "{t}");
    }

    #[test]
    fn list_document_round_trips_to_a_table() {
        let v = json!({"listen": "127.0.0.1:18100", "routes": [
            {"prefix": "/a", "project": "a", "port": 3000, "default": false, "health": null, "upstream": "x", "extra": 1}]});
        let (t, addr) = table_from_list(&v).unwrap();
        assert_eq!(t.routes[0].prefix, "/a");
        assert_eq!(addr.port(), 18100);
    }

    #[test]
    fn plan_renders_noop_and_steps() {
        let router: SocketAddr = "127.0.0.1:18100".parse().unwrap();
        let noop = router_serve::ServePlan { steps: vec![], notes: vec!["n".into()] };
        assert!(render_plan(&noop, "tailscale", router).contains("already configured"));
        let p = router_serve::ServePlan { steps: vec![vec!["serve".into(), "--bg".into()]], notes: vec![] };
        assert!(render_plan(&p, "tailscale", router).contains("    tailscale serve --bg\n"));
    }
}
