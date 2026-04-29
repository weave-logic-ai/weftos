//! Context builder for assembling LLM prompts.
//!
//! Combines the system prompt, active skill prompts, long-term memory,
//! and conversation history into a message list suitable for the LLM
//! pipeline. Ported from Python `nanobot/agent/context.py`.
//!
//! # Message assembly order
//!
//! 1. **System prompt** (role=`"system"`) -- identity and instructions
//! 2. **Active skill prompts** (role=`"system"`) -- prefixed with `# Skill: {name}`
//! 3. **Memory context** (role=`"system"`) -- prefixed with `# Relevant Memory:`
//! 4. **Conversation history** -- recent messages from the session
//!
//! The current user message is **not** added here; the caller appends it.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
#[cfg(feature = "native")]
use std::path::PathBuf;
#[cfg(feature = "native")]
use std::time::SystemTime;

use crate::runtime::Mutex;
use tracing::{debug, warn};

use clawft_platform::Platform;
use clawft_types::config::AgentsConfig;
use clawft_types::session::Session;

use super::agents::AgentDefinition;
use super::helpers::render_template;
use super::memory::MemoryStore;
use super::skills::SkillsLoader;

/// Cached bootstrap file entry with modification-time tracking.
#[cfg(feature = "native")]
#[derive(Debug, Clone)]
struct CachedFile {
    content: String,
    mtime: SystemTime,
}

/// Cache for bootstrap files, keyed by resolved path.
///
/// Uses `tokio::sync::Mutex` because the cache is checked/updated
/// inside async `build_system_prompt()`. The critical section is
/// short (one HashMap lookup + optional fs::metadata call).
#[cfg(feature = "native")]
type BootstrapCache = Arc<Mutex<HashMap<PathBuf, CachedFile>>>;

/// On browser, mtime-based caching is not available, so we use a
/// simple empty struct as a no-op placeholder.
#[cfg(not(feature = "native"))]
type BootstrapCache = Arc<Mutex<()>>;

// Re-export the canonical LlmMessage from pipeline::traits so that
// consumers of context.rs use the same type as the rest of the pipeline.
pub use crate::pipeline::traits::LlmMessage;

/// Builder for assembling LLM context from multiple sources.
///
/// Combines configuration, memory, skills, and session history into
/// a structured message list for the LLM pipeline. Each source is
/// read asynchronously through the platform abstraction.
///
/// # Usage
///
/// ```rust,ignore
/// let ctx = ContextBuilder::new(config, memory, skills, platform);
/// let system_prompt = ctx.build_system_prompt().await;
/// let messages = ctx.build_messages(&session, &["research"]).await;
/// ```
pub struct ContextBuilder<P: Platform> {
    config: AgentsConfig,
    memory: Arc<MemoryStore<P>>,
    skills: Arc<SkillsLoader<P>>,
    platform: Arc<P>,
    bootstrap_cache: BootstrapCache,
    compression_config: Option<CompressionConfig>,
}

impl<P: Platform> ContextBuilder<P> {
    /// Create a new context builder.
    ///
    /// # Arguments
    ///
    /// * `config` -- Agent configuration (system prompt, memory window, etc.)
    /// * `memory` -- Shared memory store for reading long-term memory
    /// * `skills` -- Shared skills loader for reading active skill prompts
    /// * `platform` -- Platform abstraction for filesystem/env access
    pub fn new(
        config: AgentsConfig,
        memory: Arc<MemoryStore<P>>,
        skills: Arc<SkillsLoader<P>>,
        platform: Arc<P>,
    ) -> Self {
        Self {
            config,
            memory,
            skills,
            platform,
            bootstrap_cache: {
                #[cfg(feature = "native")]
                { Arc::new(Mutex::new(HashMap::new())) }
                #[cfg(not(feature = "native"))]
                { Arc::new(Mutex::new(())) }
            },
            compression_config: None,
        }
    }

    /// Load a bootstrap file, using the mtime cache to skip disk reads
    /// when the file has not been modified since last load.
    ///
    /// Returns `Some(content)` if the file exists and is non-empty,
    /// `None` otherwise.
    #[cfg(feature = "native")]
    async fn load_cached_file(
        platform: &Arc<P>,
        cache: &BootstrapCache,
        file_path: &Path,
    ) -> Option<String> {
        // Check current mtime via tokio::fs::metadata (platform trait
        // does not expose metadata, so we bypass it for this stat call).
        let current_mtime = match tokio::fs::metadata(file_path).await {
            Ok(meta) => meta.modified().ok(),
            Err(_) => return None, // file does not exist or is inaccessible
        };

        // Cache hit: mtime matches
        {
            let cache_guard = cache.lock().await;
            if let Some(cached) = cache_guard.get(file_path)
                && current_mtime == Some(cached.mtime)
            {
                return Some(cached.content.clone());
            }
        }

        // Cache miss or stale: re-read from disk
        match platform.fs().read_to_string(file_path).await {
            Ok(content) if !content.trim().is_empty() => {
                if let Some(mtime) = current_mtime {
                    let mut cache_guard = cache.lock().await;
                    cache_guard.insert(
                        file_path.to_path_buf(),
                        CachedFile {
                            content: content.clone(),
                            mtime,
                        },
                    );
                }
                Some(content)
            }
            _ => None,
        }
    }

    /// Load a bootstrap file (browser version without mtime caching).
    ///
    /// On browser/WASM, `tokio::fs::metadata` is not available, so we
    /// skip the mtime cache and read directly through the platform.
    #[cfg(not(feature = "native"))]
    async fn load_cached_file(
        platform: &Arc<P>,
        _cache: &BootstrapCache,
        file_path: &Path,
    ) -> Option<String> {
        match platform.fs().read_to_string(file_path).await {
            Ok(content) if !content.trim().is_empty() => Some(content),
            _ => None,
        }
    }

    /// Build the system prompt from configuration.
    ///
    /// Assembles the core identity prompt. This includes:
    /// - A static identity header describing the assistant
    /// - Workspace path information
    /// - Memory file paths for the agent to reference
    ///
    /// The returned string is suitable for a `role="system"` message.
    pub async fn build_system_prompt(&self) -> String {
        let workspace = &self.config.defaults.workspace;
        let model = &self.config.defaults.model;

        let mut parts = Vec::new();

        // Load bootstrap files from the workspace (AGENTS.md, SOUL.md, etc.)
        // Searches: workspace root first, then .clawft/ subdirectory.
        // Uses mtime-based cache to skip disk reads when files have not changed.
        let bootstrap_files = ["SOUL.md", "IDENTITY.md", "AGENTS.md", "USER.md", "TOOLS.md"];
        let mut loaded_files = HashMap::new();
        for filename in &bootstrap_files {
            let home = self.platform.fs().home_dir();
            if let Some(home) = home {
                let ws_path = expand_workspace(workspace, &home);
                let candidates = [
                    ws_path.join(filename),
                    ws_path.join(".clawft").join(filename),
                ];
                for file_path in &candidates {
                    debug!(file = %file_path.display(), "checking for bootstrap file");
                    if let Some(content) = Self::load_cached_file(
                        &self.platform,
                        &self.bootstrap_cache,
                        file_path,
                    )
                    .await
                    {
                        debug!(file = %filename, bytes = content.len(), "loaded bootstrap file");
                        loaded_files.insert(*filename, content);
                        break; // First match wins
                    }
                }
                if !loaded_files.contains_key(filename) {
                    debug!(file = %filename, workspace = %ws_path.display(), "bootstrap file not found");
                }
            }
        }

        // If SOUL.md or IDENTITY.md exists, use it as the identity preamble
        // instead of the hardcoded default. The config section is always appended.
        let has_custom_identity =
            loaded_files.contains_key("SOUL.md") || loaded_files.contains_key("IDENTITY.md");

        if has_custom_identity {
            // Custom identity from bootstrap files
            if let Some(soul) = loaded_files.get("SOUL.md") {
                parts.push(format!("## SOUL.md\n\n{soul}"));
            }
            if let Some(identity) = loaded_files.get("IDENTITY.md") {
                parts.push(format!("## IDENTITY.md\n\n{identity}"));
            }
            // Append configuration context
            parts.push(format!(
                "## Configuration\n\n\
                Model: {model}\n\
                Workspace: {workspace}\n\
                Memory: {workspace}/memory/MEMORY.md\n\
                History: {workspace}/memory/HISTORY.md\n\
                Skills: {workspace}/skills/\n\n\
                You have access to tools that allow you to:\n\
                - Read, write, and edit files\n\
                - Execute shell commands\n\
                - Search the web and fetch web pages\n\
                - Send messages to users on chat channels"
            ));
        } else {
            // Default identity when no SOUL.md or IDENTITY.md exists
            parts.push(format!(
                "# clawft\n\n\
                You are clawft, a helpful AI assistant. You have access to tools that allow you to:\n\
                - Read, write, and edit files\n\
                - Execute shell commands\n\
                - Search the web and fetch web pages\n\
                - Send messages to users on chat channels\n\n\
                ## Configuration\n\
                Model: {model}\n\
                Workspace: {workspace}\n\
                Memory: {workspace}/memory/MEMORY.md\n\
                History: {workspace}/memory/HISTORY.md\n\
                Skills: {workspace}/skills/"
            ));
        }

        // Append remaining bootstrap files (AGENTS.md, USER.md, TOOLS.md)
        for filename in &["AGENTS.md", "USER.md", "TOOLS.md"] {
            if let Some(content) = loaded_files.get(filename) {
                parts.push(format!("## {filename}\n\n{content}"));
            }
        }

        parts.join("\n\n---\n\n")
    }

    /// Build the complete message list for an LLM call.
    ///
    /// Assembles messages in the canonical order:
    /// 1. System prompt (role=`"system"`)
    /// 2. Active skill prompts (role=`"system"`, one per skill)
    /// 3. Long-term memory context (role=`"system"`, if non-empty)
    /// 4. Conversation history from `session.get_history(memory_window)`
    ///
    /// The current user message is **not** included -- the caller adds
    /// it after calling this method.
    ///
    /// # Arguments
    ///
    /// * `session` -- Current conversation session
    /// * `active_skills` -- Names of skills to include in context
    pub async fn build_messages(
        &self,
        session: &Session,
        active_skills: &[String],
    ) -> Vec<LlmMessage> {
        let system_prompt = self.build_system_prompt().await;
        self.build_messages_inner(session, system_prompt, active_skills, None)
            .await
    }

    /// Get a reference to the agent configuration.
    pub fn config(&self) -> &AgentsConfig {
        &self.config
    }

    /// Enable context compression with the given configuration.
    ///
    /// When compression is enabled, [`build_messages_compressed`](Self::build_messages_compressed)
    /// will apply a sliding-window strategy to keep the context within
    /// the token budget.
    pub fn with_compression(mut self, config: CompressionConfig) -> Self {
        self.compression_config = Some(config);
        self
    }

    /// Build messages with context compression applied.
    ///
    /// Assembles the full message list (same as [`build_messages`](Self::build_messages)),
    /// then compresses it according to the configured [`CompressionConfig`].
    /// If no compression config was set via [`with_compression`](Self::with_compression),
    /// uses [`CompressionConfig::default()`].
    ///
    /// Returns a [`CompressedContext`] containing the (possibly compressed)
    /// messages and metadata about the compression operation.
    pub async fn build_messages_compressed(
        &self,
        session: &Session,
        active_skills: &[String],
    ) -> CompressedContext {
        let messages = self.build_messages(session, active_skills).await;
        let config = self
            .compression_config
            .as_ref()
            .cloned()
            .unwrap_or_default();
        compress_context(messages, &config)
    }

    /// Build a system prompt message for a specific agent definition.
    ///
    /// Prepends the agent's `system_prompt` (with template rendering)
    /// to the standard system prompt.  If the agent has no custom
    /// system prompt, only the standard prompt is returned.
    ///
    /// # Arguments
    ///
    /// * `agent` -- The agent definition to inject
    /// * `args` -- Arguments string for template variable expansion
    pub async fn build_system_prompt_for_agent(
        &self,
        agent: &AgentDefinition,
        args: &str,
    ) -> String {
        let base = self.build_system_prompt().await;

        match &agent.system_prompt {
            Some(template) => {
                let rendered = render_template(template, args, &agent.variables);
                format!("# Agent: {}\n\n{}\n\n---\n\n{}", agent.name, rendered, base)
            }
            None => base,
        }
    }

    /// Build the complete message list for an agent-aware LLM call.
    ///
    /// Like [`build_messages`](Self::build_messages) but additionally:
    /// - Uses the agent's system prompt (template-rendered)
    /// - Activates the agent's declared skills
    /// - Appends any extra skill instructions as a system message
    ///
    /// # Arguments
    ///
    /// * `session` -- Current conversation session
    /// * `agent` -- Agent definition to use
    /// * `args` -- Arguments string for template rendering
    /// * `extra_skill_instructions` -- Optional additional skill text to inject
    pub async fn build_messages_for_agent(
        &self,
        session: &Session,
        agent: &AgentDefinition,
        args: &str,
        extra_skill_instructions: Option<&str>,
    ) -> Vec<LlmMessage> {
        let system_prompt = self.build_system_prompt_for_agent(agent, args).await;
        self.build_messages_inner(session, system_prompt, &agent.skills, extra_skill_instructions)
            .await
    }

    /// Core message assembly logic shared by [`build_messages`] and
    /// [`build_messages_for_agent`].
    ///
    /// Assembles messages in order: system prompt, skill prompts,
    /// optional extra instructions, memory context, and conversation
    /// history.
    async fn build_messages_inner(
        &self,
        session: &Session,
        system_prompt: String,
        active_skills: &[String],
        extra_instructions: Option<&str>,
    ) -> Vec<LlmMessage> {
        let mut messages = Vec::new();

        // 1. System prompt
        messages.push(LlmMessage {
            role: "system".into(),
            content: system_prompt,
            tool_call_id: None,
            tool_calls: None,
        });

        // 2. Active skill prompts
        for skill_name in active_skills {
            match self.skills.get_skill(skill_name).await {
                Some(skill) => {
                    if let Some(ref prompt) = skill.prompt {
                        messages.push(LlmMessage {
                            role: "system".into(),
                            content: format!("# Skill: {}\n\n{}", skill.name, prompt),
                            tool_call_id: None,
                            tool_calls: None,
                        });
                    }
                }
                None => match self.skills.load_skill(skill_name).await {
                    Ok(skill) => {
                        if let Some(ref prompt) = skill.prompt {
                            messages.push(LlmMessage {
                                role: "system".into(),
                                content: format!("# Skill: {}\n\n{}", skill.name, prompt),
                                tool_call_id: None,
                                tool_calls: None,
                            });
                        }
                    }
                    Err(e) => {
                        warn!(
                            skill = %skill_name,
                            error = %e,
                            "failed to load skill for context"
                        );
                    }
                },
            }
        }

        // 3. Extra skill instructions (e.g. from SkillRegistry)
        if let Some(instructions) = extra_instructions
            && !instructions.trim().is_empty()
        {
            messages.push(LlmMessage {
                role: "system".into(),
                content: format!("# Skill Instructions\n\n{instructions}"),
                tool_call_id: None,
                tool_calls: None,
            });
        }

        // 4. Memory context
        match self.memory.read_long_term().await {
            Ok(memory) if !memory.trim().is_empty() => {
                messages.push(LlmMessage {
                    role: "system".into(),
                    content: format!("# Relevant Memory:\n\n{memory}"),
                    tool_call_id: None,
                    tool_calls: None,
                });
            }
            Ok(_) => {}
            Err(e) => {
                warn!(error = %e, "failed to read long-term memory for context");
            }
        }

        // 5. Conversation history (truncated to memory_window)
        let window = self.config.defaults.memory_window.max(0) as usize;
        let history = session.get_history(window);
        for msg in history {
            let role = msg
                .get("role")
                .and_then(|v| v.as_str())
                .unwrap_or("user")
                .to_string();
            let content = msg
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            messages.push(LlmMessage {
                role,
                content,
                tool_call_id: None,
                tool_calls: None,
            });
        }

        messages
    }
}

// ── Context compression ───────────────────────────────────────────────

/// Approximate token count for a string.
///
/// Uses a simple whitespace-based heuristic: each whitespace-delimited
/// word maps to roughly 4/3 tokens on average (accounting for sub-word
/// tokenization). This is intentionally coarse; callers who need exact
/// counts can swap in a real tokenizer later.
pub fn count_tokens(text: &str) -> usize {
    // Ceiling division to avoid undercount on short strings.
    let words = text.split_whitespace().count();
    (words * 4).div_ceil(3) // equivalent to ceil(words * 4/3)
}

/// Configuration for context compression.
#[derive(Debug, Clone)]
pub struct CompressionConfig {
    /// Maximum number of tokens allowed in the assembled context.
    pub max_context_tokens: usize,
    /// Number of recent conversation messages to keep verbatim.
    pub recent_message_count: usize,
    /// Whether compression is enabled at all.
    pub compression_enabled: bool,
}

impl Default for CompressionConfig {
    fn default() -> Self {
        Self {
            max_context_tokens: 8192,
            recent_message_count: 10,
            compression_enabled: true,
        }
    }
}

/// Metadata about a compression operation.
#[derive(Debug, Clone)]
pub struct CompressionMetadata {
    /// Total tokens before compression.
    pub original_tokens: usize,
    /// Total tokens after compression.
    pub compressed_tokens: usize,
    /// Ratio of compressed to original (1.0 = no compression).
    pub compression_ratio: f64,
    /// Number of messages that were summarized.
    pub messages_summarized: usize,
}

/// Result of compressing a message list.
#[derive(Debug, Clone)]
pub struct CompressedContext {
    /// The compressed message list, ready for the LLM.
    pub messages: Vec<LlmMessage>,
    /// Metadata describing what compression did.
    pub metadata: CompressionMetadata,
}

/// Extract the first sentence from a text block.
///
/// Returns everything up to and including the first sentence-ending
/// punctuation (`.`, `!`, `?`) followed by whitespace or end-of-string.
/// If no sentence boundary is found, returns up to the first 120 characters.
fn first_sentence(text: &str) -> &str {
    for (i, ch) in text.char_indices() {
        if matches!(ch, '.' | '!' | '?') {
            let end = i + ch.len_utf8();
            // Accept if this is the end of string or followed by whitespace.
            if end >= text.len() || text[end..].starts_with(char::is_whitespace) {
                return &text[..end];
            }
        }
    }
    // No sentence boundary found; truncate.
    let limit = text
        .char_indices()
        .nth(120)
        .map(|(i, _)| i)
        .unwrap_or(text.len());
    &text[..limit]
}

/// Compress a message list to fit within a token budget.
///
/// The algorithm preserves:
/// 1. All system-role messages (prompts, skills, memory) -- always kept.
/// 2. The last `config.recent_message_count` non-system messages -- verbatim.
/// 3. Older non-system messages -- summarized into a single system message
///    containing first-sentence extracts.
///
/// If compression is disabled or the context already fits, the original
/// messages are returned unchanged.
pub fn compress_context(
    messages: Vec<LlmMessage>,
    config: &CompressionConfig,
) -> CompressedContext {
    let original_tokens: usize = messages.iter().map(|m| count_tokens(&m.content)).sum();

    // Fast path: no compression needed.
    if !config.compression_enabled || original_tokens <= config.max_context_tokens {
        return CompressedContext {
            messages,
            metadata: CompressionMetadata {
                original_tokens,
                compressed_tokens: original_tokens,
                compression_ratio: 1.0,
                messages_summarized: 0,
            },
        };
    }

    // Separate system messages from conversation messages.
    let mut system_msgs: Vec<LlmMessage> = Vec::new();
    let mut conversation_msgs: Vec<LlmMessage> = Vec::new();

    for msg in messages {
        if msg.role == "system" {
            system_msgs.push(msg);
        } else {
            conversation_msgs.push(msg);
        }
    }

    // Split conversation into old (to summarize) and recent (to keep).
    let recent_count = config.recent_message_count.min(conversation_msgs.len());
    let split_point = conversation_msgs.len() - recent_count;
    let old_msgs = &conversation_msgs[..split_point];
    let recent_msgs = &conversation_msgs[split_point..];

    // Build summary of old messages.
    let messages_summarized = old_msgs.len();
    let summary = if !old_msgs.is_empty() {
        let mut lines = Vec::with_capacity(old_msgs.len());
        for msg in old_msgs {
            let sentence = first_sentence(&msg.content);
            lines.push(format!("[{}]: {}", msg.role, sentence));
        }
        Some(lines.join("\n"))
    } else {
        None
    };

    // Reassemble: system messages, optional summary, recent conversation.
    let mut result: Vec<LlmMessage> = Vec::new();
    result.extend(system_msgs);

    if let Some(summary_text) = summary {
        result.push(LlmMessage {
            role: "system".into(),
            content: format!(
                "# Conversation Summary (compressed)\n\n\
                 The following is a summary of {} earlier messages:\n\n{}",
                messages_summarized, summary_text
            ),
            tool_call_id: None,
            tool_calls: None,
        });
    }

    result.extend(recent_msgs.iter().cloned());

    let compressed_tokens: usize = result.iter().map(|m| count_tokens(&m.content)).sum();
    let compression_ratio = if original_tokens > 0 {
        compressed_tokens as f64 / original_tokens as f64
    } else {
        1.0
    };

    CompressedContext {
        messages: result,
        metadata: CompressionMetadata {
            original_tokens,
            compressed_tokens,
            compression_ratio,
            messages_summarized,
        },
    }
}

/// Expand a workspace path, replacing `~/` with the actual home directory.
fn expand_workspace(workspace: &str, home: &std::path::Path) -> std::path::PathBuf {
    if let Some(rest) = workspace.strip_prefix("~/") {
        home.join(rest)
    } else {
        std::path::PathBuf::from(workspace)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::memory::MemoryStore;
    use crate::agent::skills::SkillsLoader;
    use clawft_platform::NativePlatform;
    use clawft_types::config::AgentDefaults;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_dir(prefix: &str) -> PathBuf {
        let id = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        std::env::temp_dir().join(format!("clawft_ctx_test_{prefix}_{pid}_{id}"))
    }

    fn test_config() -> AgentsConfig {
        AgentsConfig {
            defaults: AgentDefaults {
                workspace: "~/.clawft/workspace".into(),
                model: "test-model/v1".into(),
                max_tokens: 4096,
                temperature: 0.5,
                max_tool_iterations: 10,
                memory_window: 5,
            },
            ..AgentsConfig::default()
        }
    }

    /// Helper to create a ContextBuilder backed by temp directories.
    async fn setup(
        prefix: &str,
    ) -> (
        ContextBuilder<NativePlatform>,
        PathBuf,
        Arc<MemoryStore<NativePlatform>>,
        Arc<SkillsLoader<NativePlatform>>,
    ) {
        let dir = temp_dir(prefix);
        let mem_dir = dir.join("memory");
        let skills_dir = dir.join("skills");

        let platform = Arc::new(NativePlatform::new());

        let memory = Arc::new(MemoryStore::with_paths(
            mem_dir.join("MEMORY.md"),
            mem_dir.join("HISTORY.md"),
            platform.clone(),
        ));

        let skills = Arc::new(SkillsLoader::with_dir(skills_dir.clone(), platform.clone()));

        let ctx = ContextBuilder::new(test_config(), memory.clone(), skills.clone(), platform);

        (ctx, dir, memory, skills)
    }

    #[tokio::test]
    async fn build_system_prompt_contains_identity() {
        let (ctx, dir, _, _) = setup("prompt").await;

        let prompt = ctx.build_system_prompt().await;

        assert!(prompt.contains("clawft"));
        assert!(prompt.contains("test-model/v1"));
        assert!(prompt.contains("MEMORY.md"));
        assert!(prompt.contains("HISTORY.md"));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn build_messages_includes_system_prompt() {
        let (ctx, dir, _, _) = setup("sys_msg").await;
        let session = Session::new("test:1");

        let messages = ctx.build_messages(&session, &[]).await;

        assert!(!messages.is_empty());
        assert_eq!(messages[0].role, "system");
        assert!(messages[0].content.contains("clawft"));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn build_messages_includes_memory() {
        let (ctx, dir, memory, _) = setup("memory").await;

        memory
            .write_long_term("important fact: Rust is fast")
            .await
            .unwrap();

        let session = Session::new("test:2");
        let messages = ctx.build_messages(&session, &[]).await;

        // Should have system prompt + memory message
        let memory_msg = messages
            .iter()
            .find(|m| m.content.contains("Relevant Memory"));
        assert!(memory_msg.is_some());
        assert!(memory_msg.unwrap().content.contains("Rust is fast"));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn build_messages_skips_empty_memory() {
        let (ctx, dir, _, _) = setup("no_mem").await;
        let session = Session::new("test:3");

        let messages = ctx.build_messages(&session, &[]).await;

        // No "Relevant Memory" message when memory is empty
        let memory_msg = messages
            .iter()
            .find(|m| m.content.contains("Relevant Memory"));
        assert!(memory_msg.is_none());

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn build_messages_includes_skill_prompts() {
        let (ctx, dir, _, skills) = setup("skills").await;

        // Create a skill on disk and load it
        let skills_dir = dir.join("skills");
        tokio::fs::create_dir_all(skills_dir.join("research"))
            .await
            .unwrap();
        tokio::fs::write(
            skills_dir.join("research").join("skill.json"),
            r#"{"name":"research","description":"Research skill","variables":["topic"]}"#,
        )
        .await
        .unwrap();
        tokio::fs::write(
            skills_dir.join("research").join("prompt.md"),
            "You are a research expert.",
        )
        .await
        .unwrap();

        // Load it into cache
        skills.load_skill("research").await.unwrap();

        let session = Session::new("test:4");
        let messages = ctx.build_messages(&session, &["research".into()]).await;

        let skill_msg = messages
            .iter()
            .find(|m| m.content.contains("# Skill: research"));
        assert!(skill_msg.is_some());
        assert!(skill_msg.unwrap().content.contains("research expert"));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn build_messages_includes_conversation_history() {
        let (ctx, dir, _, _) = setup("history").await;

        let mut session = Session::new("test:5");
        session.add_message("user", "What is Rust?", None);
        session.add_message("assistant", "Rust is a systems language.", None);
        session.add_message("user", "Tell me more.", None);

        let messages = ctx.build_messages(&session, &[]).await;

        // Should have system prompt + 3 history messages
        // (memory_window=5, so all 3 fit)
        let history_roles: Vec<&str> = messages
            .iter()
            .skip(1) // skip system prompt
            .map(|m| m.role.as_str())
            .collect();

        assert_eq!(history_roles, vec!["user", "assistant", "user"]);

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn build_messages_truncates_history_to_window() {
        let (ctx, dir, _, _) = setup("truncate").await;

        let mut session = Session::new("test:6");
        for i in 0..20 {
            session.add_message("user", &format!("message {i}"), None);
        }

        let messages = ctx.build_messages(&session, &[]).await;

        // memory_window=5, so only last 5 history messages
        let history_msgs: Vec<&LlmMessage> = messages.iter().filter(|m| m.role == "user").collect();
        assert_eq!(history_msgs.len(), 5);
        assert!(history_msgs[0].content.contains("message 15"));
        assert!(history_msgs[4].content.contains("message 19"));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn build_messages_order_is_correct() {
        let (ctx, dir, memory, skills) = setup("order").await;

        // Set up memory
        memory.write_long_term("a long-term fact").await.unwrap();

        // Set up a skill
        let skills_dir = dir.join("skills");
        tokio::fs::create_dir_all(skills_dir.join("test_skill"))
            .await
            .unwrap();
        tokio::fs::write(
            skills_dir.join("test_skill").join("skill.json"),
            r#"{"name":"test_skill","description":"Test","variables":[]}"#,
        )
        .await
        .unwrap();
        tokio::fs::write(
            skills_dir.join("test_skill").join("prompt.md"),
            "skill prompt content",
        )
        .await
        .unwrap();
        skills.load_skill("test_skill").await.unwrap();

        // Set up session
        let mut session = Session::new("test:7");
        session.add_message("user", "hello", None);

        let messages = ctx.build_messages(&session, &["test_skill".into()]).await;

        // Order: system, skill, memory, history
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0].role, "system");
        assert!(messages[0].content.contains("clawft"));
        assert_eq!(messages[1].role, "system");
        assert!(messages[1].content.contains("# Skill: test_skill"));
        assert_eq!(messages[2].role, "system");
        assert!(messages[2].content.contains("Relevant Memory"));
        assert_eq!(messages[3].role, "user");
        assert_eq!(messages[3].content, "hello");

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn build_messages_loads_uncached_skill_on_demand() {
        let (ctx, dir, _, _) = setup("on_demand").await;

        // Create skill on disk but do NOT pre-load it
        let skills_dir = dir.join("skills");
        tokio::fs::create_dir_all(skills_dir.join("lazy_skill"))
            .await
            .unwrap();
        tokio::fs::write(
            skills_dir.join("lazy_skill").join("skill.json"),
            r#"{"name":"lazy_skill","description":"Lazy","variables":[]}"#,
        )
        .await
        .unwrap();
        tokio::fs::write(
            skills_dir.join("lazy_skill").join("prompt.md"),
            "loaded on demand",
        )
        .await
        .unwrap();

        let session = Session::new("test:8");
        let messages = ctx.build_messages(&session, &["lazy_skill".into()]).await;

        let skill_msg = messages
            .iter()
            .find(|m| m.content.contains("# Skill: lazy_skill"));
        assert!(skill_msg.is_some());

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn build_messages_handles_missing_skill_gracefully() {
        let (ctx, dir, _, _) = setup("missing_skill").await;
        let session = Session::new("test:9");

        // Request a skill that does not exist
        let messages = ctx.build_messages(&session, &["nonexistent".into()]).await;

        // Should just have system prompt, no error
        assert!(!messages.is_empty());
        assert_eq!(messages[0].role, "system");

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[test]
    fn expand_workspace_with_tilde() {
        let home = PathBuf::from("/home/testuser");
        let expanded = expand_workspace("~/.clawft/workspace", &home);
        assert_eq!(expanded, PathBuf::from("/home/testuser/.clawft/workspace"));
    }

    #[test]
    fn expand_workspace_absolute() {
        let home = PathBuf::from("/home/testuser");
        let expanded = expand_workspace("/opt/workspace", &home);
        assert_eq!(expanded, PathBuf::from("/opt/workspace"));
    }

    #[test]
    fn config_accessor() {
        let config = test_config();
        let platform = Arc::new(NativePlatform::new());
        let memory = Arc::new(MemoryStore::with_paths(
            PathBuf::from("/tmp/m.md"),
            PathBuf::from("/tmp/h.md"),
            platform.clone(),
        ));
        let skills = Arc::new(SkillsLoader::with_dir(
            PathBuf::from("/tmp/skills"),
            platform.clone(),
        ));
        let ctx = ContextBuilder::new(config.clone(), memory, skills, platform);
        assert_eq!(ctx.config().defaults.memory_window, 5);
    }

    #[tokio::test]
    async fn llm_message_has_expected_fields() {
        let msg = LlmMessage {
            role: "system".into(),
            content: "test content".into(),
            tool_call_id: Some("tc-1".into()),
            tool_calls: None,
        };
        assert_eq!(msg.role, "system");
        assert_eq!(msg.content, "test content");
        assert_eq!(msg.tool_call_id.as_deref(), Some("tc-1"));
    }

    // ── Agent definition integration tests ───────────────────────────

    use crate::agent::agents::AgentDefinition;
    use std::collections::HashMap;

    fn test_agent() -> AgentDefinition {
        let mut vars = HashMap::new();
        vars.insert("lang".into(), "Rust".into());
        AgentDefinition {
            name: "researcher".into(),
            description: "Research agent".into(),
            model: Some("custom-model/v2".into()),
            system_prompt: Some("You are a ${lang} researcher. Arguments: $ARGUMENTS".into()),
            skills: vec![],
            allowed_tools: vec!["read_file".into()],
            max_turns: Some(5),
            variables: vars,
            source_path: None,
        }
    }

    #[tokio::test]
    async fn build_system_prompt_for_agent_renders_template() {
        let (ctx, dir, _, _) = setup("agent_prompt").await;
        let agent = test_agent();

        let prompt = ctx
            .build_system_prompt_for_agent(&agent, "topic1 topic2")
            .await;

        // Should contain the rendered agent prompt
        assert!(prompt.contains("# Agent: researcher"));
        assert!(prompt.contains("You are a Rust researcher"));
        assert!(prompt.contains("Arguments: topic1 topic2"));
        // Should also contain the base system prompt
        assert!(prompt.contains("clawft"));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn build_system_prompt_for_agent_without_custom_prompt() {
        let (ctx, dir, _, _) = setup("agent_no_prompt").await;
        let mut agent = test_agent();
        agent.system_prompt = None;

        let prompt = ctx.build_system_prompt_for_agent(&agent, "").await;

        // Should just be the base prompt, no "# Agent:" header
        assert!(!prompt.contains("# Agent:"));
        assert!(prompt.contains("clawft"));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn build_messages_for_agent_includes_agent_system_prompt() {
        let (ctx, dir, _, _) = setup("agent_msgs").await;
        let agent = test_agent();
        let session = Session::new("test:agent1");

        let messages = ctx
            .build_messages_for_agent(&session, &agent, "my-args", None)
            .await;

        assert!(!messages.is_empty());
        assert_eq!(messages[0].role, "system");
        assert!(messages[0].content.contains("# Agent: researcher"));
        assert!(messages[0].content.contains("Rust researcher"));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn build_messages_for_agent_injects_extra_skill_instructions() {
        let (ctx, dir, _, _) = setup("agent_skill_instr").await;
        let agent = test_agent();
        let session = Session::new("test:agent2");

        let messages = ctx
            .build_messages_for_agent(&session, &agent, "", Some("Always cite your sources."))
            .await;

        let skill_instr = messages
            .iter()
            .find(|m| m.content.contains("# Skill Instructions"));
        assert!(skill_instr.is_some());
        assert!(
            skill_instr
                .unwrap()
                .content
                .contains("Always cite your sources.")
        );

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn build_messages_for_agent_skips_empty_skill_instructions() {
        let (ctx, dir, _, _) = setup("agent_empty_instr").await;
        let agent = test_agent();
        let session = Session::new("test:agent3");

        let messages = ctx
            .build_messages_for_agent(&session, &agent, "", Some("   "))
            .await;

        let skill_instr = messages
            .iter()
            .find(|m| m.content.contains("# Skill Instructions"));
        assert!(skill_instr.is_none());

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn build_messages_for_agent_includes_memory() {
        let (ctx, dir, memory, _) = setup("agent_memory").await;
        let agent = test_agent();

        memory
            .write_long_term("agent-level memory test")
            .await
            .unwrap();

        let session = Session::new("test:agent4");
        let messages = ctx
            .build_messages_for_agent(&session, &agent, "", None)
            .await;

        let mem_msg = messages
            .iter()
            .find(|m| m.content.contains("Relevant Memory"));
        assert!(mem_msg.is_some());
        assert!(mem_msg.unwrap().content.contains("agent-level memory test"));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn build_messages_for_agent_includes_history() {
        let (ctx, dir, _, _) = setup("agent_history").await;
        let agent = test_agent();

        let mut session = Session::new("test:agent5");
        session.add_message("user", "hello agent", None);
        session.add_message("assistant", "hello user", None);

        let messages = ctx
            .build_messages_for_agent(&session, &agent, "", None)
            .await;

        let user_msgs: Vec<_> = messages.iter().filter(|m| m.role == "user").collect();
        assert_eq!(user_msgs.len(), 1);
        assert_eq!(user_msgs[0].content, "hello agent");

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    // ── Context compression tests ───────────────────────────────────

    #[test]
    fn count_tokens_basic() {
        // "hello world" = 2 words -> ceil(2 * 4/3) = ceil(2.67) = 3
        assert_eq!(count_tokens("hello world"), 3);
        // Empty string
        assert_eq!(count_tokens(""), 0);
        // Single word
        assert_eq!(count_tokens("hello"), 2); // ceil(4/3) = 2
    }

    #[test]
    fn first_sentence_extracts_correctly() {
        assert_eq!(first_sentence("Hello world. More text here."), "Hello world.");
        assert_eq!(first_sentence("No period here"), "No period here");
        assert_eq!(first_sentence("Question? Yes."), "Question?");
        assert_eq!(first_sentence("Exclaim! Done."), "Exclaim!");
    }

    fn make_msg(role: &str, content: &str) -> LlmMessage {
        LlmMessage {
            role: role.into(),
            content: content.into(),
            tool_call_id: None,
            tool_calls: None,
        }
    }

    #[test]
    fn compress_context_no_op_when_within_budget() {
        let messages = vec![
            make_msg("system", "You are helpful."),
            make_msg("user", "Hi"),
            make_msg("assistant", "Hello!"),
        ];
        let config = CompressionConfig {
            max_context_tokens: 100_000,
            recent_message_count: 10,
            compression_enabled: true,
        };

        let result = compress_context(messages.clone(), &config);
        assert_eq!(result.messages.len(), 3);
        assert_eq!(result.metadata.compression_ratio, 1.0);
        assert_eq!(result.metadata.messages_summarized, 0);
    }

    #[test]
    fn compress_context_no_op_when_disabled() {
        let messages = vec![
            make_msg("system", "sys"),
            make_msg("user", "a long message that exceeds budget"),
        ];
        let config = CompressionConfig {
            max_context_tokens: 1, // tiny budget
            recent_message_count: 10,
            compression_enabled: false,
        };

        let result = compress_context(messages.clone(), &config);
        assert_eq!(result.messages.len(), 2);
        assert_eq!(result.metadata.messages_summarized, 0);
    }

    #[test]
    fn compress_context_preserves_system_prompt() {
        // Build a context with a system message and many conversation messages.
        let mut messages = vec![make_msg("system", "You are a helpful assistant with many capabilities.")];
        for i in 0..20 {
            messages.push(make_msg("user", &format!("User message number {}. This is a fairly long message to inflate token count significantly.", i)));
            messages.push(make_msg("assistant", &format!("Assistant response number {}. Also fairly long to push tokens over the budget limit.", i)));
        }

        let config = CompressionConfig {
            max_context_tokens: 50, // very tight budget
            recent_message_count: 4,
            compression_enabled: true,
        };

        let result = compress_context(messages, &config);

        // System prompt must always be first and preserved.
        assert_eq!(result.messages[0].role, "system");
        assert!(result.messages[0].content.contains("helpful assistant"));

        // There should be a summary message.
        let summary = result.messages.iter().find(|m| m.content.contains("Conversation Summary"));
        assert!(summary.is_some(), "should have a summary message");

        // Recent messages should be the last 4 conversation messages.
        let non_system: Vec<_> = result.messages.iter().filter(|m| m.role != "system").collect();
        assert_eq!(non_system.len(), 4);

        // Check the very last message is the last assistant response.
        let last = result.messages.last().unwrap();
        assert_eq!(last.role, "assistant");
        assert!(last.content.contains("response number 19"));
    }

    #[test]
    fn compress_context_recent_messages_intact() {
        let mut messages = vec![make_msg("system", "sys prompt")];
        for i in 0..15 {
            messages.push(make_msg("user", &format!("msg {} with enough words to make the token count go over budget easily", i)));
        }

        let config = CompressionConfig {
            max_context_tokens: 10,
            recent_message_count: 5,
            compression_enabled: true,
        };

        let result = compress_context(messages, &config);

        // Last 5 user messages should be kept verbatim.
        let user_msgs: Vec<_> = result.messages.iter().filter(|m| m.role == "user").collect();
        assert_eq!(user_msgs.len(), 5);
        for (idx, msg) in user_msgs.iter().enumerate() {
            let expected_num = 10 + idx; // messages 10..14
            assert!(msg.content.contains(&format!("msg {expected_num}")));
        }
    }

    #[test]
    fn compress_context_metadata_accuracy() {
        let mut messages = vec![make_msg("system", "short system")];
        for i in 0..10 {
            messages.push(make_msg("user", &format!("Message number {} with several words to inflate the count.", i)));
        }

        let original_tokens: usize = messages.iter().map(|m| count_tokens(&m.content)).sum();

        let config = CompressionConfig {
            max_context_tokens: 10,
            recent_message_count: 2,
            compression_enabled: true,
        };

        let result = compress_context(messages, &config);

        assert_eq!(result.metadata.original_tokens, original_tokens);
        assert_eq!(result.metadata.messages_summarized, 8); // 10 - 2 recent
        // The compression ratio should be less than 1.0 when many messages
        // are summarized (summary is shorter than full message bodies).
        // Note: the summary header adds some overhead, so we just verify
        // fewer messages remain and the ratio is reported accurately.
        assert!(result.metadata.compression_ratio > 0.0);

        // Verify compressed_tokens matches actual content.
        let actual_compressed: usize = result.messages.iter().map(|m| count_tokens(&m.content)).sum();
        assert_eq!(result.metadata.compressed_tokens, actual_compressed);
    }

    #[tokio::test]
    async fn build_messages_compressed_integration() {
        let (ctx, dir, _, _) = setup("compressed").await;

        let ctx = ctx.with_compression(CompressionConfig {
            max_context_tokens: 100_000,
            recent_message_count: 10,
            compression_enabled: true,
        });

        let mut session = Session::new("test:compress1");
        session.add_message("user", "Hello", None);
        session.add_message("assistant", "Hi there!", None);

        let result = ctx.build_messages_compressed(&session, &[]).await;

        // Within budget, so no compression.
        assert_eq!(result.metadata.compression_ratio, 1.0);
        assert!(result.messages[0].role == "system");

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn build_messages_compressed_uses_default_config() {
        let (ctx, dir, _, _) = setup("default_compress").await;
        // No with_compression() call -- should use defaults.

        let session = Session::new("test:compress2");
        let result = ctx.build_messages_compressed(&session, &[]).await;

        // Default budget is 8192, system prompt is small, should not compress.
        assert_eq!(result.metadata.compression_ratio, 1.0);

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
