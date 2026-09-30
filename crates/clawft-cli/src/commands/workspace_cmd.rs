//! `weft workspace` -- workspace lifecycle management.
//!
//! Provides subcommands for creating, listing, loading, inspecting,
//! deleting, and configuring workspaces.
//!
//! Per ADR-021, commands attempt RPC to the kernel daemon first and fall
//! back to local file I/O when the daemon is unavailable.
//!
//! # Examples
//!
//! ```text
//! weft workspace create my-project
//! weft workspace list
//! weft workspace load my-project
//! weft workspace status
//! weft workspace delete my-project
//! weft workspace delete my-project --keep-data
//! weft workspace config set agents.defaults.model openai/gpt-4o
//! weft workspace config get agents.defaults.model
//! weft workspace config reset
//! ```

use std::path::PathBuf;

use clap::{Args, Subcommand};
use comfy_table::{Table, presets::UTF8_FULL};

use clawft_core::workspace::{WorkspaceManager, WorkspaceStatus};
use clawft_rpc::{DaemonClient, Request};

/// Arguments for the `weft workspace` subcommand.
#[derive(Args)]
pub struct WorkspaceArgs {
    #[command(subcommand)]
    pub action: WorkspaceAction,
}

/// Workspace subcommands.
#[derive(Subcommand)]
pub enum WorkspaceAction {
    /// Create a new workspace.
    Create {
        /// Workspace name.
        name: String,

        /// Parent directory for the workspace (defaults to current directory).
        #[arg(long)]
        dir: Option<String>,
    },

    /// List all registered workspaces.
    List {
        /// Show all entries including those with missing directories.
        #[arg(long)]
        all: bool,
    },

    /// Load (activate) a workspace by name or path.
    Load {
        /// Workspace name or filesystem path.
        name_or_path: String,
    },

    /// Show status of the current workspace.
    Status,

    /// Delete a workspace (registry + on-disk markers by default).
    Delete {
        /// Workspace name to delete.
        name: String,

        /// Skip confirmation prompt.
        #[arg(short = 'y', long)]
        yes: bool,

        /// Keep workspace files on disk (registry-only delete).
        ///
        /// Default behavior (FR-W06) removes `.clawft/` and `CLAWFT.md`.
        #[arg(long)]
        keep_data: bool,
    },

    /// Manage workspace configuration.
    Config {
        #[command(subcommand)]
        action: WorkspaceConfigAction,
    },
}

/// Subcommands for `weft workspace config`.
#[derive(Subcommand)]
pub enum WorkspaceConfigAction {
    /// Set a configuration key.
    Set {
        /// Dot-separated config key (e.g. "agents.defaults.model").
        key: String,

        /// Value to set.
        value: String,
    },

    /// Get the value of a configuration key.
    Get {
        /// Dot-separated config key.
        key: String,
    },

    /// Reset workspace configuration to empty.
    Reset,
}

/// Stderr note for the local-registry fallback: names the socket tried.
async fn no_daemon_note() -> String {
    super::daemon_fallback::local_note("using the local workspace registry only").await
}

/// Run the workspace command.
pub async fn run(args: WorkspaceArgs) -> anyhow::Result<()> {
    match args.action {
        WorkspaceAction::Create { name, dir } => ws_create_rpc(&name, dir.as_deref()).await,
        WorkspaceAction::List { all } => ws_list_rpc(all).await,
        WorkspaceAction::Load { name_or_path } => ws_load_rpc(&name_or_path).await,
        WorkspaceAction::Status => ws_status_rpc().await,
        WorkspaceAction::Delete {
            name,
            yes,
            keep_data,
        } => ws_delete_rpc(&name, yes, keep_data).await,
        WorkspaceAction::Config { action } => match action {
            WorkspaceConfigAction::Set { key, value } => ws_config_set_rpc(&key, &value).await,
            WorkspaceConfigAction::Get { key } => ws_config_get_rpc(&key).await,
            WorkspaceConfigAction::Reset => ws_config_reset_rpc().await,
        },
    }
}

// ── RPC-first wrappers ─────────────────────────────────────────

async fn ws_create_rpc(name: &str, dir: Option<&str>) -> anyhow::Result<()> {
    if let Some(mut client) = DaemonClient::connect().await {
        let params = serde_json::json!({ "name": name, "dir": dir });
        let req = Request::with_params("workspace.create", params);
        let resp = client.call(req).await?;
        let data = resp.into_result()?;
        if let Some(path) = data["path"].as_str() {
            println!("Workspace '{name}' created at {path}");
        } else {
            println!("{}", serde_json::to_string_pretty(&data)?);
        }
        return Ok(());
    }
    eprintln!("{}", no_daemon_note().await);
    workspace_create(name, dir)
}

async fn ws_list_rpc(show_all: bool) -> anyhow::Result<()> {
    if let Some(mut client) = DaemonClient::connect().await {
        let params = serde_json::json!({ "all": show_all });
        let req = Request::with_params("workspace.list", params);
        let resp = client.call(req).await?;
        let data = resp.into_result()?;
        if let Some(entries) = data.as_array() {
            if entries.is_empty() {
                println!("No workspaces registered.");
                println!("  Use `weft workspace create <name>` to create one.");
                return Ok(());
            }
            let mut table = Table::new();
            table.load_preset(UTF8_FULL);
            table.set_header(["NAME", "PATH", "STATUS", "LAST ACCESSED"]);
            for e in entries {
                let name = e["name"].as_str().unwrap_or("?");
                let path = e["path"].as_str().unwrap_or("?");
                let status = e["status"].as_str().unwrap_or("?");
                let accessed = e["last_accessed"].as_str().unwrap_or("-");
                table.add_row([name, path, status, accessed]);
            }
            println!("{table}");
            println!("  {} workspace(s)", entries.len());
        } else {
            println!("{}", serde_json::to_string_pretty(&data)?);
        }
        return Ok(());
    }
    eprintln!("{}", no_daemon_note().await);
    workspace_list(show_all)
}

async fn ws_load_rpc(name_or_path: &str) -> anyhow::Result<()> {
    if let Some(mut client) = DaemonClient::connect().await {
        let params = serde_json::json!({ "name_or_path": name_or_path });
        let req = Request::with_params("workspace.load", params);
        let resp = client.call(req).await?;
        let data = resp.into_result()?;
        if let Some(path) = data["path"].as_str() {
            println!("Workspace loaded: {path}");
        } else {
            println!("{}", serde_json::to_string_pretty(&data)?);
        }
        return Ok(());
    }
    eprintln!("{}", no_daemon_note().await);
    workspace_load(name_or_path)
}

async fn ws_status_rpc() -> anyhow::Result<()> {
    if let Some(mut client) = DaemonClient::connect().await {
        let resp = client.simple_call("workspace.status").await?;
        let data = resp.into_result()?;
        if let Some(name) = data["name"].as_str() {
            println!("Workspace: {name}");
            if let Some(path) = data["path"].as_str() {
                println!("  Path:       {path}");
            }
            if let Some(sessions) = data["session_count"].as_u64() {
                println!("  Sessions:   {sessions}");
            }
            if let Some(has_cfg) = data["has_config"].as_bool() {
                println!("  Has config: {}", if has_cfg { "yes" } else { "no" });
            }
            if let Some(has_md) = data["has_clawft_md"].as_bool() {
                println!("  CLAWFT.md:  {}", if has_md { "yes" } else { "no" });
            }
        } else {
            println!("{}", serde_json::to_string_pretty(&data)?);
        }
        return Ok(());
    }
    eprintln!("{}", no_daemon_note().await);
    workspace_status()
}

async fn ws_delete_rpc(name: &str, skip_confirm: bool, keep_data: bool) -> anyhow::Result<()> {
    if !skip_confirm {
        let data_note = if keep_data {
            "files on disk will be kept (--keep-data)"
        } else {
            "this will remove .clawft/ and CLAWFT.md (use --keep-data to keep them)"
        };
        eprint!("Delete workspace '{name}'? ({data_note}) [y/N] ");
        use std::io::Write;
        std::io::stderr().flush().ok();

        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Aborted.");
            return Ok(());
        }
    }

    if let Some(mut client) = DaemonClient::connect().await {
        let params = serde_json::json!({ "name": name, "keep_data": keep_data });
        let req = Request::with_params("workspace.delete", params);
        let resp = client.call(req).await?;
        resp.into_result()?;
        print_workspace_deleted(name, keep_data);
        return Ok(());
    }
    eprintln!("{}", no_daemon_note().await);
    workspace_delete_local(name, keep_data)
}

async fn ws_config_set_rpc(key: &str, value: &str) -> anyhow::Result<()> {
    if let Some(mut client) = DaemonClient::connect().await {
        let params = serde_json::json!({ "key": key, "value": value });
        let req = Request::with_params("workspace.config.set", params);
        let resp = client.call(req).await?;
        resp.into_result()?;
        println!("Set {key} = {value}");
        return Ok(());
    }
    eprintln!("{}", no_daemon_note().await);
    workspace_config_set(key, value)
}

async fn ws_config_get_rpc(key: &str) -> anyhow::Result<()> {
    if let Some(mut client) = DaemonClient::connect().await {
        let params = serde_json::json!({ "key": key });
        let req = Request::with_params("workspace.config.get", params);
        let resp = client.call(req).await?;
        let data = resp.into_result()?;
        if data.is_string() {
            println!("{}", data.as_str().unwrap());
        } else if data.is_null() {
            println!("(not set)");
        } else {
            println!("{}", serde_json::to_string_pretty(&data)?);
        }
        return Ok(());
    }
    eprintln!("{}", no_daemon_note().await);
    workspace_config_get(key)
}

async fn ws_config_reset_rpc() -> anyhow::Result<()> {
    if let Some(mut client) = DaemonClient::connect().await {
        let resp = client.simple_call("workspace.config.reset").await?;
        resp.into_result()?;
        println!("Workspace configuration reset to defaults.");
        return Ok(());
    }
    eprintln!("{}", no_daemon_note().await);
    workspace_config_reset()
}

/// Create a new workspace.
fn workspace_create(name: &str, dir: Option<&str>) -> anyhow::Result<()> {
    let parent = match dir {
        Some(d) => PathBuf::from(d),
        None => std::env::current_dir()?,
    };

    let mut mgr = WorkspaceManager::new()
        .map_err(|e| anyhow::anyhow!("failed to initialize workspace manager: {e}"))?;

    let ws_path = mgr
        .create(name, &parent)
        .map_err(|e| anyhow::anyhow!("failed to create workspace: {e}"))?;

    println!("Workspace '{}' created at {}", name, ws_path.display());
    println!();
    println!("Directories created:");
    for subdir in &["sessions", "memory", "skills", "agents", "hooks"] {
        println!("  .clawft/{subdir}/");
    }
    println!("  .clawft/config.json");
    println!("  CLAWFT.md");

    Ok(())
}

/// List all registered workspaces.
fn workspace_list(show_all: bool) -> anyhow::Result<()> {
    let mgr = WorkspaceManager::new()
        .map_err(|e| anyhow::anyhow!("failed to initialize workspace manager: {e}"))?;

    let entries = mgr.list();

    if entries.is_empty() {
        println!("No workspaces registered.");
        println!("  Use `weft workspace create <name>` to create one.");
        return Ok(());
    }

    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_header(["NAME", "PATH", "STATUS", "LAST ACCESSED"]);

    for entry in &entries {
        let exists = entry.path.join(".clawft").is_dir();

        if !show_all && !exists {
            continue;
        }

        let status = if exists { "ok" } else { "MISSING" };
        let accessed = entry
            .last_accessed
            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|| "-".into());

        table.add_row([
            entry.name.as_str(),
            &entry.path.display().to_string(),
            status,
            &accessed,
        ]);
    }

    println!("{table}");
    println!("  {} workspace(s)", entries.len());

    Ok(())
}

/// Load (activate) a workspace by name or path.
fn workspace_load(name_or_path: &str) -> anyhow::Result<()> {
    let mut mgr = WorkspaceManager::new()
        .map_err(|e| anyhow::anyhow!("failed to initialize workspace manager: {e}"))?;

    let ws_path = mgr.load(name_or_path).map_err(|e| anyhow::anyhow!("{e}"))?;

    println!("Workspace loaded: {}", ws_path.display());
    println!("  Sessions: {}", ws_path.join(".clawft/sessions").display());
    println!("  Memory:   {}", ws_path.join(".clawft/memory").display());
    println!("  Skills:   {}", ws_path.join(".clawft/skills").display());

    Ok(())
}

/// Show status of the current or discovered workspace.
fn workspace_status() -> anyhow::Result<()> {
    let ws_path = clawft_core::workspace::discover_workspace()
        .ok_or_else(|| anyhow::anyhow!("no workspace found"))?;

    let mgr = WorkspaceManager::new()
        .map_err(|e| anyhow::anyhow!("failed to initialize workspace manager: {e}"))?;

    let status = mgr
        .status(&ws_path)
        .map_err(|e| anyhow::anyhow!("failed to get workspace status: {e}"))?;

    print_workspace_status(&status);

    Ok(())
}

/// Format and print workspace status.
fn print_workspace_status(status: &WorkspaceStatus) {
    println!("Workspace: {}", status.name);
    println!("  Path:       {}", status.path.display());
    println!("  Sessions:   {}", status.session_count);
    println!(
        "  Has config: {}",
        if status.has_config { "yes" } else { "no" }
    );
    println!(
        "  CLAWFT.md:  {}",
        if status.has_clawft_md { "yes" } else { "no" }
    );

    // Show scoped resource paths
    let dot_clawft = status.path.join(".clawft");
    println!();
    println!("Scoped resource paths:");
    println!("  Sessions: {}", dot_clawft.join("sessions").display());
    println!("  Memory:   {}", dot_clawft.join("memory").display());
    println!("  Skills:   {}", dot_clawft.join("skills").display());
}

/// Delete a workspace (local fallback, no confirmation — the caller has
/// already handled the prompt).
fn workspace_delete_local(name: &str, keep_data: bool) -> anyhow::Result<()> {
    let mut mgr = WorkspaceManager::new()
        .map_err(|e| anyhow::anyhow!("failed to initialize workspace manager: {e}"))?;

    mgr.delete(name, keep_data)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    print_workspace_deleted(name, keep_data);
    Ok(())
}

fn print_workspace_deleted(name: &str, keep_data: bool) {
    if keep_data {
        println!("Workspace '{name}' removed from registry (files kept on disk).");
    } else {
        println!(
            "Workspace '{name}' deleted (registry entry, .clawft/, and CLAWFT.md removed)."
        );
    }
}

/// Set a workspace configuration key.
fn workspace_config_set(key: &str, value: &str) -> anyhow::Result<()> {
    let ws_path = clawft_core::workspace::discover_workspace()
        .ok_or_else(|| anyhow::anyhow!("no workspace found"))?;

    let config_path = ws_path.join(".clawft").join("config.json");

    let mut config: serde_json::Value = if config_path.exists() {
        let content = std::fs::read_to_string(&config_path)?;
        serde_json::from_str(&content)?
    } else {
        serde_json::json!({})
    };

    // Navigate/create nested keys using dot notation
    set_nested_value(&mut config, key, value)?;

    let output = serde_json::to_string_pretty(&config)?;
    std::fs::write(&config_path, output)?;

    println!("Set {key} = {value}");

    Ok(())
}

/// Get a workspace configuration key.
fn workspace_config_get(key: &str) -> anyhow::Result<()> {
    let ws_path = clawft_core::workspace::discover_workspace()
        .ok_or_else(|| anyhow::anyhow!("no workspace found"))?;

    let config_path = ws_path.join(".clawft").join("config.json");

    if !config_path.exists() {
        println!("(not set)");
        return Ok(());
    }

    let content = std::fs::read_to_string(&config_path)?;
    let config: serde_json::Value = serde_json::from_str(&content)?;

    let value = get_nested_value(&config, key);
    match value {
        Some(v) => {
            if v.is_string() {
                println!("{}", v.as_str().unwrap());
            } else {
                println!("{}", serde_json::to_string_pretty(v)?);
            }
        }
        None => println!("(not set)"),
    }

    Ok(())
}

/// Reset workspace configuration to empty.
fn workspace_config_reset() -> anyhow::Result<()> {
    let ws_path = clawft_core::workspace::discover_workspace()
        .ok_or_else(|| anyhow::anyhow!("no workspace found"))?;

    let config_path = ws_path.join(".clawft").join("config.json");
    std::fs::write(&config_path, "{}\n")?;

    println!("Workspace configuration reset to defaults.");

    Ok(())
}

/// Set a value at a dot-separated path in a JSON object.
///
/// Creates intermediate objects as needed. The `value` is stored as a
/// JSON string unless it can be parsed as a number or boolean.
fn set_nested_value(root: &mut serde_json::Value, key: &str, value: &str) -> anyhow::Result<()> {
    let parts: Vec<&str> = key.split('.').collect();
    if parts.is_empty() || parts.iter().all(|p| p.is_empty()) {
        anyhow::bail!("empty config key");
    }

    let mut current = root;
    for part in &parts[..parts.len() - 1] {
        if !current.is_object() {
            *current = serde_json::json!({});
        }
        current = current
            .as_object_mut()
            .unwrap()
            .entry(*part)
            .or_insert_with(|| serde_json::json!({}));
    }

    let last = parts[parts.len() - 1];

    // Try to parse value as JSON primitive
    let json_value = if value == "true" {
        serde_json::Value::Bool(true)
    } else if value == "false" {
        serde_json::Value::Bool(false)
    } else if value == "null" {
        serde_json::Value::Null
    } else if let Ok(n) = value.parse::<i64>() {
        serde_json::Value::Number(n.into())
    } else if let Ok(n) = value.parse::<f64>() {
        serde_json::Number::from_f64(n)
            .map(serde_json::Value::Number)
            .unwrap_or_else(|| serde_json::Value::String(value.into()))
    } else {
        serde_json::Value::String(value.into())
    };

    if !current.is_object() {
        *current = serde_json::json!({});
    }
    current
        .as_object_mut()
        .unwrap()
        .insert(last.into(), json_value);

    Ok(())
}

/// Get a value at a dot-separated path in a JSON object.
fn get_nested_value<'a>(root: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
    let parts: Vec<&str> = key.split('.').collect();
    let mut current = root;
    for part in parts {
        current = current.get(part)?;
    }
    Some(current)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── set_nested_value ────────────────────────────────────────────

    #[test]
    fn set_nested_value_simple() {
        let mut root = serde_json::json!({});
        set_nested_value(&mut root, "key", "value").unwrap();
        assert_eq!(root["key"], "value");
    }

    #[test]
    fn set_nested_value_deep() {
        let mut root = serde_json::json!({});
        set_nested_value(&mut root, "a.b.c", "deep").unwrap();
        assert_eq!(root["a"]["b"]["c"], "deep");
    }

    #[test]
    fn set_nested_value_integer() {
        let mut root = serde_json::json!({});
        set_nested_value(&mut root, "count", "42").unwrap();
        assert_eq!(root["count"], 42);
    }

    #[test]
    fn set_nested_value_boolean() {
        let mut root = serde_json::json!({});
        set_nested_value(&mut root, "enabled", "true").unwrap();
        assert_eq!(root["enabled"], true);
    }

    #[test]
    fn set_nested_value_float() {
        let mut root = serde_json::json!({});
        set_nested_value(&mut root, "temp", "0.7").unwrap();
        // Compare as f64 since JSON numbers may not round-trip perfectly
        assert!((root["temp"].as_f64().unwrap() - 0.7).abs() < f64::EPSILON);
    }

    #[test]
    fn set_nested_value_preserves_existing() {
        let mut root = serde_json::json!({"a": {"x": 1}});
        set_nested_value(&mut root, "a.y", "2").unwrap();
        assert_eq!(root["a"]["x"], 1);
        assert_eq!(root["a"]["y"], 2);
    }

    #[test]
    fn set_nested_value_empty_key_fails() {
        let mut root = serde_json::json!({});
        let result = set_nested_value(&mut root, "", "value");
        assert!(result.is_err());
    }

    // ── get_nested_value ────────────────────────────────────────────

    #[test]
    fn get_nested_value_found() {
        let root = serde_json::json!({"a": {"b": {"c": "found"}}});
        let val = get_nested_value(&root, "a.b.c");
        assert_eq!(val.unwrap(), "found");
    }

    #[test]
    fn get_nested_value_not_found() {
        let root = serde_json::json!({"a": 1});
        let val = get_nested_value(&root, "a.b.c");
        assert!(val.is_none());
    }

    #[test]
    fn get_nested_value_top_level() {
        let root = serde_json::json!({"key": "val"});
        let val = get_nested_value(&root, "key");
        assert_eq!(val.unwrap(), "val");
    }
}
