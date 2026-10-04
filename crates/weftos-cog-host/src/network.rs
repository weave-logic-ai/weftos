//! Fleet / mesh snapshot for the console's Network tab. The host runs on the node, so it can gather
//! what a browser can't: the Tailscale peer list (the fleet) and the Cognitum agent's mesh peers.
//! Served at `GET /network`. All best-effort — missing pieces degrade to `available: false`.

use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::Command;
use std::time::Duration;

pub fn snapshot() -> Value {
    json!({
        "node": node_name(),
        "tailscale": tailscale(),
        "cognitum_mesh": cognitum_mesh(),
    })
}

pub fn node_name() -> String {
    Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// `tailscale status --json` → the fleet on the tailnet (self + peers).
/// `tailscale status --json`, cached for a few seconds: the CLI is slow and every `/network` and
/// `/mesh/cogs` request would otherwise spawn it. The cache lock is held while refreshing, so
/// concurrent callers wait for one refresh instead of each spawning their own.
pub fn tailscale() -> Value {
    use std::sync::Mutex;
    use std::time::Instant;
    static CACHE: Mutex<Option<(Instant, Value)>> = Mutex::new(None);
    let mut c = CACHE.lock().unwrap();
    if let Some((at, v)) = c.as_ref()
        && at.elapsed() < Duration::from_secs(5)
    {
        return v.clone();
    }
    let v = tailscale_uncached();
    *c = Some((Instant::now(), v.clone()));
    v
}

fn tailscale_uncached() -> Value {
    let Ok(out) = Command::new("tailscale").args(["status", "--json"]).output() else {
        return json!({ "available": false });
    };
    if !out.status.success() {
        return json!({ "available": false });
    }
    let Ok(d) = serde_json::from_slice::<Value>(&out.stdout) else {
        return json!({ "available": false });
    };
    let mut peers = Vec::new();
    if let Some(me) = d.get("Self") {
        peers.push(peer_row(me, true));
    }
    if let Some(obj) = d.get("Peer").and_then(|p| p.as_object()) {
        for v in obj.values() {
            peers.push(peer_row(v, false));
        }
    }
    json!({ "available": true, "peers": peers })
}

fn peer_row(p: &Value, is_self: bool) -> Value {
    let text = |k: &str| p.get(k).and_then(|v| v.as_str()).unwrap_or("");
    json!({
        "name": text("HostName"),
        "ip": p.get("TailscaleIPs").and_then(|v| v.get(0)).and_then(|v| v.as_str()).unwrap_or(""),
        "os": text("OS"),
        "online": p.get("Online").and_then(|v| v.as_bool()).unwrap_or(false),
        "self": is_self,
        // Extras the console can show (all absent-safe): when tailscale last saw the peer, whether the
        // path is direct (`cur_addr`) or relayed (`relay`), and its ACL tags.
        "last_seen": text("LastSeen"),
        "cur_addr": text("CurAddr"),
        "relay": text("Relay"),
        "tags": p.get("Tags").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|t| t.as_str()).collect::<Vec<_>>()).unwrap_or_default(),
    })
}

/// The Cognitum agent's own mesh overlay peers (`GET 127.0.0.1:80/api/v1/peers`).
fn cognitum_mesh() -> Value {
    match http_get("127.0.0.1:80", "/api/v1/peers") {
        Ok(body) => serde_json::from_slice::<Value>(&body).unwrap_or_else(|_| json!({ "available": false })),
        Err(_) => json!({ "available": false }),
    }
}

/// Minimal std-only HTTP/1.0 GET (the agent is plain HTTP on :80); stops at Connection: close.
fn http_get(host: &str, path: &str) -> Result<Vec<u8>, String> {
    let mut conn = TcpStream::connect(host).map_err(|e| e.to_string())?;
    conn.set_read_timeout(Some(Duration::from_secs(4))).ok();
    conn.set_write_timeout(Some(Duration::from_secs(4))).ok();
    let req = format!("GET {path} HTTP/1.0\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    conn.write_all(req.as_bytes()).map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    conn.read_to_end(&mut buf).map_err(|e| e.to_string())?;
    let split = buf.windows(4).position(|w| w == b"\r\n\r\n").ok_or("no header/body split")?;
    Ok(buf[split + 4..].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_row_carries_last_seen_path_and_tags_and_tolerates_their_absence() {
        let full = json!({
            "HostName": "pi5", "TailscaleIPs": ["100.1.2.3"], "OS": "linux", "Online": true,
            "LastSeen": "2026-10-04T10:00:00Z", "CurAddr": "192.168.1.5:41641", "Relay": "nyc",
            "Tags": ["tag:cog", "tag:lab"]
        });
        let r = peer_row(&full, false);
        assert_eq!(r["name"], "pi5");
        assert_eq!(r["last_seen"], "2026-10-04T10:00:00Z");
        assert_eq!(r["cur_addr"], "192.168.1.5:41641");
        assert_eq!(r["relay"], "nyc");
        assert_eq!(r["tags"], json!(["tag:cog", "tag:lab"]));

        let bare = peer_row(&json!({ "HostName": "x" }), true);
        assert_eq!(bare["self"], true);
        assert_eq!((bare["last_seen"].as_str(), bare["relay"].as_str()), (Some(""), Some("")));
        assert_eq!(bare["tags"], json!([]));
    }
}
