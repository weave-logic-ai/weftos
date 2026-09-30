//! `weaver workload place | explain | status | stop | logs | unload`
//! (ADR-099 sections 3 and 7, card mesh-placement-12).
//!
//! Each verb calls the daemon's placement control plane (the
//! `workload.*` RPC family in `workload_place_rpc`). The daemon decides,
//! chains and dispatches; this module only builds params and renders.

use std::path::PathBuf;

use clap::{Args, Subcommand};
use serde_json::{Value, json};

/// Placement subcommands of `weaver workload` (flattened into it).
#[derive(Subcommand, Debug)]
pub enum WorkloadPlaceCmd {
    /// Place a signed package on the best node (fetch, admit, load, start).
    Place(PlaceArgs),
    /// Decide and explain a placement without dispatching it.
    Explain(PlaceArgs),
    /// Placed instances and their live status (or one instance).
    Status {
        /// Instance id (omit for all placements).
        instance_id: Option<String>,
        /// Print raw JSON.
        #[arg(long)]
        json: bool,
    },
    /// Stop a placed instance (gated and chained on both nodes).
    Stop {
        /// Instance id.
        instance_id: String,
    },
    /// Captured output of a stopped instance.
    Logs {
        /// Instance id.
        instance_id: String,
    },
    /// Unload a placed instance, or with `--catalog` a catalogued workload by name.
    Unload {
        /// Instance id (or catalog name with `--catalog`).
        target: String,
        /// Remove a node-local catalog entry instead of a placed instance.
        #[arg(long)]
        catalog: bool,
    },
}

/// Options shared by `place` and `explain`.
#[derive(Args, Debug, Clone)]
pub struct PlaceArgs {
    /// Unpacked signed package directory (`weaver workload pack --out`).
    pub package: PathBuf,
    /// `workload-host` address (`host:port`) to consider, as a paired node (repeat).
    #[arg(long = "peer")]
    pub peers: Vec<String>,
    /// Place on this node id or fail naming the constraint.
    #[arg(long)]
    pub pin: Option<String>,
    /// Prefer these node ids among equally tiered candidates (repeat).
    #[arg(long)]
    pub prefer: Vec<String>,
    /// Avoid these node ids unless nothing else fits (repeat).
    #[arg(long)]
    pub avoid: Vec<String>,
    /// Allow an emulated route (operator opt-in; recorded as emulated).
    #[arg(long)]
    pub allow_emulated: bool,
    /// Run mode: once, interval, listener.
    #[arg(long, default_value = "interval")]
    pub mode: String,
    /// Seconds between cycles for `--mode interval`.
    #[arg(long, default_value_t = 10)]
    pub interval: u32,
    /// UDP port the cog binds for the sensor feed.
    #[arg(long, default_value_t = 5006)]
    pub csi_port: u16,
    /// Load without starting.
    #[arg(long)]
    pub no_start: bool,
    /// Print the full explanation (always printed for `explain`).
    #[arg(long)]
    pub explain: bool,
    /// Print raw JSON.
    #[arg(long)]
    pub json: bool,
}

/// RPC method and params for a placement verb.
pub fn request(cmd: &WorkloadPlaceCmd) -> (&'static str, Value) {
    match cmd {
        WorkloadPlaceCmd::Place(a) | WorkloadPlaceCmd::Explain(a) => {
            let m = if matches!(cmd, WorkloadPlaceCmd::Place(_)) {
                "workload.place"
            } else {
                "workload.explain"
            };
            let mut p = json!({
                "package_dir": a.package, "peers": a.peers, "prefer": a.prefer, "avoid": a.avoid,
                "allow_emulated": a.allow_emulated, "mode": a.mode, "interval": a.interval,
                "csi_port": a.csi_port, "start": !a.no_start,
            });
            if let Some(pin) = &a.pin {
                p["pin"] = json!(pin);
            }
            (m, p)
        }
        WorkloadPlaceCmd::Status { instance_id, .. } => (
            "workload.status",
            instance_id
                .as_ref()
                .map_or(json!({}), |i| json!({ "instance_id": i })),
        ),
        WorkloadPlaceCmd::Stop { instance_id } => {
            ("workload.stop", json!({ "instance_id": instance_id }))
        }
        WorkloadPlaceCmd::Logs { instance_id } => {
            ("workload.logs", json!({ "instance_id": instance_id }))
        }
        WorkloadPlaceCmd::Unload {
            target,
            catalog: true,
        } => ("workload.unload", json!({ "name": target })),
        WorkloadPlaceCmd::Unload {
            target,
            catalog: false,
        } => ("workload.unload", json!({ "instance_id": target })),
    }
}

/// Short human summary of a place / explain result.
pub fn summary(r: &Value, explain: bool) -> String {
    let mut out = String::new();
    if explain {
        out.push_str(r["explain"].as_str().unwrap_or(""));
    }
    match r.get("placed").filter(|p| !p.is_null()) {
        Some(p) => out.push_str(&format!(
            "placed {} on {} via {} (instance {})\n",
            p["workload"].as_str().unwrap_or("?"),
            p["node_id"].as_str().unwrap_or("?"),
            p["variant"].as_str().unwrap_or("?"),
            p["instance_id"].as_str().unwrap_or("?"),
        )),
        None if r["decision"]["placement"].is_null() => {
            out.push_str("unplaceable: no eligible node (see --explain)\n")
        }
        None if r["attempts"].as_array().is_some_and(|a| !a.is_empty()) => {
            out.push_str("not placed: every candidate refused (see --explain)\n")
        }
        None => out.push_str(&format!(
            "decision: {} (not dispatched)\n",
            r["decision"]["placement"]["node_id"]
                .as_str()
                .unwrap_or("?")
        )),
    }
    out.push_str(&format!(
        "decision_id {}\n",
        r["decision_id"].as_str().unwrap_or("?")
    ));
    out
}

/// Placements table for `weaver workload list` / `status`.
pub fn render_placements(status: &Value) -> String {
    let rows = status["instances"].as_array().cloned().unwrap_or_default();
    if rows.is_empty() {
        return "No placed instances\n".to_owned();
    }
    let mut out = format!(
        "{:<40} {:<16} {:<18} {:<10}\n",
        "INSTANCE", "NODE", "VARIANT", "STATE"
    );
    for r in rows {
        let p = &r["placement"];
        let state = r["status"]["Ok"]["status"]["state"]
            .as_str()
            .unwrap_or("unreachable");
        out.push_str(&format!(
            "{:<40} {:<16} {:<18} {:<10}\n",
            p["instance_id"].as_str().unwrap_or("?"),
            p["node_id"].as_str().unwrap_or("?"),
            p["variant"].as_str().unwrap_or("?"),
            state,
        ));
    }
    out
}

/// Render a verb's result.
pub fn render(cmd: &WorkloadPlaceCmd, result: &Value) -> String {
    match cmd {
        WorkloadPlaceCmd::Place(a) if !a.json => summary(result, a.explain),
        WorkloadPlaceCmd::Explain(a) if !a.json => summary(result, true),
        WorkloadPlaceCmd::Status {
            instance_id: None,
            json: false,
        } => render_placements(result),
        WorkloadPlaceCmd::Logs { .. } => format!(
            "{}{}",
            result["stdout"]
                .as_str()
                .unwrap_or(result["note"].as_str().unwrap_or("")),
            result["stderr"].as_str().unwrap_or("")
        ),
        _ => format!(
            "{}\n",
            serde_json::to_string_pretty(result).unwrap_or_default()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(pkg: &str) -> PlaceArgs {
        PlaceArgs {
            package: pkg.into(),
            peers: vec!["pi5:9471".into()],
            pin: None,
            prefer: vec![],
            avoid: vec![],
            allow_emulated: false,
            mode: "interval".into(),
            interval: 1,
            csi_port: 5006,
            no_start: false,
            explain: true,
            json: false,
        }
    }

    #[test]
    fn verbs_map_to_the_workload_rpc_family() {
        let (m, p) = request(&WorkloadPlaceCmd::Place(args("/p")));
        assert_eq!(m, "workload.place");
        assert_eq!(p["peers"][0], "pi5:9471");
        assert_eq!(p["start"], true);
        assert!(p.get("pin").is_none());
        let mut pinned = args("/p");
        pinned.pin = Some("n-1".into());
        let (m, p) = request(&WorkloadPlaceCmd::Explain(pinned));
        assert_eq!((m, p["pin"].as_str()), ("workload.explain", Some("n-1")));
        let unload = WorkloadPlaceCmd::Unload {
            target: "i-1".into(),
            catalog: false,
        };
        assert_eq!(request(&unload).1, json!({"instance_id": "i-1"}));
        let cat = WorkloadPlaceCmd::Unload {
            target: "w".into(),
            catalog: true,
        };
        assert_eq!(request(&cat).1, json!({"name": "w"}));
        assert_eq!(
            request(&WorkloadPlaceCmd::Status {
                instance_id: None,
                json: false
            })
            .1,
            json!({})
        );
    }

    #[test]
    fn summary_names_the_node_or_why_not() {
        let placed = json!({"explain": "EXPLAIN\n", "decision_id": "d1", "decision": {"placement": {"node_id": "n-pi"}},
            "attempts": [{}], "placed": {"workload": "anomaly-detect", "node_id": "n-pi", "variant": "aarch64-native", "instance_id": "i"}});
        let s = summary(&placed, true);
        assert!(s.starts_with("EXPLAIN"));
        assert!(s.contains("placed anomaly-detect on n-pi via aarch64-native"));
        let refused = json!({"decision_id": "d2", "decision": {"placement": {"node_id": "n-mac"}}, "attempts": [{}], "placed": null});
        assert!(summary(&refused, false).contains("every candidate refused"));
        let none = json!({"decision_id": "d3", "decision": {"placement": null}, "attempts": []});
        assert!(summary(&none, false).contains("unplaceable"));
        let dry = json!({"decision_id": "d4", "decision": {"placement": {"node_id": "n-pi"}}, "attempts": []});
        assert!(summary(&dry, false).contains("n-pi (not dispatched)"));
    }

    #[test]
    fn placements_table_shows_state_or_unreachable() {
        let st = json!({"instances": [
            {"placement": {"instance_id": "a-1", "node_id": "n-pi", "variant": "aarch64-native"},
             "status": {"Ok": {"status": {"state": "running"}}}},
            {"placement": {"instance_id": "b-2", "node_id": "n-gone", "variant": "aarch64-native"},
             "status": {"Err": "unreachable"}}]});
        let t = render_placements(&st);
        assert!(t.contains("running"));
        assert!(
            t.lines()
                .any(|l| l.starts_with("b-2") && l.contains("unreachable"))
        );
        assert_eq!(render_placements(&json!({})), "No placed instances\n");
    }
}
