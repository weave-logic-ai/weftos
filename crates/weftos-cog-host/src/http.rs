//! Minimal std-only HTTP control API for the manager UI. CORS `*` so the egui/WASM manager can call
//! it from the browser. Routes:
//!   GET  /status | /cogs      -> {running, root, cogs:[CogStatus]}
//!   POST /cogs/<id>/start|stop -> enable/disable + spawn/kill
//!   POST /install              -> verify (Ed25519/sha256) + write + reload  (body: InstallReq JSON)
//!   POST /reload               -> re-read records from disk
//!   GET  /healthz              -> ok
//!   GET  /hw/usb               -> USB inventory vs baseline + id-table labels (serials redacted)
//!   POST /hw/usb/baseline      -> accept the current scan (or body {keys:[..]}) as known
//!   POST /hw/usb/identify      -> body {key}; ask the configured agent what the device is

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use weftos_cog_host::fleet::Heartbeat;
use weftos_cog_host::supervise::Supervisor;
use weftos_cog_host::{install, network, usb, usb_identify, InstallReq};
use weftos_cog_market::usb::UsbIdTable;

const MAX_BODY: usize = 32 * 1024 * 1024; // 32 MB cap on an upload

pub fn serve(listener: TcpListener, sup: Arc<Mutex<Supervisor>>) {
    for stream in listener.incoming() {
        match stream {
            Ok(s) => {
                let sup = Arc::clone(&sup);
                // One thread per connection: /hw/usb/identify blocks on an agent for up to 90 s and
                // must not stall lifecycle/status requests.
                std::thread::spawn(move || {
                    if let Err(e) = handle(s, sup) {
                        eprintln!("[cog-host] request error: {e}");
                    }
                });
            }
            Err(e) => eprintln!("[cog-host] accept: {e}"),
        }
    }
}

fn handle(mut s: TcpStream, sup: Arc<Mutex<Supervisor>>) -> std::io::Result<()> {
    let peer_ip = s.peer_addr().ok().map(|a| a.ip().to_string());
    s.set_read_timeout(Some(Duration::from_secs(15))).ok();
    let mut buf: Vec<u8> = Vec::new();
    let mut tmp = [0u8; 16384];
    let mut head_end = None;
    let mut content_len = 0usize;
    // Read until headers + the full declared body are present (or the peer closes / cap hit).
    loop {
        if head_end.is_none() && let Some(p) = find(&buf, b"\r\n\r\n") {
            head_end = Some(p);
            content_len = content_length(&buf[..p]);
        }
        if let Some(p) = head_end && buf.len() >= p + 4 + content_len {
            break;
        }
        if buf.len() > MAX_BODY {
            break;
        }
        let n = s.read(&mut tmp)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
    }

    let he = head_end.unwrap_or(buf.len());
    let head = String::from_utf8_lossy(&buf[..he]);
    let mut first = head.lines().next().unwrap_or("").split_whitespace();
    let method = first.next().unwrap_or("");
    let path = first.next().unwrap_or("/");
    let body: &[u8] = if he + 4 <= buf.len() { &buf[he + 4..] } else { &[] };

    let (code, payload) = route(method, path, body, peer_ip, &sup);
    let bytes = payload.into_bytes();
    let resp = format!(
        "HTTP/1.1 {code}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: content-type\r\nConnection: close\r\n\r\n",
        bytes.len()
    );
    s.write_all(resp.as_bytes())?;
    s.write_all(&bytes)?;
    s.flush()
}

fn route(method: &str, path: &str, body: &[u8], peer_ip: Option<String>, sup: &Arc<Mutex<Supervisor>>) -> (&'static str, String) {
    if method == "OPTIONS" {
        return ("204 No Content", String::new());
    }
    let parts: Vec<&str> = path.trim_matches('/').split('/').filter(|p| !p.is_empty()).collect();
    match (method, parts.as_slice()) {
        ("GET", ["healthz"]) => ("200 OK", r#"{"ok":true}"#.to_string()),
        ("GET", ["network"]) => {
            let mut snap = weftos_cog_host::network::snapshot();
            snap["fleet"] = sup.lock().unwrap().fleet.roster();
            ("200 OK", snap.to_string())
        }
        ("POST", ["fleet", "heartbeat"]) => match serde_json::from_slice::<Heartbeat>(body) {
            Ok(h) => {
                sup.lock().unwrap().fleet.heartbeat(&h, peer_ip);
                ("200 OK", serde_json::json!({"ok":true,"id":h.id}).to_string())
            }
            Err(e) => ("400 Bad Request", serde_json::json!({"ok":false,"error":format!("bad heartbeat: {e}")}).to_string()),
        },
        ("GET", ["status"]) | ("GET", ["cogs"]) => {
            let sup = sup.lock().unwrap();
            let obj = serde_json::json!({
                "ok": true,
                "root": sup.root.display().to_string(),
                "running": sup.running_count(),
                "cogs": sup.status(),
            });
            ("200 OK", obj.to_string())
        }
        ("POST", ["reload"]) => {
            sup.lock().unwrap().reload();
            ("200 OK", r#"{"ok":true}"#.to_string())
        }
        ("POST", ["install"]) => install_route(body, sup),
        ("GET", ["hw", "usb"]) => hw_usb_get(sup),
        ("POST", ["hw", "usb", "baseline"]) => hw_usb_baseline(body, sup),
        ("POST", ["hw", "usb", "identify"]) => hw_usb_identify(body),
        ("POST", ["cogs", id, action @ ("start" | "stop")]) => {
            let mut sup = sup.lock().unwrap();
            let res = if *action == "start" { sup.start(id) } else { sup.stop(id) };
            match res {
                Ok(()) => ("200 OK", serde_json::json!({"ok":true,"id":id,"action":action}).to_string()),
                Err(e) => ("400 Bad Request", serde_json::json!({"ok":false,"error":e}).to_string()),
            }
        }
        _ => ("404 Not Found", r#"{"ok":false,"error":"not found"}"#.to_string()),
    }
}

fn json_resp(code: &'static str, v: serde_json::Value) -> (&'static str, String) {
    (code, v.to_string())
}

fn hw_usb_get(sup: &Arc<Mutex<Supervisor>>) -> (&'static str, String) {
    let root = sup.lock().unwrap().root.clone();
    let rep = usb::report(&root, &usb::scan(), &UsbIdTable::bundled(), &network::node_name(), usb::now_secs());
    json_resp("200 OK", rep)
}

fn hw_usb_baseline(body: &[u8], sup: &Arc<Mutex<Supervisor>>) -> (&'static str, String) {
    #[derive(serde::Deserialize, Default)]
    struct Req {
        keys: Option<Vec<String>>,
    }
    let req: Req = if body.iter().all(u8::is_ascii_whitespace) {
        Req::default()
    } else {
        match serde_json::from_slice(body) {
            Ok(r) => r,
            Err(e) => return json_resp("400 Bad Request", serde_json::json!({"ok":false,"error":format!("bad baseline request: {e}")})),
        }
    };
    let root = sup.lock().unwrap().root.clone();
    match usb::save_baseline(&root, &usb::scan(), req.keys.as_deref(), usb::now_secs()) {
        Ok(n) => json_resp("200 OK", serde_json::json!({"ok":true,"devices":n})),
        Err(e) => json_resp("500 Internal Server Error", serde_json::json!({"ok":false,"error":format!("write baseline: {e}")})),
    }
}

fn hw_usb_identify(body: &[u8]) -> (&'static str, String) {
    #[derive(serde::Deserialize)]
    struct Req {
        key: String,
    }
    let Ok(req) = serde_json::from_slice::<Req>(body) else {
        return json_resp("400 Bad Request", serde_json::json!({"ok":false,"error":"body must be {\"key\": ...}"}));
    };
    let Some(dev) = usb::scan().devices.into_iter().find(|d| d.key == req.key) else {
        return json_resp("404 Not Found", serde_json::json!({"ok":false,"error":"device not attached (rescan)"}));
    };
    let (ok, v) = usb_identify::identify(&dev, &UsbIdTable::bundled());
    json_resp(if ok { "200 OK" } else { "503 Service Unavailable" }, v)
}

fn install_route(body: &[u8], sup: &Arc<Mutex<Supervisor>>) -> (&'static str, String) {
    let req: InstallReq = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => return ("400 Bad Request", serde_json::json!({"ok":false,"error":format!("bad install request: {e}")}).to_string()),
    };
    let mut sup = sup.lock().unwrap();
    match install(&sup.root.clone(), &req) {
        Ok(rec) => {
            sup.reload();
            if rec.enabled {
                let _ = sup.start(&rec.id);
            }
            ("200 OK", serde_json::json!({"ok":true,"id":rec.id,"signed":rec.signed,"version":rec.version}).to_string())
        }
        Err(e) => ("400 Bad Request", serde_json::json!({"ok":false,"error":e}).to_string()),
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn content_length(head: &[u8]) -> usize {
    let s = String::from_utf8_lossy(head);
    for line in s.lines() {
        if let Some(v) = line.strip_prefix("Content-Length:").or_else(|| line.strip_prefix("content-length:")) {
            return v.trim().parse().unwrap_or(0);
        }
    }
    0
}
