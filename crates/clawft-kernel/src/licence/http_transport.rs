//! The production steward to `weft-licence` transport (ADR-106 sections 4
//! and 7, phase 3).
//!
//! One signed request in, one status and body out, over HTTP:
//!
//! - **Link rules.** A link is built only when it is pinned (`https://` with
//!   a certificate or SubjectPublicKeyInfo pin) or when the operator set the
//!   explicit lab opt-in for it (`allow_unpinned_lab_link`, read from the
//!   runtime dir). `weft-licence` serves plain HTTP on the USB link-local and
//!   tailnet interfaces only, so until it serves TLS (W4) a real deployment
//!   runs on that opt-in, and the confidentiality of the bytes comes from the
//!   link (WireGuard or the cable). Integrity never depends on it: requests
//!   and grants are signed and the bytes are checked against signed hashes.
//! - **Bounds.** Connect, call and artifact timeouts; every response body is
//!   read chunk by chunk under a cap (256 KiB, or the artifact cap for the
//!   artifact path), so a lying or endless body cannot grow memory.
//! - **Fail closed.** No redirects, no proxy, no pooled idle connections, and
//!   only `/licence/v1/` paths with safe characters. Every failure is a
//!   [`LicenceClientError::Transport`]; nothing is retried here.

use std::time::Duration;

use async_trait::async_trait;

use super::client::{
    ARTIFACT_PATH, LicenceClientError, LicenceResponse, LicenceTransport, MAX_RESPONSE_BODY,
};
use super::request::LicenceRequest;
use crate::workload_runtime::LinkSecurity;
use crate::workload_runtime::seed_http::validate_base_url;
use crate::workload_runtime::seed_tls::SeedTls;

/// Default largest artifact accepted (the `weft-licence` default limit).
pub const DEFAULT_MAX_ARTIFACT_BYTES: u64 = 64 * 1024 * 1024;

/// Timeouts and caps of one link.
#[derive(Debug, Clone)]
pub struct TransportLimits {
    /// TCP and TLS connect.
    pub connect_timeout: Duration,
    /// Whole call, for every path but the artifact one.
    pub call_timeout: Duration,
    /// Whole call for `GET /licence/v1/artifact/..` (64 MiB at the Seed's
    /// 4 MiB/s takes 16 s).
    pub artifact_timeout: Duration,
    /// Largest body read for every path but the artifact one.
    pub max_body: usize,
    /// Largest artifact body read.
    pub max_artifact: u64,
}

impl Default for TransportLimits {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(5),
            call_timeout: Duration::from_secs(30),
            artifact_timeout: Duration::from_secs(120),
            max_body: MAX_RESPONSE_BODY,
            max_artifact: DEFAULT_MAX_ARTIFACT_BYTES,
        }
    }
}

/// Where `weft-licence` is and how the link is trusted.
#[derive(Debug, Clone)]
pub struct LicenceLinkConfig {
    /// `http(s)://host[:port]`, nothing else (address the Seed by IP: it
    /// checks the `Host` header against its listen addresses).
    pub url: String,
    /// TLS trust for an `https://` link; a pin makes the link `Pinned`.
    pub tls: SeedTls,
    /// The operator's explicit opt-in for an unpinned (plain http) link.
    pub allow_unpinned_lab_link: bool,
    /// Timeouts and caps.
    pub limits: TransportLimits,
}

/// [`LicenceTransport`] over HTTP.
pub struct HttpLicenceTransport {
    base: String,
    client: reqwest::Client,
    security: LinkSecurity,
    limits: TransportLimits,
}

fn link_err(e: impl std::fmt::Display) -> LicenceClientError {
    LicenceClientError::Transport(e.to_string())
}

impl HttpLicenceTransport {
    /// A transport for `cfg`. Refused unless the link is pinned or opted in.
    pub fn new(cfg: LicenceLinkConfig) -> Result<Self, LicenceClientError> {
        let base = validate_base_url(&cfg.url).map_err(link_err)?;
        let pinned = matches!(cfg.tls, SeedTls::PinnedSha256(_) | SeedTls::PinnedSpki(_));
        let mut builder = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .pool_max_idle_per_host(0)
            .connect_timeout(cfg.limits.connect_timeout);
        if pinned {
            if !base.starts_with("https://") {
                return Err(link_err("a weft-licence TLS pin needs an https:// URL"));
            }
            let tls = cfg.tls.client_config().ok_or_else(|| link_err("tls config for the pinned link"))?;
            builder = builder.use_preconfigured_tls(tls);
        }
        let security = if pinned {
            LinkSecurity::Pinned
        } else if cfg.allow_unpinned_lab_link {
            LinkSecurity::LabOptIn
        } else {
            LinkSecurity::Unpinned
        };
        security.require_pinned("weft-licence").map_err(link_err)?;
        let client = builder.build().map_err(|e| link_err(format!("http client: {e}")))?;
        Ok(Self { base, client, security, limits: cfg.limits })
    }

    /// How this link authenticates `weft-licence`.
    pub fn link_security(&self) -> LinkSecurity {
        self.security
    }

    /// The base URL.
    pub fn base(&self) -> &str {
        &self.base
    }
}

/// Only `weft-licence` paths, short, with no traversal and safe characters.
fn valid_path(path: &str) -> bool {
    path.starts_with("/licence/v1/")
        && path.len() <= 256
        && !path.contains("..")
        && path.bytes().all(|b| b.is_ascii_alphanumeric() || b"/-_.?=&".contains(&b))
}

#[async_trait]
impl LicenceTransport for HttpLicenceTransport {
    async fn call(&self, req: LicenceRequest) -> Result<LicenceResponse, LicenceClientError> {
        if !valid_path(&req.path) {
            return Err(link_err("refused an unsafe weft-licence path"));
        }
        let artifact = req.path.starts_with(ARTIFACT_PATH);
        let (timeout, cap) = if artifact {
            (self.limits.artifact_timeout, usize::try_from(self.limits.max_artifact).unwrap_or(usize::MAX))
        } else {
            (self.limits.call_timeout, self.limits.max_body)
        };
        let url = format!("{}{}", self.base, req.path);
        let mut b = match req.method.as_str() {
            "GET" => self.client.get(&url),
            "POST" => self.client.post(&url),
            other => return Err(link_err(format!("method {other} is not used by weft-licence"))),
        }
        .timeout(timeout);
        if let Some(a) = &req.auth {
            b = b
                .header("x-licence-node", &a.node)
                .header("x-licence-ts", a.timestamp_ms.to_string())
                .header("x-licence-nonce", &a.nonce)
                .header("x-licence-sig", &a.signature);
        }
        if req.method == "POST" {
            b = b.header("content-type", "application/json").body(req.body);
        }
        let mut resp = b.send().await.map_err(|e| link_err(format!("weft-licence: {}", e.without_url())))?;
        let status = resp.status().as_u16();
        let too_large = || link_err("weft-licence: response too large");
        if resp.content_length().is_some_and(|n| n > cap as u64) {
            return Err(too_large());
        }
        let mut body = Vec::new();
        while let Some(chunk) =
            resp.chunk().await.map_err(|e| link_err(format!("weft-licence: {}", e.without_url())))?
        {
            if body.len() + chunk.len() > cap {
                return Err(too_large());
            }
            body.extend_from_slice(&chunk);
        }
        Ok(LicenceResponse { status, body })
    }
}

#[cfg(test)]
mod tests {
    use super::valid_path;

    #[test]
    fn only_weft_licence_paths_with_safe_characters_are_sent() {
        assert!(valid_path("/licence/v1/checkout"));
        assert!(valid_path("/licence/v1/grants?since=3"));
        assert!(valid_path(&format!("/licence/v1/artifact/{}", "a".repeat(64))));
        for p in ["/api/v1/identity", "/licence/v1/../x", "/licence/v1/a b", "/licence/v1/%2e", "licence/v1/x"] {
            assert!(!valid_path(p), "{p}");
        }
        assert!(!valid_path(&format!("/licence/v1/{}", "a".repeat(300))));
    }
}
