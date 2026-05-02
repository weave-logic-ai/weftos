//! `weft agents` -- CLI commands for agent discovery and management.
//!
//! Provides subcommands:
//!
//! - `weft agents list` -- list all agents (workspace, user, builtin) with
//!   source annotation.
//! - `weft agents show <name>` -- show agent details (description, model,
//!   skills, system prompt preview).
//! - `weft agents use <name>` -- set the active agent for the next
//!   `weft agent` session.
//!
//! Per ADR-021, commands attempt RPC to the kernel daemon first and fall
//! back to local file I/O when the daemon is unavailable.

use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use comfy_table::{Table, presets};

use clawft_core::agent::agents::{AgentDefinition, AgentRegistry};
use clawft_rpc::{DaemonClient, Request};

/// Arguments for the `weft agents` subcommand.
#[derive(Args)]
pub struct AgentsArgs {
    #[command(subcommand)]
    pub action: AgentsAction,
}

/// Subcommands for `weft agents`.
#[derive(Subcommand)]
pub enum AgentsAction {
    /// List all agents with source annotation.
    List,

    /// Show details of a specific agent.
    Show {
        /// Agent name to inspect.
        name: String,
    },

    /// Set the active agent for the next session.
    Use {
        /// Agent name to activate.
        name: String,
    },
}

/// Warning printed when no daemon is available.
const NO_DAEMON_WARNING: &str =
    "Warning: running without kernel daemon. Start daemon with: weaver kernel start";

/// Run the agents subcommand.
pub async fn run(args: AgentsArgs) -> anyhow::Result<()> {
    match args.action {
        AgentsAction::List => agents_list_rpc().await,
        AgentsAction::Show { ref name } => agents_show_rpc(name).await,
        AgentsAction::Use { ref name } => agents_use_rpc(name).await,
    }
}

/// Try `agents.list` via RPC, fall back to local registry.
async fn agents_list_rpc() -> anyhow::Result<()> {
    if let Some(mut client) = DaemonClient::connect().await {
        let resp = client.simple_call("agents.list").await?;
        let data = resp.into_result()?;
        // Daemon returns a JSON array of agent objects; render as a table.
        if let Some(agents) = data.as_array() {
            if agents.is_empty() {
                println!("No agents found.");
                return Ok(());
            }
            let mut table = Table::new();
            table.load_preset(presets::UTF8_FULL_CONDENSED);
            table.set_header(["NAME", "SOURCE", "MODEL", "DESCRIPTION"]);
            for a in agents {
                let name = a["name"].as_str().unwrap_or("?");
                let source = a["source"].as_str().unwrap_or("builtin");
                let model = a["model"].as_str().unwrap_or("(default)");
                let desc = truncate(a["description"].as_str().unwrap_or(""), 50);
                table.add_row([name, source, model, &desc]);
            }
            println!("{table}");
            println!();
            println!("Total: {} agent(s)", agents.len());
        } else {
            println!("{}", serde_json::to_string_pretty(&data)?);
        }
        return Ok(());
    }

    eprintln!("{NO_DAEMON_WARNING}");
    agents_list_local()
}

/// Try `agents.show` via RPC, fall back to local registry.
async fn agents_show_rpc(name: &str) -> anyhow::Result<()> {
    if let Some(mut client) = DaemonClient::connect().await {
        let req = Request::with_params("agents.show", serde_json::json!({ "name": name }));
        let resp = client.call(req).await?;
        let data = resp.into_result()?;
        print_agent_detail_json(&data);
        return Ok(());
    }

    eprintln!("{NO_DAEMON_WARNING}");
    agents_show_local(name)
}

/// Try `agents.use` via RPC, fall back to local registry.
async fn agents_use_rpc(name: &str) -> anyhow::Result<()> {
    if let Some(mut client) = DaemonClient::connect().await {
        let req = Request::with_params("agents.use", serde_json::json!({ "name": name }));
        let resp = client.call(req).await?;
        let data = resp.into_result()?;
        // The daemon acknowledges the selection; print its reply.
        if let Some(msg) = data.as_str() {
            println!("{msg}");
        } else {
            println!("Agent '{name}' selected via daemon.");
            if let Some(obj) = data.as_object()
                && let Some(model) = obj.get("model").and_then(|v| v.as_str()) {
                    println!("Model: {model}");
                }
        }
        return Ok(());
    }

    eprintln!("{NO_DAEMON_WARNING}");
    agents_use_local(name)
}

/// Print agent detail from a JSON value returned by the daemon.
fn print_agent_detail_json(data: &serde_json::Value) {
    if let Some(name) = data["name"].as_str() {
        println!("Agent: {name}");
    }
    if let Some(desc) = data["description"].as_str() {
        println!("Description: {desc}");
    }
    if let Some(model) = data["model"].as_str() {
        println!("Model: {model}");
    } else {
        println!("Model: (default)");
    }
    if let Some(source) = data["source_path"].as_str() {
        println!("Source: {source}");
    }
    if let Some(turns) = data["max_turns"].as_u64() {
        println!("Max turns: {turns}");
    }
    if let Some(skills) = data["skills"].as_array() {
        let names: Vec<&str> = skills.iter().filter_map(|v| v.as_str()).collect();
        if !names.is_empty() {
            println!("Skills: {}", names.join(", "));
        }
    }
    if let Some(tools) = data["allowed_tools"].as_array() {
        let names: Vec<&str> = tools.iter().filter_map(|v| v.as_str()).collect();
        if !names.is_empty() {
            println!("Allowed tools: {}", names.join(", "));
        }
    }
    if let Some(vars) = data["variables"].as_object()
        && !vars.is_empty() {
            println!("Variables:");
            let mut items: Vec<_> = vars.iter().collect();
            items.sort_by_key(|(k, _)| k.as_str());
            for (k, v) in items {
                let val = v.as_str().map(|s| s.to_string()).unwrap_or_else(|| v.to_string());
                println!("  {k}: {val}");
            }
        }
    if let Some(prompt) = data["system_prompt"].as_str() {
        println!();
        println!("System prompt (preview):");
        println!("---");
        let preview = truncate(prompt, 500);
        println!("{preview}");
        if prompt.len() > 500 {
            println!("... ({} chars total)", prompt.len());
        }
        println!("---");
    }
}

// ── Local fallback helpers (unchanged logic, renamed) ──────────

/// Local fallback for `agents list`.
fn agents_list_local() -> anyhow::Result<()> {
    let (ws_dir, user_dir) = discover_agent_dirs();
    let registry = AgentRegistry::discover(ws_dir.as_deref(), user_dir.as_deref(), Vec::new())
        .map_err(|e| anyhow::anyhow!("failed to discover agents: {e}"))?;
    agents_list(&registry, ws_dir.as_deref(), user_dir.as_deref())
}

/// Local fallback for `agents show`.
fn agents_show_local(name: &str) -> anyhow::Result<()> {
    let (ws_dir, user_dir) = discover_agent_dirs();
    let registry = AgentRegistry::discover(ws_dir.as_deref(), user_dir.as_deref(), Vec::new())
        .map_err(|e| anyhow::anyhow!("failed to discover agents: {e}"))?;
    agents_show(&registry, name)
}

/// Local fallback for `agents use`.
fn agents_use_local(name: &str) -> anyhow::Result<()> {
    let (ws_dir, user_dir) = discover_agent_dirs();
    let registry = AgentRegistry::discover(ws_dir.as_deref(), user_dir.as_deref(), Vec::new())
        .map_err(|e| anyhow::anyhow!("failed to discover agents: {e}"))?;
    agents_use(&registry, name)
}

/// Discover workspace and user agent directories.
fn discover_agent_dirs() -> (Option<PathBuf>, Option<PathBuf>) {
    let user_dir = dirs::home_dir().map(|h| h.join(".clawft").join("agents"));

    // Walk upward from cwd to find .clawft/agents/
    let ws_dir = std::env::current_dir().ok().and_then(|cwd| {
        let mut dir: &Path = cwd.as_path();
        loop {
            let candidate = dir.join(".clawft").join("agents");
            if candidate.is_dir() {
                return Some(candidate);
            }
            match dir.parent() {
                Some(parent) => dir = parent,
                None => return None,
            }
        }
    });

    (ws_dir, user_dir)
}

/// List all agents in a table with source annotation.
fn agents_list(
    registry: &AgentRegistry,
    ws_dir: Option<&Path>,
    user_dir: Option<&Path>,
) -> anyhow::Result<()> {
    let agents = registry.list();

    if agents.is_empty() {
        println!("No agents found.");
        println!();
        if let Some(dir) = user_dir {
            println!("User agents directory: {}", dir.display());
        }
        if let Some(dir) = ws_dir {
            println!("Workspace agents directory: {}", dir.display());
        }
        return Ok(());
    }

    let mut table = Table::new();
    table.load_preset(presets::UTF8_FULL_CONDENSED);
    table.set_header(["NAME", "SOURCE", "MODEL", "DESCRIPTION"]);

    for agent in &agents {
        let source = classify_source(agent, ws_dir, user_dir);
        let model = agent.model.as_deref().unwrap_or("(default)");
        let desc = truncate(&agent.description, 50);
        table.add_row([&agent.name, source, model, &desc]);
    }

    println!("{table}");
    println!();
    println!("Total: {} agent(s)", registry.len());

    Ok(())
}

/// Show details of a specific agent.
fn agents_show(registry: &AgentRegistry, name: &str) -> anyhow::Result<()> {
    let agent = registry.get(name).ok_or_else(|| {
        anyhow::anyhow!("agent not found: {name}\nUse 'weft agents list' to see available agents.")
    })?;

    println!("Agent: {}", agent.name);
    println!("Description: {}", agent.description);

    if let Some(ref model) = agent.model {
        println!("Model: {model}");
    } else {
        println!("Model: (default)");
    }

    if let Some(ref path) = agent.source_path {
        println!("Source: {}", path.display());
    }

    if let Some(max_turns) = agent.max_turns {
        println!("Max turns: {max_turns}");
    }

    if !agent.skills.is_empty() {
        println!("Skills: {}", agent.skills.join(", "));
    }

    if !agent.allowed_tools.is_empty() {
        println!("Allowed tools: {}", agent.allowed_tools.join(", "));
    }

    if !agent.variables.is_empty() {
        println!("Variables:");
        let mut vars: Vec<_> = agent.variables.iter().collect();
        vars.sort_by_key(|(k, _)| *k);
        for (key, value) in vars {
            println!("  {key}: {value}");
        }
    }

    if let Some(ref prompt) = agent.system_prompt {
        println!();
        println!("System prompt (preview):");
        println!("---");
        let preview = truncate(prompt, 500);
        println!("{preview}");
        if prompt.len() > 500 {
            println!("... ({} chars total)", prompt.len());
        }
        println!("---");
    }

    Ok(())
}

/// Set the active agent for the next interactive session.
///
/// Validates that the agent exists but does not persist the selection
/// beyond the current invocation. The user should pass `--agent <name>`
/// to `weft agent` or use `/agent <name>` in the REPL.
fn agents_use(registry: &AgentRegistry, name: &str) -> anyhow::Result<()> {
    let agent = registry.get(name).ok_or_else(|| {
        anyhow::anyhow!("agent not found: {name}\nUse 'weft agents list' to see available agents.")
    })?;

    println!("Selected agent: {}", agent.name);
    println!("Description: {}", agent.description);
    if let Some(ref model) = agent.model {
        println!("Model: {model}");
    }
    println!();
    println!("To use this agent, run:");
    println!(
        "  weft agent --model {} -m \"your prompt\"",
        agent.model.as_deref().unwrap_or("(default)")
    );
    println!();
    println!("Or in interactive mode, use: /agent {}", agent.name);

    Ok(())
}

/// Classify the source of an agent for display.
fn classify_source(
    agent: &AgentDefinition,
    ws_dir: Option<&Path>,
    user_dir: Option<&Path>,
) -> &'static str {
    if let Some(ref path) = agent.source_path {
        if let Some(ws) = ws_dir
            && path.starts_with(ws)
        {
            return "workspace";
        }
        if let Some(ud) = user_dir
            && path.starts_with(ud)
        {
            return "user";
        }
    }
    "builtin"
}

/// Truncate a string to `max_len` characters, appending "..." if truncated.
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len.saturating_sub(3)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_dir(prefix: &str) -> PathBuf {
        let id = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        std::env::temp_dir().join(format!("clawft_agents_cmd_{prefix}_{pid}_{id}"))
    }

    fn write_agent(dir: &Path, name: &str, description: &str) {
        let agent_dir = dir.join(name);
        std::fs::create_dir_all(&agent_dir).unwrap();
        let yaml = format!(
            "name: {name}\n\
             description: {description}\n\
             model: test-model/v1\n\
             system_prompt: \"You are a {name}.\"\n\
             skills:\n  - research\n"
        );
        std::fs::write(agent_dir.join("agent.yaml"), yaml).unwrap();
    }

    fn builtin_agent(name: &str, desc: &str) -> AgentDefinition {
        AgentDefinition {
            name: name.into(),
            description: desc.into(),
            model: None,
            system_prompt: None,
            skills: vec![],
            allowed_tools: vec![],
            max_turns: None,
            variables: HashMap::new(),
            source_path: None,
        }
    }

    #[test]
    fn truncate_short() {
        assert_eq!(truncate("hi", 10), "hi");
    }

    #[test]
    fn truncate_long() {
        let result = truncate("a very long description that exceeds the limit", 20);
        assert!(result.ends_with("..."));
        assert!(result.len() <= 20);
    }

    #[test]
    fn classify_source_workspace() {
        let ws = PathBuf::from("/project/.clawft/agents");
        let agent = AgentDefinition {
            name: "test".into(),
            description: "test".into(),
            source_path: Some(PathBuf::from("/project/.clawft/agents/test")),
            model: None,
            system_prompt: None,
            skills: vec![],
            allowed_tools: vec![],
            max_turns: None,
            variables: HashMap::new(),
        };
        assert_eq!(classify_source(&agent, Some(&ws), None), "workspace");
    }

    #[test]
    fn classify_source_user() {
        let user = PathBuf::from("/home/user/.clawft/agents");
        let agent = AgentDefinition {
            name: "test".into(),
            description: "test".into(),
            source_path: Some(PathBuf::from("/home/user/.clawft/agents/test")),
            model: None,
            system_prompt: None,
            skills: vec![],
            allowed_tools: vec![],
            max_turns: None,
            variables: HashMap::new(),
        };
        assert_eq!(classify_source(&agent, None, Some(&user)), "user");
    }

    #[test]
    fn classify_source_builtin() {
        let agent = builtin_agent("test", "test");
        assert_eq!(classify_source(&agent, None, None), "builtin");
    }

    #[test]
    fn agents_list_with_registry() {
        let dir = temp_dir("list");
        write_agent(&dir, "alpha", "Alpha agent");
        write_agent(&dir, "beta", "Beta agent");

        let registry = AgentRegistry::discover(Some(&dir), None, Vec::new()).unwrap();
        assert_eq!(registry.len(), 2);
        assert!(registry.get("alpha").is_some());
        assert!(registry.get("beta").is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn agents_show_found() {
        let dir = temp_dir("show");
        write_agent(&dir, "test_agent", "A test agent");

        let registry = AgentRegistry::discover(Some(&dir), None, Vec::new()).unwrap();
        let result = agents_show(&registry, "test_agent");
        assert!(result.is_ok());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn agents_show_not_found() {
        let registry = AgentRegistry::discover(None, None, Vec::new()).unwrap();
        let result = agents_show(&registry, "nonexistent");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("not found"));
    }

    #[test]
    fn agents_use_found() {
        let dir = temp_dir("use");
        write_agent(&dir, "my_agent", "My agent");

        let registry = AgentRegistry::discover(Some(&dir), None, Vec::new()).unwrap();
        let result = agents_use(&registry, "my_agent");
        assert!(result.is_ok());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn agents_use_not_found() {
        let registry = AgentRegistry::discover(None, None, Vec::new()).unwrap();
        let result = agents_use(&registry, "nonexistent");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("not found"));
    }

    #[test]
    fn agents_list_empty_registry() {
        let registry = AgentRegistry::discover(None, None, Vec::new()).unwrap();
        let result = agents_list(&registry, None, None);
        assert!(result.is_ok());
    }

    #[test]
    fn agents_show_all_fields() {
        let dir = temp_dir("show_fields");
        let agent_dir = dir.join("full_agent");
        std::fs::create_dir_all(&agent_dir).unwrap();
        let yaml = r#"name: full_agent
description: Full agent with all fields
model: custom/model-v2
system_prompt: "You are a full agent with extensive capabilities."
skills:
  - research
  - coding
allowed_tools:
  - read_file
  - write_file
max_turns: 15
variables:
  lang: rust
  framework: axum
"#;
        std::fs::write(agent_dir.join("agent.yaml"), yaml).unwrap();

        let registry = AgentRegistry::discover(Some(&dir), None, Vec::new()).unwrap();
        let result = agents_show(&registry, "full_agent");
        assert!(result.is_ok());

        let agent = registry.get("full_agent").unwrap();
        assert_eq!(agent.model.as_deref(), Some("custom/model-v2"));
        assert_eq!(agent.skills, vec!["research", "coding"]);
        assert_eq!(agent.allowed_tools, vec!["read_file", "write_file"]);
        assert_eq!(agent.max_turns, Some(15));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
