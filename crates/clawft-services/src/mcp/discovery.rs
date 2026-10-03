//! Dynamic MCP server management.
//!
//! [`McpServerManager`] provides **runtime** management of MCP server
//! connections, including add/remove/list operations and hot-reload
//! via drain-and-swap protocol.
//!
//! # Ownership (ADR-070 / WEFT-494)
//!
//! | Layer | Role |
//! |-------|------|
//! | **CLI** `weft mcp add\|list\|remove` (WEFT-188) | Durable config writes under `tools.mcp_servers` / `tools.mcpServers`. Works offline. |
//! | **Daemon RPC** `mcp.add` / `mcp.list` / `mcp.remove` / `mcp.reload` | Live shared registry inside the kernel process. |
//!
//! Preferred operator path: CLI mutates config → `mcp.reload` (or the
//! WEFT-187 file watcher) applies the diff into this manager. Direct
//! `mcp.add` / `mcp.remove` affect **runtime only** and do not rewrite
//! the config file.
//!
//! # CLI commands (durable config)
//!
//! ```text
//! weft mcp add <name> --command <cmd> | --url <endpoint>
//! weft mcp list
//! weft mcp remove <name>
//! ```
//!
//! # Daemon RPC (live registry)
//!
//! ```text
//! mcp.add    { name, command?, args?, env?, url? }
//! mcp.list   {}
//! mcp.remove { name, drain? }
//! mcp.reload { path? }
//! tools.mcp  {}   # alias of mcp.list (CLI compatibility)
//! ```
//!
//! # Hot-reload protocol
//!
//! When the user config (`clawft.toml` / `config.json`) changes:
//! 1. File watcher detects change (debounce 500ms) **or** `mcp.reload` is called.
//! 2. Diff old and new server lists.
//! 3. New servers: register (connect on next session materialization).
//! 4. Removed servers: drain in-flight calls (30s), then disconnect.
//! 5. Changed servers: remove + add.
//!
//! # Process boot (WEFT-493)
//!
//! Long-lived hosts (`weft gateway`, `weft ui`) call
//! [`boot_mcp_manager_with_watcher`] once at startup so the watcher is
//! armed for the process lifetime. The WEFT-187 API alone is not enough
//! without this callsite.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, mpsc};
use tokio::time::Instant;
use tracing::{debug, info, warn};

use super::transport::{
    DefaultTransportFactory, McpTransport, McpTransportFactory, TransportFactoryConfig,
    TransportSpec,
};
use crate::error::{Result, ServiceError};

/// Default debounce for config file changes (WEFT-187).
pub const DEBOUNCE_MS: u64 = 500;

/// Shared manager handle used by the config watcher and runtime callers.
pub type SharedMcpServerManager = Arc<Mutex<McpServerManager>>;

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
///
/// Full equality (command, args, env, url) is used by
/// [`McpServerManager::apply_config_diff`] for change detection (WEFT-187).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

/// Outcome of a validated config reload (WEFT-187).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigReloadResult {
    /// Servers newly present in the config.
    pub added: usize,
    /// Servers removed from the config.
    pub removed: usize,
    /// Servers whose config changed (command/args/env/url).
    pub changed: usize,
    /// Per-server validation failures that were skipped.
    pub validation_errors: Vec<String>,
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
/// Runtime layer behind daemon RPCs `mcp.add` / `mcp.list` / `mcp.remove`
/// / `mcp.reload` (WEFT-494). Durable config edits remain the CLI's job
/// (`weft mcp …`, WEFT-188); see ADR-070.
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
                // Full config equality (command, args, env, url) — WEFT-187.
                if existing.config != config {
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

    /// Validate each config via the transport factory, skip invalid entries,
    /// then apply the diff. Returns counts plus validation error messages.
    pub fn apply_config_diff_validated(
        &mut self,
        new_configs: Vec<McpServerConfig>,
    ) -> ConfigReloadResult {
        let mut validation_errors = Vec::new();
        let mut valid = Vec::with_capacity(new_configs.len());

        for config in new_configs {
            let spec = transport_spec_for(&config);
            match self.factory.validate(&spec) {
                Ok(()) => valid.push(config),
                Err(e) => {
                    let msg = format!("{}: {e}", config.name);
                    warn!(name = %config.name, error = %e, "skipping invalid MCP server config");
                    validation_errors.push(msg);
                }
            }
        }

        let (added, removed, changed) = self.apply_config_diff(valid);
        ConfigReloadResult {
            added,
            removed,
            changed,
            validation_errors,
        }
    }

    /// Reload server list from a config file path: parse → validate → apply.
    pub fn reload_from_path(&mut self, path: &Path) -> Result<ConfigReloadResult> {
        let configs = load_mcp_servers_from_path(path)?;
        Ok(self.apply_config_diff_validated(configs))
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

// ── Config file parsing (WEFT-187) ──────────────────────────────────────────

/// Wire shape for a single MCP server entry (name comes from the map key).
#[derive(Debug, Clone, Default, Deserialize)]
struct McpServerConfigWire {
    #[serde(default)]
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: HashMap<String, String>,
    #[serde(default)]
    url: Option<String>,
}

/// Nested tools section used in clawft.toml / config.json.
#[derive(Debug, Default, Deserialize)]
struct ToolsSection {
    #[serde(default, alias = "mcpServers")]
    mcp_servers: HashMap<String, McpServerConfigWire>,
}

/// Top-level config document supporting several layouts.
#[derive(Debug, Default, Deserialize)]
struct ConfigDocument {
    #[serde(default)]
    tools: Option<ToolsSection>,
    /// Top-level `mcp_servers` / `mcpServers` (Claude-style `.mcp.json`).
    #[serde(default, alias = "mcpServers")]
    mcp_servers: HashMap<String, McpServerConfigWire>,
}

fn wire_map_to_configs(map: HashMap<String, McpServerConfigWire>) -> Vec<McpServerConfig> {
    let mut out: Vec<McpServerConfig> = map
        .into_iter()
        .map(|(name, w)| McpServerConfig {
            name,
            command: w.command,
            args: w.args,
            env: w.env,
            url: w.url.filter(|s| !s.is_empty()),
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn extract_mcp_servers(doc: ConfigDocument) -> Vec<McpServerConfig> {
    if let Some(tools) = doc.tools
        && !tools.mcp_servers.is_empty()
    {
        return wire_map_to_configs(tools.mcp_servers);
    }
    wire_map_to_configs(doc.mcp_servers)
}

/// Parse MCP server configs from a TOML or JSON string.
///
/// Accepts:
/// - `[tools.mcp_servers.<name>]` / `tools.mcpServers` (TOML/JSON)
/// - top-level `mcp_servers` / `mcpServers` (e.g. `.mcp.json`)
///
/// Format is chosen from `hint` (`"toml"` / `"json"`) or auto-detected
/// from content (leading `{` → JSON, else TOML).
pub fn load_mcp_servers_from_str(content: &str, hint: Option<&str>) -> Result<Vec<McpServerConfig>> {
    let trimmed = content.trim_start();
    let is_json = match hint.map(|h| h.to_ascii_lowercase()) {
        Some(h) if h == "json" || h == ".json" || h.ends_with(".json") => true,
        Some(h) if h == "toml" || h == ".toml" || h.ends_with(".toml") => false,
        _ => trimmed.starts_with('{'),
    };

    let doc: ConfigDocument = if is_json {
        serde_json::from_str(content).map_err(ServiceError::Json)?
    } else {
        toml::from_str(content).map_err(|e| {
            ServiceError::McpProtocol(format!("toml parse error for mcp_servers: {e}"))
        })?
    };

    Ok(extract_mcp_servers(doc))
}

/// Load MCP server configs from a file path (`.toml` or `.json` by extension).
pub fn load_mcp_servers_from_path(path: &Path) -> Result<Vec<McpServerConfig>> {
    let content = std::fs::read_to_string(path)?;
    let hint = path.extension().and_then(|e| e.to_str());
    load_mcp_servers_from_str(&content, hint)
}

// ── Config file watcher (WEFT-187) ──────────────────────────────────────────

/// Handle to a running MCP config watcher. Drop or [`stop`](Self::stop) to shut down.
pub struct McpConfigWatcherHandle {
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

impl McpConfigWatcherHandle {
    /// Stop the watcher gracefully.
    pub fn stop(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

impl Drop for McpConfigWatcherHandle {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

fn event_affects_target(event: &Event, target: &Path) -> bool {
    let target_name = target.file_name();
    event.paths.iter().any(|p| {
        p == target
            || (target_name.is_some() && p.file_name() == target_name)
            // Atomic writers often use `.tmp` / `~` siblings in the same dir.
            || p.parent() == target.parent()
    })
}

/// Start watching `path` for changes with the default **500ms** debounce.
///
/// Watches the parent directory so atomic renames are observed. Emits a
/// [`ConfigReloadResult`] on each successful (or validated-with-errors) reload.
pub fn start_config_watcher(
    path: impl Into<PathBuf>,
    manager: SharedMcpServerManager,
) -> std::result::Result<(McpConfigWatcherHandle, mpsc::Receiver<ConfigReloadResult>), notify::Error>
{
    start_config_watcher_with_debounce(path, manager, Duration::from_millis(DEBOUNCE_MS))
}

// ── Daemon / gateway boot (WEFT-493) ─────────────────────────────────────────

/// Host-side handle for MCP config hot-reload started at process boot.
///
/// Keep this value alive for the lifetime of the long-running process
/// (gateway / daemon). Dropping it stops the file watcher.
///
/// The shared [`manager`](Self::manager) can be passed to runtime callers
/// (list/status/RPC) so they observe the same registry the watcher updates.
pub struct McpConfigWatcherBoot {
    /// Shared manager seeded from the config path and updated on changes.
    pub manager: SharedMcpServerManager,
    /// Path being watched (absolute or as provided).
    pub path: PathBuf,
    /// Initial seed result (from first `reload_from_path`, if the file existed).
    pub initial: ConfigReloadResult,
    /// Drop / stop shuts down the background notify task.
    handle: McpConfigWatcherHandle,
}

impl McpConfigWatcherBoot {
    /// Stop the watcher (same as dropping the boot guard).
    pub fn stop(self) {
        self.handle.stop();
    }

    /// Shared manager handle (cheap clone).
    pub fn manager(&self) -> SharedMcpServerManager {
        Arc::clone(&self.manager)
    }
}

/// Bootstrap [`McpServerManager`] and start the config file watcher (WEFT-493).
///
/// Call once at long-lived process boot (`weft gateway`, kernel daemon host,
/// etc.). This is the production callsite for the WEFT-187 watcher API:
///
/// 1. Create an empty shared manager.
/// 2. If `path` exists, seed via [`McpServerManager::reload_from_path`].
/// 3. Start [`start_config_watcher`] (500ms debounce).
/// 4. Spawn a background task that logs each [`ConfigReloadResult`].
///
/// The returned [`McpConfigWatcherBoot`] **must** be retained for the process
/// lifetime; dropping it cancels the watcher.
///
/// # Errors
///
/// Returns [`notify::Error`] when the OS file watcher cannot be created.
pub async fn boot_mcp_manager_with_watcher(
    path: impl Into<PathBuf>,
) -> std::result::Result<McpConfigWatcherBoot, notify::Error> {
    let path = path.into();
    let manager: SharedMcpServerManager = Arc::new(Mutex::new(McpServerManager::new()));

    let initial = if path.exists() {
        let mut mgr = manager.lock().await;
        match mgr.reload_from_path(&path) {
            Ok(r) => {
                info!(
                    path = %path.display(),
                    added = r.added,
                    removed = r.removed,
                    changed = r.changed,
                    validation_errors = r.validation_errors.len(),
                    "MCP manager seeded from config at boot"
                );
                r
            }
            Err(e) => {
                warn!(
                    path = %path.display(),
                    error = %e,
                    "MCP manager seed at boot failed; watching for a valid config"
                );
                ConfigReloadResult {
                    validation_errors: vec![e.to_string()],
                    ..Default::default()
                }
            }
        }
    } else {
        info!(
            path = %path.display(),
            "MCP config path does not exist yet; watcher will apply on first write"
        );
        ConfigReloadResult::default()
    };

    let (handle, mut rx) = start_config_watcher(path.clone(), Arc::clone(&manager))?;

    // Log reload outcomes so operators see hot-reload without an RPC.
    tokio::spawn(async move {
        while let Some(result) = rx.recv().await {
            if result.validation_errors.is_empty() {
                info!(
                    added = result.added,
                    removed = result.removed,
                    changed = result.changed,
                    "MCP config hot-reload (boot watcher)"
                );
            } else {
                warn!(
                    added = result.added,
                    removed = result.removed,
                    changed = result.changed,
                    errors = ?result.validation_errors,
                    "MCP config hot-reload completed with validation errors"
                );
            }
        }
    });

    info!(
        path = %path.display(),
        servers = {
            let mgr = manager.lock().await;
            mgr.server_count()
        },
        "MCP config watcher started at boot (WEFT-493)"
    );

    Ok(McpConfigWatcherBoot {
        manager,
        path,
        initial,
        handle,
    })
}

/// Start watching `path` with a custom debounce duration.
pub fn start_config_watcher_with_debounce(
    path: impl Into<PathBuf>,
    manager: SharedMcpServerManager,
    debounce: Duration,
) -> std::result::Result<(McpConfigWatcherHandle, mpsc::Receiver<ConfigReloadResult>), notify::Error>
{
    let path = path.into();
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel();
    let (event_tx, mut event_rx) = mpsc::channel::<Event>(64);
    let (result_tx, result_rx) = mpsc::channel::<ConfigReloadResult>(16);

    let mut watcher = RecommendedWatcher::new(
        move |res: notify::Result<Event>| {
            if let Ok(event) = res {
                let _ = event_tx.blocking_send(event);
            }
        },
        notify::Config::default(),
    )?;

    // Watch parent dir so atomic write+rename of the config file is seen.
    let watch_dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));

    if watch_dir.exists() {
        watcher.watch(&watch_dir, RecursiveMode::NonRecursive)?;
        debug!(path = %watch_dir.display(), "watching MCP config parent directory");
    } else {
        warn!(
            path = %watch_dir.display(),
            "MCP config parent directory does not exist; watcher may miss events"
        );
    }

    let watched_path = path.clone();

    tokio::spawn(async move {
        let _watcher = watcher;
        let mut pending = false;
        let mut debounce_deadline: Option<tokio::time::Instant> = None;

        loop {
            tokio::select! {
                event = event_rx.recv() => {
                    match event {
                        Some(ev) => {
                            match ev.kind {
                                EventKind::Create(_)
                                | EventKind::Modify(_)
                                | EventKind::Remove(_)
                                    if event_affects_target(&ev, &watched_path) => {
                                        pending = true;
                                        debounce_deadline = Some(
                                            tokio::time::Instant::now() + debounce,
                                        );
                                    }
                                _ => {}
                            }
                        }
                        None => break,
                    }
                }
                _ = async {
                    match debounce_deadline {
                        Some(deadline) => tokio::time::sleep_until(deadline).await,
                        None => std::future::pending::<()>().await,
                    }
                }, if pending => {
                    pending = false;
                    debounce_deadline = None;

                    let result = {
                        let mut mgr = manager.lock().await;
                        match mgr.reload_from_path(&watched_path) {
                            Ok(r) => r,
                            Err(e) => {
                                warn!(
                                    path = %watched_path.display(),
                                    error = %e,
                                    "MCP config reload failed"
                                );
                                ConfigReloadResult {
                                    validation_errors: vec![e.to_string()],
                                    ..Default::default()
                                }
                            }
                        }
                    };

                    info!(
                        added = result.added,
                        removed = result.removed,
                        changed = result.changed,
                        validation_errors = result.validation_errors.len(),
                        "MCP config hot-reload applied"
                    );

                    if result_tx.send(result).await.is_err() {
                        // No listeners; keep watching until shutdown.
                    }
                }
                _ = &mut shutdown_rx => {
                    info!("MCP config watcher shutting down");
                    break;
                }
            }
        }
    });

    Ok((
        McpConfigWatcherHandle {
            shutdown_tx: Some(shutdown_tx),
        },
        result_rx,
    ))
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

    // ── hot-reload parse / validate / watcher (WEFT-187) ────────────────

    #[test]
    fn debounce_default_is_500ms() {
        assert_eq!(DEBOUNCE_MS, 500);
        let mgr = McpServerManager::new();
        assert_eq!(mgr.debounce(), Duration::from_millis(500));
    }

    #[test]
    fn load_mcp_servers_from_toml_tools_section() {
        let toml = r#"
[tools.mcp_servers.github]
command = "npx"
args = ["-y", "github-mcp"]

[tools.mcp_servers.slack]
command = "npx"
args = ["-y", "slack-mcp"]
env = { SLACK_TOKEN = "x" }
"#;
        let configs = load_mcp_servers_from_str(toml, Some("toml")).unwrap();
        assert_eq!(configs.len(), 2);
        assert_eq!(configs[0].name, "github");
        assert_eq!(configs[0].command, "npx");
        assert_eq!(configs[0].args, vec!["-y", "github-mcp"]);
        assert_eq!(configs[1].name, "slack");
        assert_eq!(
            configs[1].env.get("SLACK_TOKEN").map(String::as_str),
            Some("x")
        );
    }

    #[test]
    fn load_mcp_servers_from_json_camel_case_alias() {
        let json = r#"{
            "tools": {
                "mcpServers": {
                    "claude-flow": {
                        "command": "npx",
                        "args": ["--no-install", "@claude-flow/cli", "mcp", "start"]
                    }
                }
            }
        }"#;
        let configs = load_mcp_servers_from_str(json, Some("json")).unwrap();
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].name, "claude-flow");
        assert_eq!(configs[0].command, "npx");
        assert!(configs[0].args.contains(&"mcp".to_string()));
    }

    #[test]
    fn load_mcp_servers_from_top_level_mcp_servers_json() {
        let json = r#"{
            "mcpServers": {
                "remote": { "url": "https://example.com/rpc" }
            }
        }"#;
        let configs = load_mcp_servers_from_str(json, None).unwrap();
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].name, "remote");
        assert_eq!(configs[0].url.as_deref(), Some("https://example.com/rpc"));
        assert!(configs[0].command.is_empty());
    }

    #[test]
    fn apply_config_diff_reports_changed_on_command_update() {
        let mut mgr = McpServerManager::new();
        mgr.add_server(test_config("github"));

        let mut updated = test_config("github");
        updated.command = "node".into();
        let (added, removed, changed) = mgr.apply_config_diff(vec![updated]);
        assert_eq!(added, 0);
        assert_eq!(removed, 0);
        assert_eq!(changed, 1);
        assert_eq!(mgr.get_server("github").unwrap().config.command, "node");
    }

    #[test]
    fn apply_config_diff_reports_changed_on_env_only_update() {
        let mut mgr = McpServerManager::new();
        mgr.add_server(test_config("github"));

        let mut updated = test_config("github");
        updated.env.insert("TOKEN".into(), "abc".into());
        let (_, _, changed) = mgr.apply_config_diff(vec![updated]);
        assert_eq!(changed, 1, "env-only change must count as changed");
    }

    #[test]
    fn apply_config_diff_validated_skips_bad_http_url() {
        let factory = Arc::new(DefaultTransportFactory::new(
            TransportFactoryConfig::strict(),
        ));
        let mut mgr = McpServerManager::with_factory(factory);
        mgr.add_server(test_config("github"));

        let result = mgr.apply_config_diff_validated(vec![
            test_config("github"),
            McpServerConfig {
                name: "evil".into(),
                command: String::new(),
                args: Vec::new(),
                env: HashMap::new(),
                url: Some("http://evil.example.com".into()),
            },
        ]);

        assert_eq!(result.added, 0);
        assert_eq!(result.removed, 0);
        assert_eq!(result.changed, 0);
        assert_eq!(result.validation_errors.len(), 1);
        assert!(
            result.validation_errors[0].contains("evil"),
            "got: {:?}",
            result.validation_errors
        );
        assert!(mgr.get_server("evil").is_none());
        assert!(mgr.get_server("github").is_some());
    }

    #[test]
    fn reload_from_path_applies_add_remove_change() {
        let dir = std::env::temp_dir().join(format!(
            "clawft_mcp_reload_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("clawft.toml");

        std::fs::write(
            &path,
            r#"
[tools.mcp_servers.github]
command = "npx"
args = ["-y", "github-mcp"]

[tools.mcp_servers.slack]
command = "npx"
args = ["-y", "slack-mcp"]
"#,
        )
        .unwrap();

        let mut mgr = McpServerManager::new();
        let r1 = mgr.reload_from_path(&path).unwrap();
        assert_eq!(r1.added, 2);
        assert_eq!(mgr.server_count(), 2);

        // Remove slack, change github command, add jira.
        std::fs::write(
            &path,
            r#"
[tools.mcp_servers.github]
command = "node"
args = ["-y", "github-mcp"]

[tools.mcp_servers.jira]
command = "npx"
args = ["-y", "jira-mcp"]
"#,
        )
        .unwrap();

        let r2 = mgr.reload_from_path(&path).unwrap();
        assert_eq!(r2.added, 1, "jira added");
        assert_eq!(r2.removed, 1, "slack removed");
        assert_eq!(r2.changed, 1, "github command changed");
        assert!(mgr.get_server("jira").is_some());
        assert!(mgr.get_server("slack").is_none());
        assert_eq!(mgr.get_server("github").unwrap().config.command, "node");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn watcher_edit_config_emits_added_removed_changed() {
        let dir = std::env::temp_dir().join(format!(
            "clawft_mcp_watch_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("clawft.toml");

        std::fs::write(
            &path,
            r#"
[tools.mcp_servers.github]
command = "npx"
args = ["-y", "github-mcp"]

[tools.mcp_servers.slack]
command = "npx"
args = ["-y", "slack-mcp"]
"#,
        )
        .unwrap();

        let manager = Arc::new(Mutex::new(McpServerManager::new()));
        {
            let mut mgr = manager.lock().await;
            let r = mgr.reload_from_path(&path).unwrap();
            assert_eq!(r.added, 2);
        }

        let (handle, mut rx) = start_config_watcher_with_debounce(
            path.clone(),
            manager.clone(),
            Duration::from_millis(80),
        )
        .expect("start watcher");

        // Give the OS watcher a moment to attach.
        tokio::time::sleep(Duration::from_millis(100)).await;

        std::fs::write(
            &path,
            r#"
[tools.mcp_servers.github]
command = "node"
args = ["-y", "github-mcp"]

[tools.mcp_servers.jira]
command = "npx"
args = ["-y", "jira-mcp"]
"#,
        )
        .unwrap();

        // FS events can emit multiple debounced reloads (including a zero-diff
        // race if the write is observed mid-flight). Wait for a non-empty diff.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        let mut result = None;
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(500), rx.recv()).await {
                Ok(Some(r)) if r.added > 0 || r.removed > 0 || r.changed > 0 => {
                    result = Some(r);
                    break;
                }
                Ok(Some(_)) => continue,
                Ok(None) => panic!("watcher channel closed without result"),
                Err(_) => continue,
            }
        }
        let result = result.expect("timeout waiting for non-empty reload event");

        assert_eq!(result.added, 1, "jira added: {result:?}");
        assert_eq!(result.removed, 1, "slack removed: {result:?}");
        assert_eq!(result.changed, 1, "github changed: {result:?}");

        {
            let mgr = manager.lock().await;
            assert!(mgr.get_server("jira").is_some());
            assert!(mgr.get_server("slack").is_none());
            assert_eq!(mgr.get_server("github").unwrap().config.command, "node");
        }

        handle.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn boot_mcp_manager_with_watcher_seeds_and_applies_edit() {
        let dir = std::env::temp_dir().join(format!(
            "clawft_mcp_boot_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");

        std::fs::write(
            &path,
            r#"{
  "tools": {
    "mcp_servers": {
      "alpha": { "command": "npx", "args": ["-y", "alpha-mcp"] }
    }
  }
}"#,
        )
        .unwrap();

        let boot = boot_mcp_manager_with_watcher(path.clone())
            .await
            .expect("boot watcher");
        assert_eq!(boot.initial.added, 1);
        assert_eq!(boot.path, path);
        {
            let mgr = boot.manager.lock().await;
            assert_eq!(mgr.server_count(), 1);
            assert!(mgr.get_server("alpha").is_some());
        }

        // Edit: remove alpha, add beta (drain-and-swap via apply_config_diff).
        tokio::time::sleep(Duration::from_millis(100)).await;
        std::fs::write(
            &path,
            r#"{
  "tools": {
    "mcp_servers": {
      "beta": { "command": "npx", "args": ["-y", "beta-mcp"] }
    }
  }
}"#,
        )
        .unwrap();

        // Poll manager until the watcher applies the edit (debounce + FS lag).
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            {
                let mgr = boot.manager.lock().await;
                if mgr.get_server("beta").is_some() && mgr.get_server("alpha").is_none() {
                    break;
                }
            }
            if tokio::time::Instant::now() >= deadline {
                panic!("timeout waiting for boot watcher to apply config edit");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        boot.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn boot_mcp_manager_with_watcher_missing_file_still_starts() {
        let dir = std::env::temp_dir().join(format!(
            "clawft_mcp_boot_miss_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("missing-config.json");
        assert!(!path.exists());

        let boot = boot_mcp_manager_with_watcher(path.clone())
            .await
            .expect("boot watcher on missing path");
        assert_eq!(boot.initial.added, 0);
        {
            let mgr = boot.manager.lock().await;
            assert_eq!(mgr.server_count(), 0);
        }
        boot.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
