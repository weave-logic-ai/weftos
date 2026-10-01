//! `weft mcp` — manage configured MCP servers (add / list / remove).
//!
//! Thin CLI over the user config file (`tools.mcp_servers` /
//! `tools.mcpServers`). Writes are atomic (tmp + rename).
//!
//! # Ownership (ADR-070 / WEFT-494)
//!
//! - **This CLI** owns *durable* config mutation (works offline).
//! - **Daemon RPCs** `mcp.add` / `mcp.list` / `mcp.remove` / `mcp.reload`
//!   own the *live* shared registry inside the kernel process.
//! - After add/remove, the CLI best-effort calls `mcp.reload` so a
//!   running daemon picks up the file change (fallback: `config.reload`,
//!   then a restart hint).
//! - Gateway/UI also run the WEFT-493 file watcher (WEFT-187 API) so
//!   durable edits apply without an explicit RPC when the host is up.
//!
//! # Examples
//!
//! ```text
//! weft mcp add my-server --command npx --arg -y --arg @example/mcp-server
//! weft mcp add my-server -- npx -y @example/mcp-server
//! weft mcp add remote --url https://mcp.example.com/sse
//! weft mcp add local --command uvx --env API_KEY=secret --arg my-mcp
//! weft mcp list
//! weft mcp remove my-server
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use comfy_table::{Table, presets};
use clawft_rpc::Request;

/// Arguments for the `weft mcp` subcommand.
#[derive(Args, Debug)]
pub struct McpArgs {
    #[command(subcommand)]
    pub action: McpAction,
}

/// Subcommands for `weft mcp`.
#[derive(Subcommand, Debug)]
pub enum McpAction {
    /// Add or replace an MCP server in config.
    Add {
        /// Unique server name (key under tools.mcp_servers).
        name: String,

        /// Stdio command to spawn (e.g. `npx`, `uvx`, `claude`).
        #[arg(long)]
        command: Option<String>,

        /// Command arguments (repeatable; may start with `-`).
        #[arg(
            long = "arg",
            action = clap::ArgAction::Append,
            allow_hyphen_values = true
        )]
        args: Vec<String>,

        /// Streamable HTTP endpoint URL.
        #[arg(long)]
        url: Option<String>,

        /// Extra environment variables (`KEY=VALUE`, repeatable).
        #[arg(
            long = "env",
            value_name = "KEY=VALUE",
            action = clap::ArgAction::Append,
            allow_hyphen_values = true
        )]
        env: Vec<String>,

        /// If true, tools are not registered in the ToolRegistry (default: true).
        ///
        /// Pass `--internal-only=false` to register tools for LLM use.
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set, num_args = 0..=1, default_missing_value = "true")]
        internal_only: bool,

        /// Config file path (overrides auto-discovery).
        #[arg(short, long)]
        config: Option<String>,

        /// Trailing command + args after `--` (claude-style):
        /// `weft mcp add name -- npx -y @pkg`.
        #[arg(last = true, allow_hyphen_values = true)]
        rest: Vec<String>,
    },

    /// List configured MCP servers.
    List {
        /// Config file path (overrides auto-discovery).
        #[arg(short, long)]
        config: Option<String>,
    },

    /// Remove an MCP server from config.
    Remove {
        /// Server name to remove.
        name: String,

        /// Config file path (overrides auto-discovery).
        #[arg(short, long)]
        config: Option<String>,
    },
}

/// Hint printed after a successful config mutation when reload cannot be
/// triggered via daemon RPC.
const RELOAD_HINT: &str = "Hint: if gateway is running, the MCP config watcher \
(WEFT-493) will pick this up shortly; otherwise restart the agent/gateway.";

/// Run the `weft mcp` subcommand.
pub async fn run(args: McpArgs) -> anyhow::Result<()> {
    match args.action {
        McpAction::Add {
            name,
            command,
            args,
            url,
            env,
            internal_only,
            config,
            rest,
        } => {
            mcp_add(
                &name,
                command.as_deref(),
                &args,
                url.as_deref(),
                &env,
                internal_only,
                config.as_deref(),
                &rest,
            )
            .await
        }
        McpAction::List { config } => mcp_list(config.as_deref()).await,
        McpAction::Remove { name, config } => mcp_remove(&name, config.as_deref()).await,
    }
}

// ── Add ──────────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn mcp_add(
    name: &str,
    command: Option<&str>,
    args: &[String],
    url: Option<&str>,
    env_flags: &[String],
    internal_only: bool,
    config_override: Option<&str>,
    rest: &[String],
) -> anyhow::Result<()> {
    let name = name.trim();
    if name.is_empty() {
        anyhow::bail!("server name must not be empty");
    }

    let (cmd, cmd_args) = resolve_command(command, args, rest)?;
    let url = url.unwrap_or("").trim().to_string();

    if cmd.is_empty() && url.is_empty() {
        anyhow::bail!(
            "provide --command <cmd> (or trailing args after --) and/or --url <endpoint>"
        );
    }

    let env = parse_env_flags(env_flags)?;

    let path = resolve_writable_config_path(config_override)?;
    let mut doc = load_config_document(&path)?;

    let entry = build_server_entry(&cmd, &cmd_args, &url, &env, internal_only, doc.format);
    let servers = ensure_mcp_servers_map(&mut doc)?;
    let replaced = servers.contains_key(name);
    servers.insert(name.to_string(), entry);

    persist_config_document(&path, &doc)?;

    let transport = if !url.is_empty() && cmd.is_empty() {
        "http"
    } else if !cmd.is_empty() {
        "stdio"
    } else {
        "mixed"
    };

    if replaced {
        println!("Updated MCP server '{name}' ({transport}).");
    } else {
        println!("Added MCP server '{name}' ({transport}).");
    }
    println!("Config updated: {}", path.display());
    print_server_summary(name, &cmd, &cmd_args, &url, &env, internal_only);
    maybe_trigger_reload().await;
    Ok(())
}

fn resolve_command(
    command: Option<&str>,
    args: &[String],
    rest: &[String],
) -> anyhow::Result<(String, Vec<String>)> {
    // Trailing `-- cmd arg...` takes precedence when present (claude-style).
    if !rest.is_empty() {
        let mut iter = rest.iter();
        let cmd = iter.next().cloned().unwrap_or_default();
        let mut rest_args: Vec<String> = iter.cloned().collect();
        // Also append any explicit --arg flags.
        rest_args.extend(args.iter().cloned());
        let _ = command; // trailing form owns the command name
        return Ok((cmd, rest_args));
    }

    let cmd = command.unwrap_or("").trim().to_string();
    Ok((cmd, args.to_vec()))
}

fn parse_env_flags(flags: &[String]) -> anyhow::Result<HashMap<String, String>> {
    let mut env = HashMap::new();
    for flag in flags {
        let (k, v) = flag.split_once('=').ok_or_else(|| {
            anyhow::anyhow!("invalid --env '{flag}': expected KEY=VALUE")
        })?;
        if k.is_empty() {
            anyhow::bail!("invalid --env '{flag}': empty key");
        }
        env.insert(k.to_string(), v.to_string());
    }
    Ok(env)
}

fn build_server_entry(
    command: &str,
    args: &[String],
    url: &str,
    env: &HashMap<String, String>,
    internal_only: bool,
    format: ConfigFormat,
) -> serde_json::Value {
    let mut map = serde_json::Map::new();

    if !command.is_empty() {
        map.insert("command".into(), serde_json::json!(command));
    }
    if !args.is_empty() {
        map.insert("args".into(), serde_json::json!(args));
    }
    if !env.is_empty() {
        map.insert("env".into(), serde_json::json!(env));
    }
    if !url.is_empty() {
        map.insert("url".into(), serde_json::json!(url));
    }

    // Prefer camelCase for JSON (docs/fixtures); snake_case for TOML.
    let internal_key = match format {
        ConfigFormat::Json => "internalOnly",
        ConfigFormat::Toml => "internal_only",
    };
    map.insert(internal_key.into(), serde_json::json!(internal_only));

    serde_json::Value::Object(map)
}

fn print_server_summary(
    name: &str,
    command: &str,
    args: &[String],
    url: &str,
    env: &HashMap<String, String>,
    internal_only: bool,
) {
    if !command.is_empty() {
        let joined = if args.is_empty() {
            command.to_string()
        } else {
            format!("{command} {}", args.join(" "))
        };
        println!("  command:       {joined}");
    }
    if !url.is_empty() {
        println!("  url:           {url}");
    }
    if !env.is_empty() {
        let mut keys: Vec<&String> = env.keys().collect();
        keys.sort();
        let pairs: Vec<String> = keys
            .into_iter()
            .map(|k| format!("{k}={}", env.get(k).map(|s| s.as_str()).unwrap_or("")))
            .collect();
        println!("  env:           {}", pairs.join(", "));
    }
    println!("  internal_only: {internal_only}");
    let _ = name; // reserved for future per-name detail
}

// ── List ─────────────────────────────────────────────────────────────────

async fn mcp_list(config_override: Option<&str>) -> anyhow::Result<()> {
    // Prefer live daemon registry when available (WEFT-494: mcp.list;
    // tools.mcp is the CLI-compat alias).
    for method in ["mcp.list", "tools.mcp"] {
        if let Some(result) = try_daemon_rpc(method, serde_json::json!({})).await
            && let Some(output) = result.get("output").and_then(|v| v.as_str())
        {
            print!("{output}");
            return Ok(());
        }
        // Fall through if the daemon shape is unexpected.
    }

    let path = resolve_writable_config_path(config_override).ok();
    let servers = match &path {
        Some(p) if p.exists() => {
            let doc = load_config_document(p)?;
            extract_mcp_servers(&doc.root)
        }
        _ => {
            // Fall back to merged discovery (read-only).
            let platform = clawft_platform::NativePlatform::new();
            let cfg = super::load_config(&platform, config_override).await?;
            cfg.tools
                .mcp_servers
                .iter()
                .map(|(k, v)| (k.clone(), server_to_value(v)))
                .collect()
        }
    };

    if servers.is_empty() {
        println!("No MCP servers configured.");
        println!();
        println!("Add one with:");
        println!("  weft mcp add <name> --command <cmd> [--arg ...] [--env KEY=VAL]");
        println!("  weft mcp add <name> --url https://…");
        println!("  weft mcp add <name> -- npx -y @example/mcp-server");
        if let Some(p) = path {
            println!();
            println!("Config path: {}", p.display());
        }
        return Ok(());
    }

    let mut table = Table::new();
    table.load_preset(presets::UTF8_FULL_CONDENSED);
    table.set_header(["NAME", "TRANSPORT", "COMMAND / URL", "INTERNAL"]);

    let mut names: Vec<String> = servers.keys().cloned().collect();
    names.sort();

    for name in &names {
        let entry = &servers[name];
        let command = entry
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let args = entry
            .get("args")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();
        let url = entry.get("url").and_then(|v| v.as_str()).unwrap_or("");
        let internal = entry
            .get("internal_only")
            .or_else(|| entry.get("internalOnly"))
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let (transport, detail) = if !url.is_empty() && command.is_empty() {
            ("http", url.to_string())
        } else if !command.is_empty() {
            let detail = if args.is_empty() {
                command.to_string()
            } else {
                format!("{command} {args}")
            };
            ("stdio", detail)
        } else if !url.is_empty() {
            ("http", url.to_string())
        } else {
            ("unknown", String::new())
        };

        table.add_row([
            name.as_str(),
            transport,
            &detail,
            if internal { "yes" } else { "no" },
        ]);
    }

    println!("{table}");
    println!();
    println!("Total: {} MCP server(s)", names.len());
    if let Some(p) = path {
        println!("Config: {}", p.display());
    }

    Ok(())
}

fn server_to_value(cfg: &clawft_types::config::MCPServerConfig) -> serde_json::Value {
    serde_json::json!({
        "command": cfg.command,
        "args": cfg.args,
        "env": cfg.env,
        "url": cfg.url,
        "internal_only": cfg.internal_only,
    })
}

// ── Remove ───────────────────────────────────────────────────────────────

async fn mcp_remove(name: &str, config_override: Option<&str>) -> anyhow::Result<()> {
    let name = name.trim();
    if name.is_empty() {
        anyhow::bail!("server name must not be empty");
    }

    let path = resolve_writable_config_path(config_override)?;
    if !path.exists() {
        anyhow::bail!(
            "config file not found: {}. Nothing to remove.",
            path.display()
        );
    }

    let mut doc = load_config_document(&path)?;
    let servers = ensure_mcp_servers_map(&mut doc)?;

    if !servers.contains_key(name) {
        let known: Vec<&str> = servers.keys().map(|s| s.as_str()).collect();
        if known.is_empty() {
            anyhow::bail!("MCP server '{name}' not found (no servers configured)");
        }
        anyhow::bail!(
            "MCP server '{name}' not found. Configured: {}",
            known.join(", ")
        );
    }

    servers.remove(name);
    persist_config_document(&path, &doc)?;

    println!("Removed MCP server '{name}'.");
    println!("Config updated: {}", path.display());
    maybe_trigger_reload().await;
    Ok(())
}

// ── Config document (JSON / TOML) ────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigFormat {
    Json,
    Toml,
}

#[derive(Debug, Clone)]
struct ConfigDocument {
    format: ConfigFormat,
    /// Always held as a JSON-compatible value for uniform mutation.
    root: serde_json::Value,
}

/// Resolve the path we will mutate.
///
/// Order:
/// 1. `--config` override
/// 2. `CLAWFT_CONFIG`
/// 3. cwd `clawft.toml` if present (ticket wording)
/// 4. cwd `weave.toml` if present
/// 5. `~/.clawft/config.json` (created on write)
fn resolve_writable_config_path(config_override: Option<&str>) -> anyhow::Result<PathBuf> {
    if let Some(path) = config_override {
        return Ok(PathBuf::from(path));
    }
    if let Ok(env_path) = std::env::var("CLAWFT_CONFIG")
        && !env_path.is_empty()
    {
        return Ok(PathBuf::from(env_path));
    }

    let clawft_toml = PathBuf::from("clawft.toml");
    if clawft_toml.exists() {
        return Ok(clawft_toml);
    }
    let weave_toml = PathBuf::from("weave.toml");
    if weave_toml.exists() {
        return Ok(weave_toml);
    }

    let home = dirs::home_dir().ok_or_else(|| {
        anyhow::anyhow!(
            "cannot determine home directory. Set $HOME or CLAWFT_CONFIG, or pass --config."
        )
    })?;
    Ok(home.join(".clawft").join("config.json"))
}

fn detect_format(path: &Path) -> ConfigFormat {
    match path.extension().and_then(|e| e.to_str()) {
        Some("toml") => ConfigFormat::Toml,
        _ => ConfigFormat::Json,
    }
}

fn load_config_document(path: &Path) -> anyhow::Result<ConfigDocument> {
    let format = detect_format(path);
    if !path.exists() {
        return Ok(ConfigDocument {
            format,
            root: serde_json::json!({}),
        });
    }

    let contents = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", path.display()))?;
    if contents.trim().is_empty() {
        return Ok(ConfigDocument {
            format,
            root: serde_json::json!({}),
        });
    }

    let root = match format {
        ConfigFormat::Json => serde_json::from_str(&contents)
            .map_err(|e| anyhow::anyhow!("failed to parse {}: {e}", path.display()))?,
        ConfigFormat::Toml => {
            let toml_val: toml::Value = contents
                .parse()
                .map_err(|e| anyhow::anyhow!("failed to parse {}: {e}", path.display()))?;
            // toml → JSON via serde_json intermediate.
            let json_str = serde_json::to_string(&toml_val)
                .map_err(|e| anyhow::anyhow!("failed to convert TOML: {e}"))?;
            serde_json::from_str(&json_str)
                .map_err(|e| anyhow::anyhow!("failed to re-parse TOML as JSON: {e}"))?
        }
    };

    Ok(ConfigDocument { format, root })
}

/// Ensure `tools.mcp_servers` / `tools.mcpServers` map exists; return mutable map.
fn ensure_mcp_servers_map(
    doc: &mut ConfigDocument,
) -> anyhow::Result<&mut serde_json::Map<String, serde_json::Value>> {
    let root = doc
        .root
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("config root is not an object"))?;

    let tools = root
        .entry("tools")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("tools is not an object"))?;

    // Prefer existing key style; default snake_case for TOML, camelCase for JSON.
    let key = if tools.contains_key("mcp_servers") {
        "mcp_servers"
    } else if tools.contains_key("mcpServers") {
        "mcpServers"
    } else {
        match doc.format {
            ConfigFormat::Toml => "mcp_servers",
            ConfigFormat::Json => "mcpServers",
        }
    };

    let servers = tools
        .entry(key.to_string())
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("{key} is not an object"))?;

    Ok(servers)
}

fn extract_mcp_servers(
    root: &serde_json::Value,
) -> HashMap<String, serde_json::Value> {
    let tools = match root.get("tools") {
        Some(t) => t,
        None => return HashMap::new(),
    };
    let servers = tools
        .get("mcp_servers")
        .or_else(|| tools.get("mcpServers"))
        .and_then(|v| v.as_object());

    match servers {
        Some(map) => map.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        None => HashMap::new(),
    }
}

/// Atomic write: temp file in same directory + rename; fall back to direct write.
fn persist_config_document(path: &Path, doc: &ConfigDocument) -> anyhow::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|e| {
            anyhow::anyhow!("failed to create config directory {}: {e}", parent.display())
        })?;
    }

    let output = match doc.format {
        ConfigFormat::Json => {
            let mut s = serde_json::to_string_pretty(&doc.root)?;
            s.push('\n');
            s
        }
        ConfigFormat::Toml => {
            // JSON Value → toml::Value via intermediate string.
            let json_str = serde_json::to_string(&doc.root)?;
            let toml_val: toml::Value = serde_json::from_str(&json_str).map_err(|e| {
                anyhow::anyhow!("failed to convert config to TOML intermediate: {e}")
            })?;
            let mut s = toml::to_string_pretty(&toml_val)
                .map_err(|e| anyhow::anyhow!("failed to serialize TOML: {e}"))?;
            if !s.ends_with('\n') {
                s.push('\n');
            }
            s
        }
    };

    let tmp = match doc.format {
        ConfigFormat::Json => path.with_extension("json.tmp"),
        ConfigFormat::Toml => path.with_extension("toml.tmp"),
    };

    std::fs::write(&tmp, &output)
        .map_err(|e| anyhow::anyhow!("failed to write temp config {}: {e}", tmp.display()))?;

    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        std::fs::write(path, &output).map_err(|e2| {
            anyhow::anyhow!(
                "failed to write {}: rename error {e}; write error {e2}",
                path.display()
            )
        })?;
    }

    Ok(())
}

// ── Daemon reload ────────────────────────────────────────────────────────

async fn try_daemon_rpc(method: &str, params: serde_json::Value) -> Option<serde_json::Value> {
    let mut client = crate::commands::daemon_conn::connect_opt().await?;
    let request = Request::with_params(method, params);
    match client.call(request).await {
        Ok(resp) => match resp.into_result() {
            Ok(val) => Some(val),
            Err(e) => {
                tracing::debug!(error = %e, method, "daemon RPC returned error");
                None
            }
        },
        Err(e) => {
            tracing::debug!(error = %e, method, "daemon RPC call failed");
            None
        }
    }
}

/// Best-effort reload. Prefer `mcp.reload`, then `config.reload`. Always
/// prints a human-readable outcome (success or hint).
async fn maybe_trigger_reload() {
    for method in ["mcp.reload", "config.reload"] {
        if let Some(result) = try_daemon_rpc(method, serde_json::json!({})).await {
            if let Some(msg) = result.get("message").and_then(|v| v.as_str()) {
                println!("Daemon reload: {msg}");
            } else {
                println!("Daemon reload triggered via {method}.");
            }
            return;
        }
    }
    println!("{RELOAD_HINT}");
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_json(dir: &TempDir, name: &str, body: &str) -> PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn parse_env_flags_ok() {
        let env = parse_env_flags(&["FOO=bar".into(), "BAZ=qux=more".into()]).unwrap();
        assert_eq!(env.get("FOO").map(String::as_str), Some("bar"));
        assert_eq!(env.get("BAZ").map(String::as_str), Some("qux=more"));
    }

    #[test]
    fn parse_env_flags_rejects_missing_eq() {
        let err = parse_env_flags(&["NOEQUALS".into()]).unwrap_err();
        assert!(err.to_string().contains("KEY=VALUE"));
    }

    #[test]
    fn resolve_command_from_flags() {
        let (cmd, args) =
            resolve_command(Some("npx"), &["-y".into(), "pkg".into()], &[]).unwrap();
        assert_eq!(cmd, "npx");
        assert_eq!(args, vec!["-y", "pkg"]);
    }

    #[test]
    fn resolve_command_from_trailing() {
        let rest = vec!["npx".into(), "-y".into(), "@example/mcp".into()];
        let (cmd, args) = resolve_command(None, &[], &rest).unwrap();
        assert_eq!(cmd, "npx");
        assert_eq!(args, vec!["-y", "@example/mcp"]);
    }

    #[test]
    fn detect_format_by_extension() {
        assert_eq!(detect_format(Path::new("config.json")), ConfigFormat::Json);
        assert_eq!(detect_format(Path::new("clawft.toml")), ConfigFormat::Toml);
        assert_eq!(detect_format(Path::new("weave.toml")), ConfigFormat::Toml);
    }

    #[test]
    fn add_and_list_roundtrip_json() {
        let dir = TempDir::new().unwrap();
        let path = write_json(&dir, "config.json", "{}\n");

        let mut doc = load_config_document(&path).unwrap();
        let servers = ensure_mcp_servers_map(&mut doc).unwrap();
        servers.insert(
            "demo".into(),
            build_server_entry(
                "npx",
                &["-y".into(), "demo".into()],
                "",
                &HashMap::new(),
                true,
                ConfigFormat::Json,
            ),
        );
        persist_config_document(&path, &doc).unwrap();

        let reloaded = load_config_document(&path).unwrap();
        let extracted = extract_mcp_servers(&reloaded.root);
        assert!(extracted.contains_key("demo"));
        let demo = &extracted["demo"];
        assert_eq!(demo["command"], "npx");
        assert_eq!(demo["args"][0], "-y");
        assert_eq!(demo["internalOnly"], true);

        // Atomic write should not leave the .tmp behind.
        assert!(!path.with_extension("json.tmp").exists());
    }

    #[test]
    fn add_and_remove_toml() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("clawft.toml");
        std::fs::write(&path, "").unwrap();

        let mut doc = load_config_document(&path).unwrap();
        assert_eq!(doc.format, ConfigFormat::Toml);
        {
            let servers = ensure_mcp_servers_map(&mut doc).unwrap();
            servers.insert(
                "http-srv".into(),
                build_server_entry(
                    "",
                    &[],
                    "https://example.com/mcp",
                    &HashMap::new(),
                    false,
                    ConfigFormat::Toml,
                ),
            );
        }
        persist_config_document(&path, &doc).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("mcp_servers") || text.contains("http-srv"));
        assert!(text.contains("https://example.com/mcp"));

        let mut doc2 = load_config_document(&path).unwrap();
        {
            let servers = ensure_mcp_servers_map(&mut doc2).unwrap();
            assert!(servers.remove("http-srv").is_some());
        }
        persist_config_document(&path, &doc2).unwrap();

        let doc3 = load_config_document(&path).unwrap();
        let servers = extract_mcp_servers(&doc3.root);
        assert!(!servers.contains_key("http-srv"));
    }

    #[test]
    fn preserve_mcp_servers_key_style() {
        let dir = TempDir::new().unwrap();
        let path = write_json(
            &dir,
            "config.json",
            r#"{ "tools": { "mcp_servers": { "a": { "command": "true" } } } }"#,
        );
        let mut doc = load_config_document(&path).unwrap();
        {
            let servers = ensure_mcp_servers_map(&mut doc).unwrap();
            servers.insert(
                "b".into(),
                build_server_entry("echo", &[], "", &HashMap::new(), true, ConfigFormat::Json),
            );
        }
        persist_config_document(&path, &doc).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("mcp_servers"));
        assert!(!text.contains("mcpServers"));
    }

    #[test]
    fn build_entry_http_only() {
        let v = build_server_entry(
            "",
            &[],
            "https://x",
            &HashMap::new(),
            false,
            ConfigFormat::Json,
        );
        assert_eq!(v["url"], "https://x");
        assert!(v.get("command").is_none());
        assert_eq!(v["internalOnly"], false);
    }

    #[tokio::test]
    async fn mcp_add_requires_transport() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cfg.json");
        let err = mcp_add(
            "x",
            None,
            &[],
            None,
            &[],
            true,
            Some(path.to_str().unwrap()),
            &[],
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("--command") || err.to_string().contains("--url"));
    }

    #[tokio::test]
    async fn mcp_add_remove_e2e_json() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cfg.json");
        let path_str = path.to_str().unwrap();

        mcp_add(
            "srv",
            Some("npx"),
            &["-y".into(), "foo".into()],
            None,
            &["TOKEN=abc".into()],
            false,
            Some(path_str),
            &[],
        )
        .await
        .unwrap();

        let doc = load_config_document(&path).unwrap();
        let servers = extract_mcp_servers(&doc.root);
        assert!(servers.contains_key("srv"));
        assert_eq!(servers["srv"]["command"], "npx");
        assert_eq!(servers["srv"]["env"]["TOKEN"], "abc");
        assert_eq!(servers["srv"]["internalOnly"], false);

        mcp_remove("srv", Some(path_str)).await.unwrap();
        let doc2 = load_config_document(&path).unwrap();
        assert!(!extract_mcp_servers(&doc2.root).contains_key("srv"));
    }

    #[tokio::test]
    async fn mcp_remove_missing_errors() {
        let dir = TempDir::new().unwrap();
        let path = write_json(&dir, "cfg.json", r#"{"tools":{"mcpServers":{}}}"#);
        let err = mcp_remove("nope", Some(path.to_str().unwrap()))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn mcp_add_trailing_rest() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cfg.json");
        let path_str = path.to_str().unwrap();
        mcp_add(
            "trail",
            None,
            &[],
            None,
            &[],
            true,
            Some(path_str),
            &["uvx".into(), "my-mcp".into()],
        )
        .await
        .unwrap();
        let doc = load_config_document(&path).unwrap();
        let servers = extract_mcp_servers(&doc.root);
        assert_eq!(servers["trail"]["command"], "uvx");
        assert_eq!(servers["trail"]["args"][0], "my-mcp");
    }
}
