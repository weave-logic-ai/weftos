//! Dynamic MCP server management.
//!
//! [`McpServerManager`] provides runtime management of MCP server
//! connections, including add/remove/list operations and hot-reload
//! via drain-and-swap protocol.
//!
//! # CLI commands
//!
//! ```text
//! weft mcp add <name> <command|url>
//! weft mcp list
//! weft mcp remove <name>
//! ```
//!
//! # Hot-reload protocol
//!
//! When `clawft.toml` changes:
//! 1. File watcher detects change (debounce 500ms).
//! 2. Diff old and new server lists.
//! 3. New servers: connect immediately.
//! 4. Removed servers: drain in-flight calls (30s), then disconnect.
//! 5. Changed servers: remove + add.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::time::Instant;
use tracing::{debug, info, warn};

use super::transport::{
    DefaultTransportFactory, McpTransport, McpTransportFactory, TransportFactoryConfig,
    TransportSpec,
};
use crate::error::Result;

/// Default debounce for config file changes.
const DEBOUNCE_MS: u64 = 500;

/// Default drain timeout for removing servers.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(30);

/// Polling interval while waiting for in-flight calls to drain.
const DRAIN_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Tracks in-flight `tools/call` invocations for a managed server.
///
/// Held via `Arc` inside [`ManagedMcpServer`] and cloned into a
/// [`CallGuard`] for the lifetime of each call so that
/// [`McpServerManager::remove_server`] can await drain.
#[derive(Debug, Default)]
pub struct InFlightCounter {
    count: AtomicUsize,
}

impl InFlightCounter {
    /// Current number of in-flight calls.
    pub fn load(&self) -> usize {
        self.count.load(Ordering::SeqCst)
    }

    fn inc(&self) {
        self.count.fetch_add(1, Ordering::SeqCst);
    }

    fn dec(&self) {
        // saturating dec
        let _ = self
            .count
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                Some(n.saturating_sub(1))
            });
    }
}

/// RAII guard that increments the [`InFlightCounter`] on construction
/// and decrements it on drop. Returned by
/// [`McpServerManager::begin_call`] for the duration of a `tools/call`.
#[must_use = "drop the CallGuard when the in-flight call completes"]
pub struct CallGuard {
    counter: Arc<InFlightCounter>,
}

impl CallGuard {
    fn new(counter: Arc<InFlightCounter>) -> Self {
        counter.inc();
        Self { counter }
    }
}

impl Drop for CallGuard {
    fn drop(&mut self) {
        self.counter.dec();
    }
}

/// Status of a managed MCP server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServerStatus {
    /// Server is connected and ready.
    Connected,
    /// Server is connecting (handshake in progress).
    Connecting,
    /// Server is draining in-flight requests before disconnection.
    Draining,
    /// Server is disconnected.
    Disconnected,
    /// Server connection failed.
    Error,
}

/// Configuration for a managed MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Server name (unique identifier).
    pub name: String,

    /// Command to spawn the server (e.g., "npx", "claude").
    pub command: String,

    /// Arguments for the command.
    #[serde(default)]
    pub args: Vec<String>,

    /// Optional environment variables.
    #[serde(default)]
    pub env: HashMap<String, String>,

    /// Optional URL for HTTP-based servers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// A managed MCP server with its connection state.
#[derive(Debug)]
pub struct ManagedMcpServer {
    /// Server configuration.
    pub config: McpServerConfig,
    /// Current connection status.
    pub status: ServerStatus,
    /// Tools discovered from this server (tool_name -> description).
    pub tools: Vec<String>,
    /// When the server was added.
    pub added_at: chrono::DateTime<chrono::Utc>,
    /// In-flight `tools/call` counter for drain-and-swap protocol.
    ///
    /// Cloned into each [`CallGuard`] for the lifetime of a call so
    /// that [`McpServerManager::remove_server`] can await zero before
    /// dropping the server.
    pub in_flight: Arc<InFlightCounter>,
}

/// Manager for dynamically adding, removing, and listing MCP servers.
///
/// Provides the runtime layer for `weft mcp add/list/remove` commands
/// and hot-reload on config changes.
pub struct McpServerManager {
    /// Active servers keyed by name.
    servers: HashMap<String, ManagedMcpServer>,
    /// Debounce duration for config file changes.
    debounce: Duration,
    /// Drain timeout for removing servers.
    drain_timeout: Duration,
    /// Transport factory used by [`Self::create_transport`].
    factory: Arc<dyn McpTransportFactory>,
}

impl McpServerManager {
    /// Create a new server manager with default settings.
    pub fn new() -> Self {
        Self::with_factory(Arc::new(DefaultTransportFactory::new(
            TransportFactoryConfig::lenient(),
        )))
    }

    /// Create a new server manager with the given transport factory.
    pub fn with_factory(factory: Arc<dyn McpTransportFactory>) -> Self {
        Self {
            servers: HashMap::new(),
            debounce: Duration::from_millis(DEBOUNCE_MS),
            drain_timeout: DRAIN_TIMEOUT,
            factory,
        }
    }

    /// Materialize a transport for a registered server config via the
    /// configured [`McpTransportFactory`].
    ///
    /// Validators run before any spawn; bad configs fail fast.
    pub async fn create_transport(&self, name: &str) -> Result<Box<dyn McpTransport>> {
        let server = self.servers.get(name).ok_or_else(|| {
            crate::error::ServiceError::McpTransport(format!("server '{name}' not registered"))
        })?;
        let spec = transport_spec_for(&server.config);
        self.factory.create(spec).await
    }

    /// Validate a server config without creating a transport.
    pub fn validate(&self, name: &str) -> Result<()> {
        let server = self.servers.get(name).ok_or_else(|| {
            crate::error::ServiceError::McpTransport(format!("server '{name}' not registered"))
        })?;
        let spec = transport_spec_for(&server.config);
        self.factory.validate(&spec)
    }

    /// Access the underlying factory.
    pub fn factory(&self) -> &Arc<dyn McpTransportFactory> {
        &self.factory
    }

    /// Add a new MCP server.
    ///
    /// If a server with the same name already exists, it is replaced
    /// (the old server is drained first).
    ///
    /// This method does not connect to the server -- call
    /// [`connect_server`](Self::connect_server) to initiate connection.
    pub fn add_server(&mut self, config: McpServerConfig) -> &ManagedMcpServer {
        let name = config.name.clone();

        if self.servers.contains_key(&name) {
            info!(name = %name, "replacing existing MCP server");
        }

        let server = ManagedMcpServer {
            config,
            status: ServerStatus::Disconnected,
            tools: Vec::new(),
            added_at: chrono::Utc::now(),
            in_flight: Arc::new(InFlightCounter::default()),
        };

        self.servers.insert(name.clone(), server);
        debug!(name = %name, "added MCP server");
        self.servers.get(&name).unwrap()
    }

    /// Acquire a [`CallGuard`] for an in-flight `tools/call`.
    ///
    /// Returns `None` if the server is not registered. The guard
    /// keeps the in-flight counter elevated for its lifetime so that
    /// [`Self::remove_server`] (drain-and-swap) waits before
    /// disconnecting.
    pub fn begin_call(&self, name: &str) -> Option<CallGuard> {
        self.servers
            .get(name)
            .map(|s| CallGuard::new(s.in_flight.clone()))
    }

    /// Remove an MCP server by name (synchronous, non-draining).
    ///
    /// Use [`Self::remove_server_drain`] when in-flight calls must
    /// complete before disconnection. This method takes the server
    /// out of the registry immediately and returns `true` on success.
    ///
    /// Even when called synchronously, any outstanding [`CallGuard`]
    /// keeps the server's [`InFlightCounter`] alive via its `Arc`,
    /// so the in-flight call is *not* dropped — only the registry
    /// entry is removed.
    pub fn remove_server(&mut self, name: &str) -> bool {
        if let Some(mut server) = self.servers.remove(name) {
            server.status = ServerStatus::Draining;
            let in_flight = server.in_flight.load();
            info!(
                name = %name,
                in_flight,
                "removed MCP server (synchronous remove; outstanding calls keep their CallGuards)",
            );
            true
        } else {
            warn!(name = %name, "MCP server not found for removal");
            false
        }
    }

    /// Drain-and-swap removal of an MCP server.
    ///
    /// Marks the server as `Draining`, **takes** it out of the
    /// registry so no new calls can be started against it, then
    /// awaits the in-flight `tools/call` count to reach zero (or
    /// the drain timeout to elapse). Outstanding calls hold a
    /// [`CallGuard`] which keeps their [`InFlightCounter`] alive
    /// via `Arc` even after this method returns; this method waits
    /// for them rather than cancelling them.
    ///
    /// Returns the [`ManagedMcpServer`] that was removed (so the
    /// caller can close any owned transport), or `None` if no
    /// server with that name was registered.
    pub async fn remove_server_drain(&mut self, name: &str) -> Option<ManagedMcpServer> {
        let mut server = self.servers.remove(name)?;
        server.status = ServerStatus::Draining;

        let in_flight = server.in_flight.clone();
        let timeout = self.drain_timeout;
        let deadline = Instant::now() + timeout;

        info!(
            name = %name,
            in_flight = in_flight.load(),
            "draining MCP server before disconnect",
        );

        while in_flight.load() > 0 {
            if Instant::now() >= deadline {
                warn!(
                    name = %name,
                    remaining = in_flight.load(),
                    timeout_secs = timeout.as_secs(),
                    "drain timeout elapsed; in-flight calls may still be running",
                );
                break;
            }
            tokio::time::sleep(DRAIN_POLL_INTERVAL).await;
        }

        server.status = ServerStatus::Disconnected;
        info!(name = %name, "MCP server drained and removed");
        Some(server)
    }

    /// List all managed servers.
    pub fn list_servers(&self) -> Vec<&ManagedMcpServer> {
        self.servers.values().collect()
    }

    /// Get a server by name.
    pub fn get_server(&self, name: &str) -> Option<&ManagedMcpServer> {
        self.servers.get(name)
    }

    /// Get a mutable reference to a server by name.
    pub fn get_server_mut(&mut self, name: &str) -> Option<&mut ManagedMcpServer> {
        self.servers.get_mut(name)
    }

    /// Number of managed servers.
    pub fn server_count(&self) -> usize {
        self.servers.len()
    }

    /// Mark a server as connected and set its discovered tools.
    pub fn mark_connected(&mut self, name: &str, tools: Vec<String>) {
        if let Some(server) = self.servers.get_mut(name) {
            server.status = ServerStatus::Connected;
            server.tools = tools;
            debug!(name = %name, tool_count = server.tools.len(), "server connected");
        }
    }

    /// Mark a server as errored.
    pub fn mark_error(&mut self, name: &str) {
        if let Some(server) = self.servers.get_mut(name) {
            server.status = ServerStatus::Error;
            warn!(name = %name, "server marked as error");
        }
    }

    /// Apply a config diff (hot-reload).
    ///
    /// Given the new set of server configs, determines which servers
    /// to add, remove, or update.
    ///
    /// Returns `(added, removed, changed)` counts.
    pub fn apply_config_diff(
        &mut self,
        new_configs: Vec<McpServerConfig>,
    ) -> (usize, usize, usize) {
        let new_names: HashMap<String, &McpServerConfig> =
            new_configs.iter().map(|c| (c.name.clone(), c)).collect();
        let old_names: Vec<String> = self.servers.keys().cloned().collect();

        let mut added = 0;
        let mut removed = 0;
        let mut changed = 0;

        // Remove servers no longer in config.
        for name in &old_names {
            if !new_names.contains_key(name) {
                self.remove_server(name);
                removed += 1;
            }
        }

        // Add or update servers.
        for config in new_configs {
            let name = config.name.clone();
            if let Some(existing) = self.servers.get(&name) {
                // Check if config changed.
                if existing.config.command != config.command || existing.config.args != config.args
                {
                    self.add_server(config);
                    changed += 1;
                }
            } else {
                self.add_server(config);
                added += 1;
            }
        }

        info!(added, removed, changed, "applied MCP server config diff");

        (added, removed, changed)
    }

    /// Debounce duration for config changes.
    pub fn debounce(&self) -> Duration {
        self.debounce
    }

    /// Drain timeout for removing servers.
    pub fn drain_timeout(&self) -> Duration {
        self.drain_timeout
    }
}

impl Default for McpServerManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Translate a [`McpServerConfig`] to a [`TransportSpec`].
///
/// Choice rules:
/// - If `command` is non-empty -> [`TransportSpec::Stdio`].
/// - Else if `url` is set and non-empty -> [`TransportSpec::Http`].
/// - Else falls back to a `Tempfile` spec under the empty path so
///   validators reject it with a clear error.
fn transport_spec_for(config: &McpServerConfig) -> TransportSpec {
    if !config.command.is_empty() {
        TransportSpec::Stdio {
            command: config.command.clone(),
            args: config.args.clone(),
            env: config.env.clone(),
        }
    } else if let Some(url) = config.url.as_ref().filter(|s| !s.is_empty()) {
        TransportSpec::Http { url: url.clone() }
    } else {
        // Will be rejected by validate_tempfile_path (empty path).
        TransportSpec::Tempfile {
            path: PathBuf::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config(name: &str) -> McpServerConfig {
        McpServerConfig {
            name: name.into(),
            command: "npx".into(),
            args: vec!["-y".into(), format!("{name}-mcp")],
            env: HashMap::new(),
            url: None,
        }
    }

    #[test]
    fn add_and_list_servers() {
        let mut mgr = McpServerManager::new();
        mgr.add_server(test_config("github"));
        mgr.add_server(test_config("slack"));

        assert_eq!(mgr.server_count(), 2);
        let servers = mgr.list_servers();
        assert_eq!(servers.len(), 2);
    }

    #[test]
    fn remove_server() {
        let mut mgr = McpServerManager::new();
        mgr.add_server(test_config("github"));
        assert_eq!(mgr.server_count(), 1);

        assert!(mgr.remove_server("github"));
        assert_eq!(mgr.server_count(), 0);
    }

    #[test]
    fn remove_nonexistent_returns_false() {
        let mut mgr = McpServerManager::new();
        assert!(!mgr.remove_server("nonexistent"));
    }

    #[test]
    fn get_server() {
        let mut mgr = McpServerManager::new();
        mgr.add_server(test_config("github"));

        let server = mgr.get_server("github");
        assert!(server.is_some());
        assert_eq!(server.unwrap().config.name, "github");
        assert_eq!(server.unwrap().status, ServerStatus::Disconnected);

        assert!(mgr.get_server("missing").is_none());
    }

    #[test]
    fn mark_connected() {
        let mut mgr = McpServerManager::new();
        mgr.add_server(test_config("github"));
        mgr.mark_connected("github", vec!["create_issue".into(), "list_repos".into()]);

        let server = mgr.get_server("github").unwrap();
        assert_eq!(server.status, ServerStatus::Connected);
        assert_eq!(server.tools.len(), 2);
    }

    #[test]
    fn mark_error() {
        let mut mgr = McpServerManager::new();
        mgr.add_server(test_config("github"));
        mgr.mark_error("github");

        let server = mgr.get_server("github").unwrap();
        assert_eq!(server.status, ServerStatus::Error);
    }

    #[test]
    fn replace_existing_server() {
        let mut mgr = McpServerManager::new();
        mgr.add_server(test_config("github"));
        mgr.mark_connected("github", vec!["tool1".into()]);

        // Replace with new config.
        mgr.add_server(test_config("github"));
        let server = mgr.get_server("github").unwrap();
        // Status reset to Disconnected after replacement.
        assert_eq!(server.status, ServerStatus::Disconnected);
        assert!(server.tools.is_empty());
    }

    #[test]
    fn apply_config_diff() {
        let mut mgr = McpServerManager::new();
        mgr.add_server(test_config("github"));
        mgr.add_server(test_config("slack"));

        let new_configs = vec![
            test_config("github"), // kept
            test_config("jira"),   // added
                                   // slack removed
        ];

        let (added, removed, _changed) = mgr.apply_config_diff(new_configs);
        assert_eq!(added, 1); // jira
        assert_eq!(removed, 1); // slack
        assert_eq!(mgr.server_count(), 2); // github + jira
        assert!(mgr.get_server("github").is_some());
        assert!(mgr.get_server("jira").is_some());
        assert!(mgr.get_server("slack").is_none());
    }

    #[test]
    fn server_status_serde() {
        let json = serde_json::to_string(&ServerStatus::Connected).unwrap();
        assert_eq!(json, "\"connected\"");

        let restored: ServerStatus = serde_json::from_str("\"draining\"").unwrap();
        assert_eq!(restored, ServerStatus::Draining);
    }

    #[test]
    fn debounce_and_drain_defaults() {
        let mgr = McpServerManager::new();
        assert_eq!(mgr.debounce(), Duration::from_millis(500));
        assert_eq!(mgr.drain_timeout(), Duration::from_secs(30));
    }

    // ── drain-and-swap tests (WEFT-181) ─────────────────────────────────

    #[test]
    fn begin_call_increments_in_flight() {
        let mut mgr = McpServerManager::new();
        mgr.add_server(test_config("github"));

        let counter = mgr.get_server("github").unwrap().in_flight.clone();
        assert_eq!(counter.load(), 0);
        let guard = mgr.begin_call("github").unwrap();
        assert_eq!(counter.load(), 1);
        drop(guard);
        assert_eq!(counter.load(), 0);
    }

    #[test]
    fn begin_call_unknown_returns_none() {
        let mgr = McpServerManager::new();
        assert!(mgr.begin_call("ghost").is_none());
    }

    #[tokio::test]
    async fn remove_server_drain_completes_when_idle() {
        let mut mgr = McpServerManager::new();
        mgr.add_server(test_config("github"));
        let removed = mgr.remove_server_drain("github").await;
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().status, ServerStatus::Disconnected);
        assert!(mgr.get_server("github").is_none());
    }

    #[tokio::test]
    async fn remove_server_drain_unknown_returns_none() {
        let mut mgr = McpServerManager::new();
        let removed = mgr.remove_server_drain("ghost").await;
        assert!(removed.is_none());
    }

    #[tokio::test]
    async fn remove_during_in_flight_call_does_not_drop_call() {
        // Acceptance test from WEFT-181 plan:
        // remove during a tools/call doesn't drop the in-flight call.
        let mut mgr = McpServerManager::new();
        mgr.add_server(test_config("github"));

        // Take a guard to simulate an in-flight call.
        let guard = mgr.begin_call("github").unwrap();
        let counter = mgr.get_server("github").unwrap().in_flight.clone();
        assert_eq!(counter.load(), 1);

        // Configure a short drain timeout so the test is fast.
        mgr.drain_timeout = Duration::from_millis(200);

        // Spawn the drain in a task; it should still see in_flight=1
        // and time out without prematurely dropping the call.
        let mgr_drain = tokio::spawn(async move {
            // Move mgr into the task and call drain.
            mgr.remove_server_drain("github").await
        });

        // Hold the guard for longer than the drain timeout.
        tokio::time::sleep(Duration::from_millis(400)).await;

        // The guard's counter should still be >0 (we're still holding it).
        assert_eq!(counter.load(), 1, "in-flight call must not be dropped");

        drop(guard);
        // After dropping the guard, the counter goes back to 0.
        assert_eq!(counter.load(), 0);

        // The drain task returned the removed server (post-timeout).
        let removed = mgr_drain.await.unwrap();
        assert!(removed.is_some());
    }

    // ── transport factory wiring tests (WEFT-186) ───────────────────────

    #[tokio::test]
    async fn manager_validates_url_via_factory() {
        let factory = Arc::new(DefaultTransportFactory::new(
            TransportFactoryConfig::strict(),
        ));
        let mut mgr = McpServerManager::with_factory(factory);
        mgr.add_server(McpServerConfig {
            name: "remote".into(),
            command: String::new(),
            args: Vec::new(),
            env: HashMap::new(),
            url: Some("http://evil.example.com".into()),
        });
        let err = mgr.validate("remote").unwrap_err();
        assert!(
            err.to_string().to_lowercase().contains("http"),
            "got: {err}"
        );
    }

    #[tokio::test]
    async fn manager_accepts_https_url_via_factory() {
        let factory = Arc::new(DefaultTransportFactory::new(
            TransportFactoryConfig::strict(),
        ));
        let mut mgr = McpServerManager::with_factory(factory);
        mgr.add_server(McpServerConfig {
            name: "remote".into(),
            command: String::new(),
            args: Vec::new(),
            env: HashMap::new(),
            url: Some("https://example.com/rpc".into()),
        });
        assert!(mgr.validate("remote").is_ok());
    }

    #[tokio::test]
    async fn manager_validate_unknown_server_errors() {
        let mgr = McpServerManager::new();
        assert!(mgr.validate("ghost").is_err());
    }

    #[tokio::test]
    async fn manager_create_transport_unknown_server_errors() {
        let mgr = McpServerManager::new();
        assert!(mgr.create_transport("ghost").await.is_err());
    }

    #[tokio::test]
    async fn manager_create_http_transport_via_factory() {
        let mut mgr = McpServerManager::new();
        mgr.add_server(McpServerConfig {
            name: "remote".into(),
            command: String::new(),
            args: Vec::new(),
            env: HashMap::new(),
            url: Some("https://example.com/rpc".into()),
        });
        let t = mgr.create_transport("remote").await;
        assert!(t.is_ok(), "create http transport failed: {:?}", t.err());
    }

    #[tokio::test]
    async fn remove_server_drain_waits_for_call_to_complete() {
        let mut mgr = McpServerManager::new();
        mgr.add_server(test_config("github"));
        // Generous drain timeout (default 30s would slow tests).
        mgr.drain_timeout = Duration::from_secs(2);

        let guard = mgr.begin_call("github").unwrap();
        let counter = mgr.get_server("github").unwrap().in_flight.clone();

        // Drop the guard after a short delay in another task.
        let release = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            drop(guard);
        });

        let start = std::time::Instant::now();
        let removed = mgr.remove_server_drain("github").await;
        let elapsed = start.elapsed();

        release.await.unwrap();
        assert!(removed.is_some());
        // Drain should have waited for at least the release delay.
        assert!(
            elapsed >= Duration::from_millis(100),
            "drain returned too quickly: {elapsed:?}"
        );
        // And it should have observed in_flight=0 by the end.
        assert_eq!(counter.load(), 0);
    }
}
