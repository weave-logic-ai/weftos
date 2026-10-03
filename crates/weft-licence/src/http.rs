//! A small std-only HTTP/1.1 listener. It binds only the configured
//! addresses (never `0.0.0.0`), one thread per connection with a hard cap and
//! a per-source-address cap, `Connection: close`, bounded headers and body, a
//! total read deadline (slow-loris), a `Host` check, and a byte-rate throttle
//! on artifact transfers.

use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::limits::throttle_sleep_ms;
use crate::request::Request;
use crate::service::{Body, Response, Service};

const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_BODY_BYTES: usize = 64 * 1024;
const MAX_CONNECTIONS: usize = 16;

/// Listener tuning.
#[derive(Debug, Clone, Copy)]
pub struct ServerOpts {
    /// Total time to read the request line, headers and body.
    pub read_deadline: Duration,
    /// Most open connections from one source (IPv4-mapped IPv6 counts as the
    /// IPv4 address).
    pub per_ip: usize,
}

impl Default for ServerOpts {
    fn default() -> Self {
        Self { read_deadline: Duration::from_secs(10), per_ip: 2 }
    }
}

/// A running server.
pub struct Server {
    stop: Arc<AtomicBool>,
    addrs: Vec<SocketAddr>,
    threads: Vec<std::thread::JoinHandle<()>>,
}

impl Server {
    /// The bound addresses (resolves port 0).
    pub fn addrs(&self) -> &[SocketAddr] {
        &self.addrs
    }

    /// Stop accepting and join the listener threads.
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::Release);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }

    /// Block until the listener threads end (they run until `stop`).
    pub fn wait(mut self) {
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

/// Bind `addrs` and serve `svc` with default options.
pub fn serve(svc: Arc<Service>, addrs: &[SocketAddr]) -> std::io::Result<Server> {
    serve_with(svc, addrs, ServerOpts::default())
}

/// Bind `addrs` and serve `svc`. An unspecified address is refused.
pub fn serve_with(svc: Arc<Service>, addrs: &[SocketAddr], opts: ServerOpts) -> std::io::Result<Server> {
    if addrs.is_empty() || addrs.iter().any(|a| a.ip().is_unspecified()) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "listen addresses must be named; 0.0.0.0 and :: are refused",
        ));
    }
    let stop = Arc::new(AtomicBool::new(false));
    let shared = Arc::new(Shared {
        active: AtomicUsize::new(0),
        per_ip: Mutex::new(HashMap::new()),
        opts,
        hosts: Mutex::new(Vec::new()),
        names: svc.config().allowed_hosts.iter().map(|h| h.to_ascii_lowercase()).collect(),
    });
    let (mut bound, mut threads) = (Vec::new(), Vec::new());
    for a in addrs {
        let l = TcpListener::bind(a)?;
        l.set_nonblocking(true)?;
        let local = l.local_addr()?;
        bound.push(local);
        shared.hosts.lock().unwrap_or_else(|p| p.into_inner()).push(local);
        let (svc, stop, shared) = (svc.clone(), stop.clone(), shared.clone());
        threads.push(std::thread::spawn(move || accept_loop(l, svc, stop, shared)));
    }
    Ok(Server { stop, addrs: bound, threads })
}

struct Shared {
    active: AtomicUsize,
    per_ip: Mutex<HashMap<IpAddr, usize>>,
    opts: ServerOpts,
    /// The bound listener addresses, for the Host check.
    hosts: Mutex<Vec<SocketAddr>>,
    /// Allowed host names, lower-case.
    names: Vec<String>,
}

/// Is a `Host` header value one of ours? It must be `ip:port` (an IPv6
/// address in brackets, any spelling, an optional `%zone` ignored) naming a
/// bound address, or `name:port` with `name` (case-insensitive) in `names`
/// (given lower-case) and the port a bound port. The IP is compared as an address, not a string.
pub fn host_allowed(header: &str, bound: &[SocketAddr], names: &[String]) -> bool {
    let header = header.trim();
    let (host, port) = if let Some(rest) = header.strip_prefix('[') {
        let Some((inside, tail)) = rest.split_once(']') else { return false };
        let port = tail.strip_prefix(':').unwrap_or("80");
        (inside.to_string(), port)
    } else {
        match header.rsplit_once(':') {
            Some((h, p)) if !h.contains(':') => (h.to_string(), p),
            None => (header.to_string(), "80"),
            _ => return false, // a bare IPv6 address without brackets
        }
    };
    let Ok(port) = port.parse::<u16>() else { return false };
    let bare = host.split('%').next().unwrap_or("");
    if let Ok(ip) = bare.parse::<IpAddr>() {
        return bound.iter().any(|b| b.port() == port && normalize_ip(b.ip()) == normalize_ip(ip));
    }
    let lower = host.to_ascii_lowercase();
    names.contains(&lower) && bound.iter().any(|b| b.port() == port)
}

/// An IPv4-mapped IPv6 address is its IPv4 address.
fn normalize_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v) => v.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(IpAddr::V6(v)),
        v4 => v4,
    }
}

/// Releases a connection slot (global and per source address) on drop.
struct Slot {
    shared: Arc<Shared>,
    ip: IpAddr,
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.shared.active.fetch_sub(1, Ordering::AcqRel);
        let mut m = self.shared.per_ip.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(n) = m.get_mut(&self.ip) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                m.remove(&self.ip);
            }
        }
    }
}

fn take_slot(shared: &Arc<Shared>, ip: IpAddr) -> Option<Slot> {
    if shared.active.fetch_add(1, Ordering::AcqRel) >= MAX_CONNECTIONS {
        shared.active.fetch_sub(1, Ordering::AcqRel);
        return None;
    }
    let mut m = shared.per_ip.lock().unwrap_or_else(|p| p.into_inner());
    let n = m.entry(ip).or_insert(0);
    if *n >= shared.opts.per_ip {
        drop(m);
        shared.active.fetch_sub(1, Ordering::AcqRel);
        return None;
    }
    *n += 1;
    Some(Slot { shared: shared.clone(), ip })
}

fn accept_loop(l: TcpListener, svc: Arc<Service>, stop: Arc<AtomicBool>, shared: Arc<Shared>) {
    while !stop.load(Ordering::Acquire) {
        match l.accept() {
            Ok((s, peer)) => {
                // BSD-derived systems let an accepted socket inherit the
                // listener's non-blocking mode; the handler wants blocking I/O.
                let _ = s.set_nonblocking(false);
                let Some(slot) = take_slot(&shared, normalize_ip(peer.ip())) else {
                    let _ = write_simple(s, 503, b"{\"error\":\"busy\"}");
                    continue;
                };
                let (svc, shared) = (svc.clone(), shared.clone());
                std::thread::spawn(move || {
                    let _slot = slot;
                    let _ = handle_conn(s, &svc, &shared);
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(15)),
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        408 => "Request Timeout",
        409 => "Conflict",
        410 => "Gone",
        413 => "Payload Too Large",
        422 => "Unprocessable Entity",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        507 => "Insufficient Storage",
        _ => "Error",
    }
}

fn write_simple(mut s: TcpStream, status: u16, body: &[u8]) -> std::io::Result<()> {
    s.set_write_timeout(Some(Duration::from_secs(10)))?;
    write!(
        s,
        "HTTP/1.1 {status} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        reason(status),
        body.len()
    )?;
    s.write_all(body)
}

/// Reads from the stream under one total deadline: each read gets only the
/// time that is left, so a client dripping a byte at a time cannot hold the
/// connection past it.
struct DeadlineStream {
    s: TcpStream,
    deadline: Instant,
}

impl Read for DeadlineStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let left = self.deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "request read deadline"));
        }
        self.s.set_read_timeout(Some(left))?;
        self.s.read(buf)
    }
}

/// Parse one request. `Err` carries the status to answer with.
fn read_request(s: &TcpStream, deadline: Instant) -> Result<Request, u16> {
    let timeout = |e: &std::io::Error| match e.kind() {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => 408u16,
        _ => 400u16,
    };
    let mut r = BufReader::new(DeadlineStream { s: s.try_clone().map_err(|_| 500u16)?, deadline });
    let mut line = String::new();
    let mut total = 0usize;
    r.by_ref().take(MAX_HEADER_BYTES as u64).read_line(&mut line).map_err(|e| timeout(&e))?;
    total = total.saturating_add(line.len());
    let mut it = line.split_whitespace();
    let (method, target) = (it.next().ok_or(400u16)?.to_string(), it.next().ok_or(400u16)?.to_string());
    let mut headers = BTreeMap::new();
    loop {
        let mut h = String::new();
        let n = r.by_ref().take(MAX_HEADER_BYTES as u64).read_line(&mut h).map_err(|e| timeout(&e))?;
        total = total.saturating_add(n);
        if n == 0 || total > MAX_HEADER_BYTES {
            return Err(400);
        }
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    let len: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    if len > MAX_BODY_BYTES {
        return Err(413);
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).map_err(|e| timeout(&e))?;
    Ok(Request { method, target, headers, body })
}

fn handle_conn(mut s: TcpStream, svc: &Service, shared: &Shared) -> std::io::Result<()> {
    s.set_write_timeout(Some(Duration::from_secs(10)))?;
    let deadline = Instant::now() + shared.opts.read_deadline;
    let parsed = read_request(&s, deadline).and_then(|r| {
        // A Host header, when sent, must name one of our listen addresses.
        let hosts = shared.hosts.lock().unwrap_or_else(|p| p.into_inner());
        match r.headers.get("host") {
            Some(h) if !host_allowed(h, &hosts, &shared.names) => Err(400),
            _ => Ok(r),
        }
    });
    let req = match parsed {
        Ok(r) => r,
        Err(status) => {
            let sent = write_simple(s.try_clone()?, status, b"{\"error\":\"bad_request\"}");
            // Drain what the client is still sending (bounded), so closing
            // does not reset the connection and lose the error answer.
            let _ = s.shutdown(std::net::Shutdown::Write);
            if let Ok(c) = s.try_clone() {
                let drain = DeadlineStream { s: c, deadline: Instant::now() + Duration::from_millis(300) };
                let _ = std::io::copy(&mut Read::take(drain, 256 * 1024), &mut std::io::sink());
            }
            return sent;
        }
    };
    let resp: Response = svc.handle(&req);
    match resp.body {
        Body::Json(ref b) => write_simple(s, resp.status, b),
        Body::File { ref file, len } => {
            write!(
                s,
                "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n"
            )?;
            let mut f = file.try_clone()?;
            let rate = svc.config().limits.rate_bytes_per_sec;
            let (start, mut sent) = (Instant::now(), 0u64);
            let mut buf = vec![0u8; 32 * 1024];
            loop {
                let n = f.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                s.write_all(&buf[..n])?;
                sent = sent.saturating_add(n as u64);
                let ms = throttle_sleep_ms(sent, start.elapsed().as_millis() as u64, rate);
                if ms > 0 {
                    std::thread::sleep(Duration::from_millis(ms));
                }
            }
            Ok(()) // `resp.permit` drops here, after the last byte
        }
    }
}
