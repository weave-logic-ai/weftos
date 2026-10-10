//! The reverse proxy (ADR-116 §3): longest-prefix match, the prefix kept on
//! the upstream request, `X-Forwarded-Host` / `X-Forwarded-Proto: https` /
//! `X-Forwarded-Prefix` set, bodies streamed in both directions, and
//! WebSocket (any `Upgrade`) tunnelled with `copy_bidirectional` once the
//! upstream answers 101. One upstream TCP connection per request; no pooling,
//! no HTML rewriting.
//!
//! Identity (ADR-116 R2): Tailscale Serve names the caller in
//! `Tailscale-User-Login` (user-owned devices; tagged devices send none). A
//! route with an `allow` list serves only listed logins and answers everyone
//! else with one uniform 403. Every other incoming `Tailscale-*` header is
//! stripped before proxying; the ones Serve sets go upstream unchanged.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use http_body_util::{BodyExt, Empty, Full};
use hyper::body::{Bytes, Incoming};
use hyper::header::{HeaderName, HeaderValue};
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::net::{TcpListener, TcpStream};

use crate::router_routes::{Route, prefix_matches};
use crate::router_state::RouterHandle;

/// Response body type every handler returns.
pub type BoxBody = http_body_util::combinators::BoxBody<Bytes, hyper::Error>;

/// Time allowed to open the upstream TCP connection.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// Largest body [`http_get`] keeps (health and process-compose probes).
pub const PROBE_BODY_LIMIT: usize = 256 * 1024;

const HOP_BY_HOP: &[&str] = &["connection", "keep-alive", "proxy-authenticate", "proxy-authorization", "te", "trailer", "transfer-encoding"];

/// The header Tailscale Serve sets to the caller's login (`user@domain`).
pub const LOGIN_HEADER: &str = "tailscale-user-login";
/// Headers Tailscale Serve sets itself; every other `Tailscale-*` is stripped.
pub const SERVE_HEADERS: &[&str] = &[LOGIN_HEADER, "tailscale-user-name", "tailscale-user-profile-pic", "tailscale-headers-info"];

/// Accept connections until the listener is dropped.
pub async fn accept_loop(listener: TcpListener, handle: Arc<RouterHandle>) {
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(x) => x,
            Err(e) => {
                tracing::warn!(error = %e, "router accept failed");
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
        };
        let h = handle.clone();
        tokio::spawn(async move {
            let svc = hyper::service::service_fn(move |req| handle_request(req, h.clone(), peer));
            let conn = hyper::server::conn::http1::Builder::new()
                .preserve_header_case(true)
                .serve_connection(TokioIo::new(stream), svc)
                .with_upgrades();
            if let Err(e) = conn.await {
                tracing::debug!(error = %e, "router connection ended");
            }
        });
    }
}

async fn handle_request(req: Request<Incoming>, h: Arc<RouterHandle>, peer: SocketAddr) -> Result<Response<BoxBody>, hyper::Error> {
    let path = req.uri().path().to_owned();
    if path == "/_weftos" || path.starts_with("/_weftos/") {
        return Ok(crate::router_index::respond(&path, &h).await);
    }
    let table = h.table();
    let Some(route) = table.matches(&path).cloned() else {
        return Ok(html_response(
            StatusCode::NOT_FOUND,
            "No route",
            &format!("<p>No project routes <code>{}</code>.</p><p><a href=\"/_weftos/\">Routes on this machine</a></p>", esc(&path)),
        ));
    };
    if route.restricted() && !login_allowed(&route.allow, req.headers()) {
        tracing::debug!(project = %route.project, prefix = %route.prefix, "login not on the route's allow list");
        return Ok(forbidden(&route));
    }
    match proxy(req, &route, peer).await {
        Ok(resp) => Ok(resp),
        Err(e) => {
            tracing::debug!(project = %route.project, port = route.port, error = %e, "upstream unavailable");
            Ok(bad_gateway(&route, &e))
        }
    }
}

/// Is the caller's `Tailscale-User-Login` on `allow` (exact, case-insensitive)?
/// No header (a tagged device, or no Serve in front) is never allowed.
pub fn login_allowed(allow: &[String], headers: &hyper::HeaderMap) -> bool {
    let Some(login) = headers.get(LOGIN_HEADER).and_then(|v| v.to_str().ok()) else { return false };
    let login = login.trim().to_ascii_lowercase();
    !login.is_empty() && allow.contains(&login)
}

/// Drop every incoming `Tailscale-*` header that Serve does not set itself.
fn strip_tailscale_headers(headers: &mut hyper::HeaderMap) {
    let names: Vec<HeaderName> = headers
        .keys()
        .filter(|k| k.as_str().starts_with("tailscale-") && !SERVE_HEADERS.contains(&k.as_str()))
        .cloned()
        .collect();
    for n in names {
        headers.remove(n);
    }
}

/// The 403 page: one text whether the login is missing or not listed, naming
/// the route and never the list.
pub fn forbidden(route: &Route) -> Response<BoxBody> {
    let body = format!(
        "<p>Project <strong>{p}</strong> at <code>{pre}/</code> is restricted to listed tailnet logins, \
         and this request's login is not on the list.</p>\
         <p>Ask the project's owner to add your login. Requests from tagged devices carry no login and are refused here. \
         <a href=\"/_weftos/\">Routes on this machine</a></p>",
        p = esc(&route.project),
        pre = esc(&route.prefix)
    );
    html_response(StatusCode::FORBIDDEN, "Not allowed", &body)
}

/// The 502 page: names the project and the loopback port it should be on.
pub fn bad_gateway(route: &Route, err: &str) -> Response<BoxBody> {
    let body = format!(
        "<p>Project <strong>{p}</strong> is routed at <code>{pre}</code> to <code>127.0.0.1:{port}</code>, \
         but nothing answered there.</p><p class=\"err\">{e}</p>\
         <p>Start it from that project's process-compose, then reload this page. \
         <a href=\"/_weftos/\">Routes on this machine</a></p>",
        p = esc(&route.project),
        pre = esc(&route.prefix),
        port = route.port,
        e = esc(err)
    );
    html_response(StatusCode::BAD_GATEWAY, &format!("{} is not running", route.project), &body)
}

fn is_upgrade(headers: &hyper::HeaderMap) -> bool {
    headers.get(hyper::header::UPGRADE).is_some()
        && headers
            .get_all(hyper::header::CONNECTION)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .any(|v| v.split(',').any(|t| t.trim().eq_ignore_ascii_case("upgrade")))
}

fn strip_hop_by_hop(headers: &mut hyper::HeaderMap, keep_upgrade: bool) {
    for name in HOP_BY_HOP {
        if keep_upgrade && *name == "connection" {
            continue;
        }
        headers.remove(*name);
    }
    if !keep_upgrade {
        headers.remove(hyper::header::UPGRADE);
    }
}

fn set(headers: &mut hyper::HeaderMap, name: &'static str, value: &str) {
    if let Ok(v) = HeaderValue::from_str(value) {
        headers.insert(HeaderName::from_static(name), v);
    }
}

async fn proxy(mut req: Request<Incoming>, route: &Route, peer: SocketAddr) -> Result<Response<BoxBody>, String> {
    let stream = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(("127.0.0.1", route.port)))
        .await
        .map_err(|_| format!("connect to 127.0.0.1:{} timed out", route.port))?
        .map_err(|e| format!("connect to 127.0.0.1:{}: {e}", route.port))?;
    let (mut sender, conn) = hyper::client::conn::http1::Builder::new()
        .preserve_header_case(true)
        .handshake(TokioIo::new(stream))
        .await
        .map_err(|e| format!("upstream handshake: {e}"))?;
    tokio::spawn(async move {
        if let Err(e) = conn.with_upgrades().await {
            tracing::debug!(error = %e, "upstream connection ended");
        }
    });

    let upgrade = is_upgrade(req.headers());
    let client_side = upgrade.then(|| hyper::upgrade::on(&mut req));
    let host = req.headers().get(hyper::header::HOST).and_then(|v| v.to_str().ok()).map(str::to_owned);
    let prefix = if prefix_matches(&route.prefix, req.uri().path()) { route.prefix.clone() } else { "/".to_owned() };
    let origin_form = req.uri().path_and_query().map_or_else(|| "/".to_owned(), |pq| pq.as_str().to_owned());
    *req.uri_mut() = origin_form.parse().map_err(|e| format!("request target: {e}"))?;
    {
        let headers = req.headers_mut();
        strip_hop_by_hop(headers, upgrade);
        strip_tailscale_headers(headers);
        if upgrade {
            set(headers, "connection", "upgrade");
        }
        if let Some(h) = host.as_deref() {
            set(headers, "x-forwarded-host", h);
        }
        set(headers, "x-forwarded-proto", "https");
        set(headers, "x-forwarded-prefix", &prefix);
        // Tailscale Serve already names the tailnet client; this hop is appended.
        let xff = match headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
            Some(prev) if !prev.is_empty() => format!("{prev}, {}", peer.ip()),
            _ => peer.ip().to_string(),
        };
        set(headers, "x-forwarded-for", &xff);
    }

    let mut resp = sender.send_request(req).await.map_err(|e| format!("upstream request: {e}"))?;
    let switching = resp.status() == StatusCode::SWITCHING_PROTOCOLS;
    if switching && let Some(client_side) = client_side {
        let upstream_side = hyper::upgrade::on(&mut resp);
        tokio::spawn(async move {
            match tokio::try_join!(client_side, upstream_side) {
                Ok((c, u)) => {
                    let (mut c, mut u) = (TokioIo::new(c), TokioIo::new(u));
                    if let Err(e) = tokio::io::copy_bidirectional(&mut c, &mut u).await {
                        tracing::debug!(error = %e, "upgraded tunnel ended");
                    }
                }
                Err(e) => tracing::debug!(error = %e, "upgrade did not complete"),
            }
        });
    }
    strip_hop_by_hop(resp.headers_mut(), switching);
    if switching {
        set(resp.headers_mut(), "connection", "upgrade");
    }
    Ok(resp.map(|b| b.boxed()))
}

/// HTML-escape text for the router's own pages.
pub fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

const STYLE: &str = "body{font:15px/1.45 system-ui,sans-serif;margin:2rem auto;max-width:60rem;padding:0 1rem;color:#1b1b1b}\
h1{font-size:1.4rem}table{border-collapse:collapse;width:100%}td,th{text-align:left;padding:.35rem .6rem;border-bottom:1px solid #ddd;vertical-align:top}\
code{background:#f3f3f3;padding:.1rem .3rem;border-radius:3px}.ok{color:#1a7f37}.down{color:#b42318}.muted{color:#666}.err{color:#b42318}";

/// A small HTML page in the router's house style.
pub fn html_response(status: StatusCode, title: &str, body: &str) -> Response<BoxBody> {
    let page = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <title>{t}</title><style>{STYLE}</style></head><body><h1>{t}</h1>{body}\
         <p class=\"muted\">WeftOS tailnet router (ADR-116)</p></body></html>",
        t = esc(title)
    );
    Response::builder()
        .status(status)
        .header(hyper::header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(hyper::header::CACHE_CONTROL, "no-store")
        .body(Full::new(Bytes::from(page)).map_err(|never| match never {}).boxed())
        .unwrap_or_else(|_| empty(status))
}

/// A JSON document.
pub fn json_response(status: StatusCode, v: &serde_json::Value) -> Response<BoxBody> {
    Response::builder()
        .status(status)
        .header(hyper::header::CONTENT_TYPE, "application/json")
        .header(hyper::header::CACHE_CONTROL, "no-store")
        .body(Full::new(Bytes::from(v.to_string())).map_err(|never| match never {}).boxed())
        .unwrap_or_else(|_| empty(status))
}

fn empty(status: StatusCode) -> Response<BoxBody> {
    let mut r = Response::new(Empty::<Bytes>::new().map_err(|never| match never {}).boxed());
    *r.status_mut() = status;
    r
}

/// `GET http://127.0.0.1:<port><path>` with a deadline over the whole exchange;
/// the body is capped at [`PROBE_BODY_LIMIT`]. Used for health and
/// process-compose probes, never for proxied traffic.
pub async fn http_get(port: u16, path: &str, timeout: Duration) -> Result<(u16, Vec<u8>), String> {
    tokio::time::timeout(timeout, async {
        let stream = TcpStream::connect(("127.0.0.1", port)).await.map_err(|e| format!("connect: {e}"))?;
        let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream)).await.map_err(|e| format!("handshake: {e}"))?;
        tokio::spawn(async move {
            let _ = conn.await;
        });
        let req = Request::builder()
            .uri(path)
            .header(hyper::header::HOST, format!("127.0.0.1:{port}"))
            .header(hyper::header::ACCEPT, "application/json, text/plain, */*")
            .body(Empty::<Bytes>::new())
            .map_err(|e| format!("request: {e}"))?;
        let resp = sender.send_request(req).await.map_err(|e| format!("request: {e}"))?;
        let status = resp.status().as_u16();
        let mut body = resp.into_body();
        let mut out = Vec::new();
        while let Some(frame) = body.frame().await {
            let frame = frame.map_err(|e| format!("body: {e}"))?;
            if let Some(data) = frame.data_ref() {
                let room = PROBE_BODY_LIMIT.saturating_sub(out.len());
                out.extend_from_slice(&data[..data.len().min(room)]);
                if room == 0 {
                    break;
                }
            }
        }
        Ok((status, out))
    })
    .await
    .map_err(|_| format!("no answer from 127.0.0.1:{port} within {} ms", timeout.as_millis()))?
}

#[cfg(test)]
#[path = "router_proxy_tests.rs"]
mod tests;
