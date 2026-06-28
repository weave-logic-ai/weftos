//! `weft tools` -- CLI commands for tool discovery and management.
//!
//! Provides subcommands:
//!
//! - `weft tools list` -- list all registered tools with source annotation.
//! - `weft tools show <name>` -- show tool details and parameter schema.
//! - `weft tools mcp` -- list configured MCP servers and tool counts.
//! - `weft tools search <query>` -- search tools by name or description.
//! - `weft tools deny <pattern>` -- add a glob pattern to the tool denylist.
//! - `weft tools allow <pattern>` -- remove a pattern from the tool denylist.

use std::sync::Arc;

use clap::{Args, Subcommand};
use comfy_table::{Table, presets};

use clawft_core::tools::registry::ToolRegistry;
use clawft_rpc::{DaemonClient, Request};
use clawft_types::config::Config;

/// Arguments for the `weft tools` subcommand.
#[derive(Args)]
pub struct ToolsArgs {
    #[command(subcommand)]
    pub action: ToolsAction,
}

/// Subcommands for `weft tools`.
#[derive(Subcommand)]
pub enum ToolsAction {
    /// List all registered tools with source annotation.
    List {
        /// Config file path (overrides auto-discovery).
        #[arg(short, long)]
        config: Option<String>,
    },

    /// Show details and parameter schema for a specific tool.
    Show {
        /// Tool name to inspect.
        name: String,

        /// Config file path (overrides auto-discovery).
        #[arg(short, long)]
        config: Option<String>,
    },

    /// List configured MCP servers and their tool counts.
    Mcp {
        /// Config file path (overrides auto-discovery).
        #[arg(short, long)]
        config: Option<String>,
    },

    /// Search tools by name or description.
    Search {
        /// Search query (case-insensitive substring match).
        query: String,

        /// Config file path (overrides auto-discovery).
        #[arg(short, long)]
        config: Option<String>,
    },

    /// Add a glob pattern to the tool denylist.
    Deny {
        /// Glob pattern (e.g. "exec_*", "claude-flow__browser_*").
        pattern: String,

        /// Config file path (overrides auto-discovery).
        #[arg(short, long)]
        config: Option<String>,
    },

    /// Remove a pattern from the tool denylist.
    Allow {
        /// Exact pattern to remove from the denylist.
        pattern: String,

        /// Config file path (overrides auto-discovery).
        #[arg(short, long)]
        config: Option<String>,
    },
}

/// Warning printed when falling back to local execution without daemon.
const DAEMON_FALLBACK_WARNING: &str = "Warning: running without kernel daemon — results may not reflect live kernel state. \
     Start daemon with: weaver kernel start";

/// Try to send an RPC to the daemon. Returns `Some(result_json)` on success,
/// or `None` if no daemon is running (caller should fall back to local).
async fn try_daemon_rpc(method: &str, params: serde_json::Value) -> Option<serde_json::Value> {
    let mut client = DaemonClient::connect().await?;
    let request = Request::with_params(method, params);
    match client.call(request).await {
        Ok(resp) => match resp.into_result() {
            Ok(val) => Some(val),
            Err(e) => {
                eprintln!("Daemon RPC error: {e}");
                None
            }
        },
        Err(e) => {
            eprintln!("Daemon RPC error: {e}");
            None
        }
    }
}

/// Print daemon RPC result to stdout. If the result has an `output` string
/// field it is printed verbatim; otherwise the full JSON value is pretty-printed.
fn print_daemon_result(result: &serde_json::Value) -> anyhow::Result<()> {
    if let Some(output) = result.get("output").and_then(|v| v.as_str()) {
        print!("{output}");
    } else {
        println!("{}", serde_json::to_string_pretty(result)?);
    }
    Ok(())
}

/// Run the tools subcommand.
pub async fn run(args: ToolsArgs) -> anyhow::Result<()> {
    match args.action {
        ToolsAction::List { config } => {
            if let Some(result) = try_daemon_rpc("tools.list", serde_json::json!({})).await {
                return print_daemon_result(&result);
            }
            eprintln!("{DAEMON_FALLBACK_WARNING}");
            let (cfg, platform) = load_platform_config(config.as_deref()).await?;
            let registry = build_registry(&cfg, platform).await;
            tools_list(&registry)
        }
        ToolsAction::Show { name, config } => {
            if let Some(result) =
                try_daemon_rpc("tools.show", serde_json::json!({ "name": name })).await
            {
                return print_daemon_result(&result);
            }
            eprintln!("{DAEMON_FALLBACK_WARNING}");
            let (cfg, platform) = load_platform_config(config.as_deref()).await?;
            let registry = build_registry(&cfg, platform).await;
            tools_show(&name, &registry)
        }
        ToolsAction::Mcp { config } => {
            if let Some(result) = try_daemon_rpc("tools.mcp", serde_json::json!({})).await {
                return print_daemon_result(&result);
            }
            eprintln!("{DAEMON_FALLBACK_WARNING}");
            let (cfg, platform) = load_platform_config(config.as_deref()).await?;
            let registry = build_registry(&cfg, platform).await;
            tools_mcp(&cfg, &registry)
        }
        ToolsAction::Search { query, config } => {
            if let Some(result) =
                try_daemon_rpc("tools.search", serde_json::json!({ "query": query })).await
            {
                return print_daemon_result(&result);
            }
            eprintln!("{DAEMON_FALLBACK_WARNING}");
            let (cfg, platform) = load_platform_config(config.as_deref()).await?;
            let registry = build_registry(&cfg, platform).await;
            tools_search(&query, &registry)
        }
        ToolsAction::Deny { pattern, config } => {
            if let Some(result) =
                try_daemon_rpc("tools.deny", serde_json::json!({ "pattern": pattern })).await
            {
                return print_daemon_result(&result);
            }
            eprintln!("{DAEMON_FALLBACK_WARNING}");
            tools_deny(&pattern, config.as_deref())
        }
        ToolsAction::Allow { pattern, config } => {
            if let Some(result) =
                try_daemon_rpc("tools.allow", serde_json::json!({ "pattern": pattern })).await
            {
                return print_daemon_result(&result);
            }
            eprintln!("{DAEMON_FALLBACK_WARNING}");
            tools_allow(&pattern, config.as_deref())
        }
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────

/// Load config and create a NativePlatform.
async fn load_platform_config(
    config_override: Option<&str>,
) -> anyhow::Result<(Config, Arc<clawft_platform::NativePlatform>)> {
    let platform = Arc::new(clawft_platform::NativePlatform::new());
    let cfg = super::load_config(platform.as_ref(), config_override).await?;
    Ok((cfg, platform))
}

/// Build the full tool registry (same set as `weft agent` / `weft gateway`).
async fn build_registry(
    config: &Config,
    platform: Arc<clawft_platform::NativePlatform>,
) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    super::register_core_tools(&mut registry, config, platform).await;
    registry
}

/// Classify a tool's source from its name.
///
/// - Contains `__` -> `mcp:{server}` (prefix before first `__`).
/// - Equals `delegate_task` -> `delegation`.
/// - Otherwise -> `builtin`.
fn classify_source(name: &str) -> String {
    if name == "delegate_task" {
        return "delegation".into();
    }
    if let Some((server, _)) = name.split_once("__") {
        return format!("mcp:{server}");
    }
    "builtin".into()
}

/// Truncate a string to `max_len` characters, appending "..." if truncated.
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len.saturating_sub(3)])
    }
}

/// Discover the config file path for write-back operations.
fn discover_config_file(config_override: Option<&str>) -> anyhow::Result<std::path::PathBuf> {
    if let Some(path) = config_override {
        return Ok(std::path::PathBuf::from(path));
    }
    let platform = clawft_platform::NativePlatform::new();
    super::discover_config_path(&platform).ok_or_else(|| {
        anyhow::anyhow!(
            "no config file found. Create one at ~/.clawft/config.json \
             or set CLAWFT_CONFIG, or pass --config."
        )
    })
}

// ── Subcommand implementations ──────────────────────────────────────────

/// List all registered tools.
fn tools_list(registry: &ToolRegistry) -> anyhow::Result<()> {
    let names = registry.list();

    if names.is_empty() {
        println!("No tools registered.");
        return Ok(());
    }

    let mut table = Table::new();
    table.load_preset(presets::UTF8_FULL_CONDENSED);
    table.set_header(["NAME", "SOURCE", "DESCRIPTION"]);

    for name in &names {
        let source = classify_source(name);
        let desc = registry
            .get(name)
            .map(|t| truncate(t.description(), 60))
            .unwrap_or_default();
        table.add_row([name.as_str(), &source, &desc]);
    }

    println!("{table}");
    println!();
    println!("Total: {} tool(s)", names.len());

    Ok(())
}

/// Show details and parameter schema for a specific tool.
fn tools_show(name: &str, registry: &ToolRegistry) -> anyhow::Result<()> {
    let tool = registry.get(name).ok_or_else(|| {
        anyhow::anyhow!("tool not found: {name}\nUse 'weft tools list' to see available tools.")
    })?;

    println!("Tool: {}", tool.name());
    println!("Source: {}", classify_source(name));
    println!("Description: {}", tool.description());

    // Show metadata if present.
    if let Some(meta) = registry.get_metadata(name) {
        println!("Permission level: {:?}", meta.required_permission_level);
        if !meta.required_custom_permissions.is_empty() {
            let keys: Vec<&str> = meta
                .required_custom_permissions
                .keys()
                .map(|k| k.as_str())
                .collect();
            println!("Custom permissions: {}", keys.join(", "));
        }
    }

    // Show parameters schema.
    let params = tool.parameters();
    println!();
    println!("Parameters:");
    match serde_json::to_string_pretty(&params) {
        Ok(json) => println!("{json}"),
        Err(e) => eprintln!("  (failed to serialize: {e})"),
    }

    Ok(())
}

/// List configured MCP servers with tool counts.
fn tools_mcp(config: &Config, registry: &ToolRegistry) -> anyhow::Result<()> {
    let servers = &config.tools.mcp_servers;

    if servers.is_empty() {
        println!("No MCP servers configured.");
        println!();
        println!("Add MCP servers in config.json under tools.mcp_servers.");
        return Ok(());
    }

    let names = registry.list();

    let mut table = Table::new();
    table.load_preset(presets::UTF8_FULL_CONDENSED);
    table.set_header(["SERVER", "TRANSPORT", "INTERNAL", "TOOLS"]);

    let mut sorted_servers: Vec<(&String, &clawft_types::config::MCPServerConfig)> =
        servers.iter().collect();
    sorted_servers.sort_by_key(|(k, _)| k.as_str());

    for (server_name, server_cfg) in &sorted_servers {
        let transport = if !server_cfg.url.is_empty() {
            "http"
        } else if !server_cfg.command.is_empty() {
            "stdio"
        } else {
            "unknown"
        };

        let internal = if server_cfg.internal_only {
            "yes"
        } else {
            "no"
        };

        let prefix = format!("{server_name}__");
        let tool_count = names.iter().filter(|n| n.starts_with(&prefix)).count();

        table.add_row([
            server_name.as_str(),
            transport,
            internal,
            &tool_count.to_string(),
        ]);
    }

    println!("{table}");
    println!();
    println!("Total: {} MCP server(s)", sorted_servers.len());

    Ok(())
}

/// Search tools by name or description (case-insensitive).
fn tools_search(query: &str, registry: &ToolRegistry) -> anyhow::Result<()> {
    let names = registry.list();
    let query_lower = query.to_lowercase();

    let matches: Vec<&String> = names
        .iter()
        .filter(|name| {
            let name_match = name.to_lowercase().contains(&query_lower);
            let desc_match = registry
                .get(name)
                .map(|t| t.description().to_lowercase().contains(&query_lower))
                .unwrap_or(false);
            name_match || desc_match
        })
        .collect();

    if matches.is_empty() {
        println!("No tools matching '{query}'.");
        return Ok(());
    }

    let mut table = Table::new();
    table.load_preset(presets::UTF8_FULL_CONDENSED);
    table.set_header(["NAME", "SOURCE", "DESCRIPTION"]);

    for name in &matches {
        let source = classify_source(name);
        let desc = registry
            .get(name)
            .map(|t| truncate(t.description(), 60))
            .unwrap_or_default();
        table.add_row([name.as_str(), &source, &desc]);
    }

    println!("{table}");
    println!();
    println!("Found: {} tool(s) matching '{query}'", matches.len());

    Ok(())
}

/// Add a glob pattern to the admin tool denylist.
fn tools_deny(pattern: &str, config_override: Option<&str>) -> anyhow::Result<()> {
    let config_path = discover_config_file(config_override)?;

    let mut raw: serde_json::Value = if config_path.exists() {
        let contents = std::fs::read_to_string(&config_path)
            .map_err(|e| anyhow::anyhow!("failed to read config: {e}"))?;
        serde_json::from_str(&contents)
            .map_err(|e| anyhow::anyhow!("failed to parse config: {e}"))?
    } else {
        serde_json::json!({})
    };

    // Navigate to routing.permissions.admin.tool_denylist, creating as needed.
    let denylist = raw
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("config root is not an object"))?
        .entry("routing")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("routing is not an object"))?
        .entry("permissions")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("permissions is not an object"))?
        .entry("admin")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("admin is not an object"))?
        .entry("tool_denylist")
        .or_insert_with(|| serde_json::json!([]));

    let arr = denylist
        .as_array_mut()
        .ok_or_else(|| anyhow::anyhow!("tool_denylist is not an array"))?;

    // Check if pattern already exists.
    let pattern_val = serde_json::Value::String(pattern.to_string());
    if arr.contains(&pattern_val) {
        println!("Pattern '{pattern}' is already in the denylist.");
        return Ok(());
    }

    arr.push(pattern_val);

    let output = serde_json::to_string_pretty(&raw)?;
    std::fs::write(&config_path, output)
        .map_err(|e| anyhow::anyhow!("failed to write config: {e}"))?;

    println!("Added '{pattern}' to tool denylist.");
    println!("Config updated: {}", config_path.display());

    Ok(())
}

/// Remove a pattern from the admin tool denylist.
fn tools_allow(pattern: &str, config_override: Option<&str>) -> anyhow::Result<()> {
    let config_path = discover_config_file(config_override)?;

    if !config_path.exists() {
        anyhow::bail!(
            "config file not found: {}. Nothing to update.",
            config_path.display()
        );
    }

    let contents = std::fs::read_to_string(&config_path)
        .map_err(|e| anyhow::anyhow!("failed to read config: {e}"))?;
    let mut raw: serde_json::Value = serde_json::from_str(&contents)
        .map_err(|e| anyhow::anyhow!("failed to parse config: {e}"))?;

    // Navigate to routing.permissions.admin.tool_denylist.
    let denylist = raw
        .pointer_mut("/routing/permissions/admin/tool_denylist")
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no tool_denylist found in config. \
                 Use 'weft tools deny' to create one first."
            )
        })?;

    let arr = denylist
        .as_array_mut()
        .ok_or_else(|| anyhow::anyhow!("tool_denylist is not an array"))?;

    let pattern_val = serde_json::Value::String(pattern.to_string());
    let original_len = arr.len();
    arr.retain(|v| v != &pattern_val);

    if arr.len() == original_len {
        println!("Pattern '{pattern}' was not in the denylist.");
        return Ok(());
    }

    let output = serde_json::to_string_pretty(&raw)?;
    std::fs::write(&config_path, output)
        .map_err(|e| anyhow::anyhow!("failed to write config: {e}"))?;

    println!("Removed '{pattern}' from tool denylist.");
    println!("Config updated: {}", config_path.display());

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── classify_source ──────────────────────────────────────────────

    #[test]
    fn classify_builtin() {
        assert_eq!(classify_source("read_file"), "builtin");
        assert_eq!(classify_source("web_search"), "builtin");
    }

    #[test]
    fn classify_mcp() {
        assert_eq!(
            classify_source("claude-flow__agent_spawn"),
            "mcp:claude-flow"
        );
        assert_eq!(classify_source("github__create_pr"), "mcp:github");
    }

    #[test]
    fn classify_delegation() {
        assert_eq!(classify_source("delegate_task"), "delegation");
    }

    // ── truncate ─────────────────────────────────────────────────────

    #[test]
    fn truncate_short() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_long() {
        let result = truncate("this is a very long description text", 20);
        assert!(result.ends_with("..."));
        assert!(result.len() <= 20);
    }

    #[test]
    fn truncate_exact() {
        assert_eq!(truncate("hello", 5), "hello");
    }

    // ── deny / allow config manipulation ─────────────────────────────

    #[test]
    fn deny_adds_pattern_to_config() {
        let dir = std::env::temp_dir().join(format!("clawft_tools_deny_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let config_path = dir.join("config.json");
        std::fs::write(&config_path, "{}").unwrap();

        let result = tools_deny("exec_*", Some(config_path.to_str().unwrap()));
        assert!(result.is_ok());

        let contents = std::fs::read_to_string(&config_path).unwrap();
        let raw: serde_json::Value = serde_json::from_str(&contents).unwrap();
        let denylist = raw
            .pointer("/routing/permissions/admin/tool_denylist")
            .unwrap()
            .as_array()
            .unwrap();
        assert!(denylist.contains(&serde_json::json!("exec_*")));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn deny_idempotent() {
        let dir =
            std::env::temp_dir().join(format!("clawft_tools_deny_idem_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let config_path = dir.join("config.json");
        std::fs::write(&config_path, "{}").unwrap();

        tools_deny("exec_*", Some(config_path.to_str().unwrap())).unwrap();
        tools_deny("exec_*", Some(config_path.to_str().unwrap())).unwrap();

        let contents = std::fs::read_to_string(&config_path).unwrap();
        let raw: serde_json::Value = serde_json::from_str(&contents).unwrap();
        let denylist = raw
            .pointer("/routing/permissions/admin/tool_denylist")
            .unwrap()
            .as_array()
            .unwrap();
        assert_eq!(
            denylist
                .iter()
                .filter(|v| v == &&serde_json::json!("exec_*"))
                .count(),
            1,
            "duplicate entry should not be added"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn allow_removes_pattern() {
        let dir = std::env::temp_dir().join(format!("clawft_tools_allow_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let config_path = dir.join("config.json");

        // Seed with a denylist entry.
        let initial = serde_json::json!({
            "routing": {
                "permissions": {
                    "admin": {
                        "tool_denylist": ["exec_*", "browser_*"]
                    }
                }
            }
        });
        std::fs::write(
            &config_path,
            serde_json::to_string_pretty(&initial).unwrap(),
        )
        .unwrap();

        let result = tools_allow("exec_*", Some(config_path.to_str().unwrap()));
        assert!(result.is_ok());

        let contents = std::fs::read_to_string(&config_path).unwrap();
        let raw: serde_json::Value = serde_json::from_str(&contents).unwrap();
        let denylist = raw
            .pointer("/routing/permissions/admin/tool_denylist")
            .unwrap()
            .as_array()
            .unwrap();
        assert!(!denylist.contains(&serde_json::json!("exec_*")));
        assert!(denylist.contains(&serde_json::json!("browser_*")));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn allow_pattern_not_found() {
        let dir =
            std::env::temp_dir().join(format!("clawft_tools_allow_nf_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let config_path = dir.join("config.json");

        let initial = serde_json::json!({
            "routing": {
                "permissions": {
                    "admin": {
                        "tool_denylist": ["exec_*"]
                    }
                }
            }
        });
        std::fs::write(
            &config_path,
            serde_json::to_string_pretty(&initial).unwrap(),
        )
        .unwrap();

        let result = tools_allow("nonexistent_*", Some(config_path.to_str().unwrap()));
        assert!(result.is_ok());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn allow_no_config_file() {
        let result = tools_allow("anything", Some("/nonexistent/path/config.json"));
        assert!(result.is_err());
    }
}
