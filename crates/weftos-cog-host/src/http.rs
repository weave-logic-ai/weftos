//! Minimal std-only HTTP control API for the manager UI. CORS `*` so the egui/WASM manager can call
//! it from the browser. Routes:
//!   GET  /status | /cogs      -> {running, root, cogs:[CogStatus]}
//!   POST /cogs/<id>/start      -> enable + spawn
//!   POST /cogs/<id>/stop       -> disable + kill
//!   POST /reload               -> re-read records from disk (after an install)
//!   GET  /healthz              -> ok

use weftos_cog_host::supervise::Supervisor;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

pub fn serve(listener: TcpListener, sup: Arc<Mutex<Supervisor>>) {
    for stream in listener.incoming() {
        match stream {
            Ok(s) => {
                let sup = Arc::clone(&sup);
                // One request per connection (Connection: close); cheap control API, handle inline.
                if let Err(e) = handle(s, sup) {
                    eprintln!("[cog-host] request error: {e}");
                }
            }
            Err(e) => eprintln!("[cog-host] accept: {e}"),
        }
    }
}

fn handle(mut s: TcpStream, sup: Arc<Mutex<Supervisor>>) -> std::io::Result<()> {
    let mut buf = [0u8; 4096];
    let n = s.read(&mut buf)?;
    let req = String::from_utf8_lossy(&buf[..n]);
    let mut line = req.lines().next().unwrap_or("").split_whitespace();
    let method = line.next().unwrap_or("");
    let path = line.next().unwrap_or("/");

    let (code, body) = route(method, path, &sup);
    let payload = body.into_bytes();
    let resp = format!(
        "HTTP/1.1 {code}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nConnection: close\r\n\r\n",
        payload.len()
    );
    s.write_all(resp.as_bytes())?;
    s.write_all(&payload)?;
    s.flush()
}

fn route(method: &str, path: &str, sup: &Arc<Mutex<Supervisor>>) -> (&'static str, String) {
    if method == "OPTIONS" {
        return ("204 No Content", String::new());
    }
    let parts: Vec<&str> = path.trim_matches('/').split('/').filter(|p| !p.is_empty()).collect();
    match (method, parts.as_slice()) {
        ("GET", ["healthz"]) => ("200 OK", r#"{"ok":true}"#.to_string()),
        ("GET", ["status"]) | ("GET", ["cogs"]) => {
            let sup = sup.lock().unwrap();
            let cogs = sup.status();
            let obj = serde_json::json!({
                "ok": true,
                "root": sup.root.display().to_string(),
                "running": sup.running_count(),
                "cogs": cogs,
            });
            ("200 OK", obj.to_string())
        }
        ("POST", ["reload"]) => {
            sup.lock().unwrap().reload();
            ("200 OK", r#"{"ok":true}"#.to_string())
        }
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
