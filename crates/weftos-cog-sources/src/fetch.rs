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

/// Follow up to 5 redirects, but never from https down to http: Cognitum
/// binaries are trusted by sha256 alone, so a downgrade would let a network
/// attacker replace the binary.
#[cfg(feature = "net")]
fn redirect_decision(attempt: reqwest::redirect::Attempt<'_>) -> reqwest::redirect::Action {
    if attempt.previous().len() >= 5 {
        return attempt.error("too many redirects");
    }
    let from_https = attempt.previous().last().is_some_and(|u| u.scheme() == "https");
    if from_https && attempt.url().scheme() != "https" {
        return attempt.error("refusing a redirect from https to http");
    }
    attempt.follow()
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
