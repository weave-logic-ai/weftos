//! Minimal std-only HTTP control API for the manager UI. CORS `*` so the egui/WASM manager can call
//! it from the browser. Routes:
//!   GET  /status | /cogs      -> {running, root, cogs:[CogStatus]}
//!   POST /cogs/<id>/start|stop -> enable/disable + spawn/kill
//!   POST /install              -> verify (Ed25519/sha256) + write + reload  (body: InstallReq JSON)
//!   POST /reload               -> re-read records from disk
//!   GET  /healthz              -> ok

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use weftos_cog_host::supervise::Supervisor;
use weftos_cog_host::{install, InstallReq};

const MAX_BODY: usize = 32 * 1024 * 1024; // 32 MB cap on an upload

pub fn serve(listener: TcpListener, sup: Arc<Mutex<Supervisor>>) {
    for stream in listener.incoming() {
        match stream {
            Ok(s) => {
                let sup = Arc::clone(&sup);
                if let Err(e) = handle(s, sup) {
                    eprintln!("[cog-host] request error: {e}");
                }
            }
            Err(e) => eprintln!("[cog-host] accept: {e}"),
        }
    }
}

fn handle(mut s: TcpStream, sup: Arc<Mutex<Supervisor>>) -> std::io::Result<()> {
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

    let (code, payload) = route(method, path, body, &sup);
    let bytes = payload.into_bytes();
    let resp = format!(
        "HTTP/1.1 {code}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: content-type\r\nConnection: close\r\n\r\n",
        bytes.len()
    );
    s.write_all(resp.as_bytes())?;
    s.write_all(&bytes)?;
    s.flush()
}

fn route(method: &str, path: &str, body: &[u8], sup: &Arc<Mutex<Supervisor>>) -> (&'static str, String) {
    if method == "OPTIONS" {
        return ("204 No Content", String::new());
    }
    let parts: Vec<&str> = path.trim_matches('/').split('/').filter(|p| !p.is_empty()).collect();
    match (method, parts.as_slice()) {
        ("GET", ["healthz"]) => ("200 OK", r#"{"ok":true}"#.to_string()),
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
        ("GET", ["network"]) => ("200 OK", weftos_cog_host::network::snapshot().to_string()),
        ("POST", ["reload"]) => {
            sup.lock().unwrap().reload();
            ("200 OK", r#"{"ok":true}"#.to_string())
        }
        ("POST", ["install"]) => install_route(body, sup),
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
