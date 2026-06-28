//! Talk Mode controller for continuous voice conversation.
//!
//! Manages the lifecycle of a voice conversation session,
//! coordinating the VoiceChannel with the agent pipeline.

use std::sync::Arc;

use tracing::info;

use crate::error::PluginError;
use crate::traits::{CancellationToken, ChannelAdapter, ChannelAdapterHost};

use super::channel::{VoiceChannel, VoiceStatus};

/// Controller for Talk Mode -- continuous voice conversation.
///
/// Wraps a [`VoiceChannel`] and manages the listen -> transcribe ->
/// agent -> speak loop. The controller runs until the cancellation
/// token is triggered (e.g., by Ctrl+C in the CLI).
pub struct TalkModeController {
    channel: Arc<VoiceChannel>,
    cancel: CancellationToken,
}

impl TalkModeController {
    /// Create a new Talk Mode controller.
    ///
    /// # Arguments
    ///
    /// * `channel` - The voice channel to control.
    /// * `cancel` - Cancellation token to stop the session.
    pub fn new(channel: Arc<VoiceChannel>, cancel: CancellationToken) -> Self {
        Self { channel, cancel }
    }

    /// Run the Talk Mode loop until cancelled.
    ///
    /// Starts the voice channel and blocks until the cancellation
    /// token is triggered. In the real implementation, the voice
    /// channel would continuously capture audio, detect speech,
    /// transcribe it, and deliver it to the agent pipeline.
    pub async fn run(&self, host: Arc<dyn ChannelAdapterHost>) -> Result<(), PluginError> {
        info!("Talk Mode starting");
        let result = self.channel.start(host, self.cancel.clone()).await;
        info!("Talk Mode ended");
        result
    }

    /// Get the current voice status.
    pub async fn status(&self) -> VoiceStatus {
        self.channel.current_status().await
    }

    /// Get a reference to the underlying voice channel.
    pub fn channel(&self) -> &Arc<VoiceChannel> {
        &self.channel
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use crate::message::MessagePayload;
    use async_trait::async_trait;

    struct StubHost;

    #[async_trait]
    impl ChannelAdapterHost for StubHost {
        async fn deliver_inbound(
            &self,
            _channel: &str,
            _sender_id: &str,
            _chat_id: &str,
            _payload: MessagePayload,
            _metadata: HashMap<String, serde_json::Value>,
        ) -> Result<(), PluginError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn talk_mode_status_starts_idle() {
        let (channel, _rx) = VoiceChannel::new();
        let cancel = CancellationToken::new();
        let controller = TalkModeController::new(Arc::new(channel), cancel);
        assert_eq!(controller.status().await, VoiceStatus::Idle);
    }

    #[tokio::test]
    async fn talk_mode_run_and_cancel() {
        let (channel, _rx) = VoiceChannel::new();
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();
        let controller = Arc::new(TalkModeController::new(Arc::new(channel), cancel_clone));
        let host: Arc<dyn ChannelAdapterHost> = Arc::new(StubHost);

        let handle = tokio::spawn({
            let controller = Arc::clone(&controller);
            async move { controller.run(host).await }
        });

        // Give the channel a moment to start.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        cancel.cancel();
        let result = handle.await.unwrap();
        assert!(result.is_ok());
    }
}
