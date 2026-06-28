//! Plugin trait definitions.
//!
//! Defines the 6 core plugin traits plus supporting traits:
//! - [`Tool`] -- tool execution interface
//! - [`ChannelAdapter`] -- channel message handling
//! - [`PipelineStage`] -- pipeline processing stage
//! - [`Skill`] -- skill definition with tool list
//! - [`MemoryBackend`] -- memory storage interface
//! - [`VoiceHandler`] -- forward-compat placeholder; no impl in 0.7.x (Workstream G)
//! - [`KeyValueStore`] -- key-value storage for plugins
//! - [`ToolContext`] -- execution context for tools
//! - [`ChannelAdapterHost`] -- host services for channel adapters
//!
//! All traits are `Send + Sync`. Async methods use `#[async_trait]`.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

#[cfg(feature = "native")]
pub use tokio_util::sync::CancellationToken;

#[cfg(not(feature = "native"))]
mod cancellation {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// A lightweight cancellation token for non-native (WASM) targets.
    ///
    /// On native targets, this is re-exported from `tokio_util`.
    /// On WASM, we provide a minimal `Arc<AtomicBool>` implementation
    /// that supports `cancel()` and `is_cancelled()`.
    #[derive(Clone)]
    pub struct CancellationToken {
        cancelled: Arc<AtomicBool>,
    }

    impl CancellationToken {
        /// Create a new token that is not yet cancelled.
        pub fn new() -> Self {
            Self {
                cancelled: Arc::new(AtomicBool::new(false)),
            }
        }

        /// Signal cancellation.
        pub fn cancel(&self) {
            self.cancelled.store(true, Ordering::SeqCst);
        }

        /// Check whether the token has been cancelled.
        pub fn is_cancelled(&self) -> bool {
            self.cancelled.load(Ordering::SeqCst)
        }
    }

    impl Default for CancellationToken {
        fn default() -> Self {
            Self::new()
        }
    }
}

#[cfg(not(feature = "native"))]
pub use cancellation::CancellationToken;

use crate::error::PluginError;
use crate::message::MessagePayload;

// ---------------------------------------------------------------------------
// Tool
// ---------------------------------------------------------------------------

/// A tool that can be invoked by an agent or exposed via MCP.
///
/// Tools are the primary extension point for adding new capabilities.
/// Each tool declares its name, description, and a JSON Schema for
/// its parameters. The host routes `execute()` calls based on the
/// tool name.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Unique tool name (e.g., `"web_search"`, `"file_read"`).
    fn name(&self) -> &str;

    /// Human-readable description of what the tool does.
    fn description(&self) -> &str;

    /// JSON Schema describing the tool's parameters.
    ///
    /// Returns a `serde_json::Value` representing a JSON Schema object.
    /// The host uses this schema for validation and for MCP `tools/list`.
    fn parameters_schema(&self) -> serde_json::Value;

    /// Execute the tool with the given parameters and context.
    ///
    /// `params` is a JSON object matching `parameters_schema()`.
    /// Returns a JSON value with the tool's result, or a `PluginError`.
    async fn execute(
        &self,
        params: serde_json::Value,
        ctx: &dyn ToolContext,
    ) -> Result<serde_json::Value, PluginError>;
}

// ---------------------------------------------------------------------------
// ChannelAdapter
// ---------------------------------------------------------------------------

/// A channel adapter for connecting to external messaging platforms.
///
/// Replaces the existing `Channel` trait with a plugin-oriented design
/// that supports [`MessagePayload`] variants for text, structured, and
/// binary (voice) content.
#[async_trait]
pub trait ChannelAdapter: Send + Sync {
    /// Unique channel identifier (e.g., `"telegram"`, `"slack"`).
    fn name(&self) -> &str;

    /// Human-readable display name.
    fn display_name(&self) -> &str;

    /// Whether this adapter supports threaded conversations.
    fn supports_threads(&self) -> bool;

    /// Whether this adapter supports media/binary payloads.
    fn supports_media(&self) -> bool;

    /// Start receiving messages. Long-lived -- runs until cancelled.
    async fn start(
        &self,
        host: Arc<dyn ChannelAdapterHost>,
        cancel: CancellationToken,
    ) -> Result<(), PluginError>;

    /// Send a message payload through this channel.
    ///
    /// Returns a message ID on success.
    async fn send(&self, target: &str, payload: &MessagePayload) -> Result<String, PluginError>;
}

/// Host services exposed to channel adapters.
#[async_trait]
pub trait ChannelAdapterHost: Send + Sync {
    /// Deliver an inbound message payload to the agent pipeline.
    async fn deliver_inbound(
        &self,
        channel: &str,
        sender_id: &str,
        chat_id: &str,
        payload: MessagePayload,
        metadata: HashMap<String, serde_json::Value>,
    ) -> Result<(), PluginError>;
}

// ---------------------------------------------------------------------------
// PipelineStage
// ---------------------------------------------------------------------------

/// Types of pipeline stages.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PipelineStageType {
    /// Pre-processing (input validation, normalization).
    PreProcess,
    /// Core processing (LLM calls, tool routing).
    Process,
    /// Post-processing (response formatting, filtering).
    PostProcess,
    /// Observation / logging (read-only tap on the pipeline).
    Observer,
}

/// A stage in the agent processing pipeline.
///
/// Pipeline stages are composed in order: PreProcess -> Process ->
/// PostProcess, with Observers receiving copies at each step.
#[async_trait]
pub trait PipelineStage: Send + Sync {
    /// Stage name (e.g., `"assembler"`, `"tool_router"`).
    fn name(&self) -> &str;

    /// What type of stage this is.
    fn stage_type(&self) -> PipelineStageType;

    /// Process input and return output.
    ///
    /// `input` is a JSON value representing the current pipeline state.
    /// Returns the transformed pipeline state.
    async fn process(&self, input: serde_json::Value) -> Result<serde_json::Value, PluginError>;
}

// ---------------------------------------------------------------------------
// Skill
// ---------------------------------------------------------------------------

/// A skill is a high-level agent capability composed of tools,
/// instructions, and configuration.
///
/// Skills are the primary unit of agent customization. They can be
/// loaded from SKILL.md files, bundled with plugins, or
/// auto-generated. Skills can contribute tools that appear in MCP
/// `tools/list` and can be invoked via slash commands.
#[async_trait]
pub trait Skill: Send + Sync {
    /// Skill name (e.g., `"code-review"`, `"git-commit"`).
    fn name(&self) -> &str;

    /// Human-readable description.
    fn description(&self) -> &str;

    /// Semantic version string.
    fn version(&self) -> &str;

    /// Template variables the skill accepts (name -> description).
    fn variables(&self) -> HashMap<String, String>;

    /// Tool names this skill is allowed to invoke.
    fn allowed_tools(&self) -> Vec<String>;

    /// System instructions injected when the skill is active.
    fn instructions(&self) -> &str;

    /// Whether this skill can be invoked directly by users (e.g., via /command).
    fn is_user_invocable(&self) -> bool;

    /// Execute a tool provided by this skill.
    ///
    /// `tool_name` is the specific tool within this skill to call.
    /// `params` is a JSON object of tool parameters.
    /// `ctx` is the execution context providing key-value store access.
    async fn execute_tool(
        &self,
        tool_name: &str,
        params: serde_json::Value,
        ctx: &dyn ToolContext,
    ) -> Result<serde_json::Value, PluginError>;
}

// ---------------------------------------------------------------------------
// MemoryBackend
// ---------------------------------------------------------------------------

/// A pluggable memory storage backend.
///
/// Supports key-value storage with optional namespace isolation,
/// TTL, tags, and semantic search. Implementations may use
/// in-memory stores, SQLite, HNSW indices, or external services.
#[async_trait]
pub trait MemoryBackend: Send + Sync {
    /// Store a value with optional metadata.
    async fn store(
        &self,
        key: &str,
        value: &str,
        namespace: Option<&str>,
        ttl_seconds: Option<u64>,
        tags: Option<Vec<String>>,
    ) -> Result<(), PluginError>;

    /// Retrieve a value by key.
    async fn retrieve(
        &self,
        key: &str,
        namespace: Option<&str>,
    ) -> Result<Option<String>, PluginError>;

    /// Search for values matching a query string.
    ///
    /// Returns a list of `(key, value, relevance_score)` tuples.
    async fn search(
        &self,
        query: &str,
        namespace: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<(String, String, f64)>, PluginError>;

    /// Delete a value by key. Returns `true` if the key existed.
    async fn delete(&self, key: &str, namespace: Option<&str>) -> Result<bool, PluginError>;
}

// ---------------------------------------------------------------------------
// VoiceHandler (placeholder for Workstream G)
// ---------------------------------------------------------------------------

/// **Forward-compat placeholder for voice/audio processing (Workstream G).**
///
/// > **Status (0.7.x):** Reserved API surface only — no production
/// > implementations are shipped, no plugin loader path exercises this
/// > trait, and no end-to-end audio pipeline is wired through it.
/// > Treat any concrete impl you build against it as experimental.
///
/// The trait exists so the [`PluginCapability::Voice`] capability type
/// and the [`VoiceCapability`] manifest grants have a stable shape for
/// downstream crates to depend on. The full voice pipeline (VAD, STT,
/// TTS, wake-word) lands in Workstream G; that work will fill in real
/// implementations and may extend (additively) the method set on this
/// trait.
///
/// The `voice` feature umbrella (and the `voice-vad` / `voice-stt` /
/// `voice-tts` / `voice-wake` granular flags) gate the heavy
/// dependencies; the trait itself is always compiled so `dyn
/// VoiceHandler` references remain valid across feature combinations.
///
/// Decision (release-gate WEFT-77): keep `pub` for forward-compat with a
/// banner doc comment. Do not `#[doc(hidden)]` — external integrators
/// reading the public surface should see this trait *and* be told
/// plainly that it is not load-bearing yet.
///
/// [`PluginCapability::Voice`]: crate::manifest::PluginCapability::Voice
/// [`VoiceCapability`]: crate::manifest::VoiceCapability
#[async_trait]
pub trait VoiceHandler: Send + Sync {
    /// Process raw audio input and return a transcription or response.
    ///
    /// `audio_data` is raw audio bytes. `mime_type` indicates the format
    /// (e.g., `"audio/wav"`, `"audio/opus"`).
    async fn process_audio(
        &self,
        audio_data: &[u8],
        mime_type: &str,
    ) -> Result<String, PluginError>;

    /// Synthesize text into audio output.
    ///
    /// Returns audio bytes and the MIME type of the output format.
    async fn synthesize(&self, text: &str) -> Result<(Vec<u8>, String), PluginError>;
}

// ---------------------------------------------------------------------------
// KeyValueStore
// ---------------------------------------------------------------------------

/// Key-value store interface exposed to plugins via [`ToolContext`].
///
/// This is the cross-element contract defined in the integration spec.
/// Implementations may be backed by in-memory maps, SQLite, or the
/// agent's memory system.
#[async_trait]
pub trait KeyValueStore: Send + Sync {
    /// Get a value by key. Returns `None` if not found.
    async fn get(&self, key: &str) -> Result<Option<String>, PluginError>;

    /// Set a value for a key.
    async fn set(&self, key: &str, value: &str) -> Result<(), PluginError>;

    /// Delete a key. Returns `true` if the key existed.
    async fn delete(&self, key: &str) -> Result<bool, PluginError>;

    /// List all keys with an optional prefix filter.
    async fn list_keys(&self, prefix: Option<&str>) -> Result<Vec<String>, PluginError>;
}

// ---------------------------------------------------------------------------
// ToolContext
// ---------------------------------------------------------------------------

/// Execution context passed to [`Tool::execute()`] and [`Skill::execute_tool()`].
///
/// Provides access to the key-value store, plugin identity, and
/// agent identity. This is the plugin's window into the host.
pub trait ToolContext: Send + Sync {
    /// Access the key-value store for plugin state.
    fn key_value_store(&self) -> &dyn KeyValueStore;

    /// The ID of the plugin that owns this tool.
    fn plugin_id(&self) -> &str;

    /// The ID of the agent invoking this tool.
    fn agent_id(&self) -> &str;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Compile-time assertion that a type is Send + Sync.
    fn assert_send_sync<T: Send + Sync + ?Sized>() {}

    #[test]
    fn test_traits_are_send_sync() {
        // All 6 plugin traits as trait objects
        assert_send_sync::<dyn Tool>();
        assert_send_sync::<dyn ChannelAdapter>();
        assert_send_sync::<dyn PipelineStage>();
        assert_send_sync::<dyn Skill>();
        assert_send_sync::<dyn MemoryBackend>();
        assert_send_sync::<dyn VoiceHandler>();

        // Supporting traits
        assert_send_sync::<dyn KeyValueStore>();
        assert_send_sync::<dyn ToolContext>();
        assert_send_sync::<dyn ChannelAdapterHost>();
    }

    #[test]
    fn test_pipeline_stage_type_serde_roundtrip() {
        let types = vec![
            PipelineStageType::PreProcess,
            PipelineStageType::Process,
            PipelineStageType::PostProcess,
            PipelineStageType::Observer,
        ];
        for t in &types {
            let json = serde_json::to_string(t).unwrap();
            let restored: PipelineStageType = serde_json::from_str(&json).unwrap();
            assert_eq!(&restored, t);
        }
    }

    #[test]
    fn test_pipeline_stage_type_json_values() {
        assert_eq!(
            serde_json::to_string(&PipelineStageType::PreProcess).unwrap(),
            "\"pre_process\""
        );
        assert_eq!(
            serde_json::to_string(&PipelineStageType::Process).unwrap(),
            "\"process\""
        );
        assert_eq!(
            serde_json::to_string(&PipelineStageType::PostProcess).unwrap(),
            "\"post_process\""
        );
        assert_eq!(
            serde_json::to_string(&PipelineStageType::Observer).unwrap(),
            "\"observer\""
        );
    }

    // -----------------------------------------------------------------------
    // Mock implementations to verify trait usability
    // -----------------------------------------------------------------------

    struct MockKvStore;

    #[async_trait]
    impl KeyValueStore for MockKvStore {
        async fn get(&self, _key: &str) -> Result<Option<String>, PluginError> {
            Ok(None)
        }
        async fn set(&self, _key: &str, _value: &str) -> Result<(), PluginError> {
            Ok(())
        }
        async fn delete(&self, _key: &str) -> Result<bool, PluginError> {
            Ok(false)
        }
        async fn list_keys(&self, _prefix: Option<&str>) -> Result<Vec<String>, PluginError> {
            Ok(vec![])
        }
    }

    struct MockToolContext;

    impl ToolContext for MockToolContext {
        fn key_value_store(&self) -> &dyn KeyValueStore {
            &MockKvStore
        }
        fn plugin_id(&self) -> &str {
            "mock-plugin"
        }
        fn agent_id(&self) -> &str {
            "mock-agent"
        }
    }

    struct MockTool;

    #[async_trait]
    impl Tool for MockTool {
        fn name(&self) -> &str {
            "mock_tool"
        }
        fn description(&self) -> &str {
            "A mock tool for testing"
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "input": { "type": "string" }
                }
            })
        }
        async fn execute(
            &self,
            params: serde_json::Value,
            _ctx: &dyn ToolContext,
        ) -> Result<serde_json::Value, PluginError> {
            Ok(serde_json::json!({
                "result": format!("processed: {}", params)
            }))
        }
    }

    struct MockChannelAdapter;

    #[async_trait]
    impl ChannelAdapter for MockChannelAdapter {
        fn name(&self) -> &str {
            "mock"
        }
        fn display_name(&self) -> &str {
            "Mock Channel"
        }
        fn supports_threads(&self) -> bool {
            false
        }
        fn supports_media(&self) -> bool {
            true
        }
        async fn start(
            &self,
            _host: Arc<dyn ChannelAdapterHost>,
            cancel: CancellationToken,
        ) -> Result<(), PluginError> {
            cancel.cancelled().await;
            Ok(())
        }
        async fn send(
            &self,
            _target: &str,
            _payload: &MessagePayload,
        ) -> Result<String, PluginError> {
            Ok("msg-001".into())
        }
    }

    struct MockPipelineStage;

    #[async_trait]
    impl PipelineStage for MockPipelineStage {
        fn name(&self) -> &str {
            "mock_stage"
        }
        fn stage_type(&self) -> PipelineStageType {
            PipelineStageType::PreProcess
        }
        async fn process(
            &self,
            input: serde_json::Value,
        ) -> Result<serde_json::Value, PluginError> {
            Ok(input)
        }
    }

    struct MockSkill;

    #[async_trait]
    impl Skill for MockSkill {
        fn name(&self) -> &str {
            "mock-skill"
        }
        fn description(&self) -> &str {
            "A mock skill"
        }
        fn version(&self) -> &str {
            "1.0.0"
        }
        fn variables(&self) -> HashMap<String, String> {
            HashMap::new()
        }
        fn allowed_tools(&self) -> Vec<String> {
            vec!["mock_tool".into()]
        }
        fn instructions(&self) -> &str {
            "Do mock things."
        }
        fn is_user_invocable(&self) -> bool {
            true
        }
        async fn execute_tool(
            &self,
            tool_name: &str,
            _params: serde_json::Value,
            _ctx: &dyn ToolContext,
        ) -> Result<serde_json::Value, PluginError> {
            Ok(serde_json::json!({ "tool": tool_name, "status": "ok" }))
        }
    }

    struct MockMemoryBackend;

    #[async_trait]
    impl MemoryBackend for MockMemoryBackend {
        async fn store(
            &self,
            _key: &str,
            _value: &str,
            _namespace: Option<&str>,
            _ttl_seconds: Option<u64>,
            _tags: Option<Vec<String>>,
        ) -> Result<(), PluginError> {
            Ok(())
        }
        async fn retrieve(
            &self,
            _key: &str,
            _namespace: Option<&str>,
        ) -> Result<Option<String>, PluginError> {
            Ok(Some("stored-value".into()))
        }
        async fn search(
            &self,
            _query: &str,
            _namespace: Option<&str>,
            _limit: Option<usize>,
        ) -> Result<Vec<(String, String, f64)>, PluginError> {
            Ok(vec![("key".into(), "value".into(), 0.95)])
        }
        async fn delete(&self, _key: &str, _namespace: Option<&str>) -> Result<bool, PluginError> {
            Ok(true)
        }
    }

    struct MockVoiceHandler;

    #[async_trait]
    impl VoiceHandler for MockVoiceHandler {
        async fn process_audio(
            &self,
            _audio_data: &[u8],
            _mime_type: &str,
        ) -> Result<String, PluginError> {
            Ok("transcribed text".into())
        }
        async fn synthesize(&self, _text: &str) -> Result<(Vec<u8>, String), PluginError> {
            Ok((vec![0u8; 100], "audio/wav".into()))
        }
    }

    #[tokio::test]
    async fn test_tool_trait_implementation() {
        let tool = MockTool;
        let ctx = MockToolContext;
        assert_eq!(tool.name(), "mock_tool");
        assert_eq!(tool.description(), "A mock tool for testing");
        assert!(tool.parameters_schema().is_object());
        let result = tool
            .execute(serde_json::json!({"input": "test"}), &ctx)
            .await
            .unwrap();
        assert!(result["result"].as_str().unwrap().contains("test"));
    }

    #[tokio::test]
    async fn test_channel_adapter_trait_implementation() {
        let adapter = MockChannelAdapter;
        assert_eq!(adapter.name(), "mock");
        assert_eq!(adapter.display_name(), "Mock Channel");
        assert!(!adapter.supports_threads());
        assert!(adapter.supports_media());
        let payload = MessagePayload::text("hello");
        let msg_id = adapter.send("target", &payload).await.unwrap();
        assert_eq!(msg_id, "msg-001");
    }

    #[tokio::test]
    async fn test_pipeline_stage_trait_implementation() {
        let stage = MockPipelineStage;
        assert_eq!(stage.name(), "mock_stage");
        assert_eq!(stage.stage_type(), PipelineStageType::PreProcess);
        let input = serde_json::json!({"data": "test"});
        let output = stage.process(input.clone()).await.unwrap();
        assert_eq!(output, input);
    }

    #[tokio::test]
    async fn test_skill_trait_implementation() {
        let skill = MockSkill;
        let ctx = MockToolContext;
        assert_eq!(skill.name(), "mock-skill");
        assert_eq!(skill.description(), "A mock skill");
        assert_eq!(skill.version(), "1.0.0");
        assert!(skill.variables().is_empty());
        assert_eq!(skill.allowed_tools(), vec!["mock_tool"]);
        assert_eq!(skill.instructions(), "Do mock things.");
        assert!(skill.is_user_invocable());
        let result = skill
            .execute_tool("mock_tool", serde_json::json!({}), &ctx)
            .await
            .unwrap();
        assert_eq!(result["tool"], "mock_tool");
        assert_eq!(result["status"], "ok");
    }

    #[tokio::test]
    async fn test_memory_backend_trait_implementation() {
        let backend = MockMemoryBackend;
        backend
            .store("key", "value", None, None, None)
            .await
            .unwrap();
        let val = backend.retrieve("key", None).await.unwrap();
        assert_eq!(val, Some("stored-value".into()));
        let results = backend.search("query", None, Some(10)).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "key");
        let deleted = backend.delete("key", None).await.unwrap();
        assert!(deleted);
    }

    #[tokio::test]
    async fn test_voice_handler_trait_implementation() {
        let handler = MockVoiceHandler;
        let text = handler
            .process_audio(&[0u8; 100], "audio/wav")
            .await
            .unwrap();
        assert_eq!(text, "transcribed text");
        let (audio, mime) = handler.synthesize("hello").await.unwrap();
        assert!(!audio.is_empty());
        assert_eq!(mime, "audio/wav");
    }

    #[tokio::test]
    async fn test_key_value_store_trait_implementation() {
        let store = MockKvStore;
        let val = store.get("missing").await.unwrap();
        assert!(val.is_none());
        store.set("key", "value").await.unwrap();
        let deleted = store.delete("key").await.unwrap();
        assert!(!deleted); // Mock always returns false
        let keys = store.list_keys(None).await.unwrap();
        assert!(keys.is_empty());
    }

    #[test]
    fn test_tool_context_trait_implementation() {
        let ctx = MockToolContext;
        assert_eq!(ctx.plugin_id(), "mock-plugin");
        assert_eq!(ctx.agent_id(), "mock-agent");
        // key_value_store() returns a reference -- just verify it compiles
        let _kv = ctx.key_value_store();
    }

    #[test]
    fn test_trait_objects_can_be_boxed() {
        // Verify all trait objects can be put behind Box<dyn Trait>
        let _tool: Box<dyn Tool> = Box::new(MockTool);
        let _channel: Box<dyn ChannelAdapter> = Box::new(MockChannelAdapter);
        let _stage: Box<dyn PipelineStage> = Box::new(MockPipelineStage);
        let _skill: Box<dyn Skill> = Box::new(MockSkill);
        let _memory: Box<dyn MemoryBackend> = Box::new(MockMemoryBackend);
        let _voice: Box<dyn VoiceHandler> = Box::new(MockVoiceHandler);
        let _kv: Box<dyn KeyValueStore> = Box::new(MockKvStore);
        let _ctx: Box<dyn ToolContext> = Box::new(MockToolContext);
    }

    #[test]
    fn test_trait_objects_can_be_arced() {
        // Verify all trait objects can be put behind Arc<dyn Trait>
        let _tool: Arc<dyn Tool> = Arc::new(MockTool);
        let _channel: Arc<dyn ChannelAdapter> = Arc::new(MockChannelAdapter);
        let _stage: Arc<dyn PipelineStage> = Arc::new(MockPipelineStage);
        let _skill: Arc<dyn Skill> = Arc::new(MockSkill);
        let _memory: Arc<dyn MemoryBackend> = Arc::new(MockMemoryBackend);
        let _voice: Arc<dyn VoiceHandler> = Arc::new(MockVoiceHandler);
        let _kv: Arc<dyn KeyValueStore> = Arc::new(MockKvStore);
    }
}
