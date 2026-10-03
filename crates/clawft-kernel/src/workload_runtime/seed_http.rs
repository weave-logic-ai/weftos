//! HTTP transport and credential storage for the Cognitum Seed adapter.
//!
//! The per-Seed bearer token lives in the operator secret store
//! ([`SeedCredentials`], persisted by [`super::seed_creds::FileCredentials`]);
//! it is only ever placed in an `Authorization` header and never appears
//! in errors, logs or chain payloads.

use std::time::Duration;

use async_trait::async_trait;
use clawft_types::secret::SecretString;
use serde_json::Value;

use super::seed_tls::SeedTls;
use super::types::{LinkSecurity, RuntimeError};

/// HTTP method subset the Seed API uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// GET.
    Get,
    /// POST.
    Post,
    /// PUT.
    Put,
    /// DELETE.
    Delete,
}

/// Largest response body accepted from a Seed.
pub const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

/// Sends requests to one Seed.
#[async_trait]
pub trait SeedTransport: Send + Sync {
    /// Send `method path` with an optional JSON body; returns status and
    /// parsed JSON (`Value::Null` for an empty or non-JSON body).
    async fn request(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
        token: &SecretString,
        timeout: Duration,
    ) -> Result<(u16, Value), RuntimeError>;

    /// How this link authenticates the Seed. Fails closed.
    fn link_security(&self) -> LinkSecurity {
        LinkSecurity::Unpinned
    }
}

/// Validate a Seed base URL: `http(s)://host[:port]`, nothing else.
pub fn validate_base_url(url: &str) -> Result<String, RuntimeError> {
    let bad = || RuntimeError::InvalidConfig("seed base URL must be http(s)://host[:port]".into());
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .ok_or_else(bad)?;
    let host = rest.trim_end_matches('/');
    if host.is_empty()
        || host.len() > 253
        || !host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-:[]".contains(&b))
    {
        return Err(bad());
    }
    Ok(url.trim_end_matches('/').to_string())
}

/// Validate an API path: `/api/v1/...` with safe characters only.
pub fn validate_path(path: &str) -> Result<(), RuntimeError> {
    let ok = path.starts_with("/api/v1/")
        && path.len() <= 256
        && !path.contains("..")
        && path
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/-_.?=&".contains(&b));
    if ok {
        Ok(())
    } else {
        Err(RuntimeError::InvalidConfig(format!(
            "unsafe seed API path {path:?}"
        )))
    }
}

/// reqwest-backed transport.
pub struct HttpSeedTransport {
    base: String,
    client: reqwest::Client,
    pinned: bool,
    lab_opt_in: bool,
}

impl HttpSeedTransport {
    /// Transport for `base_url`.
    ///
    /// An `https://` Seed is trusted per `tls`: WebPKI, or exactly the
    /// operator-pinned certificate or key ([`SeedTls::PinnedSha256`],
    /// [`SeedTls::PinnedSpki`]; a Seed's own certificate is self-signed). Certificate checking is never switched
    /// off. A pin on an `http://` base is refused (it would protect nothing
    /// while looking as if it did).
    pub fn new(base_url: &str, tls: SeedTls) -> Result<Self, RuntimeError> {
        let base = validate_base_url(base_url)?;
        // A Seed is reached directly (USB link-local, LAN or tailnet), never
        // through a proxy: a system or environment proxy cannot route
        // `169.254.0.0/16` and would also see the bearer token. Idle
        // connections are not pooled: the Seed closes a keep-alive
        // connection after a few seconds, and reusing the stale one fails
        // with "error sending request".
        let mut builder = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .pool_max_idle_per_host(0);
        let pinned = matches!(tls, SeedTls::PinnedSha256(_) | SeedTls::PinnedSpki(_));
        if pinned {
            if !base.starts_with("https://") {
                return Err(RuntimeError::InvalidConfig(
                    "a seed TLS pin needs an https:// base URL".into(),
                ));
            }
            let cfg = tls
                .client_config()
                .ok_or_else(|| RuntimeError::Backend("tls config for the pinned seed".into()))?;
            builder = builder.use_preconfigured_tls(cfg);
        }
        let client = builder
            .build()
            .map_err(|e| RuntimeError::Backend(format!("http client: {e}")))?;
        Ok(Self {
            base,
            client,
            pinned,
            lab_opt_in: false,
        })
    }

    /// Operator opt-in for a USB or lab link that cannot be pinned (plain
    /// http, or https verified only by WebPKI). Off by default: without it
    /// a Seed on this link cannot be bound or placed on.
    pub fn allow_unpinned_lab_link(mut self) -> Self {
        tracing::warn!(base = %self.base, "unpinned Seed lab link enabled by operator opt-in");
        self.lab_opt_in = true;
        self
    }
}

#[async_trait]
impl SeedTransport for HttpSeedTransport {
    fn link_security(&self) -> LinkSecurity {
        if self.pinned {
            LinkSecurity::Pinned
        } else if self.lab_opt_in {
            LinkSecurity::LabOptIn
        } else {
            LinkSecurity::Unpinned
        }
    }

    async fn request(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
        token: &SecretString,
        timeout: Duration,
    ) -> Result<(u16, Value), RuntimeError> {
        validate_path(path)?;
        let url = format!("{}{path}", self.base);
        let mut req = match method {
            Method::Get => self.client.get(&url),
            Method::Post => self.client.post(&url),
            Method::Put => self.client.put(&url),
            Method::Delete => self.client.delete(&url),
        }
        .timeout(timeout);
        if !token.is_empty() {
            req = req.bearer_auth(token.expose());
        }
        if let Some(b) = body {
            req = req.json(b);
        }
        // Errors are rendered without the request, which carries the token.
        let resp = req
            .send()
            .await
            .map_err(|e| RuntimeError::Backend(format!("seed {path}: {}", e.without_url())))?;
        let status = resp.status().as_u16();
        let too_large = || RuntimeError::Backend(format!("seed {path}: response too large"));
        if resp
            .content_length()
            .is_some_and(|n| n > MAX_RESPONSE_BYTES as u64)
        {
            return Err(too_large());
        }
        // Read chunk by chunk so the cap bounds memory (a body without a
        // Content-Length, or one that lies about it, is cut off at the cap).
        let mut resp = resp;
        let mut bytes = Vec::new();
        while let Some(chunk) = resp
            .chunk()
            .await
            .map_err(|e| RuntimeError::Backend(format!("seed {path}: {}", e.without_url())))?
        {
            if bytes.len() + chunk.len() > MAX_RESPONSE_BYTES {
                return Err(too_large());
            }
            bytes.extend_from_slice(&chunk);
        }
        let v = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        Ok((status, v))
    }
}

/// Where per-Seed tokens live.
pub trait SeedCredentials: Send + Sync {
    /// Token for `node_id`.
    fn get(&self, node_id: &str) -> Result<SecretString, RuntimeError>;
    /// Store a token for `node_id`.
    fn put(&self, node_id: &str, token: SecretString) -> Result<(), RuntimeError>;
}
