use super::*;
use serde_json::json;

fn net() -> Net {
    serde_json::from_value(json!({
        "node": "cog-a",
        "tailscale": { "available": true, "peers": [
            { "name": "cog-a", "ip": "100.64.0.18", "os": "linux", "online": true, "self": true },
            { "name": "studio-mac", "ip": "100.64.0.1", "os": "macOS", "online": true },
            { "name": "gallery", "ip": "100.64.0.2", "os": "linux", "online": false },
            { "name": "localhost", "ip": "100.64.0.9", "os": "iOS", "online": false },
            { "name": "localhost", "ip": "100.64.0.10", "os": "android", "online": false }
        ]},
        "cognitum_mesh": { "count": 0 },
        "fleet": [ { "id": "c5-01", "kind": "esp32", "ip": "10.0.0.51", "fw": "0.8.12", "online": true, "chip": "esp32c5" } ]
    }))
    .unwrap()
}

fn snap() -> Value {
    json!({ "nodes": [
        { "node_id": "n-mac", "local": true, "name": { "value": "studio-mac", "provenance": "peer_claimed" },
          "announced": { "value": { "address": "100.64.0.1:9471" }, "provenance": "peer_claimed" } },
        { "node_id": "n-gal", "name": { "value": "gallery", "provenance": "peer_claimed" },
          "mesh": { "value": { "class": "node", "verified": true, "heartbeat": "alive" }, "provenance": "daemon_observed" } }
    ]})
}

fn seed(url: &str, device: &str, auto: bool) -> SeedView {
    SeedView {
        url: url.into(),
        identity: Some(json!({ "device_id": device, "firmware_version": "0.24.2" })),
        auto,
        ..Default::default()
    }
}

#[test]
fn every_source_lands_in_one_list_and_the_same_machine_merges() {
    let seeds = [seed("http://100.64.0.18", "aaaa1111-x", true), seed("http://10.0.0.235", "c7059200-y", false)];
    let s = snap();
    let n = net();
    let all = unify(Some(&s), Some(&n), &seeds);
    let by = |name: &str| all.iter().find(|e| e.name == name).unwrap_or_else(|| panic!("no {name}: {all:#?}"));

    // A mesh node seen on the tailnet is one entry with both sources.
    let mac = by("studio-mac");
    assert_eq!((mac.class, mac.node_id.as_deref(), mac.os.as_deref()), (Class::Node, Some("n-mac"), Some("macOS")));
    assert_eq!(mac.sources.iter().map(|s| s.kind).collect::<Vec<_>>(), ["snapshot", "tailnet"]);
    // The mesh says alive; the tailnet's offline does not override the daemon's view.
    assert_eq!(by("gallery").online, Some(true));

    // The connected host's own Seed agent merges onto its tailnet entry.
    let cog = by("cog-a");
    assert_eq!((cog.class, cog.seed.as_deref(), cog.this_host), (Class::Seed, Some("http://100.64.0.18"), true));
    assert_eq!(cog.firmware.as_deref(), Some("0.24.2"));

    // A LAN-only Seed the operator listed is its own entry.
    let lan = all.iter().find(|e| e.key == "http://10.0.0.235").unwrap();
    assert_eq!((lan.class, lan.name.as_str(), lan.online), (Class::Seed, "seed c7059200", Some(true)));

    // Two unnamed "localhost" phones stay two entries (merged by address, not by that name).
    assert_eq!(all.iter().filter(|e| e.name == "localhost").count(), 2);

    // Edge node, self-reported.
    let e = by("c5-01");
    assert_eq!((e.class, e.firmware.as_deref(), e.sources[0].provenance), (Class::Edge, Some("0.8.12"), "self_reported"));

    // Order: nodes, seeds, hosts, edges.
    let classes: Vec<Class> = all.iter().map(|e| e.class).collect();
    let mut sorted = classes.clone();
    sorted.sort();
    assert_eq!(classes, sorted);
}

#[test]
fn without_a_gateway_the_tailnet_seeds_and_edges_still_show() {
    let n = net();
    let all = unify(None, Some(&n), &[]);
    assert_eq!(all.len(), 6, "{all:#?}");
    assert!(all.iter().all(|e| e.class != Class::Node));
    assert!(all.iter().any(|e| e.name == "studio-mac" && e.class == Class::Host));
}

#[test]
fn an_auto_probed_host_without_an_agent_is_not_listed_but_an_operator_seed_is() {
    let silent = SeedView { url: "http://100.64.0.1".into(), auto: true, error: Some("refused".into()), ..Default::default() };
    let listed = SeedView { url: "http://10.0.0.7".into(), auto: false, error: Some("timeout".into()), ..Default::default() };
    let all = unify(None, None, &[silent, listed]);
    assert_eq!(all.len(), 1);
    assert_eq!((all[0].key.as_str(), all[0].online), ("http://10.0.0.7", Some(false)));
}

#[test]
fn host_of_strips_scheme_port_and_path() {
    assert_eq!(host_of("http://10.0.0.7:80/api"), "10.0.0.7");
    assert_eq!(host_of("100.64.0.1:9471"), "100.64.0.1");
    assert_eq!(host_of("seed.local"), "seed.local");
    assert_eq!(host_of(""), "");
}
