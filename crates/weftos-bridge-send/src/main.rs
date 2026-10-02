//! weft-bridge-send (COG-007): on a WeftOS node, poll one of our cog's export `/status` and
//! forward a compact reading to the `bridge` cog on a Cognitum Seed, so sensing done on this
//! board joins the Seed's memory.
//!
//!   weft-bridge-send --from http://127.0.0.1:8047 --to http://<seed>:8048 --source pi5 --interval 1
//!
//! It reads the cog's `/status`, takes the 8-float `vector` and a short set of scalar metrics,
//! and `POST`s `{source, cog, ts_ms, vector, metrics}` to the bridge's `/ingest`. std only, so
//! it builds on any ARM board without a TLS stack (the links are plain HTTP on a trusted LAN).

use serde_json::{Map, Value};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

struct Opts {
    from: String,
    to: String,
    source: String,
    cog: Option<String>,
    token: Option<String>,
    interval: u64,
    once: bool,
}

fn arg<'a>(a: &'a [String], f: &str) -> Option<&'a str> {
    a.iter().position(|x| x == f).and_then(|i| a.get(i + 1)).map(String::as_str)
}

fn usage() -> ! {
    eprintln!(
        "usage: weft-bridge-send --from <cog export url> --to <bridge url> --source <node name>\n\
         \t[--cog <id>] [--token <t>] [--interval <s>] [--once]\n\
         example: weft-bridge-send --from http://127.0.0.1:8047 --to http://169.254.42.1:8048 --source pi5 --interval 1"
    );
    std::process::exit(2);
}

fn parse_opts(a: &[String]) -> Opts {
    let (Some(from), Some(to), Some(source)) = (arg(a, "--from"), arg(a, "--to"), arg(a, "--source")) else {
        usage();
    };
    Opts {
        from: from.trim_end_matches('/').to_string(),
        to: to.trim_end_matches('/').to_string(),
        source: source.to_string(),
        cog: arg(a, "--cog").map(String::from),
        token: arg(a, "--token").map(String::from),
        interval: arg(a, "--interval").and_then(|v| v.parse().ok()).unwrap_or(1).clamp(1, 3600),
        once: a.iter().any(|x| x == "--once"),
    }
}

fn unix_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

/// Splits `http://host:port/path` into (host:port, "/path").
fn split_url(url: &str) -> Result<(String, String), String> {
    let rest = url.strip_prefix("http://").ok_or_else(|| format!("only http:// urls are supported: {url}"))?;
    match rest.split_once('/') {
        Some((hp, p)) => Ok((hp.to_string(), format!("/{p}"))),
        None => Ok((rest.to_string(), "/".to_string())),
    }
}

/// Minimal HTTP/1.0 request; returns the response body. The server sends Content-Length, so the
/// read stops there rather than waiting for EOF on a kept-open socket.
fn http(method: &str, url: &str, body: Option<&[u8]>, token: Option<&str>) -> Result<Vec<u8>, String> {
    let (host, path) = split_url(url)?;
    let mut conn = TcpStream::connect(&host).map_err(|e| format!("connect {host}: {e}"))?;
    conn.set_read_timeout(Some(Duration::from_secs(5))).ok();
    conn.set_write_timeout(Some(Duration::from_secs(5))).ok();
    let mut req = format!("{method} {path} HTTP/1.0\r\nHost: {host}\r\nConnection: close\r\n");
    if let Some(t) = token {
        req.push_str(&format!("X-Bridge-Token: {t}\r\n"));
    }
    if let Some(b) = body {
        req.push_str(&format!("Content-Type: application/json\r\nContent-Length: {}\r\n", b.len()));
    }
    req.push_str("\r\n");
    conn.write_all(req.as_bytes()).map_err(|e| format!("write: {e}"))?;
    if let Some(b) = body {
        conn.write_all(b).map_err(|e| format!("body: {e}"))?;
    }
    let mut resp = Vec::new();
    conn.read_to_end(&mut resp).map_err(|e| format!("read: {e}"))?;
    let split = resp.windows(4).position(|w| w == b"\r\n\r\n").ok_or("no header/body split")?;
    let status_line = String::from_utf8_lossy(&resp[..split]);
    let code = status_line.split_whitespace().nth(1).unwrap_or("");
    if !code.starts_with('2') {
        return Err(format!("HTTP {code}: {}", String::from_utf8_lossy(&resp[split + 4..]).chars().take(120).collect::<String>()));
    }
    Ok(resp[split + 4..].to_vec())
}

/// Short scalar metrics from a cog's /status: top-level number/bool/short-string fields, minus
/// the big ones. Keeps the reading small and human-readable in the bridge.
fn metrics_from(status: &Value) -> Value {
    const SKIP: [&str; 9] = ["vector", "frame", "detail", "zone_noise_mm", "background_mm", "occupied_zones", "r_peaks_ms", "rr_ms", "samples_raw_v"];
    let mut m = Map::new();
    if let Some(obj) = status.as_object() {
        for (k, v) in obj {
            if SKIP.contains(&k.as_str()) {
                continue;
            }
            let keep = match v {
                Value::Number(_) | Value::Bool(_) => true,
                Value::String(s) => s.len() <= 48,
                _ => false,
            };
            if keep {
                m.insert(k.clone(), v.clone());
            }
        }
    }
    Value::Object(m)
}

fn send_once(o: &Opts) -> Result<String, String> {
    let body = http("GET", &format!("{}/status", o.from), None, None)?;
    let status: Value = serde_json::from_slice(&body).map_err(|e| format!("parse /status: {e}"))?;
    let vector = status["vector"].as_array().ok_or("the cog's /status has no 'vector' (needs cog 0.1.1+)")?;
    if vector.is_empty() || vector.len() > 8 {
        return Err(format!("vector has {} values, want 1-8", vector.len()));
    }
    let cog = o.cog.clone().or_else(|| status["cog"].as_str().map(String::from)).unwrap_or_else(|| "sensor".into());
    let reading = serde_json::json!({
        "source": o.source,
        "cog": cog,
        "ts_ms": unix_ms(),
        "vector": vector,
        "metrics": metrics_from(&status),
    });
    let resp = http("POST", &format!("{}/ingest", o.to), Some(reading.to_string().as_bytes()), o.token.as_deref())?;
    let v: Value = serde_json::from_slice(&resp).unwrap_or(Value::Null);
    if v["ok"].as_bool() == Some(true) {
        Ok(v["stored"].as_str().unwrap_or("stored").to_string())
    } else {
        Err(v["error"].as_str().unwrap_or("rejected").to_string())
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        usage();
    }
    let o = parse_opts(&args);
    eprintln!("[weft-bridge-send] {} -> {} as source '{}', every {}s", o.from, o.to, o.source, o.interval);
    let mut ok = 0u64;
    let mut fail = 0u64;
    loop {
        match send_once(&o) {
            Ok(msg) => {
                ok += 1;
                if ok == 1 || ok.is_multiple_of(30) {
                    eprintln!("[weft-bridge-send] {msg} ({ok} sent, {fail} failed)");
                }
            }
            Err(e) => {
                fail += 1;
                if fail <= 3 || fail.is_multiple_of(30) {
                    eprintln!("[weft-bridge-send] {e} ({ok} sent, {fail} failed)");
                }
            }
        }
        if o.once {
            return;
        }
        std::thread::sleep(Duration::from_secs(o.interval));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_split_into_host_and_path() {
        assert_eq!(split_url("http://h:8048/ingest").unwrap(), ("h:8048".into(), "/ingest".into()));
        assert_eq!(split_url("http://h:80").unwrap(), ("h:80".into(), "/".into()));
        assert!(split_url("https://h/x").is_err());
    }

    #[test]
    fn metrics_keep_scalars_and_drop_the_big_fields() {
        let s = serde_json::json!({
            "status": "ok", "cog": "sen0628-tof", "nearest": {"mm": 1193}, "valid_pct": 98.0,
            "presence": true, "vector": [1,2,3], "frame": {"mm": [1,2,3]}, "mode": "8x8"
        });
        let m = metrics_from(&s);
        assert_eq!(m["status"], "ok");
        assert_eq!(m["valid_pct"], 98.0);
        assert_eq!(m["presence"], true);
        assert_eq!(m["mode"], "8x8");
        assert!(m.get("vector").is_none() && m.get("frame").is_none() && m.get("nearest").is_none());
    }

    #[test]
    fn interval_is_clamped() {
        let o = parse_opts(&["x", "--from", "http://a/", "--to", "http://b", "--source", "n", "--interval", "99999"].iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(o.interval, 3600);
        assert_eq!(o.from, "http://a");
    }
}
