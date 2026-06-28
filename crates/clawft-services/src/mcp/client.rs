//! Full MCP client features (F9b).
//!
//! Extends the core MCP client (F9a, in `mod.rs`) with:
//! - **Auto-discovery**: Find MCP servers from `~/.clawft/mcp/` config
//! - **Connection pooling**: Reuse sessions across tool calls
//! - **Schema caching**: Cache tool schemas with configurable TTL
//! - **Health checks**: Detect and reconnect failed servers
//! - **Per-agent MCP configuration**: Agent-level server overrides

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::ToolDefinition;
use super::discovery::McpServerConfig;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the full MCP client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpClientConfig {
    /// Directory to scan for MCP server configuration files.
    #[serde(default = "default_config_dir")]
    pub config_dir: PathBuf,

    /// TTL for cached tool schemas (seconds).
    #[serde(default = "default_schema_ttl_secs")]
    pub schema_ttl_secs: u64,

    /// Interval between health checks (seconds).
    #[serde(default = "default_health_check_interval_secs")]
    pub health_check_interval_secs: u64,

    /// Maximum connection pool size per server.
    #[serde(default = "default_max_connections")]
    pub max_connections_per_server: usize,

    /// Maximum number of reconnect attempts before marking server as failed.
    #[serde(default = "default_max_reconnect_attempts")]
    pub max_reconnect_attempts: u32,
}

fn default_config_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".clawft")
        .join("mcp")
}

fn default_schema_ttl_secs() -> u64 {
    300 // 5 minutes
}

fn default_health_check_interval_secs() -> u64 {
    60
}

fn default_max_connections() -> usize {
    3
}

fn default_max_reconnect_attempts() -> u32 {
    5
}

impl Default for McpClientConfig {
    fn default() -> Self {
        Self {
            config_dir: default_config_dir(),
            schema_ttl_secs: default_schema_ttl_secs(),
            health_check_interval_secs: default_health_check_interval_secs(),
            max_connections_per_server: default_max_connections(),
            max_reconnect_attempts: default_max_reconnect_attempts(),
        }
    }
}

// ---------------------------------------------------------------------------
// Per-agent configuration
// ---------------------------------------------------------------------------

/// Per-agent MCP server configuration.
///
/// Agent-level entries with the same server name replace global entries.
/// New names are appended. `enabled = false` explicitly excludes a
/// global server for this agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMcpServerConfig {
    /// Server name (must match a global or define a new server).
    pub name: String,

    /// Whether this server is enabled for the agent.
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Optional URL override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,

    /// Optional command override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,

    /// Optional args override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
}

fn default_enabled() -> bool {
    true
}

// ---------------------------------------------------------------------------
// Schema cache
// ---------------------------------------------------------------------------

/// Cached tool schemas for a server with expiration.
#[derive(Debug, Clone)]
struct CachedSchemas {
    /// Tool definitions from the server.
    tools: Vec<ToolDefinition>,
    /// When the cache was populated.
    fetched_at: Instant,
    /// TTL for this cache entry.
    ttl: Duration,
}

impl CachedSchemas {
    fn new(tools: Vec<ToolDefinition>, ttl: Duration) -> Self {
        Self {
            tools,
            fetched_at: Instant::now(),
            ttl,
        }
    }

    fn is_expired(&self) -> bool {
        self.fetched_at.elapsed() > self.ttl
    }
}

// ---------------------------------------------------------------------------
// Connection pool entry
// ---------------------------------------------------------------------------

/// Health status of a pooled connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionHealth {
    /// Connection is healthy and responsive.
    Healthy,
    /// Connection is degraded (slow responses).
    Degraded,
    /// Connection has failed and needs reconnection.
    Failed,
    /// Connection has not been checked yet.
    Unknown,
}

/// A pooled server connection.
#[derive(Debug)]
struct PooledConnection {
    /// Server configuration (used when reconnecting).
    #[allow(dead_code)]
    config: McpServerConfig,
    /// Current health status.
    health: ConnectionHealth,
    /// Number of consecutive failures.
    failure_count: u32,
    /// When the last health check was performed.
    last_health_check: Option<Instant>,
    /// When the connection was established.
    #[allow(dead_code)]
    connected_at: Option<Instant>,
}

// ---------------------------------------------------------------------------
// McpClientPool
// ---------------------------------------------------------------------------

/// Full MCP client with connection pooling, schema caching, and health checks.
///
/// This is the F9b extension that builds on the F9a `McpClient` / `McpSession`.
/// It manages multiple MCP server connections, caches tool schemas, and
/// performs periodic health checks.
pub struct McpClientPool {
    config: McpClientConfig,
    /// Server connections keyed by server name.
    connections: Arc<RwLock<HashMap<String, PooledConnection>>>,
    /// Cached schemas keyed by server name.
    schema_cache: Arc<RwLock<HashMap<String, CachedSchemas>>>,
}

impl McpClientPool {
    /// Create a new MCP client pool with the given configuration.
    pub fn new(config: McpClientConfig) -> Self {
        Self {
            config,
            connections: Arc::new(RwLock::new(HashMap::new())),
            schema_cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Auto-discover MCP servers from the config directory.
    ///
    /// Scans `~/.clawft/mcp/` for JSON or TOML config files, each
    /// defining one or more MCP server configurations.
    pub async fn auto_discover(&self) -> Vec<McpServerConfig> {
        let config_dir = &self.config.config_dir;
        let mut discovered = Vec::new();

        if !config_dir.is_dir() {
            debug!(path = %config_dir.display(), "MCP config directory not found");
            return discovered;
        }

        let entries = match std::fs::read_dir(config_dir) {
            Ok(entries) => entries,
            Err(e) => {
                warn!(
                    path = %config_dir.display(),
                    error = %e,
                    "failed to read MCP config directory"
                );
                return discovered;
            }
        };

        for entry in entries.flatten() {
            let path = entry.path();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

            if ext == "json" {
                match std::fs::read_to_string(&path) {
                    Ok(content) => match serde_json::from_str::<Vec<McpServerConfig>>(&content) {
                        Ok(configs) => {
                            info!(
                                path = %path.display(),
                                count = configs.len(),
                                "discovered MCP servers"
                            );
                            discovered.extend(configs);
                        }
                        Err(_) => {
                            // Try single config
                            if let Ok(config) = serde_json::from_str::<McpServerConfig>(&content) {
                                info!(
                                    path = %path.display(),
                                    name = %config.name,
                                    "discovered MCP server"
                                );
                                discovered.push(config);
                            }
                        }
                    },
                    Err(e) => {
                        warn!(
                            path = %path.display(),
                            error = %e,
                            "failed to read MCP config file"
                        );
                    }
                }
            }
        }

        discovered
    }

    /// Apply per-agent overrides to the global server list.
    ///
    /// Agent-level entries with the same name replace globals.
    /// `enabled = false` removes the server from the agent's view.
    pub fn apply_agent_overrides(
        global: &[McpServerConfig],
        agent_overrides: &[AgentMcpServerConfig],
    ) -> Vec<McpServerConfig> {
        let override_map: HashMap<&str, &AgentMcpServerConfig> = agent_overrides
            .iter()
            .map(|o| (o.name.as_str(), o))
            .collect();

        let mut result: Vec<McpServerConfig> = Vec::new();

        // Process global servers with overrides
        for server in global {
            if let Some(override_cfg) = override_map.get(server.name.as_str()) {
                if !override_cfg.enabled {
                    // Explicitly disabled for this agent
                    continue;
                }
                // Apply overrides
                let mut merged = server.clone();
                if let Some(ref url) = override_cfg.url {
                    merged.url = Some(url.clone());
                }
                if let Some(ref command) = override_cfg.command {
                    merged.command = command.clone();
                }
                if let Some(ref args) = override_cfg.args {
                    merged.args = args.clone();
                }
                result.push(merged);
            } else {
                result.push(server.clone());
            }
        }

        // Add agent-only servers (not in global list)
        let global_names: std::collections::HashSet<&str> =
            global.iter().map(|s| s.name.as_str()).collect();
        for override_cfg in agent_overrides {
            if !global_names.contains(override_cfg.name.as_str())
                && override_cfg.enabled
                && let Some(ref command) = override_cfg.command
            {
                result.push(McpServerConfig {
                    name: override_cfg.name.clone(),
                    command: command.clone(),
                    args: override_cfg.args.clone().unwrap_or_default(),
                    env: HashMap::new(),
                    url: override_cfg.url.clone(),
                });
            }
        }

        result
    }

    /// Register a server connection in the pool.
    pub async fn register_server(&self, config: McpServerConfig) {
        let name = config.name.clone();
        let conn = PooledConnection {
            config,
            health: ConnectionHealth::Unknown,
            failure_count: 0,
            last_health_check: None,
            connected_at: None,
        };
        let mut connections = self.connections.write().await;
        connections.insert(name.clone(), conn);
        debug!(name = %name, "registered MCP server in pool");
    }

    /// Get cached schemas for a server, returning `None` if expired or missing.
    pub async fn get_cached_schemas(&self, server_name: &str) -> Option<Vec<ToolDefinition>> {
        let cache = self.schema_cache.read().await;
        cache
            .get(server_name)
            .filter(|c| !c.is_expired())
            .map(|c| c.tools.clone())
    }

    /// Store schemas in the cache for a server.
    pub async fn cache_schemas(&self, server_name: &str, tools: Vec<ToolDefinition>) {
        let ttl = Duration::from_secs(self.config.schema_ttl_secs);
        let entry = CachedSchemas::new(tools, ttl);
        let mut cache = self.schema_cache.write().await;
        cache.insert(server_name.to_string(), entry);
        debug!(
            server = %server_name,
            ttl_secs = self.config.schema_ttl_secs,
            "cached tool schemas"
        );
    }

    /// Invalidate cached schemas for a server.
    pub async fn invalidate_cache(&self, server_name: &str) {
        let mut cache = self.schema_cache.write().await;
        cache.remove(server_name);
    }

    /// Update the health status of a server.
    pub async fn update_health(&self, server_name: &str, health: ConnectionHealth) {
        let mut connections = self.connections.write().await;
        if let Some(conn) = connections.get_mut(server_name) {
            conn.health = health;
            conn.last_health_check = Some(Instant::now());
            if health == ConnectionHealth::Failed {
                conn.failure_count += 1;
            } else {
                conn.failure_count = 0;
            }
        }
    }

    /// Get the health status of a server.
    pub async fn get_health(&self, server_name: &str) -> Option<ConnectionHealth> {
        let connections = self.connections.read().await;
        connections.get(server_name).map(|c| c.health)
    }

    /// Check if a server should be reconnected (exceeded max failures).
    pub async fn needs_reconnect(&self, server_name: &str) -> bool {
        let connections = self.connections.read().await;
        connections.get(server_name).is_some_and(|c| {
            c.health == ConnectionHealth::Failed
                && c.failure_count < self.config.max_reconnect_attempts
        })
    }

    /// List all servers that need health checks.
    pub async fn servers_needing_health_check(&self) -> Vec<String> {
        let interval = Duration::from_secs(self.config.health_check_interval_secs);
        let connections = self.connections.read().await;
        connections
            .iter()
            .filter(|(_, conn)| {
                conn.last_health_check
                    .is_none_or(|t| t.elapsed() > interval)
            })
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Number of pooled connections.
    pub async fn connection_count(&self) -> usize {
        self.connections.read().await.len()
    }

    /// Number of cached schemas.
    pub async fn cache_size(&self) -> usize {
        self.schema_cache.read().await.len()
    }

    /// Get the client configuration.
    pub fn config(&self) -> &McpClientConfig {
        &self.config
    }
}

impl std::fmt::Debug for McpClientPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpClientPool")
            .field("config_dir", &self.config.config_dir)
            .finish_non_exhaustive()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> McpClientConfig {
        McpClientConfig {
            config_dir: PathBuf::from("/tmp/test-mcp"),
            schema_ttl_secs: 1, // short TTL for testing
            health_check_interval_secs: 1,
            max_connections_per_server: 2,
            max_reconnect_attempts: 3,
        }
    }

    fn test_server(name: &str) -> McpServerConfig {
        McpServerConfig {
            name: name.into(),
            command: "npx".into(),
            args: vec!["-y".into(), format!("{name}-mcp")],
            env: HashMap::new(),
            url: None,
        }
    }

    #[test]
    fn default_config() {
        let config = McpClientConfig::default();
        assert_eq!(config.schema_ttl_secs, 300);
        assert_eq!(config.health_check_interval_secs, 60);
        assert_eq!(config.max_connections_per_server, 3);
        assert_eq!(config.max_reconnect_attempts, 5);
    }

    #[tokio::test]
    async fn register_and_count_servers() {
        let pool = McpClientPool::new(test_config());
        assert_eq!(pool.connection_count().await, 0);

        pool.register_server(test_server("github")).await;
        pool.register_server(test_server("slack")).await;
        assert_eq!(pool.connection_count().await, 2);
    }

    #[tokio::test]
    async fn schema_cache_hit_and_miss() {
        let pool = McpClientPool::new(test_config());

        // Cache miss
        assert!(pool.get_cached_schemas("github").await.is_none());

        // Cache schemas
        let tools = vec![ToolDefinition {
            name: "create_issue".into(),
            description: "Create an issue".into(),
            input_schema: serde_json::json!({"type": "object"}),
        }];
        pool.cache_schemas("github", tools.clone()).await;

        // Cache hit
        let cached = pool.get_cached_schemas("github").await;
        assert!(cached.is_some());
        assert_eq!(cached.unwrap().len(), 1);

        // Invalidate
        pool.invalidate_cache("github").await;
        assert!(pool.get_cached_schemas("github").await.is_none());
    }

    #[tokio::test]
    async fn schema_cache_expires() {
        let config = McpClientConfig {
            schema_ttl_secs: 0, // instant expiry
            ..test_config()
        };
        let pool = McpClientPool::new(config);

        let tools = vec![ToolDefinition {
            name: "test".into(),
            description: "Test".into(),
            input_schema: serde_json::json!({"type": "object"}),
        }];
        pool.cache_schemas("server", tools).await;

        // Should be expired immediately
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(pool.get_cached_schemas("server").await.is_none());
    }

    #[tokio::test]
    async fn health_updates() {
        let pool = McpClientPool::new(test_config());
        pool.register_server(test_server("github")).await;

        // Initial health is Unknown
        assert_eq!(
            pool.get_health("github").await,
            Some(ConnectionHealth::Unknown)
        );

        // Update to Healthy
        pool.update_health("github", ConnectionHealth::Healthy)
            .await;
        assert_eq!(
            pool.get_health("github").await,
            Some(ConnectionHealth::Healthy)
        );

        // Update to Failed
        pool.update_health("github", ConnectionHealth::Failed).await;
        assert_eq!(
            pool.get_health("github").await,
            Some(ConnectionHealth::Failed)
        );
    }

    #[tokio::test]
    async fn needs_reconnect_logic() {
        let pool = McpClientPool::new(test_config());
        pool.register_server(test_server("github")).await;

        // Healthy server doesn't need reconnect
        pool.update_health("github", ConnectionHealth::Healthy)
            .await;
        assert!(!pool.needs_reconnect("github").await);

        // Failed server needs reconnect (under max attempts)
        pool.update_health("github", ConnectionHealth::Failed).await;
        assert!(pool.needs_reconnect("github").await);

        // Exceed max attempts
        pool.update_health("github", ConnectionHealth::Failed).await;
        pool.update_health("github", ConnectionHealth::Failed).await;
        assert!(!pool.needs_reconnect("github").await); // 3 failures = max
    }

    #[tokio::test]
    async fn servers_needing_health_check() {
        let pool = McpClientPool::new(test_config());
        pool.register_server(test_server("github")).await;
        pool.register_server(test_server("slack")).await;

        // All servers need check (no checks done yet)
        let needing = pool.servers_needing_health_check().await;
        assert_eq!(needing.len(), 2);

        // After health check, should not need immediate recheck
        pool.update_health("github", ConnectionHealth::Healthy)
            .await;
        // github was just checked, slack still needs it
        let needing = pool.servers_needing_health_check().await;
        assert_eq!(needing.len(), 1);
        assert!(needing.contains(&"slack".to_string()));
    }

    #[test]
    fn apply_agent_overrides_disables_server() {
        let global = vec![test_server("github"), test_server("slack")];
        let agent = vec![AgentMcpServerConfig {
            name: "slack".into(),
            enabled: false,
            url: None,
            command: None,
            args: None,
        }];

        let result = McpClientPool::apply_agent_overrides(&global, &agent);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "github");
    }

    #[test]
    fn apply_agent_overrides_adds_server() {
        let global = vec![test_server("github")];
        let agent = vec![AgentMcpServerConfig {
            name: "custom".into(),
            enabled: true,
            url: Some("stdio://custom-mcp".into()),
            command: Some("custom-mcp".into()),
            args: None,
        }];

        let result = McpClientPool::apply_agent_overrides(&global, &agent);
        assert_eq!(result.len(), 2);
        assert!(result.iter().any(|s| s.name == "custom"));
    }

    #[test]
    fn apply_agent_overrides_replaces_url() {
        let global = vec![test_server("github")];
        let agent = vec![AgentMcpServerConfig {
            name: "github".into(),
            enabled: true,
            url: Some("http://custom:8080".into()),
            command: None,
            args: None,
        }];

        let result = McpClientPool::apply_agent_overrides(&global, &agent);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].url, Some("http://custom:8080".into()));
    }

    #[test]
    fn apply_agent_overrides_empty() {
        let global = vec![test_server("github")];
        let result = McpClientPool::apply_agent_overrides(&global, &[]);
        assert_eq!(result.len(), 1);
    }

    #[tokio::test]
    async fn auto_discover_empty_dir() {
        let pool = McpClientPool::new(McpClientConfig {
            config_dir: PathBuf::from("/nonexistent-path-12345"),
            ..Default::default()
        });
        let discovered = pool.auto_discover().await;
        assert!(discovered.is_empty());
    }

    #[test]
    fn agent_server_config_serde() {
        let config = AgentMcpServerConfig {
            name: "test".into(),
            enabled: true,
            url: Some("stdio://test".into()),
            command: None,
            args: None,
        };
        let json = serde_json::to_string(&config).unwrap();
        let restored: AgentMcpServerConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.name, "test");
        assert!(restored.enabled);
        assert_eq!(restored.url, Some("stdio://test".into()));
    }

    #[test]
    fn connection_health_serde() {
        let json = serde_json::to_string(&ConnectionHealth::Healthy).unwrap();
        assert_eq!(json, "\"healthy\"");
        let restored: ConnectionHealth = serde_json::from_str("\"failed\"").unwrap();
        assert_eq!(restored, ConnectionHealth::Failed);
    }
}
