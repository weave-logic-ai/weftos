//! `weft agent` -- interactive agent session or single-message mode.
//!
//! In single-message mode (`--message "..."`), sends one prompt to the agent
//! and prints the response. In interactive mode (no `--message`), reads from
//! stdin in a REPL loop.
//!
//! Messages are processed through the full 6-stage pipeline via [`AgentLoop`]:
//! Classifier -> Router -> Assembler -> Transport -> Scorer -> Learner.
//! Tool calls are executed automatically up to `max_tool_iterations`.
//!
//! # Examples
//!
//! ```text
//! # Single message
//! weft agent -m "What is Rust?"
//!
//! # Interactive mode
//! weft agent
//! > What is Rust?
//! [agent response]
//! > /exit
//!
//! # Override model
//! weft agent --model openai/gpt-4o -m "hello"
//! ```

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::Utc;
use clap::Args;
use tokio::io::AsyncBufReadExt;
use tracing::info;

use clawft_core::agent::loop_core::AutoDelegation;
use clawft_core::agent::skills_v2::SkillRegistry;
use clawft_core::bootstrap::AppContext;
use clawft_core::bus::MessageBus;
use clawft_platform::NativePlatform;
use clawft_types::event::InboundMessage;

use super::load_config;
use crate::interactive::builtins::{QUIT_SENTINEL, register_builtins, register_skill_commands};
use crate::interactive::registry::{InteractiveContext, SlashCommandRegistry};

/// Arguments for the `weft agent` subcommand.
#[derive(Args)]
pub struct AgentArgs {
    /// Send a single message and exit (non-interactive mode).
    #[arg(short, long)]
    pub message: Option<String>,

    /// Model to use (overrides config).
    #[arg(long)]
    pub model: Option<String>,

    /// Config file path (overrides auto-discovery).
    #[arg(short, long)]
    pub config: Option<String>,

    /// Enable intelligent routing (requires vector-memory feature).
    #[arg(long)]
    pub intelligent_routing: bool,

    /// Trust workspace-level (project) skills.
    ///
    /// Without this flag, only user and built-in skills are loaded.
    /// Workspace skills in `.clawft/skills/` are skipped as a security
    /// measure (SEC-SKILL-05).
    #[arg(long)]
    pub trust_project_skills: bool,
}

/// Run the agent command.
///
/// Loads configuration, bootstraps [`AppContext`], registers tools,
/// and enters either single-message or interactive mode. The agent
/// loop processes messages through the full pipeline, including
/// tool execution.
pub async fn run(args: AgentArgs) -> anyhow::Result<()> {
    let platform = Arc::new(NativePlatform::new());
    let mut config = load_config(&*platform, args.config.as_deref()).await?;

    // Apply model override if provided.
    if let Some(ref model) = args.model {
        config.agents.defaults.model = model.clone();
    }

    let effective_model = &config.agents.defaults.model;
    info!(model = %effective_model, "initializing agent");

    // Bootstrap the application context (bus, sessions, memory, skills, pipeline).
    let mut ctx = AppContext::new(config.clone(), platform.clone())
        .await
        .map_err(|e| anyhow::anyhow!("bootstrap failed: {e}"))?;

    // Register core tools (built-in + MCP proxied + delegation).
    super::register_core_tools(ctx.tools_mut(), &config, platform.clone()).await;

    // Register message tool (needs bus reference, cannot go in register_all).
    let bus_ref = ctx.bus().clone();
    ctx.tools_mut()
        .register(Arc::new(clawft_tools::message_tool::MessageTool::new(
            bus_ref,
        )));

    let tool_count = ctx.tools().len();
    let tool_names: Vec<String> = ctx.tools().list();
    info!(tools = tool_count, "tool registry initialized");

    // Wire the live LLM-backed pipeline so real provider calls work.
    ctx.enable_live_llm();

    // Wire auto-delegation: when delegation is enabled and the delegate_task
    // tool is registered, install a pre-LLM router that checks delegation
    // rules against each message before sending to the local LLM.
    #[cfg(feature = "delegate")]
    {
        if config.delegation.claude_enabled && ctx.tools().has("delegate_task") {
            let auto_del = build_auto_delegation(&config.delegation);
            ctx.set_auto_delegation(auto_del);
            info!("auto-delegation enabled (pre-LLM routing active)");
        }
    }

    // Intelligent routing (vector-memory feature gate).
    if args.intelligent_routing {
        #[cfg(feature = "vector-memory")]
        {
            info!("intelligent routing enabled");
            // IntelligentRouter wiring would go here when fully implemented.
            // For now, log that it's enabled.
        }
        #[cfg(not(feature = "vector-memory"))]
        {
            anyhow::bail!(
                "intelligent routing requires the 'vector-memory' feature. \
                 Rebuild with: cargo build --features vector-memory"
            );
        }
    }

    // Discover v2 skills from workspace (.clawft/skills/) and user (~/.clawft/skills/).
    let (ws_skill_dir, user_skill_dir) = discover_skill_dirs();
    let trust_ws = args.trust_project_skills;
    let skill_registry = match SkillRegistry::discover_with_trust(
        ws_skill_dir.as_deref(),
        user_skill_dir.as_deref(),
        Vec::new(),
        trust_ws,
    )
    .await
    {
        Ok(reg) => {
            info!(skills = reg.len(), "v2 skill discovery complete");
            reg
        }
        Err(e) => {
            tracing::warn!("v2 skill discovery failed: {e}");
            SkillRegistry::discover(None, None, Vec::new())
                .await
                .expect("empty registry should never fail")
        }
    };

    // Clone the bus before consuming the context.
    let bus = ctx.bus().clone();

    // Convert context into the agent loop (consumes ctx).
    let agent = ctx.into_agent_loop();

    if let Some(ref message) = args.message {
        return run_single_message(message, &bus, agent, effective_model).await;
    }

    run_interactive(&bus, agent, &tool_names, effective_model, &skill_registry).await
}

/// Process a single message through the agent loop and exit.
///
/// Publishes the message to the bus, spawns the agent loop in the
/// background, waits for the outbound response, and prints it.
async fn run_single_message(
    message: &str,
    bus: &Arc<MessageBus>,
    agent: clawft_core::agent::loop_core::AgentLoop<NativePlatform>,
    model: &str,
) -> anyhow::Result<()> {
    info!(model = %model, "single-message mode");

    // Create and publish the inbound message.
    let inbound = InboundMessage {
        channel: "cli".into(),
        sender_id: "local".into(),
        chat_id: "cli-session".into(),
        content: message.to_owned(),
        timestamp: Utc::now(),
        media: vec![],
        metadata: HashMap::new(),
    };
    bus.publish_inbound(inbound)
        .map_err(|e| anyhow::anyhow!("failed to publish message: {e}"))?;

    // Spawn the agent loop in the background.
    let agent_handle = tokio::spawn(async move {
        if let Err(e) = agent.run().await {
            tracing::error!("agent loop error: {e}");
        }
    });

    // Wait for the outbound response.
    let response = bus.consume_outbound().await;

    match response {
        Some(msg) => {
            println!("{}", msg.content);
        }
        None => {
            eprintln!("error: no response from agent");
        }
    }

    // Signal the agent loop to stop by dropping the inbound sender.
    // The bus holds its own sender, so we close it by dropping the bus.
    // Since the bus is shared via Arc, we just drop the handle and abort.
    agent_handle.abort();
    let _ = agent_handle.await;

    Ok(())
}

/// Run an interactive REPL loop reading from stdin.
///
/// Spawns the agent loop in the background, then reads user input
/// line-by-line. Slash commands (including v2 skill activations) are
/// dispatched locally via [`SlashCommandRegistry`]. All other input is
/// published to the bus for the agent loop to process.
///
/// When a skill is active, its `instructions` and `allowed_tools` are
/// injected into the message metadata so the agent loop can use them.
async fn run_interactive(
    bus: &Arc<MessageBus>,
    agent: clawft_core::agent::loop_core::AgentLoop<NativePlatform>,
    tool_names: &[String],
    model: &str,
    skill_registry: &SkillRegistry,
) -> anyhow::Result<()> {
    println!("weft agent -- interactive mode (type /help for commands)");
    println!("Model: {model}");

    // Set up slash command registry with builtins.
    let mut cmd_registry = SlashCommandRegistry::new();
    register_builtins(&mut cmd_registry);

    // Register user-invocable v2 skills as slash commands.
    let skill_entries: Vec<(String, String)> = skill_registry
        .list()
        .iter()
        .filter(|s| s.user_invocable)
        .map(|s| (s.name.clone(), s.description.clone()))
        .collect();
    let skill_count = register_skill_commands(&mut cmd_registry, &skill_entries);
    if skill_count > 0 {
        println!("Skills: {skill_count} registered");
    }
    println!();

    // Initialize interactive context.
    let mut ctx = InteractiveContext::new(model.to_string());
    ctx.tool_names = tool_names.to_vec();
    ctx.skill_names = skill_registry
        .names()
        .iter()
        .map(|s| s.to_string())
        .collect();

    // Spawn the agent loop in the background.
    let agent_handle = tokio::spawn(async move {
        if let Err(e) = agent.run().await {
            tracing::error!("agent loop error: {e}");
        }
    });

    let stdin = tokio::io::stdin();
    let mut reader = tokio::io::BufReader::new(stdin).lines();

    loop {
        eprint!("> ");
        // Flush stderr so the prompt appears before blocking on read.
        use std::io::Write;
        std::io::stderr().flush().ok();

        let line = match reader.next_line().await? {
            Some(l) => l,
            None => break, // EOF
        };
        let input = line.trim();

        if input.is_empty() {
            continue;
        }

        // Dispatch through slash command registry (handles /help, /skills,
        // /use, /tools, /status, /quit, and skill-contributed commands).
        if input.starts_with('/')
            && let Some(result) = cmd_registry.dispatch(input, &mut ctx)
        {
            match result {
                Ok(output) => {
                    if output == QUIT_SENTINEL {
                        break;
                    }
                    println!("{output}");
                    println!();
                }
                Err(e) => {
                    eprintln!("error: {e}");
                }
            }
            continue;
            // Unknown slash command -- fall through to send to agent.
        }

        // Build metadata with active skill info for the agent loop.
        let mut metadata = HashMap::new();
        if !ctx.active_skill.is_empty()
            && let Some(skill) = skill_registry.get(&ctx.active_skill)
        {
            if !skill.instructions.is_empty() {
                metadata.insert(
                    "skill_instructions".into(),
                    serde_json::json!(skill.instructions),
                );
            }
            if !skill.allowed_tools.is_empty() {
                metadata.insert(
                    "allowed_tools".into(),
                    serde_json::json!(skill.allowed_tools),
                );
            }
        }

        // Publish the user message to the bus.
        let inbound = InboundMessage {
            channel: "cli".into(),
            sender_id: "local".into(),
            chat_id: "cli-session".into(),
            content: input.to_owned(),
            timestamp: Utc::now(),
            media: vec![],
            metadata,
        };

        if let Err(e) = bus.publish_inbound(inbound) {
            eprintln!("error: failed to send message: {e}");
            break;
        }

        // Wait for the outbound response.
        match bus.consume_outbound().await {
            Some(msg) => {
                println!("{}", msg.content);
                println!();
            }
            None => {
                eprintln!("error: agent loop closed unexpectedly");
                break;
            }
        }
    }

    // Signal the agent loop to stop.
    agent_handle.abort();
    let _ = agent_handle.await;

    println!("Goodbye.");
    Ok(())
}

/// Build a [`WebSearchConfig`] from the tools configuration.
///
/// Maps the `ToolsConfig.web.search` fields (api_key, max_results) into the
/// web search tool's configuration struct. Resolves the API key from the
/// environment variable `BRAVE_SEARCH_API_KEY` if the config value is empty.
pub(crate) fn build_web_search_config(
    config: &clawft_types::config::ToolsConfig,
) -> clawft_tools::web_search::WebSearchConfig {
    let search = &config.web.search;
    let api_key = if search.api_key.is_empty() {
        std::env::var("BRAVE_SEARCH_API_KEY").ok()
    } else {
        Some(search.api_key.expose().to_owned())
    };

    clawft_tools::web_search::WebSearchConfig {
        api_key,
        endpoint: None, // Custom endpoint support can be added to config later.
        max_results: search.max_results,
    }
}

/// Build a [`CommandPolicy`] from the configuration.
pub(crate) fn build_command_policy(
    config: &clawft_types::config::CommandPolicyConfig,
) -> clawft_tools::security_policy::CommandPolicy {
    use clawft_tools::security_policy::{CommandPolicy, PolicyMode};

    let mut policy = CommandPolicy::safe_defaults();

    if config.mode == "denylist" {
        policy.mode = PolicyMode::Denylist;
    }
    if !config.allowlist.is_empty() {
        policy.allowlist = config.allowlist.iter().cloned().collect();
    }
    if !config.denylist.is_empty() {
        policy.denylist = config.denylist.clone();
    }

    policy
}

/// Build a [`UrlPolicy`] from the configuration.
pub(crate) fn build_url_policy(
    config: &clawft_types::config::UrlPolicyConfig,
) -> clawft_tools::url_safety::UrlPolicy {
    use clawft_tools::url_safety::UrlPolicy;

    UrlPolicy::new(
        config.enabled,
        config.allow_private,
        config.allowed_domains.iter().cloned().collect(),
        config.blocked_domains.iter().cloned().collect(),
    )
}

/// Discover workspace and user skill directories for v2 skill loading.
///
/// Walks upward from `cwd` to find `.clawft/skills/` (workspace) and
/// checks `~/.clawft/skills/` (user). Returns `(workspace_dir, user_dir)`.
fn discover_skill_dirs() -> (Option<PathBuf>, Option<PathBuf>) {
    let user_dir = dirs::home_dir().map(|h| h.join(".clawft").join("skills"));

    // Walk upward from cwd to find .clawft/skills/
    let ws_dir = std::env::current_dir().ok().and_then(|cwd| {
        let mut dir: &Path = cwd.as_path();
        loop {
            let candidate = dir.join(".clawft").join("skills");
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

// ---------------------------------------------------------------------------
// Auto-delegation router implementation
// ---------------------------------------------------------------------------

/// Pre-LLM auto-delegation router backed by compiled regex rules.
///
/// Wraps the [`DelegationEngine`] from `clawft-services` to check inbound
/// messages against delegation rules before the local LLM sees them.
/// When a rule matches a non-Local target, the message is routed to
/// `delegate_task` directly.
#[cfg(feature = "delegate")]
struct AutoDelegationRouter {
    engine: clawft_services::delegation::DelegationEngine,
}

#[cfg(feature = "delegate")]
impl AutoDelegation for AutoDelegationRouter {
    fn should_delegate(&self, content: &str) -> Option<serde_json::Value> {
        use clawft_types::delegation::DelegationTarget;

        let target = self.engine.decide(content, true);
        match target {
            DelegationTarget::Local => None,
            _ => {
                tracing::debug!(
                    target = ?target,
                    content_preview = &content[..content.len().min(80)],
                    "auto-delegation matched"
                );
                Some(serde_json::json!({ "task": content }))
            }
        }
    }
}

/// Build an [`AutoDelegation`] router from delegation config.
#[cfg(feature = "delegate")]
fn build_auto_delegation(
    config: &clawft_types::delegation::DelegationConfig,
) -> Arc<dyn AutoDelegation> {
    let engine = clawft_services::delegation::DelegationEngine::new(config.clone());
    Arc::new(AutoDelegationRouter { engine })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_args_defaults() {
        // Verify the struct can be constructed with all-None fields.
        let args = AgentArgs {
            message: None,
            model: None,
            config: None,
            intelligent_routing: false,
            trust_project_skills: false,
        };
        assert!(args.message.is_none());
        assert!(args.model.is_none());
        assert!(args.config.is_none());
    }

    #[test]
    fn agent_args_with_message() {
        let args = AgentArgs {
            message: Some("test message".into()),
            model: None,
            config: None,
            intelligent_routing: false,
            trust_project_skills: false,
        };
        assert_eq!(args.message.as_deref(), Some("test message"));
    }

    #[test]
    fn agent_args_with_model_override() {
        let args = AgentArgs {
            message: None,
            model: Some("openai/gpt-4".into()),
            config: None,
            intelligent_routing: false,
            trust_project_skills: false,
        };
        assert_eq!(args.model.as_deref(), Some("openai/gpt-4"));
    }

    #[test]
    fn agent_args_with_config_path() {
        let args = AgentArgs {
            message: None,
            model: None,
            config: Some("/tmp/test-config.json".into()),
            intelligent_routing: false,
            trust_project_skills: false,
        };
        assert_eq!(args.config.as_deref(), Some("/tmp/test-config.json"));
    }

    #[test]
    fn discover_skill_dirs_returns_pair() {
        // Smoke test: discovery should not panic, and returns a tuple.
        let (ws, user) = discover_skill_dirs();
        // user dir should be set if $HOME is set
        if std::env::var("HOME").is_ok() {
            assert!(user.is_some());
        }
        // ws dir may or may not exist depending on cwd
        let _ = ws;
    }

    #[cfg(feature = "delegate")]
    mod auto_delegation_tests {
        use super::*;
        use clawft_types::delegation::{DelegationConfig, DelegationRule, DelegationTarget};

        #[test]
        fn auto_delegation_router_delegates_matching_messages() {
            let config = DelegationConfig {
                claude_enabled: true,
                rules: vec![
                    DelegationRule {
                        pattern: r"(?i)deploy|orchestrate|swarm".into(),
                        target: DelegationTarget::Flow,
                    },
                    DelegationRule {
                        pattern: r"(?i)research|analyze".into(),
                        target: DelegationTarget::Claude,
                    },
                    DelegationRule {
                        pattern: r"(?i)^list\b".into(),
                        target: DelegationTarget::Local,
                    },
                ],
                ..Default::default()
            };
            let router = build_auto_delegation(&config);

            // "swarm" matches → should delegate
            assert!(
                router
                    .should_delegate("run a swarm security review")
                    .is_some(),
                "swarm should trigger delegation"
            );

            // "deploy" matches → should delegate
            assert!(
                router.should_delegate("deploy to production").is_some(),
                "deploy should trigger delegation"
            );

            // "analyze" matches → should delegate
            assert!(
                router.should_delegate("analyze the codebase").is_some(),
                "analyze should trigger delegation"
            );

            // "list" matches Local → should NOT delegate
            assert!(
                router.should_delegate("list all files").is_none(),
                "list should be local, not delegated"
            );

            // Simple message, no match → auto-decide by complexity (low = Local)
            assert!(
                router.should_delegate("hello").is_none(),
                "hello should not be delegated"
            );
        }

        #[test]
        fn auto_delegation_router_returns_correct_args() {
            let config = DelegationConfig {
                claude_enabled: true,
                rules: vec![DelegationRule {
                    pattern: r"(?i)swarm".into(),
                    target: DelegationTarget::Claude,
                }],
                ..Default::default()
            };
            let router = build_auto_delegation(&config);

            let args = router
                .should_delegate("run a swarm task")
                .expect("should delegate");
            let task = args.get("task").and_then(|v| v.as_str());
            assert_eq!(task, Some("run a swarm task"));
        }
    }
}
