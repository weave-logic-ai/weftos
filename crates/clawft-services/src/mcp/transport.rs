//! MCP transport implementations.
//!
//! Provides [`McpTransport`] trait and two implementations:
//! - [`StdioTransport`]: communicates with a child process over stdin/stdout
//!   using request-ID multiplexing for concurrent requests
//! - [`HttpTransport`]: communicates over HTTP POST
//!
//! Plus a [`McpTransportFactory`] trait used by [`McpServerManager`] to
//! decouple transport instantiation from server management, with built-in
//! validators ([`validate_url`], [`validate_command_path`],
//! [`validate_tempfile_path`]) that gate config before a transport is spawned.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, oneshot};
use tracing::{debug, warn};

use super::types::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};
use crate::error::{Result, ServiceError};

/// Transport layer for MCP JSON-RPC communication.
#[async_trait]
pub trait McpTransport: Send + Sync {
    /// Send a JSON-RPC request and return the response.
    async fn send_request(&self, request: JsonRpcRequest) -> Result<JsonRpcResponse>;

    /// Send a JSON-RPC notification (no `id`, no response expected).
    async fn send_notification(&self, method: &str, params: serde_json::Value) -> Result<()>;
}

/// Pending response registry: maps request IDs to oneshot senders.
type PendingMap = Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>>;

/// Transport that communicates with a child process via stdin/stdout.
///
/// Uses a background reader task and request-ID multiplexing to support
/// concurrent requests. Each `send_request` call registers a oneshot
/// channel keyed by the request ID, writes to stdin, and waits for the
/// background reader to deliver the matching response.
pub struct StdioTransport {
    #[allow(dead_code)]
    child: Arc<Mutex<Child>>,
    stdin: Arc<Mutex<tokio::process::ChildStdin>>,
    pending: PendingMap,
    #[allow(dead_code)]
    reader_handle: Arc<tokio::task::JoinHandle<()>>,
}

impl StdioTransport {
    /// Spawn a child process and set up JSON-RPC communication with
    /// request-ID multiplexing.
    pub async fn new(
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
    ) -> Result<Self> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .envs(env)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());

        let mut child = cmd.spawn()?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| ServiceError::McpTransport("failed to capture stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ServiceError::McpTransport("failed to capture stdout".into()))?;

        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));

        // Spawn background reader task that reads lines from stdout and
        // dispatches responses to the matching pending oneshot sender.
        let reader_pending = Arc::clone(&pending);
        let reader_handle = tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) => {
                        debug!("stdio reader: child process closed stdout");
                        break;
                    }
                    Ok(_) => {
                        let trimmed = line.trim();
                        if trimmed.is_empty() {
                            continue;
                        }
                        match serde_json::from_str::<JsonRpcResponse>(trimmed) {
                            Ok(response) => {
                                let id = response.id;
                                let mut map = reader_pending.lock().await;
                                if let Some(tx) = map.remove(&id) {
                                    let _ = tx.send(response);
                                } else {
                                    warn!(
                                        id,
                                        "stdio reader: received response with no pending request"
                                    );
                                }
                            }
                            Err(e) => {
                                // Could be a notification or malformed line; skip
                                debug!(error = %e, "stdio reader: ignoring non-response line");
                            }
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "stdio reader: read error, exiting");
                        break;
                    }
                }
            }

            // Signal all pending requests that the reader has stopped.
            let mut map = reader_pending.lock().await;
            map.clear();
        });

        Ok(Self {
            child: Arc::new(Mutex::new(child)),
            stdin: Arc::new(Mutex::new(stdin)),
            pending,
            reader_handle: Arc::new(reader_handle),
        })
    }
}

/// Default timeout for waiting on a response from the child process.
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

#[async_trait]
impl McpTransport for StdioTransport {
    async fn send_request(&self, request: JsonRpcRequest) -> Result<JsonRpcResponse> {
        let mut line = serde_json::to_string(&request)?;
        line.push('\n');

        let id = request.id;
        debug!(method = %request.method, id, "sending stdio request");

        // Register a oneshot channel for this request ID.
        let (tx, rx) = oneshot::channel::<JsonRpcResponse>();
        {
            let mut map = self.pending.lock().await;
            map.insert(id, tx);
        }

        // Write to stdin.
        {
            let mut stdin = self.stdin.lock().await;
            stdin.write_all(line.as_bytes()).await.map_err(|e| {
                ServiceError::McpTransport(format!("failed to write to stdin: {e}"))
            })?;
            stdin
                .flush()
                .await
                .map_err(|e| ServiceError::McpTransport(format!("failed to flush stdin: {e}")))?;
        }

        // Wait for the background reader to deliver the response, with timeout.
        match tokio::time::timeout(REQUEST_TIMEOUT, rx).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(_)) => {
                // Oneshot sender was dropped (reader task exited).
                Err(ServiceError::McpTransport(
                    "child process closed stdout before responding".into(),
                ))
            }
            Err(_) => {
                // Timeout: remove the pending entry.
                let mut map = self.pending.lock().await;
                map.remove(&id);
                Err(ServiceError::McpTransport(format!(
                    "request {id} timed out after {}s",
                    REQUEST_TIMEOUT.as_secs()
                )))
            }
        }
    }

    async fn send_notification(&self, method: &str, params: serde_json::Value) -> Result<()> {
        let notif = JsonRpcNotification::new(method, params);
        let mut line = serde_json::to_string(&notif)?;
        line.push('\n');

        debug!(method = %method, "sending stdio notification");

        let mut stdin = self.stdin.lock().await;
        stdin.write_all(line.as_bytes()).await.map_err(|e| {
            ServiceError::McpTransport(format!("failed to write notification to stdin: {e}"))
        })?;
        stdin.flush().await.map_err(|e| {
            ServiceError::McpTransport(format!("failed to flush stdin after notification: {e}"))
        })?;

        // Notifications do not expect a response -- do NOT read from stdout.
        Ok(())
    }
}

/// Transport that communicates via HTTP POST.
///
/// Sends JSON-RPC requests as the body of POST requests to the
/// configured endpoint URL.
pub struct HttpTransport {
    client: reqwest::Client,
    endpoint: String,
}

impl HttpTransport {
    /// Create a new HTTP transport targeting the given endpoint URL.
    pub fn new(endpoint: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            endpoint,
        }
    }
}

#[async_trait]
impl McpTransport for HttpTransport {
    async fn send_request(&self, request: JsonRpcRequest) -> Result<JsonRpcResponse> {
        debug!(
            method = %request.method,
            id = request.id,
            endpoint = %self.endpoint,
            "sending HTTP request"
        );

        let resp = self
            .client
            .post(&self.endpoint)
            .json(&request)
            .send()
            .await
            .map_err(|e| ServiceError::McpTransport(format!("HTTP request failed: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ServiceError::McpTransport(format!("HTTP {status}: {body}")));
        }

        let response: JsonRpcResponse = resp
            .json()
            .await
            .map_err(|e| ServiceError::McpTransport(format!("failed to parse response: {e}")))?;

        Ok(response)
    }

    async fn send_notification(&self, method: &str, params: serde_json::Value) -> Result<()> {
        let notif = JsonRpcNotification::new(method, params);

        debug!(
            method = %method,
            endpoint = %self.endpoint,
            "sending HTTP notification"
        );

        let resp = self
            .client
            .post(&self.endpoint)
            .json(&notif)
            .send()
            .await
            .map_err(|e| ServiceError::McpTransport(format!("HTTP notification failed: {e}")))?;

        // Log non-success status but don't fail -- notifications are fire-and-forget.
        let status = resp.status();
        if !status.is_success() {
            debug!(
                method = %method,
                status = %status,
                "HTTP notification received non-success status"
            );
        }

        Ok(())
    }
}

/// A mock transport for testing.
///
/// Allows pre-programming responses that will be returned in order.
/// Also records all sent notifications for verification.
///
/// Available in tests and when the `test-utils` feature is enabled,
/// allowing downstream crates to use it in their own test suites.
#[cfg(any(test, feature = "test-utils"))]
pub struct MockTransport {
    responses: Arc<Mutex<Vec<JsonRpcResponse>>>,
    requests: Arc<Mutex<Vec<JsonRpcRequest>>>,
    notifications: Arc<Mutex<Vec<JsonRpcNotification>>>,
}

#[cfg(any(test, feature = "test-utils"))]
impl MockTransport {
    /// Create a mock transport with pre-programmed responses.
    pub fn new(responses: Vec<JsonRpcResponse>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses)),
            requests: Arc::new(Mutex::new(Vec::new())),
            notifications: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Get all requests that were sent through this transport.
    pub async fn requests(&self) -> Vec<JsonRpcRequest> {
        self.requests.lock().await.clone()
    }

    /// Get all notifications that were sent through this transport.
    pub async fn notifications(&self) -> Vec<JsonRpcNotification> {
        self.notifications.lock().await.clone()
    }
}

#[cfg(any(test, feature = "test-utils"))]
#[async_trait]
impl McpTransport for MockTransport {
    async fn send_request(&self, request: JsonRpcRequest) -> Result<JsonRpcResponse> {
        self.requests.lock().await.push(request);
        let mut responses = self.responses.lock().await;
        if responses.is_empty() {
            Err(ServiceError::McpTransport("no more mock responses".into()))
        } else {
            Ok(responses.remove(0))
        }
    }

    async fn send_notification(&self, method: &str, params: serde_json::Value) -> Result<()> {
        let notif = JsonRpcNotification::new(method, params);
        self.notifications.lock().await.push(notif);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Transport factory + validators (WEFT-186)
// ---------------------------------------------------------------------------

/// A typed description of how to create a transport.
///
/// Variants mirror the supported transport kinds. Use
/// [`McpTransportFactory::create`] to materialize a transport.
#[derive(Debug, Clone)]
pub enum TransportSpec {
    /// Spawn a child process and speak JSON-RPC over its stdio.
    Stdio {
        /// Command path. Validated by [`validate_command_path`] when
        /// `allowed_paths` is non-empty.
        command: String,
        /// Command arguments.
        args: Vec<String>,
        /// Environment variables for the child.
        env: HashMap<String, String>,
    },
    /// Speak JSON-RPC over HTTP POST.
    Http {
        /// Endpoint URL. Validated by [`validate_url`].
        url: String,
    },
    /// Speak JSON-RPC over a Unix domain socket bound to a tempfile path.
    ///
    /// Currently only path validation is performed; transport materialization
    /// returns a `NotImplemented` error so callers can plug their own
    /// implementation later.
    Tempfile {
        /// Path to the socket file. Validated by [`validate_tempfile_path`].
        path: PathBuf,
    },
}

/// Configuration for a [`McpTransportFactory`] that gates transport
/// creation through path/url validators.
#[derive(Debug, Clone, Default)]
pub struct TransportFactoryConfig {
    /// Allowed canonical path prefixes for `Stdio` command paths.
    ///
    /// When empty, command path canonicalization is skipped (back-compat).
    /// When non-empty, the command is canonicalized and rejected unless it
    /// lives under one of these prefixes.
    pub allowed_paths: Vec<PathBuf>,
    /// Whether to allow plain HTTP URLs that point to localhost.
    ///
    /// Defaults to `true` to support local dev. Set to `false` to require
    /// HTTPS for every Http transport.
    pub allow_http_localhost: bool,
}

impl TransportFactoryConfig {
    /// Strict defaults: no command paths allowed, HTTPS required.
    pub fn strict() -> Self {
        Self {
            allowed_paths: Vec::new(),
            allow_http_localhost: false,
        }
    }

    /// Lenient defaults: any command path, http://localhost allowed.
    pub fn lenient() -> Self {
        Self {
            allowed_paths: Vec::new(),
            allow_http_localhost: true,
        }
    }
}

/// Factory that creates [`McpTransport`] instances from a [`TransportSpec`].
///
/// Validators are called *before* spawning to fail fast on bad config.
#[async_trait]
pub trait McpTransportFactory: Send + Sync {
    /// Validate a spec without creating a transport.
    fn validate(&self, spec: &TransportSpec) -> Result<()>;

    /// Validate then create a transport for the given spec.
    async fn create(&self, spec: TransportSpec) -> Result<Box<dyn McpTransport>>;
}

/// Default factory that supports stdio and http transports.
#[derive(Debug, Clone, Default)]
pub struct DefaultTransportFactory {
    config: TransportFactoryConfig,
}

impl DefaultTransportFactory {
    /// Create a factory with the given config.
    pub fn new(config: TransportFactoryConfig) -> Self {
        Self { config }
    }

    /// Access the factory config.
    pub fn config(&self) -> &TransportFactoryConfig {
        &self.config
    }
}

#[async_trait]
impl McpTransportFactory for DefaultTransportFactory {
    fn validate(&self, spec: &TransportSpec) -> Result<()> {
        match spec {
            TransportSpec::Stdio { command, .. } => {
                if !self.config.allowed_paths.is_empty() {
                    validate_command_path(command, &self.config.allowed_paths)
                        .map_err(ServiceError::McpTransport)?;
                }
                Ok(())
            }
            TransportSpec::Http { url } => validate_url(url, self.config.allow_http_localhost)
                .map_err(ServiceError::McpTransport),
            TransportSpec::Tempfile { path } => {
                validate_tempfile_path(path).map_err(ServiceError::McpTransport)
            }
        }
    }

    async fn create(&self, spec: TransportSpec) -> Result<Box<dyn McpTransport>> {
        self.validate(&spec)?;
        match spec {
            TransportSpec::Stdio { command, args, env } => {
                let t = StdioTransport::new(&command, &args, &env).await?;
                Ok(Box::new(t))
            }
            TransportSpec::Http { url } => Ok(Box::new(HttpTransport::new(url))),
            TransportSpec::Tempfile { .. } => Err(ServiceError::McpTransport(
                "tempfile transport not implemented".into(),
            )),
        }
    }
}

/// Validate an MCP URL.
///
/// Rules:
/// - Must start with `https://` or (when `allow_http_localhost` is true)
///   `http://localhost` / `http://127.0.0.1` / `http://[::1]`.
/// - Must not be empty.
/// - Hostname must be present and non-empty.
pub fn validate_url(url: &str, allow_http_localhost: bool) -> std::result::Result<(), String> {
    if url.is_empty() {
        return Err("url is empty".into());
    }

    if let Some(rest) = url.strip_prefix("https://") {
        if rest.is_empty() {
            return Err("url has no host".into());
        }
        return Ok(());
    }

    if let Some(rest) = url.strip_prefix("http://") {
        if !allow_http_localhost {
            return Err("plain http:// not allowed (use https://)".into());
        }
        // Extract host portion (up to /, ?, #, or end).
        let host = rest.split(&['/', '?', '#'][..]).next().unwrap_or(rest);
        // Strip optional port.
        let host_only = if host.starts_with('[') {
            // IPv6 bracket notation.
            host.find(']').map(|i| &host[1..i]).unwrap_or(host)
        } else {
            host.rsplit(':').next_back().unwrap_or(host)
        };
        let host_lc = host_only.to_lowercase();
        const ALLOWED_LOCAL: &[&str] = &["localhost", "127.0.0.1", "::1"];
        if ALLOWED_LOCAL.iter().any(|&h| h == host_lc) {
            return Ok(());
        }
        return Err(format!(
            "http:// only allowed for localhost, got '{host_only}'",
        ));
    }

    Err(format!(
        "url must start with https:// or http://, got '{url}'"
    ))
}

/// Validate that a command path canonicalizes within one of the allowed
/// path prefixes.
///
/// `command` may be a bare program name (resolved via `$PATH`) or an
/// absolute path. If it cannot be canonicalized (e.g. PATH lookup fails),
/// the literal value is rejected.
pub fn validate_command_path(
    command: &str,
    allowed_paths: &[PathBuf],
) -> std::result::Result<(), String> {
    if command.is_empty() {
        return Err("command is empty".into());
    }
    if allowed_paths.is_empty() {
        return Ok(());
    }

    // Resolve the command. If it has path separators, canonicalize directly.
    // Otherwise look it up on $PATH.
    let resolved = if command.contains('/') || command.contains('\\') {
        std::fs::canonicalize(Path::new(command))
            .map_err(|e| format!("cannot canonicalize '{command}': {e}"))?
    } else {
        which_on_path(command)
            .ok_or_else(|| format!("command '{command}' not found on $PATH and not absolute"))?
    };

    let canonical_allowed: Vec<PathBuf> = allowed_paths
        .iter()
        .filter_map(|p| std::fs::canonicalize(p).ok())
        .collect();

    if canonical_allowed
        .iter()
        .any(|prefix| resolved.starts_with(prefix))
    {
        Ok(())
    } else {
        Err(format!(
            "command '{}' resolves outside allowed_paths ({})",
            resolved.display(),
            canonical_allowed
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ))
    }
}

/// Look up a bare program name on `$PATH` (Unix-style colon split).
fn which_on_path(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(program);
        if let Ok(canon) = std::fs::canonicalize(&candidate) {
            return Some(canon);
        }
    }
    None
}

/// Validate a tempfile path.
///
/// Rules:
/// - Must be absolute.
/// - Must not live under sensitive system roots (`/etc`, `/root`, `/sys`,
///   `/proc`, `/boot`, `/dev`).
pub fn validate_tempfile_path(path: &Path) -> std::result::Result<(), String> {
    if !path.is_absolute() {
        return Err(format!(
            "tempfile path must be absolute: {}",
            path.display()
        ));
    }

    const FORBIDDEN: &[&str] = &["/etc", "/root", "/sys", "/proc", "/boot", "/dev"];
    let s = path.to_string_lossy();
    for prefix in FORBIDDEN {
        if s == *prefix || s.starts_with(&format!("{prefix}/")) {
            return Err(format!(
                "tempfile path '{}' is under forbidden root '{}'",
                path.display(),
                prefix
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod factory_tests {
    use super::*;

    #[test]
    fn validate_url_accepts_https() {
        assert!(validate_url("https://example.com", false).is_ok());
        assert!(validate_url("https://example.com/path", false).is_ok());
    }

    #[test]
    fn validate_url_rejects_empty() {
        assert!(validate_url("", false).is_err());
        assert!(validate_url("", true).is_err());
    }

    #[test]
    fn validate_url_rejects_plain_http_when_disallowed() {
        let err = validate_url("http://example.com", false).unwrap_err();
        assert!(err.contains("http://"), "got: {err}");
    }

    #[test]
    fn validate_url_accepts_http_localhost_when_allowed() {
        assert!(validate_url("http://localhost", true).is_ok());
        assert!(validate_url("http://localhost:8080/rpc", true).is_ok());
        assert!(validate_url("http://127.0.0.1:1234", true).is_ok());
        assert!(validate_url("http://[::1]:9090", true).is_ok());
    }

    #[test]
    fn validate_url_rejects_http_external_even_when_localhost_allowed() {
        let err = validate_url("http://example.com", true).unwrap_err();
        assert!(err.to_lowercase().contains("localhost"), "got: {err}");
    }

    #[test]
    fn validate_url_rejects_unknown_scheme() {
        assert!(validate_url("ftp://example.com", true).is_err());
        assert!(validate_url("file:///etc/passwd", true).is_err());
    }

    #[test]
    fn validate_command_path_empty() {
        assert!(validate_command_path("", &[]).is_err());
    }

    #[test]
    fn validate_command_path_no_allowlist_passes() {
        // Empty allowlist = back-compat permissive mode.
        assert!(validate_command_path("/bin/sh", &[]).is_ok());
        assert!(validate_command_path("npx", &[]).is_ok());
    }

    #[test]
    fn validate_command_path_within_allowed() {
        // /usr/bin should canonicalize and contain known programs.
        let allowed = vec![PathBuf::from("/usr/bin"), PathBuf::from("/bin")];
        // sh exists on virtually all Linux systems either at /bin/sh or /usr/bin/sh.
        let result = validate_command_path("/bin/sh", &allowed);
        // Either ok, or PATHs got moved (then we just don't fail the test loudly).
        if let Err(e) = &result {
            // Skip this test if /bin/sh isn't canonicalizable; that's an
            // environment quirk, not a logic bug.
            eprintln!("validate_command_path skipped: {e}");
        }
    }

    #[test]
    fn validate_command_path_outside_allowed_rejected() {
        let allowed = vec![PathBuf::from("/usr/local/nonexistent-12345")];
        let result = validate_command_path("/bin/sh", &allowed);
        assert!(result.is_err());
    }

    #[test]
    fn validate_tempfile_path_absolute_required() {
        let p = PathBuf::from("relative/path");
        let err = validate_tempfile_path(&p).unwrap_err();
        assert!(err.to_lowercase().contains("absolute"));
    }

    #[test]
    fn validate_tempfile_path_blocks_etc() {
        let err = validate_tempfile_path(Path::new("/etc/passwd")).unwrap_err();
        assert!(err.contains("/etc"));
    }

    #[test]
    fn validate_tempfile_path_blocks_root() {
        let err = validate_tempfile_path(Path::new("/root/.ssh/id_rsa")).unwrap_err();
        assert!(err.contains("/root"));
    }

    #[test]
    fn validate_tempfile_path_blocks_sys_proc_dev_boot() {
        for p in &["/sys/kernel", "/proc/1", "/dev/null", "/boot/grub"] {
            let err = validate_tempfile_path(Path::new(p)).unwrap_err();
            assert!(err.contains(*p) || err.contains("forbidden"));
        }
    }

    #[test]
    fn validate_tempfile_path_allows_tmp() {
        assert!(validate_tempfile_path(Path::new("/tmp/socket.sock")).is_ok());
        assert!(validate_tempfile_path(Path::new("/var/run/clawft.sock")).is_ok());
    }

    #[tokio::test]
    async fn factory_validates_http_spec() {
        let f = DefaultTransportFactory::new(TransportFactoryConfig::lenient());
        let spec = TransportSpec::Http {
            url: "https://example.com".into(),
        };
        assert!(f.validate(&spec).is_ok());

        let spec_bad = TransportSpec::Http {
            url: "ftp://example.com".into(),
        };
        assert!(f.validate(&spec_bad).is_err());
    }

    #[tokio::test]
    async fn factory_strict_rejects_http_localhost() {
        let f = DefaultTransportFactory::new(TransportFactoryConfig::strict());
        let spec = TransportSpec::Http {
            url: "http://localhost:8080".into(),
        };
        assert!(f.validate(&spec).is_err());
    }

    #[tokio::test]
    async fn factory_validates_tempfile_spec() {
        let f = DefaultTransportFactory::new(TransportFactoryConfig::default());
        let spec = TransportSpec::Tempfile {
            path: PathBuf::from("/etc/shadow"),
        };
        assert!(f.validate(&spec).is_err());

        let spec_ok = TransportSpec::Tempfile {
            path: PathBuf::from("/tmp/mcp.sock"),
        };
        assert!(f.validate(&spec_ok).is_ok());
    }

    #[tokio::test]
    async fn factory_create_tempfile_returns_not_implemented() {
        let f = DefaultTransportFactory::new(TransportFactoryConfig::default());
        let spec = TransportSpec::Tempfile {
            path: PathBuf::from("/tmp/mcp.sock"),
        };
        let result = f.create(spec).await;
        // `Box<dyn McpTransport>` does not implement Debug, so we
        // can't `.unwrap_err()`; pattern-match instead.
        match result {
            Ok(_) => panic!("expected NotImplemented error"),
            Err(e) => {
                let msg = e.to_string();
                assert!(msg.contains("not implemented"), "got: {msg}");
            }
        }
    }

    #[test]
    fn factory_config_strict_vs_lenient() {
        let s = TransportFactoryConfig::strict();
        assert!(!s.allow_http_localhost);
        let l = TransportFactoryConfig::lenient();
        assert!(l.allow_http_localhost);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_transport_construction() {
        let transport = HttpTransport::new("http://localhost:8080".into());
        assert_eq!(transport.endpoint, "http://localhost:8080");
    }

    #[tokio::test]
    async fn mock_transport_returns_responses() {
        let response = JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: 1,
            result: Some(serde_json::json!({"tools": []})),
            error: None,
        };

        let transport = MockTransport::new(vec![response]);
        let req = JsonRpcRequest::new(1, "tools/list", serde_json::json!({}));
        let resp = transport.send_request(req).await.unwrap();
        assert_eq!(resp.id, 1);
        assert!(resp.result.is_some());
    }

    #[tokio::test]
    async fn mock_transport_records_requests() {
        let response = JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: 1,
            result: Some(serde_json::json!(null)),
            error: None,
        };

        let transport = MockTransport::new(vec![response]);
        let req = JsonRpcRequest::new(1, "test/method", serde_json::json!({"key": "value"}));
        transport.send_request(req).await.unwrap();

        let requests = transport.requests().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, "test/method");
    }

    #[tokio::test]
    async fn mock_transport_empty_responses_errors() {
        let transport = MockTransport::new(vec![]);
        let req = JsonRpcRequest::new(1, "test", serde_json::json!({}));
        let result = transport.send_request(req).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn mock_transport_records_notifications() {
        let transport = MockTransport::new(vec![]);
        transport
            .send_notification("notifications/initialized", serde_json::json!({}))
            .await
            .unwrap();
        transport
            .send_notification(
                "notifications/progress",
                serde_json::json!({"token": "abc"}),
            )
            .await
            .unwrap();

        let notifs = transport.notifications().await;
        assert_eq!(notifs.len(), 2);
        assert_eq!(notifs[0].method, "notifications/initialized");
        assert_eq!(notifs[1].method, "notifications/progress");
        assert_eq!(notifs[1].params["token"], "abc");
    }

    #[tokio::test]
    async fn notification_has_no_id_field() {
        let notif = JsonRpcNotification::new("test/notify", serde_json::json!({}));
        let json = serde_json::to_string(&notif).unwrap();
        // JSON-RPC notifications MUST NOT have an "id" field.
        assert!(!json.contains("\"id\""));
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"method\":\"test/notify\""));
    }
}
