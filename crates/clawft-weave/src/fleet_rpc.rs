//! `fleet.snapshot` and `fleet.location.set` (fleet manager P1).
//!
//! `fleet.snapshot` (Read) is the one read-only document a fleet console
//! needs: every node this daemon knows, with what is known about it and where
//! each fact came from. It composes sources the daemon already holds and
//! contacts no peer and no device (that is why it does not call
//! `workload.status`, a Write verb because it does):
//!
//! | section | source | provenance |
//! |---|---|---|
//! | `cluster` | cluster membership (`cluster.nodes`, with `last_seen`) | `daemon_observed` |
//! | `facts` | signed node facts cache (`cluster.facts`) | `signed_fact` |
//! | `mesh` | live connection detail (class, verified, heartbeat) | `daemon_observed` |
//! | `revoked` | host revocation list (`mesh.revoked`) | `daemon_observed` |
//! | `instances` | placement controller's own records and lifecycle | `daemon_observed` |
//! | `location` | operator labels (`fleet.location.set`) | `operator_claimed` |
//! | `infer`, `licence`, `placement`, `revocations` | `infer.status`, `workload.node.binding`, controller targets, revocation list | `daemon_observed` / `signed_fact` |
//!
//! `self_reported` is the fifth label: heartbeat fields an edge node says
//! about itself (the cog-host roster). The daemon holds none today.
//!
//! Round-trip time is reported as `null` with a note: no code in the runtime
//! measures it, and a zero would read as a perfect link.
//!
//! `fleet.location.set` (Admin) records `{node, site, room}` on the chain and
//! saves it ([`crate::fleet_labels`]).

use std::collections::BTreeMap;

use clawft_kernel::boot::Kernel;
use clawft_platform::NativePlatform;
use clawft_rpc::Response;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::fleet_labels::{self, Location};
use crate::rpc_ext::{ExtCall, ExtFuture, KernelRef};

/// Snapshot document version.
pub const SCHEMA: u32 = 1;
/// How long a console may reuse a snapshot, seconds.
pub const TTL_SECS: u64 = 5;

/// Where a field came from.
pub mod provenance {
    /// Signed by the node it describes, verified by this daemon.
    pub const SIGNED_FACT: &str = "signed_fact";
    /// Observed or held by this daemon.
    pub const DAEMON_OBSERVED: &str = "daemon_observed";
    /// Set by an operator (location labels).
    pub const OPERATOR_CLAIMED: &str = "operator_claimed";
    /// Said by an unauthenticated edge node about itself; display only.
    pub const SELF_REPORTED: &str = "self_reported";
}
use provenance::{DAEMON_OBSERVED, OPERATOR_CLAIMED, SIGNED_FACT};

/// Route handler for both methods.
pub fn handle(call: ExtCall) -> ExtFuture {
    Box::pin(async move {
        match call.method.as_str() {
            "fleet.snapshot" => Response::success(snapshot(&call.ctx.kernel).await),
            "fleet.location.set" => location_set(call.params, &call.ctx.kernel).await,
            other => Response::error(format!("unknown method {other}")),
        }
    })
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn field(value: Value, provenance: &str) -> Value {
    json!({ "value": value, "provenance": provenance })
}

/// A daemon-wide section that may be unavailable (with the reason).
fn section(value: Option<Value>, why: &str, provenance: &str, degraded: &mut Vec<String>, name: &str) -> Value {
    match value {
        Some(v) => field(v, provenance),
        None => {
            degraded.push(format!("{name}: {why}"));
            json!({ "value": null, "provenance": provenance, "unavailable": why })
        }
    }
}

/// What one pass over the kernel collects (everything synchronous).
pub(crate) struct Raw {
    pub local_id: String,
    pub peers: Vec<clawft_kernel::PeerNode>,
    pub facts: Vec<crate::node_facts_rpc::FactsEntry>,
    /// `None` when the mesh runtime is not in this process (service mode).
    pub mesh: Option<Vec<MeshPeer>>,
    pub revoked: Vec<clawft_kernel::revocation::RevokedHost>,
}

/// One live mesh connection.
pub(crate) struct MeshPeer {
    pub node_id: String,
    pub class: &'static str,
    pub verified: bool,
    pub licensed: bool,
    pub connected_at: chrono::DateTime<chrono::Utc>,
    /// `alive`, `suspect` or `dead`; `None` when not tracked.
    pub heartbeat: Option<&'static str>,
}

#[cfg(feature = "mesh")]
fn mesh_peers(k: &Kernel<NativePlatform>) -> Option<Vec<MeshPeer>> {
    use clawft_kernel::HeartbeatState::{Alive, Dead, Suspect};
    let rt = k.a2a_router().mesh_runtime()?;
    Some(
        rt.peer_details()
            .into_iter()
            .map(|d| MeshPeer {
                node_id: d.node_id,
                class: d.class.as_str(),
                verified: d.verified,
                licensed: d.licensed,
                connected_at: d.connected_at,
                heartbeat: match d.heartbeat {
                    Some(Alive) => Some("alive"),
                    Some(Suspect) => Some("suspect"),
                    Some(Dead) => Some("dead"),
                    _ => None,
                },
            })
            .collect(),
    )
}
#[cfg(not(feature = "mesh"))]
fn mesh_peers(_: &Kernel<NativePlatform>) -> Option<Vec<MeshPeer>> {
    None
}

pub(crate) fn collect(k: &Kernel<NativePlatform>) -> Raw {
    let membership = k.cluster_membership();
    let peers = membership
        .list_peers()
        .iter()
        .filter_map(|(id, _, _)| membership.get_peer(id))
        .collect();
    Raw {
        local_id: membership.local_node_id().to_owned(),
        peers,
        facts: crate::node_facts_rpc::facts_entries(membership, None),
        mesh: mesh_peers(k),
        revoked: k.revocation_list().list_revoked(),
    }
}

/// Daemon-wide inputs besides [`Raw`].
#[derive(Default)]
pub(crate) struct Extra {
    pub controller: Option<Value>,
    pub infer: Option<Value>,
    pub infer_why: String,
    pub licence: Option<Value>,
    pub labels: Option<Result<BTreeMap<String, Location>, String>>,
}

type Nodes = BTreeMap<String, Map<String, Value>>;

fn touch<'a>(nodes: &'a mut Nodes, id: &str) -> &'a mut Map<String, Value> {
    nodes.entry(id.to_owned()).or_insert_with(|| {
        let mut m = Map::new();
        m.insert("node_id".into(), json!(id));
        m
    })
}

fn rfc3339(t: chrono::DateTime<chrono::Utc>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Build the snapshot document from collected inputs.
pub(crate) fn assemble(raw: Raw, extra: Extra, now: u64) -> Value {
    let mut degraded: Vec<String> = Vec::new();
    let mut nodes: Nodes = BTreeMap::new();
    touch(&mut nodes, &raw.local_id).insert("local".into(), json!(true));
    for p in &raw.peers {
        let n = touch(&mut nodes, &p.id);
        n.insert("name".into(), json!(p.name));
        n.insert(
            "cluster".into(),
            field(
                json!({
                    "platform": p.platform.to_string(),
                    "state": p.state.to_string(),
                    "address": p.address,
                    "first_seen": rfc3339(p.first_seen),
                    "last_seen": rfc3339(p.last_heartbeat),
                    "last_seen_unix": p.last_heartbeat.timestamp(),
                }),
                DAEMON_OBSERVED,
            ),
        );
    }
    for f in &raw.facts {
        touch(&mut nodes, &f.node_id).insert(
            "facts".into(),
            field(serde_json::to_value(f).unwrap_or(Value::Null), SIGNED_FACT),
        );
    }
    match &raw.mesh {
        Some(details) => {
            for d in details {
                touch(&mut nodes, &d.node_id).insert(
                    "mesh".into(),
                    field(
                        json!({
                            "connected": true,
                            "class": d.class,
                            "verified": d.verified,
                            "licensed": d.licensed,
                            "connected_at": rfc3339(d.connected_at),
                            "heartbeat": d.heartbeat,
                            "rtt_ms": null,
                            "rtt_note": "not measured by this build",
                        }),
                        DAEMON_OBSERVED,
                    ),
                );
            }
        }
        None => degraded.push("mesh: runtime is not in this process, peer connection detail unavailable".into()),
    }
    for r in &raw.revoked {
        touch(&mut nodes, &r.host_id).insert(
            "revoked".into(),
            field(json!({ "revoked_at": r.revoked_at, "reason": r.reason }), DAEMON_OBSERVED),
        );
    }

    let mut by_node: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    if let Some(c) = &extra.controller {
        for row in c["instances"].as_array().into_iter().flatten() {
            if let Some(node) = row["placement"]["node_id"].as_str() {
                by_node.entry(node.to_owned()).or_default().push(row.clone());
            }
        }
    }
    for (node, rows) in by_node {
        touch(&mut nodes, &node).insert("instances".into(), field(Value::Array(rows), DAEMON_OBSERVED));
    }

    match &extra.labels {
        Some(Ok(labels)) => {
            for (node, loc) in labels {
                touch(&mut nodes, node).insert(
                    "location".into(),
                    field(serde_json::to_value(loc).unwrap_or(Value::Null), OPERATOR_CLAIMED),
                );
            }
        }
        Some(Err(why)) => degraded.push(format!("location: {why}")),
        None => degraded.push("location: runtime directory not initialised".into()),
    }

    let controller = section(
        extra.controller.as_ref().map(|c| {
            json!({ "controller": c["controller"], "targets": c["targets"], "unsettled": c["unsettled"] })
        }),
        "placement control plane not started",
        DAEMON_OBSERVED,
        &mut degraded,
        "placement",
    );
    let infer = section(extra.infer, &extra.infer_why, DAEMON_OBSERVED, &mut degraded, "infer");
    let licence = section(extra.licence, "licence runtime not started", SIGNED_FACT, &mut degraded, "licence");
    let revocations = field(
        json!(raw.revoked.iter().map(|r| json!({
            "host_id": r.host_id, "revoked_at": r.revoked_at, "reason": r.reason
        })).collect::<Vec<_>>()),
        DAEMON_OBSERVED,
    );

    let mut list: Vec<Value> = nodes.into_values().map(Value::Object).collect();
    list.sort_by_key(|n| {
        let local = n["node_id"] != json!(raw.local_id);
        let name = n["name"].as_str().unwrap_or_else(|| n["node_id"].as_str().unwrap_or("")).to_owned();
        (local, name)
    });
    json!({
        "schema": SCHEMA,
        "source": "daemon",
        "fetched_at": now,
        "ttl_secs": TTL_SECS,
        "local_node_id": raw.local_id,
        "degraded": degraded,
        "nodes": list,
        "placement": controller,
        "infer": infer,
        "licence": licence,
        "revocations": revocations,
    })
}

/// The `fleet.snapshot` document. Read-only; contacts no peer.
pub async fn snapshot(kernel: &KernelRef) -> Value {
    let raw = {
        let k = kernel.read().await;
        collect(&k)
    };
    let (infer, infer_why) = infer_status().await;
    let extra = Extra {
        controller: controller_view(),
        infer,
        infer_why,
        licence: licence_status(),
        labels: fleet_labels::dir().map(fleet_labels::load),
    };
    assemble(raw, extra, now_secs())
}

#[cfg(all(feature = "placement", unix))]
fn controller_view() -> Option<Value> {
    crate::workload_place_rpc::controller_view()
}
#[cfg(not(all(feature = "placement", unix)))]
fn controller_view() -> Option<Value> {
    None
}

#[cfg(all(feature = "placement", unix))]
fn licence_status() -> Option<Value> {
    crate::licence_boot::runtime().map(|rt| crate::licence_boot::status(&rt))
}
#[cfg(not(all(feature = "placement", unix)))]
fn licence_status() -> Option<Value> {
    None
}

#[cfg(all(feature = "placement", unix))]
async fn infer_status() -> (Option<Value>, String) {
    let r = crate::infer_rpc::handle("infer.status", Value::Null).await;
    match (r.ok, r.result) {
        (true, Some(v)) => (Some(v), String::new()),
        _ => (None, "inference placement is off".into()),
    }
}
#[cfg(not(all(feature = "placement", unix)))]
async fn infer_status() -> (Option<Value>, String) {
    (None, "built without placement".into())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LocationParams {
    node: String,
    site: String,
    room: String,
}

/// `fleet.location.set`: validate, require a chain, save, then chain-record.
async fn location_set(params: Value, kernel: &KernelRef) -> Response {
    let Some(dir) = fleet_labels::dir() else {
        return Response::error("fleet labels unavailable: runtime directory not initialised");
    };
    set_location(dir, params, kernel).await
}

pub(crate) async fn set_location(dir: &std::path::Path, params: Value, kernel: &KernelRef) -> Response {
    let p: LocationParams = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(e) => return Response::error(format!("invalid params: {e}")),
    };
    if let Err(e) = fleet_labels::validate(&p.node, &p.site, &p.room) {
        return Response::error(e);
    }
    #[cfg(feature = "exochain")]
    {
        let chain = kernel.read().await.chain_manager().cloned();
        let Some(chain) = chain else {
            return Response::error("a location label is recorded on the chain, and this kernel has none");
        };
        let (previous, loc) = match fleet_labels::set(dir, &p.node, &p.site, &p.room, now_secs()) {
            Ok(r) => r,
            Err(e) => return Response::error(e),
        };
        let ev = chain.append(
            fleet_labels::CHAIN_SOURCE,
            fleet_labels::CHAIN_KIND,
            Some(json!({ "node": p.node, "site": loc.site, "room": loc.room, "previous": previous })),
        );
        let hash: String = ev.hash.iter().map(|b| format!("{b:02x}")).collect();
        Response::success(json!({
            "node": p.node, "site": loc.site, "room": loc.room, "set_at": loc.set_at,
            "previous": previous,
            "chain": { "sequence": ev.sequence, "hash": hash },
        }))
    }
    #[cfg(not(feature = "exochain"))]
    {
        let _ = (dir, kernel);
        Response::error("a location label is recorded on the chain, and this build has none")
    }
}

#[cfg(test)]
#[path = "fleet_rpc_tests.rs"]
mod tests;
