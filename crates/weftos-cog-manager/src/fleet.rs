//! The fleet model for the Network tab (fleet manager P2): rows and node detail read out of the
//! daemon's `fleet.snapshot` (served by the ADR-102 gateway at `GET /api/fleet/snapshot`). Every
//! section of a node there is `{value, provenance}`; this file keeps the provenance next to each
//! fact so the UI can say where it came from. Pure and tested; `views/fleet_detail.rs` draws it.

use serde_json::Value;

/// One node in the fleet list.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FleetRow {
    pub id: String,
    pub name: String,
    pub local: bool,
    pub state: String,
    pub trust: String,
    pub address: String,
    /// Mesh class (`node`, `leaf`, ...) when the peer is connected now.
    pub class: Option<String>,
    pub verified: bool,
    pub heartbeat: Option<String>,
    pub rtt_ms: Option<f64>,
    /// Unix seconds of the last liveness pong, else of the last announce.
    pub seen_unix: Option<u64>,
    pub cogs: usize,
    pub location: Option<String>,
    pub revoked: bool,
    pub unknown: bool,
}

/// A section's value (`Null` when the section is missing).
pub fn val<'a>(node: &'a Value, section: &str) -> &'a Value {
    &node[section]["value"]
}

/// A section's provenance label, if the section is present.
pub fn provenance<'a>(node: &'a Value, section: &str) -> Option<&'a str> {
    node.get(section)?.get("provenance")?.as_str()
}

fn text(v: &Value) -> Option<String> {
    v.as_str().filter(|s| !s.is_empty()).map(str::to_owned)
}

/// The list, this node first, then by name.
pub fn rows(snap: &Value) -> Vec<FleetRow> {
    let mut out: Vec<FleetRow> = snap["nodes"].as_array().into_iter().flatten().map(row).collect();
    out.sort_by_key(|r| (!r.local, r.name.to_lowercase()));
    out
}

fn row(n: &Value) -> FleetRow {
    let id = n["node_id"].as_str().unwrap_or("?").to_owned();
    let facts = val(n, "facts");
    let mesh = val(n, "mesh");
    let cluster = val(n, "cluster");
    let loc = val(n, "location");
    let trust = match (text(&facts["trust_tier"]), text(&facts["tier_source"])) {
        (Some(t), Some(s)) => format!("{t} ({s})"),
        (Some(t), None) => t,
        _ => "-".into(),
    };
    FleetRow {
        name: text(val(n, "name")).unwrap_or_else(|| short(&id)),
        local: n["local"] == true,
        state: text(&cluster["state"]).unwrap_or_else(|| if mesh.is_object() { "connected".into() } else { "-".into() }),
        trust,
        address: text(&val(n, "announced")["address"]).unwrap_or_default(),
        class: mesh.is_object().then(|| text(&mesh["class"]).unwrap_or_else(|| "?".into())),
        verified: mesh["verified"] == true,
        heartbeat: text(&mesh["heartbeat"]),
        rtt_ms: mesh["rtt_ms"].as_f64(),
        seen_unix: mesh["last_seen_unix"].as_u64().or_else(|| cluster["last_announce_unix"].as_u64()),
        cogs: val(n, "instances").as_array().map_or(0, Vec::len),
        location: loc.is_object().then(|| {
            format!("{} / {}", text(&loc["site"]).unwrap_or_else(|| "-".into()), text(&loc["room"]).unwrap_or_else(|| "-".into()))
        }),
        revoked: val(n, "revoked").is_object(),
        unknown: n["unknown_node"] == true,
        id,
    }
}

/// First 8 characters of an id plus an ellipsis (ids are long hex).
pub fn short(id: &str) -> String {
    if id.chars().count() > 12 {
        format!("{}...", id.chars().take(8).collect::<String>())
    } else {
        id.to_owned()
    }
}

/// The node with this id.
pub fn node<'a>(snap: &'a Value, id: &str) -> Option<&'a Value> {
    snap["nodes"].as_array()?.iter().find(|n| n["node_id"] == id)
}

/// Every section the node has, with its provenance (for the Overview legend and the Raw tab).
pub fn sections(node: &Value) -> Vec<(String, String)> {
    node.as_object()
        .into_iter()
        .flatten()
        .filter_map(|(k, v)| Some((k.clone(), v.get("provenance")?.as_str()?.to_owned())))
        .collect()
}

/// One placed workload on a node.
#[derive(Debug, Clone, PartialEq)]
pub struct Instance {
    pub id: String,
    pub workload: String,
    pub state: String,
    pub restarts: u64,
    pub project: String,
}

pub fn instances(node: &Value) -> Vec<Instance> {
    val(node, "instances")
        .as_array()
        .into_iter()
        .flatten()
        .map(|r| {
            let p = &r["placement"];
            Instance {
                id: text(&p["instance_id"]).unwrap_or_else(|| "?".into()),
                workload: text(&p["workload"]).unwrap_or_else(|| "-".into()),
                // `lifecycle` is null until the controller has seen the instance report.
                state: text(&r["lifecycle"]["state"]).unwrap_or_else(|| "pending".into()),
                restarts: r["lifecycle"]["restarts"].as_u64().unwrap_or(0),
                project: text(&p["project_id"]).unwrap_or_default(),
            }
        })
        .collect()
}

/// What a provenance label means, for the hover text.
pub fn explain(provenance: &str) -> &'static str {
    match provenance {
        "signed_fact" => "signed by the node's key and verified by this daemon",
        "daemon_observed" => "seen by this daemon itself",
        "operator_claimed" => "set by an operator; not verified",
        "peer_claimed" => "what the peer says about itself; an unverified peer chooses it",
        "self_reported" => "an edge node's own report; unauthenticated",
        _ => "unknown provenance",
    }
}

/// `12s ago`, `5m ago`, ... from unix seconds.
pub fn age(now: u64, then: u64) -> String {
    let d = now.saturating_sub(then);
    match d {
        0..=59 => format!("{d}s ago"),
        60..=3599 => format!("{}m ago", d / 60),
        3600..=86_399 => format!("{}h ago", d / 3600),
        _ => format!("{}d ago", d / 86_400),
    }
}

/// The gateway URL for the snapshot, from the configured gateway base (empty = not configured).
pub fn snapshot_url(gateway: &str) -> Option<String> {
    let g = gateway.trim().trim_end_matches('/');
    if g.is_empty() {
        return None;
    }
    let g = if g.starts_with("http://") || g.starts_with("https://") { g.to_owned() } else { format!("http://{g}") };
    Some(format!("{g}/api/fleet/snapshot"))
}

#[cfg(test)]
#[path = "fleet_tests.rs"]
mod tests;
