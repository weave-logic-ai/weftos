//! Reading registries and binaries. The library never opens a socket on its
//! own: callers pass a [`Reader`]. [`FsReader`] handles local paths, and the
//! `net` feature adds [`HttpReader`] for http(s) URLs.

use std::io::Read;
use std::path::Path;

use crate::error::{Result, SourceError};

/// Largest registry file read.
pub const MAX_REGISTRY_BYTES: u64 = 8 * 1024 * 1024;
/// Largest cog binary read.
pub const MAX_BINARY_BYTES: u64 = 256 * 1024 * 1024;

/// Reads bytes from a path or URL. `max` bounds the read.
pub trait Reader {
    /// Read `location`, refusing anything larger than `max` bytes.
    fn read(&self, location: &str, max: u64) -> Result<Vec<u8>>;
}

/// True for `http://` and `https://` locations.
pub fn is_url(s: &str) -> bool {
    s.starts_with("http://") || s.starts_with("https://")
}

/// Join a base (URL or directory) and a relative path.
pub fn join(base: &str, rel: &str) -> String {
    let rel = rel.trim_start_matches('/');
    if is_url(base) {
        format!("{}/{}", base.trim_end_matches('/'), rel)
    } else {
        Path::new(base).join(rel).to_string_lossy().into_owned()
    }
}

/// The directory (or URL prefix) that holds a file location.
pub fn parent(location: &str) -> String {
    if is_url(location) {
        match location.rfind('/') {
            Some(i) if i > "https://".len() => location[..i].to_string(),
            _ => location.to_string(),
        }
    } else {
        Path::new(location)
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| ".".into())
    }
}

/// Reader for local paths. A URL is refused so a test or an offline run can
/// never reach the network by accident.
pub struct FsReader;

impl Reader for FsReader {
    fn read(&self, location: &str, max: u64) -> Result<Vec<u8>> {
        if is_url(location) {
            return Err(SourceError::Fetch {
                location: location.into(),
                msg: "network access is not enabled in this reader".into(),
            });
        }
        let f = |e: std::io::Error| SourceError::Fetch { location: location.into(), msg: e.to_string() };
        let file = std::fs::File::open(location).map_err(f)?;
        let mut out = Vec::new();
        file.take(max + 1).read_to_end(&mut out).map_err(f)?;
        if out.len() as u64 > max {
            return Err(SourceError::Fetch { location: location.into(), msg: format!("larger than {max} bytes") });
        }
        Ok(out)
    }
}

/// Reader for http(s) URLs, falling back to the filesystem for paths.
#[cfg(feature = "net")]
pub struct HttpReader {
    client: reqwest::blocking::Client,
}

#[cfg(feature = "net")]
impl HttpReader {
    /// New reader with a 60 s timeout.
    pub fn new() -> Self {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .redirect(reqwest::redirect::Policy::custom(redirect_decision))
            .build()
            .expect("reqwest client");
        Self { client }
    }
}

/// Why a redirect must be refused, if it must: more than 5 hops, or a step from https down to
/// http. Cognitum binaries are trusted by sha256 alone, so a downgrade would let a network
/// attacker replace the binary.
pub fn redirect_blocked(previous_scheme: Option<&str>, next_scheme: &str, hops: usize) -> Option<&'static str> {
    if hops >= 5 {
        return Some("too many redirects");
    }
    if previous_scheme == Some("https") && next_scheme != "https" {
        return Some("refusing a redirect from https to http");
    }
    None
}

#[cfg(feature = "net")]
fn redirect_decision(attempt: reqwest::redirect::Attempt<'_>) -> reqwest::redirect::Action {
    let prev = attempt.previous().last().map(|u| u.scheme().to_string());
    match redirect_blocked(prev.as_deref(), attempt.url().scheme(), attempt.previous().len()) {
        Some(why) => attempt.error(why),
        None => attempt.follow(),
    }
}

#[cfg(feature = "net")]
impl Default for HttpReader {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "net")]
impl Reader for HttpReader {
    fn read(&self, location: &str, max: u64) -> Result<Vec<u8>> {
        if !is_url(location) {
            return FsReader.read(location, max);
        }
        let f = |m: String| SourceError::Fetch { location: location.into(), msg: m };
        let resp = self.client.get(location).send().map_err(|e| f(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(f(format!("HTTP {}", resp.status())));
        }
        let mut out = Vec::new();
        resp.take(max + 1).read_to_end(&mut out).map_err(|e| f(e.to_string()))?;
        if out.len() as u64 > max {
            return Err(f(format!("larger than {max} bytes")));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirect_policy_refuses_downgrade_and_loops() {
        assert!(redirect_blocked(Some("https"), "http", 1).unwrap().contains("https to http"));
        assert!(redirect_blocked(Some("https"), "https", 1).is_none());
        assert!(redirect_blocked(Some("http"), "http", 1).is_none());
        assert!(redirect_blocked(Some("http"), "https", 1).is_none());
        assert!(redirect_blocked(Some("https"), "https", 5).unwrap().contains("too many"));
    }

    /// Loopback server: connection `i` gets `respond(i, base_url)`.
    #[cfg(feature = "net")]
    fn serve(n: usize, respond: impl Fn(usize, &str) -> String + Send + 'static) -> String {
        use std::io::{Read, Write};
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        let b = base.clone();
        std::thread::spawn(move || {
            for i in 0..n {
                let Ok((mut c, _)) = l.accept() else { return };
                let mut buf = [0u8; 2048];
                let _ = c.read(&mut buf);
                let _ = c.write_all(respond(i, &b).as_bytes());
            }
        });
        base
    }

    #[cfg(feature = "net")]
    fn redirect_to(base: &str, path: &str) -> String {
        format!("HTTP/1.1 302 Found\r\nLocation: {base}{path}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
    }

    /// Loopback only (an https server needs certificates this crate does not carry), so the
    /// https-to-http step itself is covered by `redirect_blocked`; here the live client is shown
    /// to follow a 302 to http and to stop a loop.
    #[cfg(feature = "net")]
    #[test]
    fn http_reader_follows_a_302_to_http_but_stops_a_redirect_loop() {
        let r = HttpReader::new();
        let base = serve(2, |i, b| {
            if i == 0 {
                redirect_to(b, "/ok")
            } else {
                "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nhi".to_string()
            }
        });
        assert_eq!(r.read(&format!("{base}/start"), 1024).unwrap(), b"hi");

        let looping = serve(8, |_, b| redirect_to(b, "/again"));
        let err = r.read(&format!("{looping}/loop"), 1024).unwrap_err();
        assert_eq!(err.code(), "fetch_failed", "{err}");
    }
}
