//! Application bootstrap and dependency wiring.
//!
//! Provides [`AppContext`], a convenience struct that initializes all
//! core components from a [`Config`] and a [`Platform`], then produces
//! an [`AgentLoop`] ready to run.
//!
//! # Usage
//!
//! ```rust,ignore
//! use clawft_core::bootstrap::AppContext;
//! use clawft_platform::NativePlatform;
//! use clawft_types::config::Config;
//!
//! let config = Config::default();
//! let platform = Arc::new(NativePlatform::new());
//! let ctx = AppContext::new(config, platform).await?;
//! let agent_loop = ctx.into_agent_loop();
//! agent_loop.run().await?;
//! ```

use std::sync::Arc;

use tracing::{debug, info};

use clawft_platform::Platform;
use clawft_types::config::Config;

use crate::agent::context::ContextBuilder;
use crate::agent::loop_core::{AgentLoop, AutoDelegation};
use crate::agent::memory::MemoryStore;
use crate::agent::skills::SkillsLoader;
use crate::bus::MessageBus;
use crate::pipeline::assembler::TokenBudgetAssembler;
use crate::pipeline::classifier::KeywordClassifier;
use crate::pipeline::cost_tracker::CostTracker;
use crate::pipeline::rate_limiter::RateLimiter;
use crate::pipeline::router::StaticRouter;
use crate::pipeline::tiered_router::TieredRouter;
use crate::pipeline::traits::{ModelRouter, Pipeline, PipelineRegistry};
use crate::pipeline::transport::OpenAiCompatTransport;
use crate::session::SessionManager;
use crate::tools::registry::ToolRegistry;

/// Fully initialized application context.
///
/// Holds all dependencies needed to run the agent loop. Created via
/// [`AppContext::new`] and consumed via [`AppContext::into_agent_loop`].
///
/// The generic parameter `P` is the platform implementation (e.g.
/// [`NativePlatform`](clawft_platform::NativePlatform) for native,
/// a WASM platform for browser environments).
pub struct AppContext<P: Platform> {
    /// Root configuration.
    config: Config,

    /// Platform abstraction.
    platform: Arc<P>,

    /// Message bus for inbound/outbound message routing.
    bus: Arc<MessageBus>,

    /// Session manager for conversation persistence.
    sessions: Arc<SessionManager<P>>,

    /// Tool registry with registered tools.
    tools: Arc<ToolRegistry>,

    /// Pipeline registry with all 6 stages wired.
    pipeline: PipelineRegistry,

    /// Context builder for assembling LLM prompts.
    context: ContextBuilder<P>,

    /// Shared memory store reference (for external access).
    memory: Arc<MemoryStore<P>>,

    /// Shared skills loader reference (for external access).
    skills: Arc<SkillsLoader<P>>,

    /// Optional auto-delegation router for pre-LLM routing.
    auto_delegation: Option<Arc<dyn AutoDelegation>>,

    /// Optional inbound-message → agent router.
    ///
    /// Routes incoming [`InboundMessage`](clawft_types::event::InboundMessage)s
    /// to a specific agent persona based on channel/user rules. Not
    /// used by the single-agent CLI flow today; consumed by the
    /// daemon's multi-agent dispatcher when set.
    agent_router: Option<Arc<crate::agent_routing::AgentRouter>>,

    /// Optional agent-to-agent message bus.
    ///
    /// Provides per-agent inboxes with TTL enforcement and
    /// inbox-scoped delivery. The CLI runs a single agent so doesn't
    /// need it, but multi-agent hosts (the daemon's spawn manager)
    /// register each spawned agent and route IPC through the bus.
    agent_bus: Option<Arc<crate::agent_bus::AgentBus>>,
}

impl<P: Platform> AppContext<P> {
    /// Initialize all components from configuration and platform.
    ///
    /// This is the primary constructor. It:
    /// 1. Creates the [`MessageBus`]
    /// 2. Initializes the [`SessionManager`] (discovers/creates sessions dir)
    /// 3. Initializes the [`MemoryStore`] (discovers memory dir)
    /// 4. Initializes the [`SkillsLoader`] (discovers skills dir)
    /// 5. Creates the [`ContextBuilder`]
    /// 6. Creates an empty [`ToolRegistry`] (caller registers tools after)
    /// 7. Wires the default Level 0 pipeline (keyword classifier, static
    ///    router, token budget assembler, stub transport, noop scorer,
    ///    noop learner)
    ///
    /// # Errors
    ///
    /// Returns [`ClawftError`] if the home directory cannot be determined
    /// or the sessions directory cannot be created.
    pub async fn new(config: Config, platform: Arc<P>) -> clawft_types::Result<Self> {
        info!("bootstrapping application context");

        // 1. Message bus
        let bus = Arc::new(MessageBus::new());
        debug!("message bus created");

        // 2. Session manager
        let sessions = SessionManager::new(platform.clone()).await?;
        debug!("session manager initialized");

        // 3. Memory store
        let memory = Arc::new(MemoryStore::new(platform.clone())?);
        debug!(
            memory_path = %memory.memory_path().display(),
            "memory store initialized"
        );

        // 4. Skills loader
        let mut skills_loader = SkillsLoader::new(platform.clone())?;

        // Also scan extra `skills/` directories if they exist:
        // - workspace/skills/  (from config workspace path)
        // - ./skills/          (current working directory)
        for candidate in [
            config.workspace_path().join("skills"),
            std::env::current_dir().unwrap_or_default().join("skills"),
        ] {
            if candidate.is_dir() {
                skills_loader.add_extra_dir(candidate);
            }
        }

        let skills = Arc::new(skills_loader);
        debug!(
            skills_dir = %skills.skills_dir().display(),
            "skills loader initialized"
        );

        // 5. Context builder
        let context = ContextBuilder::new(
            config.agents.clone(),
            memory.clone(),
            skills.clone(),
            platform.clone(),
        );
        debug!("context builder created");

        // 6. Tool registry (empty -- caller adds tools)
        let tools = Arc::new(ToolRegistry::new());

        // 7. Default Level 0 pipeline
        let pipeline = build_default_pipeline(&config);
        debug!("default pipeline wired");

        info!("bootstrap complete");

        Ok(Self {
            config,
            platform,
            bus,
            sessions: Arc::new(sessions),
            tools,
            pipeline,
            context,
            memory,
            skills,
            auto_delegation: None,
            agent_router: None,
            agent_bus: None,
        })
    }

    /// Consume the context and produce a running [`AgentLoop`].
    ///
    /// All dependencies are moved into the agent loop. After this call,
    /// the `AppContext` is consumed and cannot be reused.
    pub fn into_agent_loop(self) -> AgentLoop<P> {
        let resolver = crate::pipeline::permissions::PermissionResolver::new(
            &self.config.routing,
            None, // workspace config not yet supported
        );
        let mut agent = AgentLoop::new(
            self.config.agents,
            self.platform,
            self.bus,
            self.pipeline,
            self.tools.clone(),
            self.context,
            self.sessions.clone(),
            resolver,
        );
        if let Some(delegation) = self.auto_delegation {
            agent = agent.with_auto_delegation(delegation);
        }
        agent
    }

    /// Get a reference to the root configuration.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Get a reference to the platform.
    pub fn platform(&self) -> &Arc<P> {
        &self.platform
    }

    /// Get a reference to the message bus.
    ///
    /// Channel adapters should clone the inbound sender from here
    /// before the context is consumed by [`into_agent_loop`].
    pub fn bus(&self) -> &Arc<MessageBus> {
        &self.bus
    }

    /// Get a mutable reference to the tool registry.
    ///
    /// Call this to register tools before converting to an agent loop.
    ///
    /// # Panics
    ///
    /// Panics if Arc clones have already been taken (e.g. via `tools_arc()`).
    /// Always call `tools_mut()` for registration *before* sharing the registry.
    pub fn tools_mut(&mut self) -> &mut ToolRegistry {
        Arc::get_mut(&mut self.tools).expect("tools already shared -- register tools before cloning Arc")
    }

    /// Get a reference to the tool registry.
    pub fn tools(&self) -> &Arc<ToolRegistry> {
        &self.tools
    }

    /// Get a clone of the `Arc<ToolRegistry>` for sharing with other components.
    pub fn tools_arc(&self) -> Arc<ToolRegistry> {
        self.tools.clone()
    }

    /// Get a reference to the session manager.
    pub fn sessions(&self) -> &Arc<SessionManager<P>> {
        &self.sessions
    }

    /// Get a reference to the shared memory store.
    pub fn memory(&self) -> &Arc<MemoryStore<P>> {
        &self.memory
    }

    /// Get a reference to the shared skills loader.
    pub fn skills(&self) -> &Arc<SkillsLoader<P>> {
        &self.skills
    }

    /// Start a [`SkillWatcher`](crate::agent::skill_watcher::start_watching)
    /// over the same directories the [`SkillsLoader`] scans.
    ///
    /// Returns a handle that the caller MUST keep alive for the
    /// duration of the agent loop — dropping it stops the watcher.
    /// Returns `None` when the underlying file-system watcher could
    /// not be started (e.g. inotify quota exhausted); the agent loop
    /// still runs, just without hot-reload.
    ///
    /// The watcher is opt-in: bootstrap does NOT start it
    /// automatically because the agent loop's existing reads go
    /// through `SkillsLoader` which is already responsive to disk
    /// changes on the cold path. Hot-reload via this watcher reflects
    /// changes into a parallel `SkillRegistry` (v2) that's
    /// non-disruptively available for callers that want it.
    ///
    /// Native-only: the `notify` crate doesn't compile to wasm.
    #[cfg(feature = "native")]
    pub async fn start_skill_watcher(
        &self,
    ) -> Option<crate::agent::skill_watcher::SkillWatcherHandle> {
        use std::sync::Arc as ArcAlias;
        use tokio::sync::RwLock;

        use crate::agent::skill_watcher::{start_watching, SkillWatcherConfig};
        use crate::agent::skills_v2::SkillRegistry;

        let workspace_dir = Some(self.skills.skills_dir().clone());
        let registry = match SkillRegistry::discover(workspace_dir.as_deref(), None, vec![]).await
        {
            Ok(r) => ArcAlias::new(RwLock::new(r)),
            Err(e) => {
                tracing::warn!(error = %e, "skill watcher: initial discover failed; not starting");
                return None;
            }
        };

        let config = SkillWatcherConfig {
            workspace_dir: workspace_dir.clone(),
            user_dir: None,
            extra_dirs: Vec::new(),
            debounce: std::time::Duration::from_millis(500),
            builtin_skills: Vec::new(),
            trust_workspace: true,
        };

        match start_watching(config, registry) {
            Ok(handle) => {
                tracing::info!(
                    skills_dir = %self.skills.skills_dir().display(),
                    "skill hot-reload watcher started"
                );
                Some(handle)
            }
            Err(e) => {
                tracing::warn!(error = %e, "failed to start skill watcher");
                None
            }
        }
    }

    /// Set an auto-delegation router for pre-LLM routing.
    ///
    /// When set, inbound messages are checked against delegation rules
    /// before the local LLM is invoked. Matching messages are routed
    /// directly to `delegate_task`.
    pub fn set_auto_delegation(&mut self, delegation: Arc<dyn AutoDelegation>) {
        self.auto_delegation = Some(delegation);
    }

    /// Attach an [`AgentRouter`](crate::agent_routing::AgentRouter)
    /// for inbound-message-to-agent routing. Multi-agent dispatchers
    /// (e.g. the daemon) consume this; the single-agent CLI flow
    /// ignores it.
    pub fn set_agent_router(
        &mut self,
        router: Arc<crate::agent_routing::AgentRouter>,
    ) {
        self.agent_router = Some(router);
    }

    /// Borrow the optional agent router. `None` means there's no
    /// multi-agent routing layer in front of the loop.
    pub fn agent_router(&self) -> Option<&Arc<crate::agent_routing::AgentRouter>> {
        self.agent_router.as_ref()
    }

    /// Attach a shared [`AgentBus`](crate::agent_bus::AgentBus). The
    /// daemon's agent supervisor registers each spawned agent with
    /// this bus so A2A messaging is inbox-scoped.
    pub fn set_agent_bus(&mut self, bus: Arc<crate::agent_bus::AgentBus>) {
        self.agent_bus = Some(bus);
    }

    /// Borrow the optional agent bus. `None` means A2A messaging is
    /// disabled (single-agent process).
    pub fn agent_bus(&self) -> Option<&Arc<crate::agent_bus::AgentBus>> {
        self.agent_bus.as_ref()
    }

    /// Replace the pipeline registry with a custom one.
    ///
    /// Use this to inject a transport backed by a real LLM provider
    /// or to register specialized pipelines for specific task types.
    pub fn set_pipeline(&mut self, pipeline: PipelineRegistry) {
        self.pipeline = pipeline;
    }

    /// Replace the pipeline with a live LLM-backed pipeline.
    ///
    /// Convenience method that calls [`build_live_pipeline`] and sets it
    /// as the active pipeline, enabling real LLM calls through the
    /// `clawft-llm` provider resolved from configuration.
    ///
    /// Only available with the `native` feature.
    #[cfg(feature = "native")]
    pub fn enable_live_llm(&mut self) {
        let pipeline = build_live_pipeline(&self.config);
        self.set_pipeline(pipeline);
    }
}

/// Build a live pipeline with a real LLM provider from configuration.
///
/// Uses [`ClawftLlmAdapter`](crate::pipeline::llm_adapter::ClawftLlmAdapter)
/// to bridge `clawft-llm` providers into the pipeline's transport layer,
/// enabling real LLM calls.
///
/// This is the production counterpart of [`build_default_pipeline`], which
/// uses a stub transport. All other pipeline stages are identical.
///
/// Only available with the `native` feature (requires clawft-llm HTTP providers).
#[cfg(feature = "native")]
pub fn build_live_pipeline(config: &Config) -> PipelineRegistry {
    crate::pipeline::llm_adapter::build_live_pipeline(config)
}

/// Build the default pipeline from configuration.
///
/// Uses the appropriate router based on `config.routing.mode`:
/// - `"tiered"` -> [`TieredRouter`] with cost tracking and rate limiting
/// - `"static"` (default) -> [`StaticRouter`] from config defaults
///
/// Other stages: [`KeywordClassifier`], [`TokenBudgetAssembler`],
/// [`OpenAiCompatTransport`] wired with [`ServiceLlmAdapter`] against
/// the local llama-server resolved from
/// [`LlmConfig::from_env`](clawft_service_llm::LlmConfig::from_env)
/// (so the agent loop, the daemon's `llm.prompt` RPC, and the chat
/// panel all share one model server), [`NoopScorer`], [`NoopLearner`].
///
/// On `LlmClient` construction failure (e.g. malformed env URL) the
/// transport falls back to the no-provider stub so the rest of the
/// pipeline still wires; the agent will surface a clear error on the
/// first call.
///
/// Browser builds keep the original stubbed transport — service-llm
/// pulls in `reqwest` and is native-only.
fn build_default_pipeline(config: &Config) -> PipelineRegistry {
    let classifier = Arc::new(KeywordClassifier::new());
    let router: Arc<dyn ModelRouter> = build_router_from_config(config);
    let assembler = Arc::new(TokenBudgetAssembler::new(
        config.agents.defaults.max_tokens.max(1) as usize,
    ));
    let transport = build_default_transport();
    let scorer = crate::pipeline::build_scorer(&config.pipeline);
    let learner = crate::pipeline::build_learner(&config.pipeline);

    let pipeline = Pipeline {
        classifier,
        router,
        assembler,
        transport,
        scorer,
        learner,
    };

    PipelineRegistry::new(pipeline)
}

/// Native: wrap a [`ServiceLlmAdapter`] over a freshly-constructed
/// [`LlmClient`] from the env-resolved config. Falls back to the stub
/// transport if the client cannot be built (e.g. invalid base URL).
#[cfg(feature = "native")]
fn build_default_transport() -> Arc<OpenAiCompatTransport> {
    use clawft_service_llm::{LlmClient, LlmConfig};

    use crate::pipeline::service_llm_adapter::ServiceLlmAdapter;
    use crate::pipeline::transport::LlmProvider;

    let llm_config = LlmConfig::from_env();
    match LlmClient::new(llm_config) {
        Ok(client) => {
            let adapter: Arc<dyn LlmProvider> =
                Arc::new(ServiceLlmAdapter::new(Arc::new(client)));
            tracing::info!(
                "pipeline: transport wired to clawft-service-llm (LlmClient over llama-server)"
            );
            Arc::new(OpenAiCompatTransport::with_provider(adapter))
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "pipeline: failed to construct LlmClient — falling back to stub transport"
            );
            Arc::new(OpenAiCompatTransport::new())
        }
    }
}

/// Browser: stub transport (service-llm pulls reqwest, native-only).
#[cfg(not(feature = "native"))]
fn build_default_transport() -> Arc<OpenAiCompatTransport> {
    Arc::new(OpenAiCompatTransport::new())
}

/// Construct an [`AgentLoop`] suitable for daemon-side hosting via
/// `clawft-service-agent::AgentService`.
///
/// This is a thin factory used by `clawft-weave::daemon` (Phase C2 of
/// `docs/plans/agent-core-v1.md`). It accepts the handles the daemon
/// already has on hand — an `LlmClient`, a populated `ToolRegistry`,
/// an `IdentityLoader`, and a workspace path — and wires the rest
/// (message bus, session manager, context builder, pipeline) with
/// daemon-flavored defaults:
///
/// - `Config::default()` agents config (the daemon's chat path doesn't
///   need the workspace `Config` — `agent.chat`'s temperature/max_tokens
///   come from the wire params).
/// - A pipeline registry whose transport is wired to the supplied
///   [`LlmClient`] via [`ServiceLlmAdapter`](crate::pipeline::service_llm_adapter::ServiceLlmAdapter)
///   so the agent loop, the daemon's `llm.prompt` RPC, and the chat
///   panel all share one model server.
/// - `NullRouter` default (Phase B1). Phase E1 swaps in
///   `LlmClassifierRouter`.
/// - `gate`: caller-supplied [`EffectGate`](crate::agent::gate::EffectGate).
///   `None` falls back to [`NoopGate`](crate::agent::gate::NoopGate)
///   (always-permit, Phase B2 default). Phase D2's daemon construction
///   site passes `Some(KernelEffectGate)` so every tool dispatch in
///   `agent.chat` gets audited via
///   `clawft_kernel::GovernanceGate::check`. See `agent-core-v1.md`
///   Phase D2.
/// - `agent_id`: caller-supplied daemon agent id used by every
///   `gate.check` call. `None` keeps the pre-D2 behaviour of
///   synthesizing `"{channel}:{sender_id}"` from the inbound message.
///   Phase D2's daemon construction site registers a single concierge
///   principal in `clawft-kernel::AgentRegistry` at boot and threads
///   the resulting id through here. v1 chat is single-tenant; per-user
///   agent ids land in a future phase.
/// - `sink`: caller-supplied [`ConversationSink`](crate::agent::sink::ConversationSink).
///   `None` falls back to the in-memory sink (the C1/C2 default). The
///   C3 daemon construction site passes `Some(SubstrateConversationSink)`
///   so per-turn JSONL lands in substrate; CLI/test bootstrap callers
///   pass `None`. See `agent-core-v1.md` Phase C3.
/// - `identity_provider`: caller-supplied
///   [`IdentityProvider`](crate::agent::identity::IdentityProvider).
///   When `Some`, this factory wraps it in a
///   [`SystemPromptBuilder`](crate::agent::system_prompt::SystemPromptBuilder)
///   and attaches it to the loop so each turn emits an identity-aware
///   leading system message (agent-core-v1 Phase D1). When `None`, the
///   loop falls back to the legacy `ContextBuilder`-only system prompt
///   so existing CLI / test callers see no behaviour change.
///
/// The legacy `_identity_loader` argument is retained but unused — the
/// builder consumes `IdentityProvider` directly. Phase F1 will retire
/// the loader entirely once `weaver init` seeds local `.clawft/`.
///
/// Native-only: `NativePlatform` and `LlmClient` are both native-gated.
#[cfg(feature = "native")]
#[allow(clippy::too_many_arguments)]
pub async fn build_daemon_agent_loop(
    llm: Arc<clawft_service_llm::LlmClient>,
    tools: Arc<ToolRegistry>,
    _identity_loader: Arc<crate::agent::identity::IdentityLoader>,
    workspace: &std::path::Path,
    agent_id: Option<String>,
    gate: Option<Arc<dyn crate::agent::gate::EffectGate>>,
    sink: Option<Arc<dyn crate::agent::sink::ConversationSink>>,
    identity_provider: Option<Arc<dyn crate::agent::identity::IdentityProvider>>,
) -> Arc<crate::agent::loop_core::AgentLoop<clawft_platform::NativePlatform>> {
    use clawft_platform::NativePlatform;

    use crate::pipeline::service_llm_adapter::ServiceLlmAdapter;
    use crate::pipeline::transport::{LlmProvider, OpenAiCompatTransport};

    // Daemon-side config: pull a Config::default() and stamp the
    // workspace path. The agent.chat wire params override
    // temperature/max_tokens per turn, so the defaults here only
    // matter for fields the wire doesn't carry (e.g. permission
    // resolver baselines).
    let mut config = Config::default();
    config.agents.defaults.workspace = workspace.display().to_string();

    let platform = Arc::new(NativePlatform::new());

    // Build the supporting infrastructure inline (analogous to
    // `AppContext::new` minus the parts the daemon doesn't need
    // — skill watcher, agent router, agent bus). We allow this to
    // be `expect(...)` because the daemon already validated the
    // workspace earlier at boot; a failure here is a hard boot
    // failure rather than a recoverable error.
    let bus = Arc::new(MessageBus::new());

    // SessionManager::new and MemoryStore::new can fail when the
    // platform's home_dir resolution fails. The daemon already
    // resolved the runtime dir earlier; we propagate via expect.
    let sessions = SessionManager::new(platform.clone())
        .await
        .expect("daemon: SessionManager init failed");
    let memory = Arc::new(
        crate::agent::memory::MemoryStore::new(platform.clone())
            .expect("daemon: MemoryStore init failed"),
    );
    let skills_loader = crate::agent::skills::SkillsLoader::new(platform.clone())
        .expect("daemon: SkillsLoader init failed");
    let skills = Arc::new(skills_loader);
    let context = ContextBuilder::new(
        config.agents.clone(),
        memory,
        skills,
        platform.clone(),
    );

    // Pipeline transport: bridge the daemon's already-constructed
    // LlmClient through the ServiceLlmAdapter so the agent loop
    // shares the daemon's single LLM connection.
    let adapter: Arc<dyn LlmProvider> = Arc::new(ServiceLlmAdapter::new(llm));
    let transport = Arc::new(OpenAiCompatTransport::with_provider(adapter));
    let classifier = Arc::new(KeywordClassifier::new());
    let router: Arc<dyn ModelRouter> = build_router_from_config(&config);
    let assembler = Arc::new(TokenBudgetAssembler::new(
        config.agents.defaults.max_tokens.max(1) as usize,
    ));
    let scorer = crate::pipeline::build_scorer(&config.pipeline);
    let learner = crate::pipeline::build_learner(&config.pipeline);
    let pipeline = PipelineRegistry::new(Pipeline {
        classifier,
        router,
        assembler,
        transport,
        scorer,
        learner,
    });

    let resolver = crate::pipeline::permissions::PermissionResolver::new(
        &config.routing,
        None,
    );
    let mut agent = crate::agent::loop_core::AgentLoop::new(
        config.agents,
        platform,
        bus,
        pipeline,
        tools,
        context,
        Arc::new(sessions),
        resolver,
    );
    // C3 attaches the caller's sink (substrate-backed at the daemon
    // construction site; falls back to the in-memory default for CLI
    // / non-substrate callers).
    if let Some(s) = sink {
        agent = agent.with_sink(s);
    }
    // D1 attaches the identity-aware system-prompt builder. The
    // builder is wrapped in an Arc so every turn re-uses the same
    // provider (and its cache, in the FileIdentityProvider case)
    // without per-turn re-construction cost.
    if let Some(provider) = identity_provider {
        let builder = Arc::new(crate::agent::system_prompt::SystemPromptBuilder::new(
            provider,
            workspace.to_path_buf(),
        ));
        agent = agent.with_system_prompt_builder(builder);
    }
    // D2 attaches the kernel-backed gate. When `None`, AgentLoop's
    // `NoopGate` default keeps behaviour identical to the pre-D2
    // path so CLI / test callers see no change. The daemon
    // construction site passes `Some(KernelEffectGate)` so every
    // tool dispatch hits `GovernanceGate::check`.
    if let Some(g) = gate {
        agent = agent.with_gate(g);
    }
    // D2 also threads through the daemon-supplied concierge agent
    // id used by every `gate.check`. Without this, the loop falls
    // back to the per-message `"{channel}:{sender_id}"` synthesis
    // (the CLI / test path).
    if let Some(id) = agent_id {
        agent = agent.with_daemon_agent_id(id);
    }
    Arc::new(agent)
}

/// Build the appropriate router based on `config.routing.mode`.
fn build_router_from_config(config: &Config) -> Arc<dyn ModelRouter> {
    if config.routing.mode == "tiered" {
        let routing = config.routing.clone();
        let cost_tracker = Arc::new(
            CostTracker::new(routing.cost_budgets.reset_hour_utc),
        );
        let rate_limiter = Arc::new(
            RateLimiter::new(routing.rate_limiting.window_seconds, routing.rate_limiting.global_rate_limit_rpm),
        );
        Arc::new(
            TieredRouter::new(routing)
                .with_cost_tracker(cost_tracker)
                .with_rate_limiter(rate_limiter),
        )
    } else {
        Arc::new(StaticRouter::from_config(&config.agents))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clawft_platform::NativePlatform;
    use clawft_types::config::{AgentDefaults, AgentsConfig};
    use std::sync::Arc;

    fn test_config() -> Config {
        Config {
            agents: AgentsConfig {
                defaults: AgentDefaults {
                    workspace: "~/.clawft/workspace".into(),
                    model: "deepseek/deepseek-chat".into(),
                    max_tokens: 4096,
                    temperature: 0.7,
                    max_tool_iterations: 10,
                    memory_window: 50,
                },
            },
            ..Config::default()
        }
    }

    #[tokio::test]
    async fn new_creates_app_context() {
        let platform = Arc::new(NativePlatform::new());
        let ctx = AppContext::new(test_config(), platform).await;
        assert!(ctx.is_ok());
    }

    #[tokio::test]
    async fn config_accessor() {
        let platform = Arc::new(NativePlatform::new());
        let ctx = AppContext::new(test_config(), platform).await.unwrap();
        assert_eq!(
            ctx.config().agents.defaults.model,
            "deepseek/deepseek-chat"
        );
        assert_eq!(ctx.config().agents.defaults.max_tokens, 4096);
    }

    #[tokio::test]
    async fn platform_accessor() {
        let platform = Arc::new(NativePlatform::new());
        let ctx = AppContext::new(test_config(), platform).await.unwrap();
        let _p = ctx.platform();
    }

    #[tokio::test]
    async fn bus_accessor() {
        let platform = Arc::new(NativePlatform::new());
        let ctx = AppContext::new(test_config(), platform).await.unwrap();
        let bus = ctx.bus();
        // Should be able to get an inbound sender
        let _tx = bus.inbound_sender();
    }

    #[tokio::test]
    async fn tools_starts_empty() {
        let platform = Arc::new(NativePlatform::new());
        let ctx = AppContext::new(test_config(), platform).await.unwrap();
        assert!(ctx.tools().is_empty());
    }

    #[tokio::test]
    async fn tools_mut_allows_registration() {
        let platform = Arc::new(NativePlatform::new());
        let mut ctx = AppContext::new(test_config(), platform).await.unwrap();

        use crate::tools::registry::Tool;
        use async_trait::async_trait;

        struct DummyTool;

        #[async_trait]
        impl Tool for DummyTool {
            fn name(&self) -> &str {
                "dummy"
            }
            fn description(&self) -> &str {
                "A dummy tool"
            }
            fn parameters(&self) -> serde_json::Value {
                serde_json::json!({"type": "object", "properties": {}})
            }
            async fn execute(
                &self,
                _args: serde_json::Value,
            ) -> Result<serde_json::Value, crate::tools::registry::ToolError> {
                Ok(serde_json::json!({}))
            }
        }

        ctx.tools_mut().register(Arc::new(DummyTool));
        assert_eq!(ctx.tools().len(), 1);
        assert_eq!(ctx.tools().list(), vec!["dummy"]);
    }

    #[tokio::test]
    async fn memory_accessor() {
        let platform = Arc::new(NativePlatform::new());
        let ctx = AppContext::new(test_config(), platform).await.unwrap();
        let memory = ctx.memory();
        assert!(memory.memory_path().is_absolute());
    }

    #[tokio::test]
    async fn skills_accessor() {
        let platform = Arc::new(NativePlatform::new());
        let ctx = AppContext::new(test_config(), platform).await.unwrap();
        let skills = ctx.skills();
        assert!(skills.skills_dir().is_absolute());
    }

    #[tokio::test]
    async fn into_agent_loop_produces_valid_loop() {
        let platform = Arc::new(NativePlatform::new());
        let ctx = AppContext::new(test_config(), platform).await.unwrap();
        let agent = ctx.into_agent_loop();
        assert_eq!(agent.config().defaults.model, "deepseek/deepseek-chat");
        assert_eq!(agent.config().defaults.max_tokens, 4096);
    }

    #[tokio::test]
    async fn set_pipeline_replaces_default() {
        let platform = Arc::new(NativePlatform::new());
        let mut ctx = AppContext::new(test_config(), platform).await.unwrap();

        // Build a custom pipeline with a different router
        let custom_pipeline = build_default_pipeline(&test_config());
        ctx.set_pipeline(custom_pipeline);

        // Should still produce a valid agent loop
        let agent = ctx.into_agent_loop();
        assert_eq!(agent.config().defaults.model, "deepseek/deepseek-chat");
    }

    #[tokio::test]
    async fn default_config_bootstrap() {
        let platform = Arc::new(NativePlatform::new());
        let ctx = AppContext::new(Config::default(), platform).await;
        assert!(ctx.is_ok());
    }

    #[test]
    fn build_default_pipeline_creates_registry() {
        let config = test_config();
        let _registry = build_default_pipeline(&config);
        // Should not panic.
    }

    #[test]
    fn build_default_pipeline_with_defaults() {
        let config = Config::default();
        let _registry = build_default_pipeline(&config);
    }

    #[tokio::test]
    async fn bus_clone_survives_into_agent_loop() {
        let platform = Arc::new(NativePlatform::new());
        let ctx = AppContext::new(test_config(), platform).await.unwrap();

        // Clone the bus sender before consuming the context
        let tx = ctx.bus().inbound_sender();
        let _agent = ctx.into_agent_loop();

        // The sender should still be usable
        use chrono::Utc;
        use clawft_types::event::InboundMessage;
        use std::collections::HashMap;

        let msg = InboundMessage {
            channel: "test".into(),
            sender_id: "user1".into(),
            chat_id: "chat1".into(),
            content: "hello".into(),
            timestamp: Utc::now(),
            media: vec![],
            metadata: HashMap::new(),
        };
        assert!(tx.send(msg).await.is_ok());
    }

    #[tokio::test]
    async fn app_context_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<AppContext<NativePlatform>>();
    }

    // ── Integration: build_live_pipeline transport is configured ─────

    #[tokio::test]
    async fn build_live_pipeline_transport_is_configured() {
        use crate::pipeline::traits::{LlmMessage, TaskType, TransportRequest};

        // The live pipeline should have a configured (non-stub) transport.
        // When we call complete() on the stub transport, it returns an error
        // containing "transport not configured". The live pipeline should
        // NOT produce that error -- it should attempt a real HTTP call
        // (which will fail differently without an API key).
        let config = test_config();
        let registry = build_live_pipeline(&config);

        // Access the default pipeline's transport via get() with an unregistered type.
        let pipeline = registry.get(&TaskType::Unknown);

        let transport_req = TransportRequest {
            provider: "anthropic".into(),
            model: "claude-opus-4-5".into(),
            messages: vec![LlmMessage {
                role: "user".into(),
                content: "test".into(),
                tool_call_id: None,
                tool_calls: None,
            }],
            tools: vec![],
            max_tokens: Some(10),
            temperature: Some(0.0),
        };

        let result = pipeline.transport.complete(&transport_req).await;

        // The live transport IS configured (has a provider), so it should
        // NOT fail with "transport not configured". It will fail with an
        // HTTP/auth error instead because there's no real API key.
        match result {
            Ok(_) => {} // unlikely without API key, but acceptable
            Err(e) => {
                let msg = e.to_string();
                assert!(
                    !msg.contains("transport not configured"),
                    "live pipeline should not use stub transport, but got: {msg}"
                );
            }
        }
    }

    // ── Integration: enable_live_llm replaces the pipeline ──────────

    #[tokio::test]
    async fn app_context_enable_live_llm() {
        let platform = Arc::new(NativePlatform::new());
        let mut ctx = AppContext::new(test_config(), platform).await.unwrap();

        // Before enable_live_llm, the default pipeline uses a stub transport.
        // We cannot directly access ctx.pipeline, but we can verify behavior
        // indirectly: enable_live_llm should not panic and should replace
        // the pipeline such that transport errors no longer say "not configured".

        ctx.enable_live_llm();

        // Convert to agent loop and verify it's still valid.
        let agent = ctx.into_agent_loop();
        assert_eq!(agent.config().defaults.model, "deepseek/deepseek-chat");
    }

    // ── Integration: default pipeline uses stub (negative test) ─────

    #[test]
    fn default_pipeline_builds_with_service_llm_transport() {
        // The default pipeline now wires a ServiceLlmAdapter over an
        // LlmClient resolved from LlmConfig::from_env (instead of the
        // earlier stubbed transport). We can't fire a request from a
        // unit test — that would hit the network (or wait for a
        // timeout) depending on whether llama-server happens to be
        // running on the test host. Just confirm the build path
        // succeeds; the wiremock-backed round-trip in
        // `pipeline::service_llm_adapter::tests` covers the actual
        // request path end-to-end.
        use crate::pipeline::traits::TaskType;

        let config = test_config();
        let registry = build_default_pipeline(&config);
        // Smoke-check: pipeline is reachable for the default task type.
        let _pipeline = registry.get(&TaskType::Unknown);
    }

    // ── Integration: build_live_pipeline creates a valid registry ────

    #[test]
    fn build_live_pipeline_creates_registry() {
        let config = test_config();
        let registry = build_live_pipeline(&config);
        // Registry should be usable for any task type (falls back to default).
        use crate::pipeline::traits::TaskType;
        let _pipeline = registry.get(&TaskType::Chat);
        let _pipeline = registry.get(&TaskType::CodeGeneration);
        let _pipeline = registry.get(&TaskType::Unknown);
    }
}
