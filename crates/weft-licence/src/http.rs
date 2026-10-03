//! A small std-only HTTP/1.1 listener. It binds only the configured
//! addresses (never `0.0.0.0`), one thread per connection with a hard cap,
//! `Connection: close`, bounded headers and body, and a byte-rate throttle on
//! artifact transfers.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::limits::throttle_sleep_ms;
use crate::request::Request;
use crate::service::{Body, Response, Service};

const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_BODY_BYTES: usize = 64 * 1024;
const MAX_CONNECTIONS: usize = 16;
const IO_TIMEOUT: Duration = Duration::from_secs(10);

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

/// Bind `addrs` and serve `svc`. An unspecified address is refused.
pub fn serve(svc: Arc<Service>, addrs: &[SocketAddr]) -> std::io::Result<Server> {
    if addrs.is_empty() || addrs.iter().any(|a| a.ip().is_unspecified()) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "listen addresses must be named; 0.0.0.0 and :: are refused",
        ));
    }
    let stop = Arc::new(AtomicBool::new(false));
    let active = Arc::new(AtomicUsize::new(0));
    let (mut bound, mut threads) = (Vec::new(), Vec::new());
    for a in addrs {
        let l = TcpListener::bind(a)?;
        l.set_nonblocking(true)?;
        bound.push(l.local_addr()?);
        let (svc, stop, active) = (svc.clone(), stop.clone(), active.clone());
        threads.push(std::thread::spawn(move || accept_loop(l, svc, stop, active)));
    }
    Ok(Server { stop, addrs: bound, threads })
}

fn accept_loop(l: TcpListener, svc: Arc<Service>, stop: Arc<AtomicBool>, active: Arc<AtomicUsize>) {
    while !stop.load(Ordering::Acquire) {
        match l.accept() {
            Ok((s, _)) => {
                // BSD-derived systems let an accepted socket inherit the
                // listener's non-blocking mode; the handler wants blocking I/O.
                let _ = s.set_nonblocking(false);
                if active.fetch_add(1, Ordering::AcqRel) >= MAX_CONNECTIONS {
                    active.fetch_sub(1, Ordering::AcqRel);
                    let _ = write_simple(s, 503, b"{\"error\":\"busy\"}");
                    continue;
                }
                let (svc, active) = (svc.clone(), active.clone());
                std::thread::spawn(move || {
                    let _ = handle_conn(s, &svc);
                    active.fetch_sub(1, Ordering::AcqRel);
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
    s.set_write_timeout(Some(IO_TIMEOUT))?;
    write!(
        s,
        "HTTP/1.1 {status} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        reason(status),
        body.len()
    )?;
    s.write_all(body)
}

/// Parse one request. `Err` carries the status to answer with.
fn read_request(s: &mut TcpStream) -> Result<Request, u16> {
    let mut r = BufReader::new(s.try_clone().map_err(|_| 500u16)?);
    let mut line = String::new();
    let mut total = 0usize;
    r.by_ref().take(MAX_HEADER_BYTES as u64).read_line(&mut line).map_err(|_| 400u16)?;
    total += line.len();
    let mut it = line.split_whitespace();
    let (method, target) = (it.next().ok_or(400u16)?.to_string(), it.next().ok_or(400u16)?.to_string());
    let mut headers = BTreeMap::new();
    loop {
        let mut h = String::new();
        let n = r.by_ref().take(MAX_HEADER_BYTES as u64).read_line(&mut h).map_err(|_| 400u16)?;
        total += n;
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
    r.read_exact(&mut body).map_err(|_| 400u16)?;
    Ok(Request { method, target, headers, body })
}

fn handle_conn(mut s: TcpStream, svc: &Service) -> std::io::Result<()> {
    s.set_read_timeout(Some(IO_TIMEOUT))?;
    s.set_write_timeout(Some(IO_TIMEOUT))?;
    let req = match read_request(&mut s) {
        Ok(r) => r,
        Err(status) => {
            let sent = write_simple(s.try_clone()?, status, b"{\"error\":\"bad_request\"}");
            // Drain what the client is still sending (bounded), so closing
            // does not reset the connection and lose the error answer.
            let _ = s.shutdown(std::net::Shutdown::Write);
            let _ = s.set_read_timeout(Some(Duration::from_millis(500)));
            let _ = std::io::copy(&mut Read::take(&mut s, 256 * 1024), &mut std::io::sink());
            return sent;
        }
    };
    let resp: Response = svc.handle(&req);
    match &resp.body {
        Body::Json(b) => write_simple(s, resp.status, b),
        Body::File { path, len } => {
            write!(
                s,
                "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n"
            )?;
            let mut f = std::fs::File::open(path)?;
            let rate = svc.config().limits.rate_bytes_per_sec;
            let (start, mut sent) = (Instant::now(), 0u64);
            let mut buf = vec![0u8; 32 * 1024];
            loop {
                let n = f.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                s.write_all(&buf[..n])?;
                sent += n as u64;
                let ms = throttle_sleep_ms(sent, start.elapsed().as_millis() as u64, rate);
                if ms > 0 {
                    std::thread::sleep(Duration::from_millis(ms));
                }
            }
            Ok(()) // `resp.permit` drops here, after the last byte
        }
    }
}
