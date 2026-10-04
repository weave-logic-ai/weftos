//! Mesh-wide cog view for the console (`GET /mesh/cogs`): which nodes run which cogs, at what
//! version and state, with each cog's output-log size and age.
//!
//! The console is a browser-capable client and must not poll every host itself, so the host the
//! console is connected to does the fan-out: it reads its own supervisor and asks each online
//! tailnet peer's `/status` (the peer's own cog-host, read-only, GET). A peer that is not a cog
//! host, or is slow, is reported as unreachable with the reason; it never fails the answer. With no
//! reachable peer the answer says `this_host_only`, so the console can label it plainly. The
//! daemon's placement layer (`workload.list`) is not consulted here: this crate has no daemon link.

use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const PEER_PORT_DEFAULT: u16 = 9480;
const PEER_TIMEOUT: Duration = Duration::from_millis(1500);
const CACHE_TTL: Duration = Duration::from_secs(5);
/// Most peers asked per request (a tailnet can be large; the console only needs cog hosts).
pub const MAX_PEERS: usize = 16;
/// Largest `/status` body accepted from a peer.
const BODY_CAP: usize = 256 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Peer {
    pub name: String,
    pub ip: String,
}

/// Online, non-self tailnet peers from the `tailscale` block of the network snapshot.
pub fn peer_targets(tailscale: &Value) -> Vec<Peer> {
    let mut seen = std::collections::BTreeSet::new();
    tailscale
        .get("peers")
        .and_then(|p| p.as_array())
        .into_iter()
        .flatten()
        .filter(|p| p.get("online").and_then(|v| v.as_bool()).unwrap_or(false) && !p.get("self").and_then(|v| v.as_bool()).unwrap_or(false))
        .filter_map(|p| {
            let ip = p.get("ip").and_then(|v| v.as_str())?.to_string();
            let name = p.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
            // only a literal IP is ever dialled, never a name taken from the peer list
            (ip.parse::<std::net::IpAddr>().is_ok() && seen.insert(ip.clone())).then_some(Peer { name, ip })
        })
        .take(MAX_PEERS)
        .collect()
}

/// Assemble the answer from this host's cogs and the peers' results. Pure.
pub fn build(self_name: &str, local_cogs: Value, peers: &[(Peer, Result<Value, String>)]) -> Value {
    let mut nodes = vec![json!({"node": self_name, "ip": "", "self": true, "reachable": true, "cogs": local_cogs})];
    let mut reachable = 0;
    for (p, res) in peers {
        let name = if p.name.is_empty() { p.ip.clone() } else { p.name.clone() };
        match res {
            Ok(v) if v.get("cogs").is_some_and(|c| c.is_array()) => {
                reachable += 1;
                nodes.push(json!({"node": name, "ip": p.ip, "self": false, "reachable": true, "cogs": v["cogs"]}));
            }
            Ok(_) => nodes.push(json!({"node": name, "ip": p.ip, "self": false, "reachable": false, "error": "not a cog-host /status answer", "cogs": []})),
            Err(e) => nodes.push(json!({"node": name, "ip": p.ip, "self": false, "reachable": false, "error": e, "cogs": []})),
        }
    }
    let (scope, reason) = if reachable > 0 {
        ("mesh", format!("{reachable} peer cog-host(s) answered"))
    } else if peers.is_empty() {
        ("this_host_only", "no online tailnet peers".to_string())
    } else {
        ("this_host_only", "no tailnet peer answered as a cog-host".to_string())
    };
    json!({"ok": true, "scope": scope, "reason": reason, "self": self_name, "nodes": nodes})
}

fn peer_port() -> u16 {
    std::env::var("WEFT_COG_HOST_PEER_PORT").ok().and_then(|v| v.parse().ok()).unwrap_or(PEER_PORT_DEFAULT)
}

/// Minimal std HTTP/1.0 GET of a peer's `/status`, bounded in time and size.
fn fetch_status(ip: &str, port: u16) -> Result<Value, String> {
    let addr: SocketAddr = format!("{ip}:{port}").parse().map_err(|e| format!("bad address: {e}"))?;
    let mut conn = TcpStream::connect_timeout(&addr, PEER_TIMEOUT).map_err(|e| e.to_string())?;
    conn.set_read_timeout(Some(PEER_TIMEOUT)).ok();
    conn.set_write_timeout(Some(PEER_TIMEOUT)).ok();
    conn.write_all(format!("GET /status HTTP/1.0\r\nHost: {ip}:{port}\r\nConnection: close\r\n\r\n").as_bytes()).map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    conn.take(BODY_CAP as u64).read_to_end(&mut buf).map_err(|e| e.to_string())?;
    let split = buf.windows(4).position(|w| w == b"\r\n\r\n").ok_or("no header/body split")?;
    if !buf.starts_with(b"HTTP/1.0 200") && !buf.starts_with(b"HTTP/1.1 200") {
        return Err("peer did not answer 200".into());
    }
    serde_json::from_slice(&buf[split + 4..]).map_err(|e| format!("bad json: {e}"))
}

type PeerResults = Vec<(Peer, Result<Value, String>)>;
static CACHE: Mutex<Option<(Instant, PeerResults)>> = Mutex::new(None);

/// Ask every peer in parallel (cached for a few seconds so a console refresh is not a storm).
pub fn fan_out(peers: &[Peer]) -> PeerResults {
    if let Some((at, r)) = CACHE.lock().unwrap().as_ref()
        && at.elapsed() < CACHE_TTL
    {
        return r.clone();
    }
    let port = peer_port();
    let handles: Vec<_> = peers
        .iter()
        .cloned()
        .map(|p| std::thread::spawn(move || {
            let r = fetch_status(&p.ip, port);
            (p, r)
        }))
        .collect();
    let out: PeerResults = handles.into_iter().filter_map(|h| h.join().ok()).collect();
    *CACHE.lock().unwrap() = Some((Instant::now(), out.clone()));
    out
}

/// `GET /mesh/cogs` body for this host.
pub fn snapshot(local_cogs: Value) -> Value {
    let ts = crate::network::tailscale();
    let peers = fan_out(&peer_targets(&ts));
    build(&crate::network::node_name(), local_cogs, &peers)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts() -> Value {
        json!({"available": true, "peers": [
            {"name":"cog0","ip":"100.64.0.2","online":true,"self":true},
            {"name":"pi5","ip":"100.64.0.3","online":true,"self":false},
            {"name":"off","ip":"100.64.0.4","online":false,"self":false},
            {"name":"evil","ip":"not-an-ip.example","online":true,"self":false},
            {"name":"dup","ip":"100.64.0.3","online":true,"self":false}
        ]})
    }

    #[test]
    fn only_online_non_self_literal_ips_once() {
        let p = peer_targets(&ts());
        assert_eq!(p, vec![Peer { name: "pi5".into(), ip: "100.64.0.3".into() }]);
        assert!(peer_targets(&json!({"available": false})).is_empty());
    }

    #[test]
    fn peers_answering_make_it_a_mesh_view() {
        let peer = Peer { name: "pi5".into(), ip: "100.64.0.3".into() };
        let ok = Ok(json!({"cogs": [{"id": "ld2450-radar", "version": "0.1.0", "running": true}]}));
        let v = build("cog0", json!([{"id": "rd-03e"}]), &[(peer, ok)]);
        assert_eq!(v["scope"], "mesh");
        assert_eq!(v["nodes"].as_array().unwrap().len(), 2);
        assert_eq!(v["nodes"][1]["node"], "pi5");
        assert_eq!(v["nodes"][1]["cogs"][0]["id"], "ld2450-radar");
        assert_eq!(v["nodes"][0]["self"], true);
    }

    #[test]
    fn no_answering_peer_is_labelled_this_host_only() {
        let peer = Peer { name: "".into(), ip: "100.64.0.9".into() };
        let v = build("cog0", json!([]), &[(peer.clone(), Err("connection refused".into()))]);
        assert_eq!(v["scope"], "this_host_only");
        assert_eq!(v["nodes"][1]["node"], "100.64.0.9");
        assert_eq!(v["nodes"][1]["reachable"], false);
        assert!(v["nodes"][1]["error"].as_str().unwrap().contains("refused"));
        // a 200 that is not a cog-host body is unreachable too, not a crash
        let v = build("cog0", json!([]), &[(peer, Ok(json!({"hello": 1})))]);
        assert_eq!(v["scope"], "this_host_only");
        let v = build("cog0", json!([]), &[]);
        assert_eq!(v["reason"], "no online tailnet peers");
    }

    #[test]
    fn fetch_status_reads_a_local_peer() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut b = [0u8; 512];
            let _ = s.read(&mut b);
            let body = r#"{"ok":true,"cogs":[{"id":"x"}]}"#;
            let _ = s.write_all(format!("HTTP/1.0 200 OK\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes());
        });
        let v = fetch_status("127.0.0.1", port).unwrap();
        assert_eq!(v["cogs"][0]["id"], "x");
        assert!(fetch_status("127.0.0.1", 1).is_err());
    }
}
