//! Voice channel adapter implementing [`ChannelAdapter`].
//!
//! Provides a `VoiceChannel` that bridges the voice pipeline
//! (capture -> VAD -> STT -> agent -> TTS -> playback) as a
//! channel adapter. Currently a stub implementation -- real
//! audio processing deferred until sherpa-rs/cpal VP completes.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{Mutex, mpsc};
use tracing::{debug, info, warn};

use crate::error::PluginError;
use crate::message::MessagePayload;
use crate::traits::{CancellationToken, ChannelAdapter, ChannelAdapterHost};

/// Voice channel status for WebSocket reporting.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceStatus {
    /// Channel is idle, not actively listening.
    Idle,
    /// Listening for speech via VAD.
    Listening,
    /// Transcribing detected speech via STT.
    Transcribing,
    /// Processing transcribed text through the agent pipeline.
    Processing,
    /// Speaking agent response via TTS.
    Speaking,
}

impl std::fmt::Display for VoiceStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Idle => write!(f, "idle"),
            Self::Listening => write!(f, "listening"),
            Self::Transcribing => write!(f, "transcribing"),
            Self::Processing => write!(f, "processing"),
            Self::Speaking => write!(f, "speaking"),
        }
    }
}

/// Voice channel adapter for continuous voice conversation.
///
/// Implements [`ChannelAdapter`] to integrate the voice pipeline
/// with the agent system. Status transitions are reported via an
/// `mpsc` channel for WebSocket broadcasting.
///
/// # Stub Behavior
///
/// This implementation is a stub. The `start()` method waits for
/// cancellation rather than processing real audio. The `send()`
/// method logs the outbound text that would be spoken via TTS.
pub struct VoiceChannel {
    status_tx: mpsc::Sender<VoiceStatus>,
    status: Arc<Mutex<VoiceStatus>>,
}

impl VoiceChannel {
    /// Create a new voice channel.
    ///
    /// Returns the channel and a receiver for status updates.
    /// The receiver can be used to broadcast status changes
    /// to WebSocket clients.
    pub fn new() -> (Self, mpsc::Receiver<VoiceStatus>) {
        let (status_tx, status_rx) = mpsc::channel(32);
        let channel = Self {
            status_tx,
            status: Arc::new(Mutex::new(VoiceStatus::Idle)),
        };
        (channel, status_rx)
    }

    /// Get the current voice status.
    pub async fn current_status(&self) -> VoiceStatus {
        *self.status.lock().await
    }

    /// Update the voice status and notify listeners.
    async fn set_status(&self, new_status: VoiceStatus) {
        let mut status = self.status.lock().await;
        *status = new_status;
        // Best-effort send -- if the receiver is dropped, we just log.
        if let Err(e) = self.status_tx.try_send(new_status) {
            debug!(
                status = %new_status,
                error = %e,
                "Status notification dropped (receiver full or closed)"
            );
        }
    }
}

#[async_trait]
impl ChannelAdapter for VoiceChannel {
    fn name(&self) -> &str {
        "voice"
    }

    fn display_name(&self) -> &str {
        "Voice (Talk Mode)"
    }

    fn supports_threads(&self) -> bool {
        false
    }

    fn supports_media(&self) -> bool {
        true
    }

    /// Start the voice channel loop.
    ///
    /// In the real implementation this would:
    /// 1. Start audio capture
    /// 2. Run VAD to detect speech segments
    /// 3. Feed speech to STT for transcription
    /// 4. Deliver transcribed text to the agent pipeline via `host`
    /// 5. Loop until cancelled
    ///
    /// The stub simply sets status to Listening and waits for cancellation.
    async fn start(
        &self,
        _host: Arc<dyn ChannelAdapterHost>,
        cancel: CancellationToken,
    ) -> Result<(), PluginError> {
        info!("Voice channel starting (stub mode)");
        self.set_status(VoiceStatus::Listening).await;

        // Stub: wait for cancellation.
        // In the real implementation, this loop would poll audio
        // capture, run VAD, and deliver transcribed utterances.
        cancel.cancelled().await;

        info!("Voice channel shutting down");
        self.set_status(VoiceStatus::Idle).await;
        Ok(())
    }

    /// Send a message through the voice channel (TTS output).
    ///
    /// In the real implementation this would synthesize the text
    /// via TTS and play it through the speaker. The stub logs the
    /// text and transitions status: Speaking -> Listening.
    async fn send(&self, _target: &str, payload: &MessagePayload) -> Result<String, PluginError> {
        let text = match payload.as_text() {
            Some(t) => t,
            None => {
                warn!("Voice channel received non-text payload, ignoring");
                return Ok("voice-skipped".into());
            }
        };

        info!(text = %text, "Voice channel would speak via TTS (stub)");
        self.set_status(VoiceStatus::Speaking).await;

        // Stub: in real implementation, TTS synthesis and playback happen here.
        // For now, just transition back to listening.
        self.set_status(VoiceStatus::Listening).await;

        Ok(format!("voice-{}", chrono::Utc::now().timestamp_millis()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_status_display() {
        assert_eq!(VoiceStatus::Idle.to_string(), "idle");
        assert_eq!(VoiceStatus::Listening.to_string(), "listening");
        assert_eq!(VoiceStatus::Transcribing.to_string(), "transcribing");
        assert_eq!(VoiceStatus::Processing.to_string(), "processing");
        assert_eq!(VoiceStatus::Speaking.to_string(), "speaking");
    }

    #[test]
    fn voice_status_serde_roundtrip() {
        let statuses = vec![
            VoiceStatus::Idle,
            VoiceStatus::Listening,
            VoiceStatus::Transcribing,
            VoiceStatus::Processing,
            VoiceStatus::Speaking,
        ];
        for status in &statuses {
            let json = serde_json::to_string(status).unwrap();
            let restored: VoiceStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(&restored, status);
        }
    }

    #[test]
    fn voice_status_json_values() {
        assert_eq!(
            serde_json::to_string(&VoiceStatus::Idle).unwrap(),
            "\"idle\""
        );
        assert_eq!(
            serde_json::to_string(&VoiceStatus::Listening).unwrap(),
            "\"listening\""
        );
        assert_eq!(
            serde_json::to_string(&VoiceStatus::Transcribing).unwrap(),
            "\"transcribing\""
        );
        assert_eq!(
            serde_json::to_string(&VoiceStatus::Processing).unwrap(),
            "\"processing\""
        );
        assert_eq!(
            serde_json::to_string(&VoiceStatus::Speaking).unwrap(),
            "\"speaking\""
        );
    }

    #[tokio::test]
    async fn voice_channel_name() {
        let (channel, _rx) = VoiceChannel::new();
        assert_eq!(channel.name(), "voice");
    }

    #[tokio::test]
    async fn voice_channel_display_name() {
        let (channel, _rx) = VoiceChannel::new();
        assert_eq!(channel.display_name(), "Voice (Talk Mode)");
    }

    #[tokio::test]
    async fn voice_channel_no_threads() {
        let (channel, _rx) = VoiceChannel::new();
        assert!(!channel.supports_threads());
    }

    #[tokio::test]
    async fn voice_channel_supports_media() {
        let (channel, _rx) = VoiceChannel::new();
        assert!(channel.supports_media());
    }

    #[tokio::test]
    async fn voice_channel_initial_status_is_idle() {
        let (channel, _rx) = VoiceChannel::new();
        assert_eq!(channel.current_status().await, VoiceStatus::Idle);
    }

    #[tokio::test]
    async fn voice_channel_send_with_text() {
        let (channel, mut rx) = VoiceChannel::new();
        let payload = MessagePayload::text("Hello from the agent");
        let msg_id = channel.send("user", &payload).await.unwrap();
        assert!(msg_id.starts_with("voice-"));

        // Should have received Speaking then Listening status updates.
        let s1 = rx.recv().await.unwrap();
        assert_eq!(s1, VoiceStatus::Speaking);
        let s2 = rx.recv().await.unwrap();
        assert_eq!(s2, VoiceStatus::Listening);
    }

    #[tokio::test]
    async fn voice_channel_send_with_non_text_returns_skipped() {
        let (channel, _rx) = VoiceChannel::new();
        let payload = MessagePayload::structured(serde_json::json!({"key": "val"}));
        let msg_id = channel.send("user", &payload).await.unwrap();
        assert_eq!(msg_id, "voice-skipped");
    }

    #[tokio::test]
    async fn voice_channel_start_and_cancel() {
        use std::collections::HashMap;

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

        let (channel, mut rx) = VoiceChannel::new();
        let channel = Arc::new(channel);
        let host: Arc<dyn ChannelAdapterHost> = Arc::new(StubHost);
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        let handle = tokio::spawn({
            let channel = Arc::clone(&channel);
            async move { channel.start(host, cancel_clone).await }
        });

        // Wait for the Listening status.
        let status = rx.recv().await.unwrap();
        assert_eq!(status, VoiceStatus::Listening);

        // Cancel and wait for shutdown.
        cancel.cancel();
        let result = handle.await.unwrap();
        assert!(result.is_ok());

        // Should have received Idle status on shutdown.
        let status = rx.recv().await.unwrap();
        assert_eq!(status, VoiceStatus::Idle);
    }
}
