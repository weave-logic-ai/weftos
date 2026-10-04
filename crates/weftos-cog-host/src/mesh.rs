//! Mesh-wide cog view for the console (`GET /mesh/cogs`, host token required): which nodes run
//! which cogs, at what version and state.
//!
//! The console is a browser-capable client and must not poll every host itself, so the host the
//! console is connected to does the fan-out: it reads its own supervisor and asks each online
//! tailnet peer's `/status` (the peer's own cog-host, read-only, GET). Only literal tailnet
//! addresses are dialled, every peer has one overall deadline, concurrent requests share one
//! fan-out, and each peer's rows are re-read through the typed [`CogStatus`] with string and count
//! caps, so a hostile peer cannot hand the console arbitrary JSON. A peer that is not a cog host, or
//! is slow, is reported as unreachable with the reason; it never fails the answer. With no
//! reachable peer the answer says `this_host_only`. The daemon's placement layer (`workload.list`)
//! is not consulted: this crate has no daemon link.

use crate::supervise::CogStatus;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use weftos_cog_market::net::tailnet_ip;

const PEER_PORT_DEFAULT: u16 = 9480;
/// One deadline for the whole exchange with a peer (connect + write + read), not per read.
const PEER_DEADLINE: Duration = Duration::from_millis(2500);
const CACHE_TTL: Duration = Duration::from_secs(5);
/// Most peers asked per request (a tailnet can be large; the console only needs cog hosts).
pub const MAX_PEERS: usize = 16;
/// Most cog rows kept per peer.
pub const MAX_COGS: usize = 64;
/// Largest `/status` body accepted from a peer.
const BODY_CAP: usize = 256 * 1024;
const FIELD_CAP: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Peer {
    pub name: String,
    pub ip: String,
}

fn clean(s: &str, cap: usize) -> String {
    s.chars().filter(|c| !c.is_control()).take(cap).collect()
}

/// Online, non-self tailnet peers with a tailnet address, from the `tailscale` block of the
/// network snapshot. Anything outside `100.64.0.0/10` / `fd7a:115c:a1e0::/48` is dropped.
pub fn peer_targets(tailscale: &Value) -> Vec<Peer> {
    let mut seen = std::collections::BTreeSet::new();
    tailscale
        .get("peers")
        .and_then(|p| p.as_array())
        .into_iter()
        .flatten()
        .filter(|p| p.get("online").and_then(|v| v.as_bool()).unwrap_or(false) && !p.get("self").and_then(|v| v.as_bool()).unwrap_or(false))
        .filter_map(|p| {
            let ip = tailnet_ip(p.get("ip").and_then(|v| v.as_str())?)?.to_string();
            let name = clean(p.get("name").and_then(|v| v.as_str()).unwrap_or(""), FIELD_CAP);
            seen.insert(ip.clone()).then_some(Peer { name, ip })
        })
        .take(MAX_PEERS)
        .collect()
}

/// One peer's rows, cleaned: strings capped and stripped of control characters, ports and counts
/// bounded. Unknown fields never survive the typed round trip.
fn sanitize(mut c: CogStatus) -> CogStatus {
    c.id = clean(&c.id, 64);
    c.version = clean(&c.version, 32);
    c.source = clean(&c.source, 16);
    c.last_exit = c.last_exit.map(|e| clean(&e, 200));
    c.licence_refusal = c.licence_refusal.map(|e| clean(&e, 64));
    c.licence_grant = None; // a peer's grant id is not ours to show
    c.export_ports.truncate(8);
    c
}

/// Assemble the answer from this host's cogs and the peers' results. Pure.
pub fn build(self_name: &str, local_cogs: Vec<CogStatus>, peers: &[(Peer, Result<Vec<CogStatus>, String>)]) -> Value {
    let mut nodes = vec![json!({"node": clean(self_name, FIELD_CAP), "ip": "", "self": true, "reachable": true, "cogs": local_cogs})];
    let mut reachable = 0;
    for (p, res) in peers {
        let name = if p.name.is_empty() { p.ip.clone() } else { p.name.clone() };
        match res {
            Ok(cogs) => {
                reachable += 1;
                let cogs: Vec<CogStatus> = cogs.iter().take(MAX_COGS).cloned().map(sanitize).collect();
                nodes.push(json!({"node": name, "ip": p.ip, "self": false, "reachable": true, "cogs": cogs}));
            }
            Err(e) => nodes.push(json!({"node": name, "ip": p.ip, "self": false, "reachable": false, "error": clean(e, 200), "cogs": []})),
        }
    }
    let (scope, reason) = if reachable > 0 {
        ("mesh", format!("{reachable} peer cog-host(s) answered"))
    } else if peers.is_empty() {
        ("this_host_only", "no online tailnet peers".to_string())
    } else {
        ("this_host_only", "no tailnet peer answered as a cog-host".to_string())
    };
    json!({"ok": true, "scope": scope, "reason": reason, "self": clean(self_name, FIELD_CAP), "nodes": nodes})
}

fn peer_port() -> u16 {
    std::env::var("WEFT_COG_HOST_PEER_PORT").ok().and_then(|v| v.parse().ok()).unwrap_or(PEER_PORT_DEFAULT)
}

fn remaining(deadline: Instant) -> Result<Duration, String> {
    let r = deadline.saturating_duration_since(Instant::now());
    if r.is_zero() { Err("timed out".into()) } else { Ok(r) }
}

/// Minimal std HTTP/1.0 GET of a peer's `/status` under one overall deadline, size-capped.
fn fetch_status(ip: &str, port: u16, deadline: Instant) -> Result<Vec<CogStatus>, String> {
    let addr: SocketAddr = format!("{ip}:{port}").parse().map_err(|e| format!("bad address: {e}"))?;
    let mut conn = TcpStream::connect_timeout(&addr, remaining(deadline)?).map_err(|e| e.to_string())?;
    conn.set_write_timeout(Some(remaining(deadline)?)).ok();
    conn.write_all(format!("GET /status HTTP/1.0\r\nHost: {ip}:{port}\r\nConnection: close\r\n\r\n").as_bytes()).map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        conn.set_read_timeout(Some(remaining(deadline)?)).ok();
        let n = conn.read(&mut chunk).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() > BODY_CAP {
            return Err("peer answer too large".into());
        }
    }
    let split = buf.windows(4).position(|w| w == b"\r\n\r\n").ok_or("no header/body split")?;
    if !buf.starts_with(b"HTTP/1.0 200") && !buf.starts_with(b"HTTP/1.1 200") {
        return Err("peer did not answer 200".into());
    }
    let v: Value = serde_json::from_slice(&buf[split + 4..]).map_err(|e| format!("bad json: {e}"))?;
    let rows = v.get("cogs").and_then(|c| c.as_array()).ok_or("not a cog-host /status answer")?;
    Ok(rows.iter().take(MAX_COGS).filter_map(|r| serde_json::from_value::<CogStatus>(r.clone()).ok()).filter(|c| !c.id.is_empty()).collect())
}

type PeerResults = Vec<(Peer, Result<Vec<CogStatus>, String>)>;
type FanCache = Mutex<Option<(Instant, PeerResults)>>;
static CACHE: FanCache = Mutex::new(None);
/// Held for a whole fan-out: concurrent requests wait for it and then read the cache, so a burst of
/// console refreshes makes one round of peer requests, not one per request.
static FLIGHT: Mutex<()> = Mutex::new(());

fn cached() -> Option<PeerResults> {
    CACHE.lock().unwrap().as_ref().filter(|(at, _)| at.elapsed() < CACHE_TTL).map(|(_, r)| r.clone())
}

/// Ask every peer in parallel, each under one deadline (cached, single-flight).
pub fn fan_out(peers: &[Peer]) -> PeerResults {
    if let Some(r) = cached() {
        return r;
    }
    let _flight = FLIGHT.lock().unwrap();
    if let Some(r) = cached() {
        return r; // another request finished the fan-out while this one waited
    }
    let port = peer_port();
    let deadline = Instant::now() + PEER_DEADLINE;
    let handles: Vec<_> = peers
        .iter()
        .cloned()
        .map(|p| {
            std::thread::spawn(move || {
                let r = fetch_status(&p.ip, port, deadline);
                (p, r)
            })
        })
        .collect();
    let out: PeerResults = handles.into_iter().filter_map(|h| h.join().ok()).collect();
    *CACHE.lock().unwrap() = Some((Instant::now(), out.clone()));
    out
}

/// `GET /mesh/cogs` body for this host.
pub fn snapshot(local_cogs: Vec<CogStatus>) -> Value {
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
            {"name":"lan","ip":"192.168.1.9","online":true,"self":false},
            {"name":"pub","ip":"8.8.8.8","online":true,"self":false},
            {"name":"v6","ip":"fd7a:115c:a1e0::7","online":true,"self":false},
            {"name":"dup","ip":"100.64.0.3","online":true,"self":false}
        ]})
    }

    fn cog(id: &str) -> CogStatus {
        CogStatus { id: id.into(), version: "0.1.0".into(), running: true, enabled: true, ..Default::default() }
    }

    #[test]
    fn only_online_non_self_tailnet_ips_once() {
        let p = peer_targets(&ts());
        assert_eq!(p, vec![Peer { name: "pi5".into(), ip: "100.64.0.3".into() }, Peer { name: "v6".into(), ip: "fd7a:115c:a1e0::7".into() }]);
        assert!(peer_targets(&json!({"available": false})).is_empty());
    }

    #[test]
    fn peers_answering_make_it_a_mesh_view() {
        let peer = Peer { name: "pi5".into(), ip: "100.64.0.3".into() };
        let v = build("cog0", vec![cog("rd-03e")], &[(peer, Ok(vec![cog("ld2450-radar")]))]);
        assert_eq!(v["scope"], "mesh");
        assert_eq!(v["nodes"].as_array().unwrap().len(), 2);
        assert_eq!(v["nodes"][1]["node"], "pi5");
        assert_eq!(v["nodes"][1]["cogs"][0]["id"], "ld2450-radar");
        assert_eq!(v["nodes"][0]["self"], true);
    }

    #[test]
    fn a_hostile_peer_is_cut_down_to_typed_capped_rows() {
        let peer = Peer { name: "x".into(), ip: "100.64.0.9".into() };
        let mut big: Vec<CogStatus> = (0..200).map(|i| cog(&format!("c{i}"))).collect();
        big[0].id = format!("a{}\u{7}b", "x".repeat(500));
        big[0].last_exit = Some("y".repeat(5000));
        big[0].licence_grant = Some("secret-grant".into());
        big[0].export_ports = (1..=100).collect();
        let v = build("cog0", vec![], &[(peer, Ok(big))]);
        let cogs = v["nodes"][1]["cogs"].as_array().unwrap();
        assert_eq!(cogs.len(), MAX_COGS);
        assert!(cogs[0]["id"].as_str().unwrap().chars().count() <= 64 && !cogs[0]["id"].as_str().unwrap().contains('\u{7}'));
        assert!(cogs[0]["last_exit"].as_str().unwrap().len() <= 200);
        assert!(cogs[0].get("licence_grant").is_none());
        assert!(cogs[0]["export_ports"].as_array().unwrap().len() <= 8);
    }

    #[test]
    fn no_answering_peer_is_labelled_this_host_only() {
        let peer = Peer { name: "".into(), ip: "100.64.0.9".into() };
        let v = build("cog0", vec![], &[(peer, Err("connection refused".into()))]);
        assert_eq!(v["scope"], "this_host_only");
        assert_eq!(v["nodes"][1]["node"], "100.64.0.9");
        assert_eq!(v["nodes"][1]["reachable"], false);
        assert!(v["nodes"][1]["error"].as_str().unwrap().contains("refused"));
        let v = build("cog0", vec![], &[]);
        assert_eq!(v["reason"], "no online tailnet peers");
    }

    fn serve_once(body: &'static str, delay: Option<Duration>) -> u16 {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut b = [0u8; 512];
            let _ = s.read(&mut b);
            if let Some(d) = delay {
                std::thread::sleep(d);
            }
            let _ = s.write_all(format!("HTTP/1.0 200 OK\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes());
        });
        port
    }

    #[test]
    fn fetch_status_reads_a_local_peer_as_typed_rows() {
        let port = serve_once(r#"{"ok":true,"cogs":[{"id":"x","extra":"dropped"},{"nope":1}]}"#, None);
        let rows = fetch_status("127.0.0.1", port, Instant::now() + Duration::from_secs(2)).unwrap();
        assert_eq!(rows.len(), 1, "a row with no id is dropped");
        assert_eq!(rows[0].id, "x");
        assert!(fetch_status("127.0.0.1", 1, Instant::now() + Duration::from_secs(1)).is_err());
    }

    #[test]
    fn a_slow_peer_hits_the_overall_deadline() {
        let port = serve_once(r#"{"cogs":[]}"#, Some(Duration::from_secs(3)));
        let t = Instant::now();
        let r = fetch_status("127.0.0.1", port, Instant::now() + Duration::from_millis(300));
        assert!(r.is_err());
        assert!(t.elapsed() < Duration::from_secs(2), "the deadline, not the peer, ends the wait");
    }

    #[test]
    fn a_trickling_peer_cannot_outlast_the_deadline() {
        // sends one byte every 100 ms: each read succeeds, so only an overall deadline stops it
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut b = [0u8; 512];
            let _ = s.read(&mut b);
            for _ in 0..100 {
                if s.write_all(b"x").is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        });
        let t = Instant::now();
        assert!(fetch_status("127.0.0.1", port, Instant::now() + Duration::from_millis(500)).is_err());
        assert!(t.elapsed() < Duration::from_secs(2));
    }
}
