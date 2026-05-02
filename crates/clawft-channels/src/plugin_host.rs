//! Unified plugin host extensions for C7: PluginHost Unification.
//!
//! Provides:
//! - [`ChannelAdapterShim`] -- bridges the existing [`Channel`] trait to the
//!   new [`ChannelAdapter`] trait from `clawft-plugin`, allowing existing
//!   Telegram, Discord, and Slack channels to work through the unified
//!   plugin system without behavior changes.
//! - [`SoulConfig`] -- loads and injects SOUL.md personality content into
//!   the Assembler pipeline stage system prompt.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::traits::{Channel, ChannelHost, ChannelStatus, MessageId};
use clawft_plugin::error::PluginError;
use clawft_plugin::message::MessagePayload;
use clawft_plugin::traits::{ChannelAdapter, ChannelAdapterHost};
use clawft_types::event::OutboundMessage;

// ---------------------------------------------------------------------------
// ChannelAdapterShim
// ---------------------------------------------------------------------------

/// Bridges an existing [`Channel`] implementation to the [`ChannelAdapter`]
/// trait from `clawft-plugin`.
///
/// This shim allows Telegram, Discord, Slack, and other channels that
/// implement the original `Channel` trait to participate in the unified
/// plugin host system without requiring rewrites.
///
/// The shim converts between the two trait signatures:
/// - `Channel::start(host: Arc<dyn ChannelHost>, cancel)` <->
///   `ChannelAdapter::start(host: Arc<dyn ChannelAdapterHost>, cancel)`
/// - `Channel::send(msg: &OutboundMessage) -> MessageId` <->
///   `ChannelAdapter::send(target, payload: &MessagePayload) -> String`
pub struct ChannelAdapterShim {
    /// The underlying channel implementation.
    channel: Arc<dyn Channel>,
    /// Bridge host for converting between trait interfaces.
    bridge_host: Arc<dyn ChannelHost>,
}

impl ChannelAdapterShim {
    /// Create a new shim wrapping an existing channel.
    pub fn new(channel: Arc<dyn Channel>, host: Arc<dyn ChannelHost>) -> Self {
        Self {
            channel,
            bridge_host: host,
        }
    }

    /// Get the underlying channel's status.
    pub fn status(&self) -> ChannelStatus {
        self.channel.status()
    }
}

#[async_trait]
impl ChannelAdapter for ChannelAdapterShim {
    fn name(&self) -> &str {
        self.channel.name()
    }

    fn display_name(&self) -> &str {
        // Use metadata for display name
        // We return name() since the metadata is not cheaply cloneable
        self.channel.name()
    }

    fn supports_threads(&self) -> bool {
        self.channel.metadata().supports_threads
    }

    fn supports_media(&self) -> bool {
        self.channel.metadata().supports_media
    }

    async fn start(
        &self,
        _host: Arc<dyn ChannelAdapterHost>,
        cancel: clawft_plugin::CancellationToken,
    ) -> Result<(), PluginError> {
        // Bridge clawft_plugin's CancellationToken (poll-based) to tokio_util's
        // (async-based) by spawning a polling task.
        let tokio_cancel = CancellationToken::new();
        let tokio_cancel_clone = tokio_cancel.clone();
        let poll_handle = tokio::spawn(async move {
            loop {
                if cancel.is_cancelled() {
                    tokio_cancel_clone.cancel();
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        });
        let result = self.channel
            .start(self.bridge_host.clone(), tokio_cancel)
            .await
            .map_err(|e| PluginError::ExecutionFailed(format!("channel start: {e}")));
        poll_handle.abort();
        result
    }

    async fn send(
        &self,
        target: &str,
        payload: &MessagePayload,
    ) -> Result<String, PluginError> {
        let content = match payload {
            MessagePayload::Text { content } => content.clone(),
            MessagePayload::Structured { data } => serde_json::to_string(data)
                .unwrap_or_else(|_| data.to_string()),
            MessagePayload::Binary { mime_type, data } => {
                return Err(PluginError::NotImplemented(format!(
                    "binary payload ({mime_type}, {} bytes) not supported by legacy channel shim",
                    data.len()
                )));
            }
            _ => return Err(PluginError::NotImplemented("unknown payload variant".into())),
        };

        let msg = OutboundMessage {
            channel: self.channel.name().to_owned(),
            chat_id: target.to_owned(),
            content,
            reply_to: None,
            media: vec![],
            metadata: HashMap::new(),
        };

        let MessageId(id) = self
            .channel
            .send(&msg)
            .await
            .map_err(|e| PluginError::ExecutionFailed(format!("channel send: {e}")))?;

        Ok(id)
    }
}

// ---------------------------------------------------------------------------
// SoulConfig -- SOUL.md injection
// ---------------------------------------------------------------------------

/// SOUL.md configuration for personality injection.
///
/// Loads a `SOUL.md` file and provides its content for injection into
/// the Assembler pipeline stage system prompt. The SOUL.md format is
/// a markdown file containing personality directives, tone, and
/// behavioral guidelines for the agent.
#[derive(Debug, Clone)]
pub struct SoulConfig {
    /// Raw content of the SOUL.md file.
    pub content: String,
    /// Path to the SOUL.md file (for hot-reload detection).
    pub source_path: Option<PathBuf>,
}

impl SoulConfig {
    /// Create a new SoulConfig from content.
    pub fn new(content: String) -> Self {
        Self {
            content,
            source_path: None,
        }
    }

    /// Load SOUL.md from a workspace directory.
    ///
    /// Searches for `SOUL.md` in:
    /// 1. `{workspace}/.clawft/SOUL.md`
    /// 2. `{workspace}/SOUL.md`
    /// 3. `~/.clawft/SOUL.md` (global fallback)
    ///
    /// Returns `None` if no SOUL.md file is found.
    pub fn load_from_workspace(workspace: &Path) -> Option<Self> {
        let candidates = [
            workspace.join(".clawft").join("SOUL.md"),
            workspace.join("SOUL.md"),
        ];

        for path in &candidates {
            if let Ok(content) = std::fs::read_to_string(path)
                && !content.trim().is_empty()
            {
                info!(path = %path.display(), "loaded SOUL.md");
                return Some(Self {
                    content,
                    source_path: Some(path.clone()),
                });
            }
        }

        // Global fallback
        if let Some(home) = dirs::home_dir() {
            let global_path = home.join(".clawft").join("SOUL.md");
            if let Ok(content) = std::fs::read_to_string(&global_path)
                && !content.trim().is_empty()
            {
                info!(path = %global_path.display(), "loaded global SOUL.md");
                return Some(Self {
                    content,
                    source_path: Some(global_path),
                });
            }
        }

        debug!("no SOUL.md found");
        None
    }

    /// Inject SOUL.md content into a system prompt.
    ///
    /// Appends the SOUL.md content as a personality section to the
    /// given system prompt string. If the SOUL.md content is empty,
    /// the system prompt is returned unchanged.
    pub fn inject_into_prompt(&self, system_prompt: &str) -> String {
        if self.content.trim().is_empty() {
            return system_prompt.to_owned();
        }

        format!(
            "{system_prompt}\n\n\
             ## Agent Personality (SOUL.md)\n\n\
             {}\n",
            self.content.trim()
        )
    }

    /// Check if the SOUL.md file has been modified since load.
    ///
    /// Returns `true` if the file no longer matches the loaded content,
    /// or if it has been deleted. This supports hot-reload detection.
    pub fn is_stale(&self) -> bool {
        match &self.source_path {
            Some(path) => {
                match std::fs::read_to_string(path) {
                    Ok(current) => current != self.content,
                    Err(_) => true, // File deleted or unreadable
                }
            }
            None => false, // No file to check
        }
    }
}

// ---------------------------------------------------------------------------
// ChannelAdapterHostBridge
// ---------------------------------------------------------------------------

/// Bridges [`ChannelAdapterHost`] to the existing [`ChannelHost`] interface.
///
/// Converts inbound `MessagePayload` variants to the `InboundMessage` format
/// expected by the existing pipeline.
pub struct ChannelAdapterHostBridge {
    inner: Arc<dyn ChannelHost>,
}

impl ChannelAdapterHostBridge {
    /// Create a new bridge wrapping an existing ChannelHost.
    pub fn new(host: Arc<dyn ChannelHost>) -> Self {
        Self { inner: host }
    }
}

#[async_trait]
impl ChannelAdapterHost for ChannelAdapterHostBridge {
    async fn deliver_inbound(
        &self,
        channel: &str,
        sender_id: &str,
        chat_id: &str,
        payload: MessagePayload,
        metadata: HashMap<String, serde_json::Value>,
    ) -> Result<(), PluginError> {
        let content = match payload {
            MessagePayload::Text { content } => content,
            MessagePayload::Structured { data } => serde_json::to_string(&data)
                .unwrap_or_else(|_| data.to_string()),
            MessagePayload::Binary { mime_type, data } => {
                warn!(
                    channel = channel,
                    mime_type = %mime_type,
                    size = data.len(),
                    "binary payload received but not yet supported by pipeline"
                );
                format!("[binary: {mime_type}, {} bytes]", data.len())
            }
            _ => {
                warn!(channel = channel, "unknown payload variant received");
                "[unknown payload]".to_string()
            }
        };

        self.inner
            .publish_inbound(channel, sender_id, chat_id, &content, vec![], metadata)
            .await
            .map_err(|e| PluginError::ExecutionFailed(format!("deliver inbound: {e}")))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traits::ChannelMetadata;
    use clawft_types::error::ChannelError;
    use clawft_types::event::InboundMessage;
    use std::sync::atomic::{AtomicU8, Ordering};

    // -- Mock channel for shim testing --

    struct MockChannel {
        name: String,
        status: AtomicU8,
    }

    impl MockChannel {
        fn new(name: &str) -> Self {
            Self {
                name: name.to_owned(),
                status: AtomicU8::new(0),
            }
        }
    }

    #[async_trait]
    impl Channel for MockChannel {
        fn name(&self) -> &str {
            &self.name
        }

        fn metadata(&self) -> ChannelMetadata {
            ChannelMetadata {
                name: self.name.clone(),
                display_name: format!("Mock {}", self.name),
                supports_threads: true,
                supports_media: false,
            }
        }

        fn status(&self) -> ChannelStatus {
            match self.status.load(Ordering::SeqCst) {
                1 => ChannelStatus::Running,
                _ => ChannelStatus::Stopped,
            }
        }

        fn is_allowed(&self, _sender_id: &str) -> bool {
            true
        }

        async fn start(
            &self,
            _host: Arc<dyn ChannelHost>,
            cancel: CancellationToken,
        ) -> Result<(), ChannelError> {
            self.status.store(1, Ordering::SeqCst);
            cancel.cancelled().await;
            self.status.store(0, Ordering::SeqCst);
            Ok(())
        }

        async fn send(&self, msg: &OutboundMessage) -> Result<MessageId, ChannelError> {
            Ok(MessageId(format!("mock-{}", msg.chat_id)))
        }
    }

    struct MockChannelHost;

    #[async_trait]
    impl ChannelHost for MockChannelHost {
        async fn deliver_inbound(&self, _msg: InboundMessage) -> Result<(), ChannelError> {
            Ok(())
        }

        async fn register_command(
            &self,
            _cmd: crate::traits::Command,
        ) -> Result<(), ChannelError> {
            Ok(())
        }

        async fn publish_inbound(
            &self,
            _channel: &str,
            _sender_id: &str,
            _chat_id: &str,
            _content: &str,
            _media: Vec<String>,
            _metadata: HashMap<String, serde_json::Value>,
        ) -> Result<(), ChannelError> {
            Ok(())
        }
    }

    // -- ChannelAdapterShim tests --

    #[test]
    fn shim_name_matches_channel() {
        let channel: Arc<dyn Channel> = Arc::new(MockChannel::new("telegram"));
        let host: Arc<dyn ChannelHost> = Arc::new(MockChannelHost);
        let shim = ChannelAdapterShim::new(channel, host);

        assert_eq!(ChannelAdapter::name(&shim), "telegram");
    }

    #[test]
    fn shim_capabilities_match_channel() {
        let channel: Arc<dyn Channel> = Arc::new(MockChannel::new("slack"));
        let host: Arc<dyn ChannelHost> = Arc::new(MockChannelHost);
        let shim = ChannelAdapterShim::new(channel, host);

        assert!(shim.supports_threads());
        assert!(!shim.supports_media());
    }

    #[tokio::test]
    async fn shim_send_text_payload() {
        let channel: Arc<dyn Channel> = Arc::new(MockChannel::new("discord"));
        let host: Arc<dyn ChannelHost> = Arc::new(MockChannelHost);
        let shim = ChannelAdapterShim::new(channel, host);

        let payload = MessagePayload::text("hello world");
        let result = shim.send("chat-123", &payload).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "mock-chat-123");
    }

    #[tokio::test]
    async fn shim_send_json_payload() {
        let channel: Arc<dyn Channel> = Arc::new(MockChannel::new("test"));
        let host: Arc<dyn ChannelHost> = Arc::new(MockChannelHost);
        let shim = ChannelAdapterShim::new(channel, host);

        let payload = MessagePayload::structured(serde_json::json!({"key": "value"}));
        let result = shim.send("chat-456", &payload).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn shim_send_binary_not_supported() {
        let channel: Arc<dyn Channel> = Arc::new(MockChannel::new("test"));
        let host: Arc<dyn ChannelHost> = Arc::new(MockChannelHost);
        let shim = ChannelAdapterShim::new(channel, host);

        let payload = MessagePayload::binary("audio/wav", vec![0u8; 100]);
        let result = shim.send("chat-789", &payload).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("not supported"), "got: {err}");
    }

    #[test]
    fn shim_status_reflects_channel() {
        let channel: Arc<dyn Channel> = Arc::new(MockChannel::new("test"));
        let host: Arc<dyn ChannelHost> = Arc::new(MockChannelHost);
        let shim = ChannelAdapterShim::new(channel, host);

        assert_eq!(shim.status(), ChannelStatus::Stopped);
    }

    // -- SoulConfig tests --

    #[test]
    fn soul_config_new() {
        let soul = SoulConfig::new("Be helpful and kind.".into());
        assert_eq!(soul.content, "Be helpful and kind.");
        assert!(soul.source_path.is_none());
    }

    #[test]
    fn soul_config_inject_into_empty_prompt() {
        let soul = SoulConfig::new("Be concise and direct.".into());
        let result = soul.inject_into_prompt("");
        assert!(result.contains("Agent Personality"));
        assert!(result.contains("Be concise and direct."));
    }

    #[test]
    fn soul_config_inject_into_existing_prompt() {
        let soul = SoulConfig::new("Speak like a pirate.".into());
        let result = soul.inject_into_prompt("You are a helpful assistant.");
        assert!(result.starts_with("You are a helpful assistant."));
        assert!(result.contains("Agent Personality"));
        assert!(result.contains("Speak like a pirate."));
    }

    #[test]
    fn soul_config_empty_content_no_injection() {
        let soul = SoulConfig::new("   ".into());
        let prompt = "System prompt.";
        let result = soul.inject_into_prompt(prompt);
        assert_eq!(result, prompt);
    }

    #[test]
    fn soul_config_load_from_workspace() {
        let dir = std::env::temp_dir().join("clawft_soul_test");
        let _ = std::fs::create_dir_all(dir.join(".clawft"));
        std::fs::write(
            dir.join(".clawft").join("SOUL.md"),
            "# Agent Soul\nBe creative and insightful.",
        )
        .unwrap();

        let soul = SoulConfig::load_from_workspace(&dir);
        assert!(soul.is_some());
        let soul = soul.unwrap();
        assert!(soul.content.contains("Be creative and insightful"));
        assert!(soul.source_path.is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn soul_config_load_missing_returns_none() {
        let dir = std::env::temp_dir().join("clawft_soul_test_missing");
        let _ = std::fs::create_dir_all(&dir);

        let soul = SoulConfig::load_from_workspace(&dir);
        // May or may not be None depending on whether ~/.clawft/SOUL.md exists
        // But the workspace paths should not match
        if let Some(soul) = &soul {
            assert!(
                soul.source_path
                    .as_ref()
                    .is_none_or(|p| !p.starts_with(&dir)),
                "should not find soul in workspace dir"
            );
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn soul_config_staleness_detection() {
        let dir = std::env::temp_dir().join("clawft_soul_stale_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("SOUL.md");
        std::fs::write(&path, "Original content.").unwrap();

        let soul = SoulConfig {
            content: "Original content.".into(),
            source_path: Some(path.clone()),
        };
        assert!(!soul.is_stale(), "fresh load should not be stale");

        // Modify the file
        std::fs::write(&path, "Modified content.").unwrap();
        assert!(soul.is_stale(), "modified file should be stale");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn soul_config_staleness_deleted_file() {
        let dir = std::env::temp_dir().join("clawft_soul_delete_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("SOUL.md");
        std::fs::write(&path, "Content.").unwrap();

        let soul = SoulConfig {
            content: "Content.".into(),
            source_path: Some(path.clone()),
        };
        assert!(!soul.is_stale());

        std::fs::remove_file(&path).unwrap();
        assert!(soul.is_stale(), "deleted file should be stale");

        let _ = std::fs::remove_dir_all(&dir);
    }

    // -- ChannelAdapterHostBridge tests --

    #[tokio::test]
    async fn bridge_delivers_text_payload() {
        let host: Arc<dyn ChannelHost> = Arc::new(MockChannelHost);
        let bridge = ChannelAdapterHostBridge::new(host);

        let result = bridge
            .deliver_inbound(
                "telegram",
                "user-1",
                "chat-1",
                MessagePayload::text("hello"),
                HashMap::new(),
            )
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn bridge_delivers_json_payload() {
        let host: Arc<dyn ChannelHost> = Arc::new(MockChannelHost);
        let bridge = ChannelAdapterHostBridge::new(host);

        let result = bridge
            .deliver_inbound(
                "slack",
                "user-2",
                "chat-2",
                MessagePayload::structured(serde_json::json!({"action": "test"})),
                HashMap::new(),
            )
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn bridge_handles_binary_payload() {
        let host: Arc<dyn ChannelHost> = Arc::new(MockChannelHost);
        let bridge = ChannelAdapterHostBridge::new(host);

        let result = bridge
            .deliver_inbound(
                "discord",
                "user-3",
                "chat-3",
                MessagePayload::binary("audio/opus", vec![0u8; 50]),
                HashMap::new(),
            )
            .await;

        // Binary is handled gracefully (logged as warning, placeholder text sent)
        assert!(result.is_ok());
    }
}
