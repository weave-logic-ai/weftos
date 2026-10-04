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
              "revoked": { "value": { "reason": "x" }, "provenance": "daemon_observed" },
              "load": { "value": { "load1": 2.0, "load5": 1.0, "cores": 4 }, "provenance": "peer_claimed" } },
            { "node_id": "aaaaaaaaaaaaaaaaaaaa", "local": true,
              "name": { "value": "mac", "provenance": "peer_claimed" },
              "announced": { "value": { "address": "10.0.0.2:9" }, "provenance": "peer_claimed" },
              "facts": { "value": { "trust_tier": "pinned", "tier_source": "operator", "received_at": 900, "expires_at": 1500,
                                    "delta_seq": 3, "signed": { "payload": "{}" },
                                    "facts": { "capabilities": [
                                        { "id": "cpu.arch.aarch64", "attrs": { "cores": 8, "model": "Apple M2" }, "state": "available" },
                                        { "id": "os.macos", "attrs": { "version": "15.1" }, "state": "available" },
                                        { "id": "mem.system", "attrs": { "total": 100, "free": 40 }, "state": "busy" },
                                        { "id": "mem.unified", "attrs": { "total": 100, "free": 30 }, "state": "available" }
                                    ] } }, "provenance": "signed_fact" },
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

#[test]
fn load_capacity_and_software_come_from_their_sections() {
    let s = snap();
    let peer = node(&s, "bbbbbbbbbbbbbbbbbbbb").unwrap();
    let l = load(peer).unwrap();
    assert_eq!((l.load1, l.cores, l.provenance.as_str()), (2.0, Some(4), "peer_claimed"));
    assert_eq!(rows(&s).iter().find(|r| r.id.starts_with('b')).unwrap().load1, Some(2.0));
    assert!(capacity(peer).is_none(), "no signed facts, no capacity");

    let me = node(&s, "aaaaaaaaaaaaaaaaaaaa").unwrap();
    assert_eq!(capacity(me), Some((1, 4, Some(30))), "unified memory wins over system");
    let sw = software(me);
    assert!(sw.contains(&("cpu.arch.aarch64".into(), String::new())));
    assert!(sw.contains(&("  model".into(), "Apple M2".into())));
    assert!(sw.contains(&("  cores".into(), "8".into())));
    assert!(sw.contains(&("  version".into(), "15.1".into())));
    assert!(!sw.iter().any(|(k, _)| k.starts_with("mem.")));
}

#[test]
fn trust_view_and_admin_commands() {
    let s = snap();
    let me = trust(node(&s, "aaaaaaaaaaaaaaaaaaaa").unwrap());
    assert_eq!((me.tier.as_deref(), me.tier_source.as_deref(), me.delta_seq), (Some("pinned"), Some("operator"), Some(3)));
    assert!(me.signed && me.revoked.is_none());
    let peer = trust(node(&s, "bbbbbbbbbbbbbbbbbbbb").unwrap());
    assert_eq!(peer.revoked.as_deref(), Some("x"));
    assert!(!peer.signed);

    let c = admin_commands("n1", false, true);
    assert!(c.iter().any(|(_, cmd)| cmd.starts_with("weaver mesh peer revoke n1")));
    assert!(c.iter().any(|(_, cmd)| cmd == "weaver workload node status"));
    let c = admin_commands("n1", true, false);
    assert!(c.iter().any(|(_, cmd)| cmd == "weaver mesh peer unrevoke n1"));
    assert!(!c.iter().any(|(_, cmd)| cmd.contains("workload node")), "binding commands only for this node");
}

#[test]
fn history_appends_once_per_snapshot_caps_and_forgets_departed_nodes() {
    let mut h = History::new();
    let mut s = snap();
    s["fetched_at"] = json!(10);
    push_history(&mut h, &s, 2);
    push_history(&mut h, &s, 2);
    assert_eq!(h["bbbbbbbbbbbbbbbbbbbb"].len(), 1, "same snapshot twice adds once");
    for t in [11, 12] {
        s["fetched_at"] = json!(t);
        push_history(&mut h, &s, 2);
    }
    let q = &h["bbbbbbbbbbbbbbbbbbbb"];
    assert_eq!((q.len(), q[0].t, q[1].rtt_ms, q[1].load1), (2, 11, Some(4.2), Some(2.0)));
    s["nodes"] = json!([]);
    s["fetched_at"] = json!(13);
    push_history(&mut h, &s, 2);
    assert!(h.is_empty());
}
