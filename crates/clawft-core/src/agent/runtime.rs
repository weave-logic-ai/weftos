//! Per-agent runtime isolation (WEFT-184).
//!
//! Each [`AgentRuntime`] bundles the four "agent-scoped" handles that
//! must NOT be shared across agents in a multi-agent dispatch:
//!
//! * [`SessionManager`] -- a per-agent sessions directory so chat
//!   history from agent A never bleeds into agent B's context window.
//! * [`ContextBuilder`] -- the assembler reads a [`MemoryStore`] +
//!   [`SkillsLoader`] that may be agent-scoped (e.g. a per-agent
//!   workspace + skills set under `~/.clawft/agents/<id>/`).
//! * [`ToolRegistry`] -- per-agent tool surface so an agent can be
//!   restricted (e.g. anonymous agents) without touching the global
//!   registry.
//! * [`AgentsConfig`] -- per-agent defaults (model, max_tokens,
//!   workspace path) so each agent can run a different LLM tier.
//!
//! The pre-WEFT-184 daemon path constructed one of each at boot and
//! shared them across every agent loop. That meant routed agents
//! observed each other's session histories and could call any tool the
//! global registry exposed. AgentRuntime gives the dispatcher a single
//! handle to hand to a freshly-spawned [`AgentLoop`](super::loop_core::AgentLoop)
//! so isolation is the default rather than an afterthought.
//!
//! # Construction
//!
//! Two constructors:
//!
//! 1. [`AgentRuntime::for_agent`] is the production path. It accepts an
//!    `agent_id`, a workspace root, the platform, and the existing
//!    `MemoryStore` / `SkillsLoader` (these can stay shared since they
//!    read from agent-scoped paths). It builds a sessions directory
//!    under `<workspace>/sessions/<agent_id>/` and an empty
//!    `ToolRegistry` ready for the dispatcher to register tools into.
//!
//! 2. [`AgentRuntime::with_components`] is the test / advanced-wiring
//!    path. It accepts an already-constructed `SessionManager`,
//!    `ContextBuilder`, `ToolRegistry`, and `AgentsConfig`. Used by
//!    the per-agent isolation test below + by callers that have
//!    bespoke wiring (e.g. fully-mocked test platforms).
//!
//! Both paths return a fully-formed runtime that the caller can hand
//! to the dispatcher.

use std::path::Path;
use std::sync::Arc;

use clawft_platform::Platform;
use clawft_types::config::AgentsConfig;

use super::context::ContextBuilder;
use super::memory::MemoryStore;
use super::skills::SkillsLoader;
use crate::session::SessionManager;
use crate::tools::registry::ToolRegistry;

/// Bundle of per-agent state.
///
/// One [`AgentRuntime`] per spawned agent. Cloning is cheap (every
/// inner component is `Arc`-wrapped) so the dispatcher can hand off
/// the runtime to whatever spawns the agent loop.
pub struct AgentRuntime<P: Platform> {
    /// Stable identifier (matches the routed `agent_id` from
    /// [`AgentRouter`](crate::agent_routing::AgentRouter)).
    agent_id: String,

    /// Per-agent sessions. The sessions directory is namespaced by
    /// `agent_id` so two agents running concurrently never share
    /// chat history.
    sessions: Arc<SessionManager<P>>,

    /// Per-agent context builder (system prompt + memory + skills).
    context: ContextBuilder<P>,

    /// Per-agent tool registry. Starts empty; the dispatcher registers
    /// the agent's allowed tools before handing the runtime to the
    /// loop.
    tools: Arc<ToolRegistry>,

    /// Per-agent defaults (model, max_tokens, workspace path).
    config: AgentsConfig,
}

impl<P: Platform> AgentRuntime<P> {
    /// Build an agent runtime against an explicit workspace root.
    ///
    /// Sessions land under `<workspace>/sessions/<agent_id>/`. The
    /// caller still owns the platform's [`MemoryStore`] and
    /// [`SkillsLoader`] — those stay shared across runtimes today
    /// (per-agent memory/skills isolation is a 0.8.x concern).
    ///
    /// The returned runtime has an empty [`ToolRegistry`]; callers
    /// register tools before constructing the agent loop.
    pub fn for_agent(
        agent_id: impl Into<String>,
        workspace: &Path,
        platform: Arc<P>,
        memory: Arc<MemoryStore<P>>,
        skills: Arc<SkillsLoader<P>>,
        config: AgentsConfig,
    ) -> Self {
        let agent_id = agent_id.into();
        let sessions_dir = workspace.join("sessions").join(&agent_id);
        let sessions = Arc::new(SessionManager::with_dir(
            platform.clone(),
            sessions_dir,
        ));
        let context = ContextBuilder::new(
            config.clone(),
            memory,
            skills,
            platform,
        );
        let tools = Arc::new(ToolRegistry::new());
        Self {
            agent_id,
            sessions,
            context,
            tools,
            config,
        }
    }

    /// Build a runtime from already-constructed components.
    ///
    /// Used by tests and by callers with bespoke wiring (e.g. when
    /// the [`SessionManager`] is constructed against a custom
    /// platform). Bypasses the workspace-derived defaults that
    /// [`Self::for_agent`] uses.
    pub fn with_components(
        agent_id: impl Into<String>,
        sessions: Arc<SessionManager<P>>,
        context: ContextBuilder<P>,
        tools: Arc<ToolRegistry>,
        config: AgentsConfig,
    ) -> Self {
        Self {
            agent_id: agent_id.into(),
            sessions,
            context,
            tools,
            config,
        }
    }

    /// The runtime's stable identifier.
    pub fn agent_id(&self) -> &str {
        &self.agent_id
    }

    /// Per-agent session manager.
    pub fn sessions(&self) -> &Arc<SessionManager<P>> {
        &self.sessions
    }

    /// Per-agent context builder.
    pub fn context(&self) -> &ContextBuilder<P> {
        &self.context
    }

    /// Mutable handle to the per-agent context builder (consumes
    /// `self` for ownership transfer into an agent loop).
    pub fn into_context(self) -> ContextBuilder<P> {
        self.context
    }

    /// Per-agent tool registry. Returns the `Arc` for direct sharing
    /// into an agent loop.
    pub fn tools(&self) -> &Arc<ToolRegistry> {
        &self.tools
    }

    /// Mutable handle to the per-agent tool registry. Panics if any
    /// other clones of the inner `Arc` exist; call BEFORE constructing
    /// dependent components.
    pub fn tools_mut(&mut self) -> &mut ToolRegistry {
        Arc::get_mut(&mut self.tools)
            .expect("AgentRuntime::tools_mut: registry already shared")
    }

    /// Per-agent agents config (model, max_tokens, workspace).
    pub fn config(&self) -> &AgentsConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clawft_platform::NativePlatform;
    use clawft_types::config::{AgentDefaults, AgentsConfig};

    fn test_config(agent_id: &str) -> AgentsConfig {
        AgentsConfig {
            defaults: AgentDefaults {
                workspace: format!("/tmp/clawft-runtime-test/{agent_id}"),
                model: "test-model".into(),
                max_tokens: 4096,
                temperature: 0.5,
                max_tool_iterations: 10,
                memory_window: 50,
            },
        }
    }

    /// Two AgentRuntime instances built with separate sessions
    /// directories never observe each other's chat history.
    ///
    /// This is the headline guarantee the audit flagged as missing
    /// (WEFT-184): "two agents running concurrently see separate
    /// session histories."
    #[tokio::test]
    async fn separate_runtimes_have_isolated_sessions() {
        let platform = Arc::new(NativePlatform::new());
        let memory = Arc::new(MemoryStore::new(platform.clone()).unwrap());
        let skills = Arc::new(SkillsLoader::new(platform.clone()).unwrap());

        // Use a unique tmp root so the test is hermetic.
        let pid = std::process::id();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let root = std::env::temp_dir()
            .join(format!("clawft-runtime-iso-{pid}-{nanos}"));
        let ws_a = root.join("agent-a");
        let ws_b = root.join("agent-b");
        std::fs::create_dir_all(ws_a.join("sessions").join("agent-a")).unwrap();
        std::fs::create_dir_all(ws_b.join("sessions").join("agent-b")).unwrap();

        let rt_a = AgentRuntime::for_agent(
            "agent-a",
            &ws_a,
            platform.clone(),
            memory.clone(),
            skills.clone(),
            test_config("agent-a"),
        );
        let rt_b = AgentRuntime::for_agent(
            "agent-b",
            &ws_b,
            platform.clone(),
            memory.clone(),
            skills.clone(),
            test_config("agent-b"),
        );

        // Each runtime has its OWN sessions instance and tool registry.
        assert_eq!(rt_a.agent_id(), "agent-a");
        assert_eq!(rt_b.agent_id(), "agent-b");
        assert!(!Arc::ptr_eq(rt_a.sessions(), rt_b.sessions()));
        assert!(!Arc::ptr_eq(rt_a.tools(), rt_b.tools()));

        // Write a session under agent-a; agent-b never sees it.
        let mut session_a = rt_a
            .sessions()
            .get_or_create("test:chat-a")
            .await
            .unwrap();
        session_a.add_message("user", "hello from a", None);
        rt_a.sessions().save_session(&session_a).await.unwrap();

        let mut session_b = rt_b
            .sessions()
            .get_or_create("test:chat-b")
            .await
            .unwrap();
        session_b.add_message("user", "hello from b", None);
        rt_b.sessions().save_session(&session_b).await.unwrap();

        // agent-b's manager has only its own session (not agent-a's).
        let b_lookup = rt_b.sessions().get_or_create("test:chat-a").await.unwrap();
        // The session_b cache shouldn't have anything from agent-a's run; the
        // freshly-created "test:chat-a" key on rt_b is empty (no message added).
        assert!(
            b_lookup.messages.is_empty(),
            "agent-b session should not contain agent-a's messages"
        );

        // Cleanup
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn with_components_assembles_runtime() {
        let platform = Arc::new(NativePlatform::new());
        let memory = Arc::new(MemoryStore::new(platform.clone()).unwrap());
        let skills = Arc::new(SkillsLoader::new(platform.clone()).unwrap());

        let pid = std::process::id();
        let dir = std::env::temp_dir().join(format!("clawft-rt-wc-{pid}"));
        std::fs::create_dir_all(&dir).unwrap();
        let sessions = Arc::new(SessionManager::with_dir(platform.clone(), dir.clone()));
        let cfg = test_config("custom");
        let context = ContextBuilder::new(
            cfg.clone(),
            memory,
            skills,
            platform,
        );
        let tools = Arc::new(ToolRegistry::new());

        let rt = AgentRuntime::with_components(
            "custom",
            sessions,
            context,
            tools,
            cfg,
        );
        assert_eq!(rt.agent_id(), "custom");
        assert_eq!(rt.config().defaults.model, "test-model");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn tools_mut_allows_registration_before_share() {
        let platform = Arc::new(NativePlatform::new());
        let memory = Arc::new(MemoryStore::new(platform.clone()).unwrap());
        let skills = Arc::new(SkillsLoader::new(platform.clone()).unwrap());

        let pid = std::process::id();
        let ws = std::env::temp_dir().join(format!("clawft-rt-tools-{pid}"));
        std::fs::create_dir_all(ws.join("sessions").join("ag")).unwrap();

        let mut rt = AgentRuntime::for_agent(
            "ag",
            &ws,
            platform,
            memory,
            skills,
            test_config("ag"),
        );

        // Tool registry starts empty.
        assert_eq!(rt.tools().len(), 0);

        // Register a dummy tool via the &mut handle.
        use crate::tools::registry::Tool;
        use async_trait::async_trait;

        struct DummyTool;
        #[async_trait]
        impl Tool for DummyTool {
            fn name(&self) -> &str { "dummy" }
            fn description(&self) -> &str { "test" }
            fn parameters(&self) -> serde_json::Value {
                serde_json::json!({"type": "object"})
            }
            async fn execute(
                &self,
                _args: serde_json::Value,
            ) -> Result<serde_json::Value, crate::tools::registry::ToolError> {
                Ok(serde_json::json!({}))
            }
        }
        rt.tools_mut().register(Arc::new(DummyTool));
        assert_eq!(rt.tools().len(), 1);

        let _ = std::fs::remove_dir_all(&ws);
    }
}
