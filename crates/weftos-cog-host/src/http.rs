//! Minimal std-only HTTP control API for the manager UI. Routes:
//!   GET  /status | /cogs      -> {running, root, cogs:[CogStatus]}
//!   POST /cogs/<id>/start|stop -> enable/disable + spawn/kill
//!   POST /install              -> verify (Ed25519/sha256) + write + reload  (body: InstallReq JSON)
//!   POST /reload               -> re-read records from disk
//!   GET  /licence              -> the ADR-106 licence state the start check uses (needs the token)
//!   POST /licence/records      -> verify + apply signed binding/grants/approvals/revocations
//!   GET  /healthz              -> ok
//!   /hw/*                      -> USB inventory, identify, Hardware Dex (see `hw_http`)
//!
//! Browser safety (the CLI binds loopback by default; operators may opt into wider access):
//! the `Host` header must be an allowed name
//! (DNS-rebinding guard, 421 otherwise); every POST except edge heartbeats needs
//! `Content-Type: application/json` + `X-Weft-Host: 1` (forcing a CORS preflight) **and** the host
//! bearer token, as does every `GET /hw/*`; CORS is answered only for allowlisted origins on
//! `/hw/*` and on every POST. See `auth`. `GET /status|/network|/healthz` stay open (read views)
//! and keep `*`. Limits: 32 concurrent connections (503 beyond), 20 s per request, 64 KB bodies on
//! `/hw/*`, 4 KB on `/fleet/heartbeat`, 2 MB on `/licence/records`.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use weftos_cog_host::auth::{Headers, Policy};
use weftos_cog_host::fleet::{Heartbeat, HEARTBEAT_BODY_CAP};
use weftos_cog_host::hw_http::{self, HW_BODY_CAP};
use weftos_cog_host::licence::{HostLicence, Records, MAX_IMPORT_BYTES};
use weftos_cog_host::supervise::Supervisor;
use weftos_cog_host::{install, InstallReq};

const MAX_BODY: usize = 32 * 1024 * 1024; // 32 MB cap on an upload
const MAX_CONNS: usize = 32;
const REQUEST_DEADLINE: Duration = Duration::from_secs(20);

static ACTIVE: AtomicUsize = AtomicUsize::new(0);

/// Counts a live connection; dropping it frees the slot.
struct Slot;
impl Drop for Slot {
    fn drop(&mut self) {
        ACTIVE.fetch_sub(1, Ordering::AcqRel);
    }
}

pub fn serve(listener: TcpListener, sup: Arc<Mutex<Supervisor>>, policy: Arc<Policy>, licence: Arc<HostLicence>) {
    for stream in listener.incoming() {
        match stream {
            Ok(mut s) => {
                if ACTIVE.fetch_add(1, Ordering::AcqRel) >= MAX_CONNS {
                    ACTIVE.fetch_sub(1, Ordering::AcqRel);
                    s.set_write_timeout(Some(Duration::from_secs(1))).ok();
                    let _ = respond(&mut s, "503 Service Unavailable", r#"{"ok":false,"error":"too many connections"}"#.into(), "");
                    continue;
                }
                let (sup, policy, licence) = (Arc::clone(&sup), Arc::clone(&policy), Arc::clone(&licence));
                // One thread per connection: /hw/usb/identify blocks on an agent for up to 90 s and
                // must not stall lifecycle/status requests.
                std::thread::spawn(move || {
                    let _slot = Slot;
                    if let Err(e) = handle(s, sup, &policy, &licence) {
                        eprintln!("[cog-host] request error: {e}");
                    }
                });
            }
            Err(e) => eprintln!("[cog-host] accept: {e}"),
        }
    }
}

fn parse_headers(head: &str) -> Headers {
    head.lines().skip(1).filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string())).collect()
}

fn respond(s: &mut TcpStream, code: &str, payload: String, cors: &str) -> std::io::Result<()> {
    let bytes = payload.into_bytes();
    let resp = format!("HTTP/1.1 {code}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{cors}Connection: close\r\n\r\n", bytes.len());
    s.write_all(resp.as_bytes())?;
    s.write_all(&bytes)?;
    s.flush()
}

/// Reads that describe a node's state (not just its identity) and so need the host token, like
/// `/hw/*`: the mesh view and a cog's last output line (health and reasons can reveal a person's
/// state, e.g. an ECG's lead-off or rhythm quality). They get no wildcard CORS either.
fn token_guarded_read(method: &str, path: &str) -> bool {
    if method != "GET" {
        return false;
    }
    let p = path.split('?').next().unwrap_or("");
    let parts: Vec<&str> = p.trim_matches('/').split('/').filter(|x| !x.is_empty()).collect();
    matches!(parts.as_slice(), ["mesh", ..] | ["cogs", _, "last"])
}

/// CORS headers for this request. Browsers get them only from an allowlisted origin on `/hw/*`
/// `/licence*` and for POSTs; the legacy GET routes (and edge heartbeats) keep `*`.
fn cors_headers(method: &str, path: &str, h: &Headers, policy: &Policy) -> String {
    let open = !path.starts_with("/hw/") && !path.starts_with("/licence") && !token_guarded_read(method, path) && (method == "GET" || path == "/fleet/heartbeat");
    if open {
        return "Access-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: content-type\r\n".into();
    }
    match h.get("origin") {
        Some(o) if policy.origin_allowed(o, h.get("host").map(String::as_str)) => format!(
            "Access-Control-Allow-Origin: {o}\r\nVary: Origin\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: content-type, x-weft-host, authorization\r\nAccess-Control-Max-Age: 600\r\n"
        ),
        _ => String::new(),
    }
}

fn handle(mut s: TcpStream, sup: Arc<Mutex<Supervisor>>, policy: &Policy, licence: &HostLicence) -> std::io::Result<()> {
    let peer_ip = s.peer_addr().ok().map(|a| a.ip().to_string());
    let deadline = Instant::now() + REQUEST_DEADLINE;
    let mut buf: Vec<u8> = Vec::new();
    let mut tmp = [0u8; 16384];
    let mut head_end = None;
    let mut content_len = 0usize;
    // Read until headers + the full declared body are present (or the peer closes / cap hit).
    loop {
        if head_end.is_none() && let Some(p) = find(&buf, b"\r\n\r\n") {
            head_end = Some(p);
            content_len = content_length(&buf[..p]);
            let head = String::from_utf8_lossy(&buf[..p]);
            let path = head.lines().next().unwrap_or("").split_whitespace().nth(1).unwrap_or("/");
            let method = head.lines().next().unwrap_or("").split_whitespace().next().unwrap_or("");
            if path.starts_with("/licence") && method != "OPTIONS" {
                // Authenticate before reading any body: the state is private and imports are big.
                // (A CORS preflight carries no token and no body; it is answered below.)
                let h = parse_headers(&head);
                let auth = if method == "POST" { policy.check_post(&h).and_then(|_| policy.check_token(&h)) } else { policy.check_token(&h) };
                if let Err((code, payload)) = auth {
                    let cors = cors_headers(method, path, &h, policy);
                    return respond(&mut s, code, payload, &cors);
                }
            }
            let cap = if path.starts_with("/hw/") {
                HW_BODY_CAP
            } else if path == "/fleet/heartbeat" {
                HEARTBEAT_BODY_CAP
            } else if path.starts_with("/licence") {
                MAX_IMPORT_BYTES
            } else {
                MAX_BODY
            };
            if content_len > cap {
                return respond(&mut s, "413 Payload Too Large", r#"{"ok":false,"error":"body too large"}"#.into(), "");
            }
        }
        if let Some(p) = head_end && buf.len() >= p + 4 + content_len {
            break;
        }
        if buf.len() > MAX_BODY + 65536 {
            break;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return respond(&mut s, "408 Request Timeout", r#"{"ok":false,"error":"request timed out"}"#.into(), "");
        }
        s.set_read_timeout(Some(left.min(Duration::from_secs(15)))).ok();
        let n = s.read(&mut tmp)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
    }

    let he = head_end.unwrap_or(buf.len());
    let head = String::from_utf8_lossy(&buf[..he]).into_owned();
    let mut first = head.lines().next().unwrap_or("").split_whitespace();
    let method = first.next().unwrap_or("");
    let path = first.next().unwrap_or("/");
    let headers = parse_headers(&head);
    let body: &[u8] = if he + 4 <= buf.len() { &buf[he + 4..] } else { &[] };
    let cors = cors_headers(method, path, &headers, policy);

    // DNS-rebinding guard: a page on an attacker's name resolving to this host sends its own Host.
    // Edge heartbeats are exempt: minimal firmware (HTTP/1.0) may send no Host or a router-domain
    // name, and that route is already CSRF-exempt, size-capped and display-only.
    let heartbeat = method == "POST" && path == "/fleet/heartbeat";
    if !heartbeat && !policy.host_allowed(headers.get("host").map(String::as_str)) {
        return respond(&mut s, "421 Misdirected Request", serde_json::json!({"ok":false,"error":"host header not allowed","hint":"use an IP, localhost or this machine's name, or set WEFT_COG_HOST_NAMES"}).to_string(), "");
    }
    if method == "OPTIONS" {
        // Preflight: 204 with CORS headers for an allowed origin, 403 (no CORS headers) otherwise.
        let allowed = headers.get("origin").is_none_or(|o| policy.origin_allowed(o, headers.get("host").map(String::as_str)));
        return respond(&mut s, if allowed { "204 No Content" } else { "403 Forbidden" }, String::new(), &if allowed { preflight(&headers, path, policy) } else { String::new() });
    }
    if method == "POST"
        && path != "/fleet/heartbeat"
        && let Err((code, payload)) = policy.check_post(&headers).and_then(|_| policy.check_token(&headers))
    {
        return respond(&mut s, code, payload, &cors);
    }
    if token_guarded_read(method, path)
        && let Err((code, payload)) = policy.check_token(&headers)
    {
        return respond(&mut s, code, payload, &cors);
    }
    let authed = policy.check_token(&headers).is_ok();
    let root: PathBuf = sup.lock().unwrap().root.clone();
    let hw = hw_http::Req { method, path, headers: &headers, body };
    let (code, payload) = match hw_http::handle(&hw, &root, policy) {
        Some(r) => r,
        None => match (method, path) {
            ("GET", "/licence") => match policy.check_token(&headers) {
                Ok(()) => ("200 OK", licence.status().to_string()),
                Err((code, payload)) => (code, payload),
            },
            ("POST", "/licence/records") => licence_route(body, licence),
            _ => route(method, path, body, peer_ip, &sup, authed),
        },
    };
    respond(&mut s, code, payload, &cors)
}

/// CORS headers for an allowed preflight (legacy open routes still get `*`).
fn preflight(h: &Headers, path: &str, policy: &Policy) -> String {
    let c = cors_headers("OPTIONS", path, h, policy);
    if c.is_empty() {
        // no Origin header (not a browser): nothing to add
        return String::new();
    }
    c
}

fn route(method: &str, path: &str, body: &[u8], peer_ip: Option<String>, sup: &Arc<Mutex<Supervisor>>, authed: bool) -> (&'static str, String) {
    let parts: Vec<&str> = path.trim_matches('/').split('/').filter(|p| !p.is_empty()).collect();
    match (method, parts.as_slice()) {
        ("GET", ["healthz"]) => ("200 OK", r#"{"ok":true}"#.to_string()),
        ("GET", ["network"]) => {
            let mut snap = weftos_cog_host::network::snapshot();
            snap["fleet"] = sup.lock().unwrap().fleet.roster();
            ("200 OK", snap.to_string())
        }
        // Mesh-wide cogs: this host plus every online tailnet peer that answers as a cog-host.
        ("GET", ["mesh", "cogs"]) => {
            ("200 OK", weftos_cog_host::mesh::snapshot(status_rows(sup, true)).to_string())
        }
        ("POST", ["fleet", "heartbeat"]) => match serde_json::from_slice::<Heartbeat>(body) {
            Ok(h) => {
                sup.lock().unwrap().fleet.heartbeat(&h, peer_ip);
                ("200 OK", serde_json::json!({"ok":true,"id":h.id}).to_string())
            }
            Err(e) => ("400 Bad Request", serde_json::json!({"ok":false,"error":format!("bad heartbeat: {e}")}).to_string()),
        },
        ("GET", ["status"]) | ("GET", ["cogs"]) => {
            let (root, running) = {
                let s = sup.lock().unwrap();
                (s.root.display().to_string(), s.running_count())
            };
            let obj = serde_json::json!({"ok": true, "root": root, "running": running, "cogs": status_rows(sup, authed)});
            ("200 OK", obj.to_string())
        }
        ("GET", ["cogs", id, "last"]) => match weftos_cog_host::introspect::last_output(&root_of(sup), id) {
            Ok(v) => ("200 OK", v.to_string()),
            Err((code, e)) => (code, serde_json::json!({"ok": false, "error": e}).to_string()),
        },
        ("GET", ["cogs", id, "guide"]) => match weftos_cog_host::introspect::installed_guide(&root_of(sup), id) {
            Ok(body) => ("200 OK", body),
            Err((code, e)) => (code, serde_json::json!({"ok": false, "error": e}).to_string()),
        },
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

/// The cog rows for `/status` and the mesh view. Output-log size and age and listening ports are
/// activity side channels (a presence radar's log grows with what it sees), so an unauthenticated
/// caller gets the rows without them. The `/proc` reads happen after the supervisor lock is
/// released.
fn status_rows(sup: &Arc<Mutex<Supervisor>>, authed: bool) -> Vec<weftos_cog_host::supervise::CogStatus> {
    let mut rows = sup.lock().unwrap().status();
    if authed {
        let tables = weftos_cog_host::proc::cached_tables();
        for r in &mut rows {
            if let Some(pid) = r.pid {
                r.export_ports = weftos_cog_host::proc::listen_ports(&tables, std::path::Path::new("/proc"), pid);
            }
        }
    } else {
        for r in &mut rows {
            r.log_bytes = None;
            r.log_age_s = None;
            r.export_ports.clear();
        }
    }
    rows
}

fn root_of(sup: &Arc<Mutex<Supervisor>>) -> PathBuf {
    sup.lock().unwrap().root.clone()
}

fn licence_route(body: &[u8], licence: &HostLicence) -> (&'static str, String) {
    let recs: Records = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => return ("400 Bad Request", serde_json::json!({"ok":false,"error":format!("bad licence records: {e}")}).to_string()),
    };
    match licence.import(&recs) {
        Ok(lines) => ("200 OK", serde_json::json!({"ok":true,"records":lines}).to_string()),
        Err(e) => ("400 Bad Request", serde_json::json!({"ok":false,"error":e}).to_string()),
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

#[cfg(test)]
mod tests {
    use super::*;

    // the connection counter is process-global: keep these tests from starving each other
    static SERIAL: Mutex<()> = Mutex::new(());
    fn serial() -> std::sync::MutexGuard<'static, ()> {
        SERIAL.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A host on an ephemeral loopback port with a temp root and token "tok".
    fn start() -> (std::net::SocketAddr, tempfile::TempDir) {
        let root = tempfile::tempdir().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let sup = Arc::new(Mutex::new(Supervisor::new(root.path().to_path_buf())));
        let policy = Arc::new(Policy::new("tok".into(), vec!["http://127.0.0.1:*".into(), "http://localhost:*".into()]));
        let licence = Arc::new(HostLicence::open(root.path().join(".licence")));
        std::thread::spawn(move || serve(listener, sup, policy, licence));
        (addr, root)
    }

    /// Raw request -> (status line, headers lowercased, body).
    fn send(addr: std::net::SocketAddr, raw: &str) -> (String, String, String) {
        let mut c = TcpStream::connect(addr).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).ok();
        c.write_all(raw.as_bytes()).unwrap();
        let mut out = String::new();
        let _ = c.read_to_string(&mut out);
        let (head, body) = out.split_once("\r\n\r\n").unwrap_or((&out, ""));
        let status = head.lines().next().unwrap_or("").to_string();
        (status, head.to_ascii_lowercase(), body.to_string())
    }

    fn post(path: &str, extra: &str, body: &str) -> String {
        format!("POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n{extra}Content-Length: {}\r\n\r\n{body}", body.len())
    }

    const JSON: &str = "Content-Type: application/json\r\nX-Weft-Host: 1\r\n";

    #[test]
    fn csrf_guard_refuses_posts_without_json_and_custom_header() {
        let _g = serial();
        let (a, _r) = start();
        for (extra, why) in [
            ("", "no headers"),
            ("Content-Type: text/plain\r\nX-Weft-Host: 1\r\n", "wrong content type"),
            ("Content-Type: application/json\r\n", "no custom header"),
            ("Content-Type: application/x-www-form-urlencoded\r\nX-Weft-Host: 1\r\n", "form post"),
        ] {
            // legacy routes are covered too, not just /hw/*
            for path in ["/reload", "/cogs/x/start", "/install", "/hw/dex/ack"] {
                let (st, _, _) = send(a, &post(path, extra, "{}"));
                assert!(st.contains("400"), "{path} {why}: {st}");
            }
        }
        // with both headers AND the token the legacy routes work
        assert!(send(a, &post("/reload", &format!("{JSON}Authorization: Bearer tok\r\n"), "{}")).0.contains("200"));
    }

    #[test]
    fn disallowed_origin_is_refused_and_gets_no_cors_headers() {
        let _g = serial();
        let (a, _r) = start();
        let evil = format!("Origin: http://evil.example\r\n{JSON}");
        let evil = format!("{evil}Authorization: Bearer tok\r\n");
        let (st, h, _) = send(a, &post("/reload", &evil, "{}"));
        assert!(st.contains("403"), "{st}");
        assert!(!h.contains("access-control-allow-origin"));
        let (st, h, _) = send(a, "OPTIONS /hw/dex/catch HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: http://evil.example\r\nAccess-Control-Request-Method: POST\r\n\r\n");
        assert!(st.contains("403") && !h.contains("access-control-allow-origin"), "{st} {h}");
    }

    #[test]
    fn allowed_origin_gets_an_exact_preflight_answer_never_star_for_posts() {
        let _g = serial();
        let (a, _r) = start();
        let (st, h, _) = send(a, "OPTIONS /hw/dex/catch HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: http://localhost:8080\r\nAccess-Control-Request-Method: POST\r\n\r\n");
        assert!(st.contains("204"), "{st}");
        assert!(h.contains("access-control-allow-origin: http://localhost:8080"), "{h}");
        assert!(h.contains("x-weft-host") && h.contains("authorization") && h.contains("vary: origin"));
        // a real POST from that origin echoes it (not `*`)
        let (_, h, _) = send(a, &post("/reload", &format!("Origin: http://localhost:8080\r\n{JSON}Authorization: Bearer tok\r\n"), "{}"));
        assert!(h.contains("access-control-allow-origin: http://localhost:8080") && !h.contains("allow-origin: *"));
        // plain legacy GETs keep `*` for the web console; /hw GETs do not
        let (_, h, _) = send(a, "GET /status HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
        assert!(h.contains("access-control-allow-origin: *"));
        let (_, h, _) = send(a, "GET /hw/dex HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: http://evil.example\r\n\r\n");
        assert!(!h.contains("access-control-allow-origin"), "{h}");
    }

    #[test]
    fn hw_mutations_need_the_bearer_token_over_http() {
        let _g = serial();
        let (a, _r) = start();
        for path in ["/hw/usb/scan", "/hw/usb/baseline", "/hw/usb/identify", "/hw/dex/catch", "/hw/dex/ack"] {
            let (st, _, body) = send(a, &post(path, JSON, "{}"));
            assert!(st.contains("401"), "{path}: {st}");
            assert!(body.contains("host token required"));
            let (st, _, _) = send(a, &post(path, &format!("{JSON}Authorization: Bearer wrong\r\n"), "{}"));
            assert!(st.contains("401"));
        }
        assert!(send(a, &post("/hw/dex/ack", &format!("{JSON}Authorization: Bearer tok\r\n"), "{}")).0.contains("200"));
    }

    #[test]
    fn oversized_hw_body_is_refused_before_reading_it() {
        let _g = serial();
        let (a, _r) = start();
        let raw = format!("POST /hw/dex/catch HTTP/1.1\r\nHost: 127.0.0.1\r\n{JSON}Authorization: Bearer tok\r\nContent-Length: {}\r\n\r\n", HW_BODY_CAP + 1);
        assert!(send(a, &raw).0.contains("413"));
    }

    #[test]
    fn connection_cap_answers_503() {
        let _g = serial();
        let (a, _r) = start();
        // hold MAX_CONNS idle connections (their threads wait for a request)
        let held: Vec<TcpStream> = (0..MAX_CONNS).map(|_| TcpStream::connect(a).unwrap()).collect();
        std::thread::sleep(Duration::from_millis(300));
        let (st, _, _) = send(a, "GET /healthz HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
        assert!(st.contains("503"), "{st}");
        drop(held);
    }

    const TOK: &str = "Authorization: Bearer tok\r\n";

    #[test]
    fn rebinding_style_requests_are_refused_on_every_route() {
        let _g = serial();
        let (a, _r) = start();
        let evil_post = format!("POST /reload HTTP/1.1\r\nHost: evil.example:9480\r\n{JSON}{TOK}Content-Length: 2\r\n\r\n{{}}");
        for raw in [
            "GET /status HTTP/1.1\r\nHost: evil.example\r\n\r\n".to_string(),
            "GET /healthz HTTP/1.1\r\nHost: 127.0.0.1.evil.example\r\n\r\n".to_string(),
            "GET /hw/dex HTTP/1.1\r\nHost: evil.example\r\nAuthorization: Bearer tok\r\n\r\n".to_string(),
            "GET /status HTTP/1.0\r\n\r\n".to_string(), // no Host at all
            evil_post,
        ] {
            let (st, h, _) = send(a, &raw);
            assert!(st.contains("421"), "{raw:?}: {st}");
            assert!(!h.contains("access-control-allow-origin"));
        }
        // the same requests with a good Host work (token included where required)
        assert!(send(a, "GET /status HTTP/1.1\r\nHost: localhost:9480\r\n\r\n").0.contains("200"));
    }

    #[test]
    fn every_post_but_heartbeat_needs_the_token_and_hw_gets_too() {
        let _g = serial();
        let (a, _r) = start();
        for path in ["/reload", "/cogs/x/start", "/cogs/x/stop", "/install", "/hw/usb/scan"] {
            let (st, _, body) = send(a, &post(path, JSON, "{}"));
            assert!(st.contains("401"), "{path}: {st}");
            assert!(body.contains("host token required"));
        }
        for path in ["/hw/usb", "/hw/dex", "/hw/buses"] {
            let (st, _, _) = send(a, &format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"));
            assert!(st.contains("401"), "GET {path}: {st}");
            let (st, _, _) = send(a, &format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n{TOK}\r\n"));
            assert!(st.contains("200"), "GET {path} with token: {st}");
        }
        // read views stay open
        for path in ["/status", "/network", "/healthz"] {
            let (st, h, _) = send(a, &format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"));
            assert!(st.contains("200") && h.contains("access-control-allow-origin: *"), "{path}: {st}");
        }
    }

    #[test]
    fn mesh_and_last_output_need_the_token_and_get_no_wildcard_cors() {
        let _g = serial();
        let (a, root) = start();
        for path in ["/mesh/cogs", "/cogs/x/last"] {
            let (st, h, _) = send(a, &format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: http://evil.example\r\n\r\n"));
            assert!(st.contains("401"), "{path}: {st}");
            assert!(!h.contains("access-control-allow-origin: *"), "{path} must not send wildcard CORS: {h}");
            assert!(!h.contains("evil.example"), "{path}");
            let (st, h, _) = send(a, &format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer tok\r\n\r\n"));
            assert!(!st.contains("401"), "{path} with token: {st}");
            assert!(!h.contains("access-control-allow-origin: *"), "{path}");
        }
        // an unknown cog is a 404 for an authorised caller, never a file read
        let (st, _, _) = send(a, "GET /cogs/..%2f..%2fetc/last HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer tok\r\n\r\n");
        assert!(st.contains("404"), "{st}");
        drop(root);
    }

    #[test]
    fn status_hides_activity_fields_from_unauthenticated_callers() {
        let _g = serial();
        let (a, root) = start();
        let dir = root.path().join("c1");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("host.log"), b"some output").unwrap();
        let rec = weftos_cog_host::CogRecord {
            id: "c1".into(),
            version: "1".into(),
            source: weftos_cog_host::Source::Local,
            enabled: false,
            binary: "cog-c1-arm".into(),
            args: vec![],
            signed: false,
        };
        weftos_cog_host::save_record(root.path(), &rec).unwrap();
        // the supervisor reloads records from disk on its own tick in production; ask for it here
        let (st, _, _) = send(a, "POST /reload HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nX-Weft-Host: 1\r\nAuthorization: Bearer tok\r\nContent-Length: 2\r\n\r\n{}");
        assert!(st.contains("200"), "{st}");
        let (_, _, open) = send(a, "GET /status HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
        assert!(open.contains("\"c1\"") && !open.contains("log_bytes") && !open.contains("log_age_s"), "{open}");
        let (_, _, authed) = send(a, "GET /status HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer tok\r\n\r\n");
        assert!(authed.contains("log_bytes"), "{authed}");
    }

    #[test]
    fn licence_records_need_the_token_and_status_is_a_read_view() {
        let _g = serial();
        let (a, root) = start();
        let (st, _, body) = send(a, &post("/licence/records", JSON, "{}"));
        assert!(st.contains("401") && body.contains("host token required"), "{st}");
        // With the token: no licence dir yet, so nothing is imported.
        let (st, _, body) = send(a, &post("/licence/records", &format!("{JSON}{TOK}"), "{}"));
        assert!(st.contains("400") && body.contains("no licence directory"), "{st} {body}");
        let (st, _, body) = send(a, &post("/licence/records", &format!("{JSON}{TOK}"), r#"{"nope":1}"#));
        assert!(st.contains("400") && body.contains("bad licence records"), "{st} {body}");
        // The state names the mesh, grants and hashes: token only, and never `*` CORS.
        let (st, h, body) = send(a, "GET /licence HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
        assert!(st.contains("401") && !body.contains("unconfigured") && !h.contains("access-control-allow-origin"), "{st} {h} {body}");
        let (st, h, body) = send(a, &format!("GET /licence HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: http://evil.example\r\n{TOK}\r\n"));
        assert!(st.contains("200") && body.contains("unconfigured") && !h.contains("access-control-allow-origin"), "{st} {h} {body}");
        // A browser preflight carries no token: it is answered (204 for an allowed origin), not 401'd.
        let (st, h, _) = send(a, "OPTIONS /licence/records HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: http://localhost:8080\r\nAccess-Control-Request-Method: POST\r\n\r\n");
        assert!(st.contains("204") && h.contains("access-control-allow-origin: http://localhost:8080"), "{st} {h}");
        let (st, h, _) = send(a, "OPTIONS /licence HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: http://evil.example\r\nAccess-Control-Request-Method: GET\r\n\r\n");
        assert!(st.contains("403") && !h.contains("access-control-allow-origin"), "{st} {h}");
        // An oversized import is refused for the missing token before its size is considered.
        let raw = format!("POST /licence/records HTTP/1.1\r\nHost: 127.0.0.1\r\n{JSON}Content-Length: {}\r\n\r\n", MAX_IMPORT_BYTES + 1);
        assert!(send(a, &raw).0.contains("401"));
        let raw = format!("POST /licence/records HTTP/1.1\r\nHost: 127.0.0.1\r\n{JSON}{TOK}Content-Length: {}\r\n\r\n", MAX_IMPORT_BYTES + 1);
        assert!(send(a, &raw).0.contains("413"));
        assert!(!root.path().join(".licence").exists());
    }

    #[test]
    fn install_with_a_path_like_id_is_a_400() {
        let _g = serial();
        let (a, root) = start();
        let body = r#"{"id":"../x","source":"cognitum","sha256":"00","binary_b64":"eA=="}"#;
        let (st, _, resp) = send(a, &post("/install", &format!("{JSON}{TOK}"), body));
        assert!(st.contains("400"), "{st}");
        assert!(resp.contains("bad cog id"), "{resp}");
        assert!(!root.path().join("../x").exists());
    }

    #[test]
    fn heartbeat_stays_open_but_is_size_capped() {
        let _g = serial();
        let (a, _r) = start();
        // edge firmware: no JSON content type, no custom header, no token
        let hb = r#"{"id":"esp-01","rssi":-50}"#;
        let raw = format!("POST /fleet/heartbeat HTTP/1.1\r\nHost: 192.168.1.9\r\nContent-Length: {}\r\n\r\n{hb}", hb.len());
        assert!(send(a, &raw).0.contains("200"));
        // minimal firmware: HTTP/1.0 with no Host header, or an unlisted router-domain name
        let nohost = format!("POST /fleet/heartbeat HTTP/1.0\r\nContent-Length: {}\r\n\r\n{hb}", hb.len());
        assert!(send(a, &nohost).0.contains("200"), "no Host header");
        let lan = format!("POST /fleet/heartbeat HTTP/1.1\r\nHost: pi5.lan\r\nContent-Length: {}\r\n\r\n{hb}", hb.len());
        assert!(send(a, &lan).0.contains("200"), "unlisted Host");
        // ...but only that route: everything else still refuses an unlisted/missing Host
        assert!(send(a, "GET /status HTTP/1.1\r\nHost: pi5.lan\r\n\r\n").0.contains("421"));
        let big = format!("POST /fleet/heartbeat HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\n\r\n", fleet_cap() + 1);
        assert!(send(a, &big).0.contains("413"));
    }

    fn fleet_cap() -> usize {
        HEARTBEAT_BODY_CAP
    }
}
