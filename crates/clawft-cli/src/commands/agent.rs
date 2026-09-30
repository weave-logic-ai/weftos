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


use crate::interactive::builtins::{QUIT_SENTINEL, register_builtins, register_skill_commands};
use crate::interactive::registry::{InteractiveContext, SlashCommandRegistry};

/// Arguments for the `weft agent` subcommand.
#[derive(Args)]
pub struct AgentArgs {
    /// Send a single message and exit (non-interactive mode).
    ///
    /// Each `-m` turn starts a fresh conversation with no prior history
    /// unless `--session` names one to continue.
    #[arg(short, long)]
    pub message: Option<String>,

    /// Continue a named conversation with `-m` (letters, digits, `.`, `_`,
    /// `-`; up to 128 characters). Two `-m` calls with the same
    /// `--session` share history. The interactive REPL keeps its own
    /// `cli-session` conversation and does not take this flag.
    #[arg(long, value_name = "ID", requires = "message")]
    pub session: Option<String>,

    /// Model to use (overrides config).
    #[arg(long)]
    pub model: Option<String>,

    /// Config file path (overrides auto-discovery).
    #[arg(short, long)]
    pub config: Option<String>,

    /// Enable intelligent routing (requires vector-memory feature).
    #[arg(long)]
    pub intelligent_routing: bool,

    /// Run the agent loop inside this process when no kernel daemon is
    /// reachable. Without it, a missing daemon is an error: an in-process
    /// agent can run tools and write files the daemon never sees.
    #[arg(long)]
    pub local: bool,

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
    // Resolve the one-shot conversation first so a bad --session fails fast.
    let one_shot_chat = match args.message {
        Some(_) => Some(one_shot_chat_id(args.session.as_deref())?),
        None => None,
    };

    let platform = Arc::new(NativePlatform::new());
    let loaded = super::load_config_layered(&*platform, args.config.as_deref()).await?;
    let mut config = loaded.config;

    // WEFT-604: bridge [kernel.llm] + LLM_* env into the standalone agent
    // path so daemon / weft agent / voice share ADR-060 defaults.
    let mut agents_model_source = loaded.agents_model_source;
    let resolution =
        clawft_core::local_llm_bridge::apply_local_llm_bridge(&mut config, agents_model_source);
    info!(
        url = %resolution.service_url,
        url_source = resolution.url_source,
        model = %resolution.routed_model,
        model_source = resolution.model_source,
        agents_model_source = resolution.agents_model_source,
        "local llm endpoint resolved for agent"
    );

    // Apply model override if provided (CLI flag wins over all layers).
    if let Some(ref model) = args.model {
        config.agents.defaults.model = model.clone();
        agents_model_source = "cli:--model";
    }

    let effective_model = &config.agents.defaults.model;
    info!(
        model = %effective_model,
        model_source = agents_model_source,
        "initializing agent"
    );

    // Daemon-first routing (M1 / ADR-061, WEFT-606): if a kernel daemon is
    // reachable, route every turn through its long-lived AgentService via the
    // `agent.chat` RPC so the daemon-hosted loop is THE single conversation
    // engine. When no daemon is running we fall through to the in-process
    // AgentLoop below with no behaviour change.
    if let Some(client) = super::agent_daemon::detect().await {
        info!("kernel daemon reachable — routing agent turns via agent.chat");
        if let (Some(message), Some(chat_id)) = (args.message.as_deref(), one_shot_chat.as_deref()) {
            return super::agent_daemon::run_single_message(client, message, chat_id).await;
        }
        let skill_registry = discover_skill_registry(args.trust_project_skills).await;
        return super::agent_daemon::run_interactive(client, &skill_registry).await;
    }
    if !args.local {
        let why = super::daemon_fallback::refuse_state_change("`weft agent`").await;
        anyhow::bail!("{why}\n  Or pass --local to run the agent loop in-process.");
    }
    eprintln!(
        "{}",
        super::daemon_fallback::local_note("running the agent loop in-process (--local)").await
    );
    info!("no kernel daemon reachable — using in-process agent loop (--local)");

    // Bootstrap the application context (bus, sessions, memory, skills, pipeline).
    // WEFT-10: attach split routing layers so PermissionResolver ceiling runs.
    let mut ctx = AppContext::new(config.clone(), platform.clone())
        .await
        .map_err(|e| anyhow::anyhow!("bootstrap failed: {e}"))?
        .with_routing_layers(loaded.global_routing, loaded.workspace_routing);

    // WEFT-185: construct AgentBus on the production agent path so A2A
    // messaging and SwarmCoordinator demos share the same wiring surface.
    let _agent_bus = ctx.enable_agent_bus(None);

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
    let skill_registry = discover_skill_registry(args.trust_project_skills).await;

    // Clone the bus before consuming the context.
    let bus = ctx.bus().clone();

    // Convert context into the agent loop (consumes ctx).
    // WEFT-67: attach skill-autogen pattern detector when opted in via
    // `skills.autogen.enabled` in ~/.clawft/config.json.
    // WEFT-66: AutogenConfig carries the active TrajectoryLearner so
    // learning-driven SKILL.md mutation runs when generation fires.
    let trajectory_learner = ctx.trajectory_learner();
    let mut agent = ctx.into_agent_loop();
    if config.skills.autogen.enabled {
        use clawft_core::agent::skill_autogen::{AutogenConfig, PatternDetector};
        use std::sync::{Arc, Mutex};
        let mut autogen_cfg = AutogenConfig {
            enabled: true,
            threshold: config.skills.autogen.threshold,
            max_pending: config.skills.autogen.max_pending,
            install_dir: None,
            learner: None,
        };
        if let Some(learner) = trajectory_learner {
            autogen_cfg = autogen_cfg.with_learner(learner);
        }
        let detector = Arc::new(Mutex::new(PatternDetector::new(autogen_cfg)));
        agent = agent.with_autogen(detector);
        info!(
            threshold = config.skills.autogen.threshold,
            max_pending = config.skills.autogen.max_pending,
            learning = config.pipeline.learner == "trajectory",
            "skill autogen enabled (pattern detection active)"
        );
    }

    // WEFT-348: attach Phase 4 skill auto-promoter when enabled.
    if config.skills.promotion.enabled {
        use clawft_core::agent::skill_autogen::{SkillPromotionConfig, SkillPromoter};
        use std::sync::{Arc, Mutex};
        let promo_cfg = SkillPromotionConfig::from_types(&config.skills.promotion);
        let promoter = Arc::new(Mutex::new(SkillPromoter::new(promo_cfg)));
        agent = agent.with_skill_promoter(promoter);
        info!(
            threshold = config.skills.promotion.threshold,
            "skill auto-promotion enabled (.claude/skills → .clawft/skills)"
        );
    }

    if let (Some(message), Some(chat_id)) = (args.message.as_deref(), one_shot_chat.as_deref()) {
        return run_single_message(message, chat_id, &bus, agent, effective_model).await;
    }

    run_interactive(&bus, agent, &tool_names, effective_model, &skill_registry).await
}

/// Chat id prefix for fresh one-shot (`-m`) conversations.
pub(crate) const ONE_SHOT_CHAT_PREFIX: &str = "oneshot-";

/// Chat id for a one-shot `-m` turn: the `--session` name when given
/// (validated so it is safe as a session file name), otherwise a fresh
/// `oneshot-<uuid>` so no earlier turn's history is loaded.
pub(crate) fn one_shot_chat_id(session: Option<&str>) -> anyhow::Result<String> {
    match session {
        Some(id) => {
            let ok = !id.is_empty()
                && id.len() <= 128
                && id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
                && !id.starts_with('.');
            if !ok {
                anyhow::bail!(
                    "invalid --session {id:?}: use letters, digits, '.', '_' or '-' (max 128, not starting with '.')"
                );
            }
            Ok(id.to_owned())
        }
        None => Ok(format!("{ONE_SHOT_CHAT_PREFIX}{}", uuid::Uuid::new_v4())),
    }
}

/// Process a single message through the agent loop and exit.
///
/// Publishes the message to the bus, spawns the agent loop in the
/// background, waits for the outbound response, and prints it.
async fn run_single_message(
    message: &str,
    chat_id: &str,
    bus: &Arc<MessageBus>,
    agent: clawft_core::agent::loop_core::AgentLoop<NativePlatform>,
    model: &str,
) -> anyhow::Result<()> {
    info!(model = %model, "single-message mode");

    // Engine line to stderr so `-m` stdout stays clean for scripting.
    eprintln!("Engine: in-process (--local)");

    // Create and publish the inbound message. The chat id is fresh per
    // invocation (or the `--session` name), never the REPL's `cli-session`.
    let inbound = InboundMessage {
        channel: "cli".into(),
        sender_id: "local".into(),
        chat_id: chat_id.to_owned(),
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

    // The loop flags failed turns (`OutboundMessage::is_error`: provider
    // not configured, max tool iterations, failed or refused delegation)
    // — exit non-zero instead of printing the error as a normal reply.
    let mut failure: Option<String> = None;
    match response {
        Some(msg) => {
            if msg.is_error() {
                eprintln!("{}", msg.content);
                failure = Some(msg.content);
            } else {
                println!("{}", msg.content);
            }
        }
        None => {
            eprintln!("error: no response from agent");
            failure = Some("no response from agent".to_string());
        }
    }

    // Signal the agent loop to stop by dropping the inbound sender.
    // The bus holds its own sender, so we close it by dropping the bus.
    // Since the bus is shared via Arc, we just drop the handle and abort.
    agent_handle.abort();
    let _ = agent_handle.await;

    if let Some(err) = failure {
        anyhow::bail!("{err}");
    }
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
    println!("Engine: in-process (--local)");
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
    // WEFT-74 / ADR-080: non-blocking notice so pending autogen skills do
    // not accumulate unseen. Review remains CLI (`weft skills pending|…`).
    if let Some(notice) = clawft_core::agent::skill_autogen::pending_review_start_notice(
        None,
        clawft_core::agent::skill_autogen::PendingReviewTiming::DEFAULT_POLICY,
    ) {
        println!("{notice}");
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

/// Discover the v2 skill registry from the workspace and user skill dirs.
///
/// Shared by the in-process and daemon-routed paths so both surface the
/// same set of `/skill` slash commands. `trust_ws` gates whether
/// workspace-level (`.clawft/skills/`) skills are loaded (SEC-SKILL-05).
/// Falls back to an empty registry if discovery fails.
async fn discover_skill_registry(trust_ws: bool) -> SkillRegistry {
    let (ws_skill_dir, user_skill_dir) = discover_skill_dirs();
    match SkillRegistry::discover_with_trust(
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
    }
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
    fn one_shot_turns_get_distinct_fresh_conversations() {
        let a = one_shot_chat_id(None).unwrap();
        let b = one_shot_chat_id(None).unwrap();
        assert!(a.starts_with(ONE_SHOT_CHAT_PREFIX));
        assert_ne!(a, b, "two -m calls must not share a conversation");
        assert_ne!(a, "cli-session", "a -m call must not use the REPL conversation");
    }

    #[test]
    fn named_session_is_reused_and_validated() {
        assert_eq!(one_shot_chat_id(Some("review-1")).unwrap(), "review-1");
        assert_eq!(
            one_shot_chat_id(Some("review-1")).unwrap(),
            one_shot_chat_id(Some("review-1")).unwrap()
        );
        for bad in ["", "../x", "a/b", ".hidden", "sp ace", &"x".repeat(129)] {
            assert!(one_shot_chat_id(Some(bad)).is_err(), "{bad:?} must be rejected");
        }
    }

    #[test]
    fn session_flag_requires_message() {
        use clap::Parser;
        #[derive(Parser)]
        struct Wrap {
            #[command(flatten)]
            a: AgentArgs,
        }
        assert!(Wrap::try_parse_from(["weft", "--session", "x"]).is_err());
        let w = Wrap::try_parse_from(["weft", "-m", "hi", "--session", "x"]).unwrap();
        assert_eq!(w.a.session.as_deref(), Some("x"));
    }

    #[test]
    fn agent_args_defaults() {
        // Verify the struct can be constructed with all-None fields.
        let args = AgentArgs {
            message: None,
            session: None,
            model: None,
            config: None,
            intelligent_routing: false,
            trust_project_skills: false,
            local: false,
        };
        assert!(args.message.is_none());
        assert!(args.model.is_none());
        assert!(args.config.is_none());
    }

    #[test]
    fn agent_args_with_message() {
        let args = AgentArgs {
            message: Some("test message".into()),
            session: None,
            model: None,
            config: None,
            intelligent_routing: false,
            trust_project_skills: false,
            local: false,
        };
        assert_eq!(args.message.as_deref(), Some("test message"));
    }

    #[test]
    fn agent_args_with_model_override() {
        let args = AgentArgs {
            message: None,
            session: None,
            model: Some("openai/gpt-4".into()),
            config: None,
            intelligent_routing: false,
            trust_project_skills: false,
            local: false,
        };
        assert_eq!(args.model.as_deref(), Some("openai/gpt-4"));
    }

    #[test]
    fn agent_args_with_config_path() {
        let args = AgentArgs {
            message: None,
            session: None,
            model: None,
            config: Some("/tmp/test-config.json".into()),
            intelligent_routing: false,
            trust_project_skills: false,
            local: false,
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
                        target: DelegationTarget::Claude,
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

        /// WEFT-201: corpus cases for pre-LLM short-circuit (see
        /// `docs/guides/auto-delegation-classifier.md`).
        #[test]
        fn auto_delegation_classifier_corpus() {
            let config = DelegationConfig {
                claude_enabled: true,
                rules: vec![
                    DelegationRule {
                        pattern: r"(?i)^list\b".into(),
                        target: DelegationTarget::Local,
                    },
                    DelegationRule {
                        pattern: r"(?i)\b(deploy|swarm|orchestrate)\b".into(),
                        target: DelegationTarget::Claude,
                    },
                ],
                ..Default::default()
            };
            let router = build_auto_delegation(&config);

            // Delegate
            assert!(router.should_delegate("deploy to prod").is_some());
            assert!(router.should_delegate("orchestrate a multi-agent review").is_some());
            // Stay local
            assert!(router.should_delegate("list all files").is_none());
            assert!(router.should_delegate("hello").is_none());
            // Anchoring: mid-sentence list without ^ only hits if Auto is high
            assert!(
                router.should_delegate("please list open bugs").is_none(),
                "short non-keyword message should not auto-delegate"
            );
        }
    }
}
