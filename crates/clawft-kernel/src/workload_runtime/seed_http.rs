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

use super::types::RuntimeError;

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
}

impl HttpSeedTransport {
    /// Transport for `base_url`. `accept_self_signed` allows the Seed's
    /// self-signed TLS certificate (it has no CA-issued one); prefer the
    /// USB `http://` address or a private overlay when using it.
    pub fn new(base_url: &str, accept_self_signed: bool) -> Result<Self, RuntimeError> {
        let base = validate_base_url(base_url)?;
        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(accept_self_signed)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| RuntimeError::Backend(format!("http client: {e}")))?;
        Ok(Self { base, client })
    }
}

#[async_trait]
impl SeedTransport for HttpSeedTransport {
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
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| RuntimeError::Backend(format!("seed {path}: {}", e.without_url())))?;
        if bytes.len() > MAX_RESPONSE_BYTES {
            return Err(RuntimeError::Backend(format!(
                "seed {path}: response too large"
            )));
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
