//! One fleet list from every source the console can read (fleet manager): the daemon's
//! `fleet.snapshot` (mesh nodes), the connected cog-host's tailnet peers, Cognitum Seeds (their
//! own agent API) and edge nodes (the cog-host roster). Entries that are the same machine are
//! merged by name or address, and each keeps the list of sources that reported it, each with its
//! provenance, so the console never presents one source's claim as another's. Pure and tested.

use crate::client::Net;
use serde_json::Value;

/// What kind of thing a fleet entry is (the strongest class any source gave it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    /// A WeftOS daemon on the mesh (it is in `fleet.snapshot`).
    Node,
    /// A leaf connected to the mesh (mesh class `leaf`: an ESP32-class device with its own key).
    Leaf,
    /// A Cognitum Seed (answers the Seed agent API).
    Seed,
    /// A machine on the tailnet that is not (yet) a mesh node.
    Host,
    /// An ESP32-class edge node that checks in to a cog-host.
    Edge,
}

impl Class {
    pub fn label(self) -> &'static str {
        match self {
            Class::Node => "weftos node",
            Class::Leaf => "mesh leaf",
            Class::Seed => "seed",
            Class::Host => "tailnet host",
            Class::Edge => "edge",
        }
    }
}

/// Where a fact about an entry came from.
#[derive(Debug, Clone, PartialEq)]
pub struct Source {
    /// `snapshot`, `tailnet`, `seed`, `edge`.
    pub kind: &'static str,
    /// Provenance label for what this source says.
    pub provenance: &'static str,
}

/// One machine or device in the fleet.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// Stable key for selection: the snapshot node id, the Seed URL, `tailnet:<ip>` or `edge:<id>`.
    pub key: String,
    pub name: String,
    pub class: Class,
    /// `Some(true)` online, `Some(false)` offline, `None` not known.
    pub online: Option<bool>,
    /// Addresses seen for it (tailnet IP, LAN IP, mesh address host).
    pub addrs: Vec<String>,
    pub os: Option<String>,
    pub firmware: Option<String>,
    pub sources: Vec<Source>,
    /// The snapshot node id when it is a mesh node (opens the node detail).
    pub node_id: Option<String>,
    /// The Seed base URL when it is a Seed (opens the Seed detail).
    pub seed: Option<String>,
    pub this_host: bool,
}

/// What the console read from one Seed agent.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SeedView {
    /// Base URL the console reads, e.g. `http://<addr>`.
    pub url: String,
    pub identity: Option<Value>,
    pub status: Option<Value>,
    pub firmware: Option<Value>,
    pub thermal: Option<Value>,
    pub error: Option<String>,
    /// True when this Seed was found on the connected host, not listed by the operator.
    pub auto: bool,
}

impl SeedView {
    /// True once the agent answered `/api/v1/identity`.
    pub fn answered(&self) -> bool {
        self.identity.is_some()
    }
}

/// Name for matching: case-folded, `.local` dropped, `-` and `_` read as spaces (macOS hostnames
/// are `BigMac-The-Max.local` where tailscale says `BigMac The Max`).
fn norm(s: &str) -> String {
    let s = s.trim().to_lowercase();
    let s = s.strip_suffix(".local").unwrap_or(&s);
    s.replace(['-', '_'], " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Addresses that say nothing about which machine it is (every daemon can report them).
fn matchable(a: &str) -> bool {
    !a.is_empty() && !a.starts_with("127.") && a != "0.0.0.0" && a != "::1" && a != "localhost"
}

/// Host part of `http://host:port/...` or `host:port`.
pub fn host_of(url: &str) -> String {
    let rest = url.trim().trim_start_matches("http://").trim_start_matches("https://");
    let hp = rest.split('/').next().unwrap_or(rest);
    match hp.rsplit_once(':') {
        Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) => h.to_owned(),
        _ => hp.to_owned(),
    }
}

fn find(out: &[Entry], name: &str, addrs: &[String]) -> Option<usize> {
    let n = norm(name);
    out.iter().position(|e| {
        (!n.is_empty() && n != "localhost" && norm(&e.name) == n) || addrs.iter().any(|a| matchable(a) && e.addrs.contains(a))
    })
}

fn add_addr(e: &mut Entry, a: &str) {
    if matchable(a) && !e.addrs.iter().any(|x| x == a) {
        e.addrs.push(a.to_owned());
    }
}

/// The unified list: mesh nodes first, then Seeds, tailnet hosts and edge nodes, each group by
/// name. `seeds` should include only agents that answered or that the operator listed.
pub fn unify(snap: Option<&Value>, net: Option<&Net>, seeds: &[SeedView]) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::new();
    if let Some(s) = snap {
        for r in crate::fleet::rows(s) {
            let host = host_of(&r.address);
            out.push(Entry {
                key: r.id.clone(),
                name: r.name.clone(),
                class: if r.class.as_deref() == Some("leaf") { Class::Leaf } else { Class::Node },
                online: if r.local {
                    Some(true)
                } else if matches!(r.state.as_str(), "left" | "dead" | "failed") {
                    Some(false)
                } else {
                    r.heartbeat.as_deref().map(|h| h == "alive")
                },
                addrs: if matchable(&host) { vec![host] } else { vec![] },
                os: None,
                firmware: None,
                sources: vec![Source { kind: "snapshot", provenance: "daemon_observed" }],
                node_id: Some(r.id.clone()),
                seed: None,
                this_host: false,
            });
        }
    }
    if let Some(n) = net
        && n.tailscale.available
    {
        for p in &n.tailscale.peers {
            let src = Source { kind: "tailnet", provenance: "tailnet" };
            match find(&out, &p.name, std::slice::from_ref(&p.ip)) {
                Some(i) => {
                    let e = &mut out[i];
                    add_addr(e, &p.ip);
                    e.online = e.online.or(Some(p.online));
                    e.os = e.os.clone().or_else(|| (!p.os.is_empty()).then(|| p.os.clone()));
                    e.this_host |= p.is_self;
                    e.sources.push(src);
                }
                None => out.push(Entry {
                    key: format!("tailnet:{}", if p.ip.is_empty() { &p.name } else { &p.ip }),
                    name: if p.name.is_empty() { "(unnamed)".into() } else { p.name.clone() },
                    class: Class::Host,
                    online: Some(p.online),
                    addrs: if p.ip.is_empty() { vec![] } else { vec![p.ip.clone()] },
                    os: (!p.os.is_empty()).then(|| p.os.clone()),
                    firmware: None,
                    sources: vec![src],
                    node_id: None,
                    seed: None,
                    this_host: p.is_self,
                }),
            }
        }
    }
    for sv in seeds.iter().filter(|s| s.answered() || !s.auto) {
        let id = sv.identity.as_ref();
        let device = id.and_then(|v| v["device_id"].as_str()).map(str::to_owned);
        let host = host_of(&sv.url);
        let name = device.as_deref().map(|d| format!("seed {}", &d[..d.len().min(8)])).unwrap_or_else(|| format!("seed {host}"));
        let fw = id.and_then(|v| v["firmware_version"].as_str()).map(str::to_owned);
        let src = Source { kind: "seed", provenance: "self_reported" };
        // The Seed agent on the connected host is that host: merge onto it below by address.
        match find(&out, "", std::slice::from_ref(&host)) {
            Some(i) => {
                let e = &mut out[i];
                e.class = e.class.min(Class::Seed);
                e.seed = Some(sv.url.clone());
                e.firmware = e.firmware.clone().or(fw);
                e.sources.push(src);
            }
            None => out.push(Entry {
                key: sv.url.clone(),
                name,
                class: Class::Seed,
                online: Some(sv.answered()),
                addrs: vec![host],
                os: None,
                firmware: fw,
                sources: vec![src],
                node_id: None,
                seed: Some(sv.url.clone()),
                this_host: false,
            }),
        }
    }
    if let Some(n) = net {
        for f in &n.fleet {
            out.push(Entry {
                key: format!("edge:{}", f.id),
                name: f.id.clone(),
                class: Class::Edge,
                online: Some(f.online),
                addrs: if f.ip.is_empty() { vec![] } else { vec![f.ip.clone()] },
                os: f.chip.clone(),
                firmware: (!f.fw.is_empty()).then(|| f.fw.clone()),
                sources: vec![Source { kind: "edge", provenance: "self_reported" }],
                node_id: None,
                seed: None,
                this_host: false,
            });
        }
    }
    out.sort_by_key(|e| (e.class, !e.this_host, e.online != Some(true), norm(&e.name)));
    out
}

#[cfg(test)]
#[path = "fleet_unify_tests.rs"]
mod tests;
