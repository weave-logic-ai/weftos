use super::*;
use serde_json::json;

fn snap() -> Value {
    json!({
        "local_node_id": "aaaaaaaaaaaaaaaaaaaa",
        "nodes": [
            { "node_id": "bbbbbbbbbbbbbbbbbbbb",
              "cluster": { "value": { "state": "suspect", "last_announce_unix": 100 }, "provenance": "daemon_observed" },
              "mesh": { "value": { "class": "node", "verified": true, "heartbeat": "alive",
                                   "rtt_ms": 4.2, "last_seen_unix": 995 }, "provenance": "daemon_observed" },
              "instances": { "value": [
                  { "placement": { "instance_id": "i-1", "node_id": "bbbbbbbbbbbbbbbbbbbb", "workload": "anomaly-detect", "project_id": "lab" },
                    "lifecycle": { "state": "running", "restarts": 2 } },
                  { "placement": { "instance_id": "i-2", "node_id": "bbbbbbbbbbbbbbbbbbbb", "workload": "cog-b" },
                    "lifecycle": null }
              ], "provenance": "daemon_observed" },
              "revoked": { "value": { "reason": "x" }, "provenance": "daemon_observed" } },
            { "node_id": "aaaaaaaaaaaaaaaaaaaa", "local": true,
              "name": { "value": "mac", "provenance": "peer_claimed" },
              "announced": { "value": { "address": "10.0.0.2:9" }, "provenance": "peer_claimed" },
              "facts": { "value": { "trust_tier": "pinned", "tier_source": "operator" }, "provenance": "signed_fact" },
              "location": { "value": { "site": "Lab", "room": "R1" }, "provenance": "operator_claimed" } },
            { "node_id": "c6-01", "unknown_node": true,
              "location": { "value": { "site": "Lab" }, "provenance": "operator_claimed" } },
        ]
    })
}

#[test]
fn rows_put_this_node_first_and_read_every_column() {
    let r = rows(&snap());
    assert_eq!(r.len(), 3);
    let me = &r[0];
    assert!(me.local);
    assert_eq!((me.name.as_str(), me.trust.as_str(), me.address.as_str()), ("mac", "pinned (operator)", "10.0.0.2:9"));
    assert_eq!(me.location.as_deref(), Some("Lab / R1"));
    assert_eq!(me.class, None);

    let peer = r.iter().find(|x| x.id.starts_with('b')).unwrap();
    assert_eq!(peer.name, "bbbbbbbb...");
    assert_eq!((peer.class.as_deref(), peer.verified, peer.heartbeat.as_deref()), (Some("node"), true, Some("alive")));
    assert_eq!(peer.rtt_ms, Some(4.2));
    assert_eq!(peer.seen_unix, Some(995), "the pong wins over the older announce");
    assert_eq!(peer.cogs, 2);
    assert!(peer.revoked);

    let edge = r.iter().find(|x| x.id == "c6-01").unwrap();
    assert!(edge.unknown);
    assert_eq!(edge.location.as_deref(), Some("Lab / -"));
    assert_eq!(edge.state, "-");
}

#[test]
fn sections_carry_their_provenance() {
    let s = snap();
    let me = node(&s, "aaaaaaaaaaaaaaaaaaaa").unwrap();
    let secs = sections(me);
    assert!(secs.contains(&("facts".into(), "signed_fact".into())));
    assert!(secs.contains(&("location".into(), "operator_claimed".into())));
    assert!(!secs.iter().any(|(k, _)| k == "node_id" || k == "local"));
    assert_eq!(provenance(me, "name"), Some("peer_claimed"));
    assert_eq!(provenance(me, "mesh"), None);
    assert!(explain("signed_fact").contains("verified"));
}

#[test]
fn instances_read_the_placed_workloads() {
    let s = snap();
    let i = instances(node(&s, "bbbbbbbbbbbbbbbbbbbb").unwrap());
    assert_eq!(i[0], Instance { id: "i-1".into(), workload: "anomaly-detect".into(), state: "running".into(), restarts: 2, project: "lab".into() });
    assert_eq!((i[1].state.as_str(), i[1].project.as_str()), ("pending", ""));
    assert!(instances(node(&s, "c6-01").unwrap()).is_empty());
}

#[test]
fn an_empty_or_odd_snapshot_gives_no_rows_not_a_panic() {
    assert!(rows(&json!({})).is_empty());
    assert!(rows(&json!({ "nodes": "nope" })).is_empty());
    assert!(node(&json!({}), "x").is_none());
}

#[test]
fn snapshot_url_is_only_built_when_a_gateway_is_set() {
    assert_eq!(snapshot_url(""), None);
    assert_eq!(snapshot_url(" http://127.0.0.1:18789/ ").as_deref(), Some("http://127.0.0.1:18789/api/fleet/snapshot"));
    assert_eq!(snapshot_url("gw.local:8080").as_deref(), Some("http://gw.local:8080/api/fleet/snapshot"));
    assert_eq!(age(1000, 995), "5s ago");
}
