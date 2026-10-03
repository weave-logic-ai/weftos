//! MCP-over-HTTP transport for [`super::fleet_inventory`]: the Cognitum
//! cloud MCP (`POST https://api.cognitum.one/v1/mcp`, streamable HTTP,
//! OAuth bearer). Live use is owner-run: no test or agent calls the real
//! cloud. Tests run against a local stub that speaks the same JSON-RPC.
//!
//! The OAuth token comes from the operator secret store
//! ([`OAuthTokens`]) per call and is only placed in an `Authorization`
//! header; errors are rendered without the request.

use std::time::Duration;

use async_trait::async_trait;
use clawft_types::secret::SecretString;
use serde_json::{Value, json};

use super::fleet_inventory::{FleetError, FleetMcp};

/// The Cognitum cloud MCP endpoint.
pub const COGNITUM_MCP_URL: &str = "https://api.cognitum.one/v1/mcp";
const TIMEOUT: Duration = Duration::from_secs(30);
const MAX_REPLY_BYTES: usize = 4 * 1024 * 1024;

/// Where the OAuth token lives (the operator secret store).
pub trait OAuthTokens: Send + Sync {
    /// The current access token.
    fn access_token(&self) -> Result<SecretString, FleetError>;
}

/// reqwest-backed MCP client. Crate-private on purpose: it can call any
/// tool, so the only public way to use it is through
/// [`ReadOnlyFleet::http`](super::fleet_inventory::ReadOnlyFleet::http),
/// which refuses everything but the read tools.
pub(crate) struct HttpFleetMcp {
    url: String,
    client: reqwest::Client,
    tokens: Box<dyn OAuthTokens>,
}

impl HttpFleetMcp {
    /// Client for `url`: `https://`, or plain `http://` to a loopback stub.
    pub(crate) fn new(url: &str, tokens: Box<dyn OAuthTokens>) -> Result<Self, FleetError> {
        let bad = || {
            FleetError::Transport(
                "the fleet MCP URL must be https:// (or http:// to localhost, 127.0.0.1 or ::1)"
                    .into(),
            )
        };
        let parsed = reqwest::Url::parse(url).map_err(|_| bad())?;
        // `Url` normalises the host, so the comparison is exact: a prefix
        // such as `127.0.0.1.evil.com` is a different host.
        let local = matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
        let ok = match parsed.scheme() {
            "https" => parsed.host_str().is_some_and(|h| !h.is_empty()),
            "http" => local && parsed.username().is_empty(),
            _ => false,
        };
        if !ok {
            return Err(bad());
        }
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(TIMEOUT)
            .build()
            .map_err(|e| FleetError::Transport(format!("http client: {e}")))?;
        Ok(Self {
            url: url.to_string(),
            client,
            tokens,
        })
    }

    async fn rpc(
        &self,
        session: Option<&str>,
        body: Value,
    ) -> Result<(Option<String>, Value), FleetError> {
        let token = self.tokens.access_token()?;
        let mut req = self
            .client
            .post(&self.url)
            .header("accept", "application/json, text/event-stream")
            .bearer_auth(token.expose())
            .json(&body);
        if let Some(s) = session {
            req = req.header("mcp-session-id", s);
        }
        let fail = |e: reqwest::Error| FleetError::Transport(e.without_url().to_string());
        let resp = req.send().await.map_err(fail)?;
        let status = resp.status().as_u16();
        if status == 401 || status == 403 {
            return Err(FleetError::Unauthorized(status));
        }
        if !(200..300).contains(&status) {
            return Err(FleetError::Transport(format!("HTTP {status}")));
        }
        let sid = resp
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let sse = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|t| t.starts_with("text/event-stream"));
        let too_large = || FleetError::Transport("reply too large".into());
        if resp
            .content_length()
            .is_some_and(|n| n > MAX_REPLY_BYTES as u64)
        {
            return Err(too_large());
        }
        // Chunk by chunk, so the cap bounds memory even without a (or with
        // a lying) Content-Length.
        let mut resp = resp;
        let mut bytes = Vec::new();
        while let Some(chunk) = resp.chunk().await.map_err(fail)? {
            if bytes.len() + chunk.len() > MAX_REPLY_BYTES {
                return Err(too_large());
            }
            bytes.extend_from_slice(&chunk);
        }
        let text = String::from_utf8_lossy(&bytes);
        let payload = if sse {
            text.lines()
                .filter_map(|l| l.strip_prefix("data:"))
                .map(str::trim)
                .find(|d| d.starts_with('{'))
                .unwrap_or("")
                .to_string()
        } else {
            text.into_owned()
        };
        let v = if payload.trim().is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&payload)
                .map_err(|_| FleetError::Transport("reply is not JSON-RPC".into()))?
        };
        Ok((sid, v))
    }
}

#[async_trait]
impl FleetMcp for HttpFleetMcp {
    async fn call_tool(&self, name: &str, args: Value) -> Result<Value, FleetError> {
        let (sid, init) = self
            .rpc(
                None,
                json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                    "protocolVersion": "2025-03-26", "capabilities": {},
                    "clientInfo": {"name": "weftos-fleet-inventory", "version": "1"}}}),
            )
            .await?;
        if let Some(e) = init.get("error") {
            return Err(FleetError::Transport(format!("initialize: {}", brief(e))));
        }
        self.rpc(
            sid.as_deref(),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        )
        .await?;
        let (_, reply) = self
            .rpc(
                sid.as_deref(),
                json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
                       "params": {"name": name, "arguments": args}}),
            )
            .await?;
        if let Some(e) = reply.get("error") {
            return Err(FleetError::Transport(format!("{name}: {}", brief(e))));
        }
        let result = reply
            .get("result")
            .ok_or_else(|| FleetError::Schema("no result".into()))?;
        if result.get("isError").and_then(Value::as_bool) == Some(true) {
            return Err(FleetError::Transport(format!("{name} reported an error")));
        }
        if let Some(s) = result.get("structuredContent") {
            return Ok(s.clone());
        }
        let text = result
            .get("content")
            .and_then(Value::as_array)
            .and_then(|c| c.iter().find_map(|p| p.get("text").and_then(Value::as_str)))
            .ok_or_else(|| FleetError::Schema("no content text".into()))?;
        serde_json::from_str(text).map_err(|_| FleetError::Schema("content is not JSON".into()))
    }
}

fn brief(e: &Value) -> String {
    e.get("message")
        .and_then(Value::as_str)
        .unwrap_or("error")
        .chars()
        .take(200)
        .collect()
}
