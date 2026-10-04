//! Daemon RPC handlers for the live MCP server registry (WEFT-494 / ADR-070).
//!
//! # Ownership
//!
//! - **CLI** (`weft mcp add|list|remove`, WEFT-188) mutates durable config.
//! - **These RPCs** mutate / inspect the daemon-process
//!   [`McpServerManager`] shared registry.
//!
//! See `docs/adr/adr-070-mcp-registry-ownership.md`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use clawft_rpc::Response;
use clawft_services::mcp::discovery::{
    ConfigReloadResult, McpServerConfig, McpServerManager, ServerStatus, SharedMcpServerManager,
};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::{info, warn};

/// Daemon-wide live MCP registry. Seeded at boot from loaded config;
/// mutated by `mcp.add` / `mcp.remove` / `mcp.reload`.
static DAEMON_MCP_MANAGER: OnceLock<SharedMcpServerManager> = OnceLock::new();

/// Optional durable config path remembered at boot for path-less `mcp.reload`.
static DAEMON_MCP_CONFIG_PATH: OnceLock<PathBuf> = OnceLock::new();

/// Access the shared manager (initializes empty if boot skipped seeding).
pub fn daemon_mcp_manager() -> SharedMcpServerManager {
    DAEMON_MCP_MANAGER
        .get_or_init(|| Arc::new(Mutex::new(McpServerManager::new())))
        .clone()
}

/// Record the config path used to load the daemon (best-effort for reload).
pub fn set_mcp_config_path(path: PathBuf) {
    let _ = DAEMON_MCP_CONFIG_PATH.set(path);
}

/// Seed the live registry from in-memory config entries (daemon boot).
pub async fn seed_from_config_servers(
    servers: &HashMap<String, clawft_types::config::MCPServerConfig>,
) {
    if servers.is_empty() {
        // Ensure manager exists even with zero servers so list RPCs work.
        let _ = daemon_mcp_manager();
        return;
    }
    let mgr = daemon_mcp_manager();
    let mut guard = mgr.lock().await;
    for (name, cfg) in servers {
        let entry = McpServerConfig {
            name: name.clone(),
            command: cfg.command.clone(),
            args: cfg.args.clone(),
            env: cfg.env.clone(),
            url: if cfg.url.is_empty() {
                None
            } else {
                Some(cfg.url.clone())
            },
        };
        guard.add_server(entry);
    }
    info!(
        count = servers.len(),
        "MCP registry seeded from config (WEFT-494)"
    );
}

// ── Wire types ───────────────────────────────────────────────────────────

/// Parameters for `mcp.add`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpAddParams {
    /// Unique server name (registry key).
    pub name: String,
    /// Stdio command (e.g. `npx`). Empty when using `url` only.
    #[serde(default)]
    pub command: String,
    /// Stdio arguments.
    #[serde(default)]
    pub args: Vec<String>,
    /// Extra environment for the stdio process.
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Streamable HTTP endpoint (optional).
    #[serde(default)]
    pub url: Option<String>,
}

/// Parameters for `mcp.remove`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpRemoveParams {
    /// Server name to remove from the live registry.
    pub name: String,
    /// When true (default), drain in-flight calls before drop.
    #[serde(default = "default_true")]
    pub drain: bool,
}

fn default_true() -> bool {
    true
}

/// Parameters for `mcp.reload`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpReloadParams {
    /// Config file path. When omitted, walks standard locations.
    #[serde(default)]
    pub path: Option<String>,
}

/// One server row returned by `mcp.list` / `tools.mcp`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerInfo {
    pub name: String,
    pub status: String,
    pub command: String,
    pub args: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub tools: Vec<String>,
    pub in_flight: usize,
    pub added_at: String,
}

// ── Handlers ─────────────────────────────────────────────────────────────

/// `mcp.add` — register (or replace) a server in the live registry.
pub async fn handle_mcp_add(params: serde_json::Value) -> Response {
    let p: McpAddParams = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(e) => return Response::error(format!("invalid params: {e}")),
    };
    let name = p.name.trim();
    if name.is_empty() {
        return Response::error("mcp.add requires a non-empty name");
    }
    let url = p
        .url
        .as_ref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    if p.command.trim().is_empty() && url.is_none() {
        return Response::error("mcp.add requires either command (stdio) or url (http)");
    }

    let config = McpServerConfig {
        name: name.to_string(),
        command: p.command,
        args: p.args,
        env: p.env,
        url,
    };

    let mgr = daemon_mcp_manager();
    let mut guard = mgr.lock().await;
    // Validate transport spec before inserting.
    let tmp_name = config.name.clone();
    guard.add_server(config);
    if let Err(e) = guard.validate(&tmp_name) {
        // Roll back invalid entry.
        guard.remove_server(&tmp_name);
        return Response::error(format!("mcp.add validation failed: {e}"));
    }

    let server = guard.get_server(&tmp_name).expect("just added");
    let info = server_info(server);
    info!(name = %tmp_name, "mcp.add: server registered in live registry");
    Response::success(serde_json::json!({
        "added": true,
        "server": info,
        "message": format!(
            "registered '{tmp_name}' in live MCP registry (runtime only; \
             use `weft mcp add` for durable config — ADR-070)"
        ),
    }))
}

/// `mcp.list` / `tools.mcp` — list live registry servers.
pub async fn handle_mcp_list(_params: serde_json::Value) -> Response {
    let mgr = daemon_mcp_manager();
    let guard = mgr.lock().await;
    let mut servers: Vec<McpServerInfo> = guard.list_servers().iter().map(|s| server_info(s)).collect();
    servers.sort_by(|a, b| a.name.cmp(&b.name));
    let output = format_list_table(&servers);
    Response::success(serde_json::json!({
        "servers": servers,
        "count": servers.len(),
        "output": output,
    }))
}

/// `mcp.remove` — drop a server from the live registry (optional drain).
pub async fn handle_mcp_remove(params: serde_json::Value) -> Response {
    let p: McpRemoveParams = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(e) => return Response::error(format!("invalid params: {e}")),
    };
    let name = p.name.trim();
    if name.is_empty() {
        return Response::error("mcp.remove requires a non-empty name");
    }

    let mgr = daemon_mcp_manager();
    let mut guard = mgr.lock().await;
    if guard.get_server(name).is_none() {
        return Response::error(format!("MCP server not found in live registry: {name}"));
    }

    if p.drain {
        match guard.remove_server_drain(name).await {
            Some(_) => {
                info!(name = %name, "mcp.remove: drained and removed");
                Response::success(serde_json::json!({
                    "removed": true,
                    "name": name,
                    "drained": true,
                    "message": format!(
                        "removed '{name}' from live registry (config file unchanged — ADR-070)"
                    ),
                }))
            }
            None => Response::error(format!("MCP server not found: {name}")),
        }
    } else if guard.remove_server(name) {
        info!(name = %name, "mcp.remove: synchronous remove");
        Response::success(serde_json::json!({
            "removed": true,
            "name": name,
            "drained": false,
            "message": format!(
                "removed '{name}' from live registry (config file unchanged — ADR-070)"
            ),
        }))
    } else {
        Response::error(format!("MCP server not found: {name}"))
    }
}

/// `mcp.reload` — re-read durable config into the live registry.
pub async fn handle_mcp_reload(params: serde_json::Value) -> Response {
    let p: McpReloadParams = match serde_json::from_value(params) {
        Ok(p) => p,
        Err(e) => return Response::error(format!("invalid params: {e}")),
    };

    let path = match resolve_reload_path(p.path.as_deref()) {
        Some(p) => p,
        None => {
            return Response::error(
                "mcp.reload: no config path found. Pass params.path or set CLAWFT_CONFIG \
                 / create ~/.clawft/config.json (or clawft.toml / weave.toml in cwd).",
            );
        }
    };

    if !path.exists() {
        return Response::error(format!(
            "mcp.reload: config file does not exist: {}",
            path.display()
        ));
    }

    let mgr = daemon_mcp_manager();
    let mut guard = mgr.lock().await;
    match guard.reload_from_path(&path) {
        Ok(result) => {
            info!(
                path = %path.display(),
                added = result.added,
                removed = result.removed,
                changed = result.changed,
                "mcp.reload applied"
            );
            Response::success(reload_result_json(&path, &result))
        }
        Err(e) => {
            warn!(path = %path.display(), error = %e, "mcp.reload failed");
            Response::error(format!("mcp.reload failed for {}: {e}", path.display()))
        }
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────

fn server_info(s: &clawft_services::mcp::discovery::ManagedMcpServer) -> McpServerInfo {
    McpServerInfo {
        name: s.config.name.clone(),
        status: status_str(s.status).to_string(),
        command: s.config.command.clone(),
        args: s.config.args.clone(),
        url: s.config.url.clone(),
        tools: s.tools.clone(),
        in_flight: s.in_flight.load(),
        added_at: s.added_at.to_rfc3339(),
    }
}

fn status_str(s: ServerStatus) -> &'static str {
    match s {
        ServerStatus::Connected => "connected",
        ServerStatus::Connecting => "connecting",
        ServerStatus::Draining => "draining",
        ServerStatus::Disconnected => "disconnected",
        ServerStatus::Error => "error",
    }
}

fn format_list_table(servers: &[McpServerInfo]) -> String {
    if servers.is_empty() {
        return "No MCP servers in live daemon registry.\n".to_string();
    }
    let mut out = String::new();
    out.push_str(&format!(
        "{:<20} {:<12} {:<12} {}\n",
        "NAME", "STATUS", "TRANSPORT", "COMMAND / URL"
    ));
    out.push_str(&format!("{:-<20} {:-<12} {:-<12} {:-<40}\n", "", "", "", ""));
    for s in servers {
        let (transport, detail) = if s.url.as_ref().is_some_and(|u| !u.is_empty())
            && s.command.is_empty()
        {
            ("http", s.url.clone().unwrap_or_default())
        } else if !s.command.is_empty() {
            let detail = if s.args.is_empty() {
                s.command.clone()
            } else {
                format!("{} {}", s.command, s.args.join(" "))
            };
            ("stdio", detail)
        } else if let Some(u) = &s.url {
            ("http", u.clone())
        } else {
            ("unknown", String::new())
        };
        out.push_str(&format!(
            "{:<20} {:<12} {:<12} {}\n",
            s.name, s.status, transport, detail
        ));
    }
    out.push_str(&format!("\nTotal: {} MCP server(s) (live registry)\n", servers.len()));
    out
}

fn reload_result_json(path: &Path, result: &ConfigReloadResult) -> serde_json::Value {
    serde_json::json!({
        "reloaded": true,
        "path": path.display().to_string(),
        "added": result.added,
        "removed": result.removed,
        "changed": result.changed,
        "validation_errors": result.validation_errors,
        "message": format!(
            "reloaded {} (added={}, removed={}, changed={})",
            path.display(),
            result.added,
            result.removed,
            result.changed
        ),
    })
}

/// Resolve config path for reload: explicit param → boot path → env → defaults.
pub fn resolve_reload_path(explicit: Option<&str>) -> Option<PathBuf> {
    if let Some(p) = explicit.map(str::trim).filter(|s| !s.is_empty()) {
        return Some(PathBuf::from(p));
    }
    if let Some(p) = DAEMON_MCP_CONFIG_PATH.get()
        && p.exists()
    {
        return Some(p.clone());
    }
    if let Ok(env_path) = std::env::var("CLAWFT_CONFIG") {
        let p = PathBuf::from(env_path.trim());
        if p.exists() {
            return Some(p);
        }
    }
    let candidates = [
        PathBuf::from("clawft.toml"),
        PathBuf::from("weave.toml"),
        PathBuf::from("config.json"),
        dirs::home_dir()
            .map(|h| h.join(".clawft").join("config.json"))
            .unwrap_or_else(|| PathBuf::from(".clawft/config.json")),
    ];
    candidates.into_iter().find(|p| p.exists())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::Mutex as AsyncMutex;

    /// Process-global registry is shared across tests; serialize mutations.
    /// A tokio mutex, so a test may hold it across awaits.
    static TEST_LOCK: AsyncMutex<()> = AsyncMutex::const_new(());

    #[tokio::test]
    async fn mcp_add_list_remove_roundtrip() {
        let _guard = TEST_LOCK.lock().await;
        let name = format!("weft494-test-{}", uuid::Uuid::new_v4());
        let add = handle_mcp_add(serde_json::json!({
            "name": name,
            "command": "echo",
            "args": ["hello"],
        }))
        .await;
        assert!(add.ok, "add failed: {:?}", add.error);

        let list = handle_mcp_list(serde_json::json!({})).await;
        assert!(list.ok);
        let servers = list
            .result
            .as_ref()
            .and_then(|v| v.get("servers"))
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        assert!(
            servers
                .iter()
                .any(|s| s.get("name").and_then(|n| n.as_str()) == Some(name.as_str())),
            "list missing added server: {servers:?}"
        );
        assert!(
            list.result
                .as_ref()
                .and_then(|v| v.get("output"))
                .and_then(|o| o.as_str())
                .is_some_and(|o| o.contains(&name)),
            "tools.mcp-compatible output missing name"
        );

        let remove = handle_mcp_remove(serde_json::json!({
            "name": name,
            "drain": false,
        }))
        .await;
        assert!(remove.ok, "remove failed: {:?}", remove.error);

        let list2 = handle_mcp_list(serde_json::json!({})).await;
        let servers2 = list2
            .result
            .as_ref()
            .and_then(|v| v.get("servers"))
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        assert!(
            !servers2
                .iter()
                .any(|s| s.get("name").and_then(|n| n.as_str()) == Some(name.as_str())),
            "server still present after remove"
        );
    }

    #[tokio::test]
    async fn mcp_add_requires_transport() {
        let _guard = TEST_LOCK.lock().await;
        let resp = handle_mcp_add(serde_json::json!({ "name": "no-transport" })).await;
        assert!(!resp.ok);
        assert!(
            resp.error
                .as_deref()
                .is_some_and(|e| e.contains("command") || e.contains("url")),
            "{:?}",
            resp.error
        );
    }

    #[tokio::test]
    async fn mcp_remove_missing_errors() {
        let _guard = TEST_LOCK.lock().await;
        let resp = handle_mcp_remove(serde_json::json!({
            "name": "definitely-not-registered-xyz",
            "drain": false,
        }))
        .await;
        assert!(!resp.ok);
    }

    #[tokio::test]
    async fn mcp_reload_from_temp_config() {
        let _guard = TEST_LOCK.lock().await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let unique = format!("reload-probe-{}", uuid::Uuid::new_v4());
        std::fs::write(
            &path,
            format!(
                r#"{{
              "tools": {{
                "mcpServers": {{
                  "{unique}": {{
                    "command": "npx",
                    "args": ["-y", "@example/mcp"]
                  }}
                }}
              }}
            }}"#
            ),
        )
        .unwrap();

        let resp = handle_mcp_reload(serde_json::json!({
            "path": path.display().to_string(),
        }))
        .await;
        assert!(resp.ok, "reload failed: {:?}", resp.error);
        let result = resp.result.unwrap();
        // `added` may be 0 if a prior run already had this name; accept
        // added+changed covering the entry.
        let added = result.get("added").and_then(|v| v.as_u64()).unwrap_or(0);
        let changed = result.get("changed").and_then(|v| v.as_u64()).unwrap_or(0);
        assert!(
            added + changed >= 1,
            "expected add or change for {unique}: {result}"
        );

        let list = handle_mcp_list(serde_json::json!({})).await;
        let servers = list
            .result
            .as_ref()
            .and_then(|v| v.get("servers"))
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        assert!(
            servers
                .iter()
                .any(|s| s.get("name").and_then(|n| n.as_str()) == Some(unique.as_str())),
            "{unique} not in list: {servers:?}"
        );

        let _ = handle_mcp_remove(serde_json::json!({
            "name": unique,
            "drain": false,
        }))
        .await;
    }

    #[test]
    fn resolve_reload_path_explicit() {
        let p = resolve_reload_path(Some("/tmp/does-not-need-to-exist-for-explicit.json"));
        assert_eq!(
            p.as_deref(),
            Some(Path::new("/tmp/does-not-need-to-exist-for-explicit.json"))
        );
    }
}
