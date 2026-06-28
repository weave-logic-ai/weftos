//! Heartbeat service.
//!
//! Periodically posts a prompt as an [`InboundMessage`] at a fixed interval,
//! useful for health checks or periodic agent nudges.
//!
//! Supports two modes:
//! - [`HeartbeatMode::Simple`] -- the original behavior: a single prompt at
//!   a fixed interval.
//! - [`HeartbeatMode::CheckIn`] -- proactive check-in mode: per-channel
//!   prompts triggered on a configurable schedule (e.g. cron).

use std::collections::HashMap;
use std::time::Duration;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::error::{Result, ServiceError};
use clawft_types::event::InboundMessage;

/// A target channel for proactive check-in heartbeats.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckInTarget {
    /// Channel name to probe (e.g. `"email"`, `"slack"`, `"discord"`).
    pub channel: String,

    /// Prompt text for this check-in (e.g. `"Check inbox for new emails"`).
    pub prompt: String,
}

/// Operating mode for the heartbeat service.
#[derive(Debug, Clone)]
pub enum HeartbeatMode {
    /// Original behavior: emit a single prompt at a fixed interval.
    Simple {
        /// Prompt text to emit.
        prompt: String,
    },
    /// Proactive check-in: emit per-channel prompts at each tick.
    /// Each target produces a separate [`InboundMessage`] with metadata
    /// identifying the heartbeat type and target channel.
    CheckIn {
        /// List of channels and their check-in prompts.
        targets: Vec<CheckInTarget>,
    },
}

/// A service that emits heartbeat messages at a regular interval.
pub struct HeartbeatService {
    interval: Duration,
    mode: HeartbeatMode,
    message_tx: mpsc::Sender<InboundMessage>,
}

impl HeartbeatService {
    /// Create a new heartbeat service in `Simple` mode.
    ///
    /// `interval_minutes` sets the delay between heartbeats.
    /// `prompt` is the message content delivered each heartbeat.
    pub fn new(
        interval_minutes: u64,
        prompt: String,
        message_tx: mpsc::Sender<InboundMessage>,
    ) -> Self {
        Self {
            interval: Duration::from_secs(interval_minutes * 60),
            mode: HeartbeatMode::Simple { prompt },
            message_tx,
        }
    }

    /// Create a new heartbeat service in `CheckIn` mode.
    ///
    /// `interval_minutes` sets the delay between check-in rounds.
    /// `targets` lists the channels and prompts for proactive check-ins.
    pub fn new_check_in(
        interval_minutes: u64,
        targets: Vec<CheckInTarget>,
        message_tx: mpsc::Sender<InboundMessage>,
    ) -> Self {
        Self {
            interval: Duration::from_secs(interval_minutes * 60),
            mode: HeartbeatMode::CheckIn { targets },
            message_tx,
        }
    }

    /// Start the heartbeat loop.
    ///
    /// Posts [`InboundMessage`](s) with `channel: "heartbeat"` at each tick.
    /// Exits gracefully when the cancellation token is triggered.
    pub async fn start(&self, cancel: CancellationToken) -> Result<()> {
        info!(
            interval_secs = self.interval.as_secs(),
            mode = match &self.mode {
                HeartbeatMode::Simple { .. } => "simple",
                HeartbeatMode::CheckIn { .. } => "check_in",
            },
            "heartbeat service started"
        );
        let mut interval = tokio::time::interval(self.interval);

        // The first tick fires immediately; skip it so the first heartbeat
        // happens after one full interval.
        interval.tick().await;

        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    info!("heartbeat service shutting down");
                    return Ok(());
                }
                _ = interval.tick() => {
                    self.emit_heartbeat()?;
                }
            }
        }
    }

    /// Emit one round of heartbeat messages based on the current mode.
    fn emit_heartbeat(&self) -> Result<()> {
        match &self.mode {
            HeartbeatMode::Simple { prompt } => {
                let msg = InboundMessage {
                    channel: "heartbeat".to_string(),
                    sender_id: "system".to_string(),
                    chat_id: "heartbeat".to_string(),
                    content: prompt.clone(),
                    timestamp: Utc::now(),
                    media: vec![],
                    metadata: HashMap::new(),
                };

                self.message_tx
                    .try_send(msg)
                    .map_err(|_| ServiceError::ChannelClosed)?;
            }
            HeartbeatMode::CheckIn { targets } => {
                for target in targets {
                    let mut metadata = HashMap::new();
                    metadata.insert("heartbeat_type".to_string(), serde_json::json!("check_in"));
                    metadata.insert(
                        "target_channel".to_string(),
                        serde_json::json!(target.channel),
                    );

                    let msg = InboundMessage {
                        channel: "heartbeat".to_string(),
                        sender_id: "system".to_string(),
                        chat_id: format!("heartbeat:{}", target.channel),
                        content: target.prompt.clone(),
                        timestamp: Utc::now(),
                        media: vec![],
                        metadata,
                    };

                    if let Err(e) = self.message_tx.try_send(msg) {
                        warn!(
                            target_channel = %target.channel,
                            "failed to send check-in heartbeat"
                        );
                        debug!(error = %e, "channel send error");
                        return Err(ServiceError::ChannelClosed);
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    // -- Existing tests (preserved unchanged) --

    #[tokio::test]
    async fn heartbeat_sends_messages() {
        let (tx, mut rx) = mpsc::channel(1024);
        let svc = HeartbeatService {
            interval: Duration::from_millis(50),
            mode: HeartbeatMode::Simple {
                prompt: "heartbeat check".into(),
            },
            message_tx: tx,
        };

        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        let handle = tokio::spawn(async move { svc.start(cancel_clone).await });

        // Wait for at least one heartbeat.
        tokio::time::sleep(Duration::from_millis(150)).await;
        cancel.cancel();

        let result = handle.await.unwrap();
        assert!(result.is_ok());

        // We should have received at least one message.
        let msg = rx.try_recv().unwrap();
        assert_eq!(msg.channel, "heartbeat");
        assert_eq!(msg.sender_id, "system");
        assert_eq!(msg.chat_id, "heartbeat");
        assert_eq!(msg.content, "heartbeat check");
    }

    #[tokio::test]
    async fn graceful_shutdown_on_cancel() {
        let (tx, _rx) = mpsc::channel(1024);
        let svc = HeartbeatService {
            interval: Duration::from_secs(3600), // long interval
            mode: HeartbeatMode::Simple {
                prompt: "test".into(),
            },
            message_tx: tx,
        };

        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        let handle = tokio::spawn(async move { svc.start(cancel_clone).await });

        // Cancel immediately.
        tokio::time::sleep(Duration::from_millis(10)).await;
        cancel.cancel();

        let result = handle.await.unwrap();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn channel_closed_returns_error() {
        let (tx, rx) = mpsc::channel(1024);
        let svc = HeartbeatService {
            interval: Duration::from_millis(10),
            mode: HeartbeatMode::Simple {
                prompt: "test".into(),
            },
            message_tx: tx,
        };

        // Drop the receiver so the channel is closed.
        drop(rx);

        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        let handle = tokio::spawn(async move { svc.start(cancel_clone).await });

        let result = handle.await.unwrap();
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ServiceError::ChannelClosed));
    }

    #[test]
    fn new_sets_interval_from_minutes() {
        let (tx, _rx) = mpsc::channel(1024);
        let svc = HeartbeatService::new(5, "test".into(), tx);
        assert_eq!(svc.interval, Duration::from_secs(300));
    }

    // -- New tests for CheckIn mode --

    #[test]
    fn new_check_in_creates_check_in_mode() {
        let (tx, _rx) = mpsc::channel(1024);
        let targets = vec![
            CheckInTarget {
                channel: "email".into(),
                prompt: "Check inbox".into(),
            },
            CheckInTarget {
                channel: "slack".into(),
                prompt: "Check Slack channels".into(),
            },
        ];
        let svc = HeartbeatService::new_check_in(10, targets, tx);
        assert_eq!(svc.interval, Duration::from_secs(600));
        assert!(matches!(svc.mode, HeartbeatMode::CheckIn { .. }));
    }

    #[tokio::test]
    async fn check_in_sends_per_channel_messages() {
        let (tx, mut rx) = mpsc::channel(1024);
        let targets = vec![
            CheckInTarget {
                channel: "email".into(),
                prompt: "Check inbox for new emails".into(),
            },
            CheckInTarget {
                channel: "slack".into(),
                prompt: "Check Slack channels for updates".into(),
            },
        ];
        let svc = HeartbeatService {
            interval: Duration::from_millis(50),
            mode: HeartbeatMode::CheckIn { targets },
            message_tx: tx,
        };

        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        let handle = tokio::spawn(async move { svc.start(cancel_clone).await });

        // Wait for at least one tick.
        tokio::time::sleep(Duration::from_millis(150)).await;
        cancel.cancel();

        let result = handle.await.unwrap();
        assert!(result.is_ok());

        // Should have at least 2 messages (one per target per tick).
        let msg1 = rx.try_recv().unwrap();
        let msg2 = rx.try_recv().unwrap();

        // Verify email check-in message.
        assert_eq!(msg1.channel, "heartbeat");
        assert_eq!(msg1.sender_id, "system");
        assert_eq!(msg1.chat_id, "heartbeat:email");
        assert_eq!(msg1.content, "Check inbox for new emails");
        assert_eq!(
            msg1.metadata.get("heartbeat_type"),
            Some(&serde_json::json!("check_in"))
        );
        assert_eq!(
            msg1.metadata.get("target_channel"),
            Some(&serde_json::json!("email"))
        );

        // Verify slack check-in message.
        assert_eq!(msg2.channel, "heartbeat");
        assert_eq!(msg2.chat_id, "heartbeat:slack");
        assert_eq!(msg2.content, "Check Slack channels for updates");
        assert_eq!(
            msg2.metadata.get("heartbeat_type"),
            Some(&serde_json::json!("check_in"))
        );
        assert_eq!(
            msg2.metadata.get("target_channel"),
            Some(&serde_json::json!("slack"))
        );
    }

    #[tokio::test]
    async fn check_in_empty_targets_sends_nothing() {
        let (tx, mut rx) = mpsc::channel(1024);
        let svc = HeartbeatService {
            interval: Duration::from_millis(50),
            mode: HeartbeatMode::CheckIn { targets: vec![] },
            message_tx: tx,
        };

        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        let handle = tokio::spawn(async move { svc.start(cancel_clone).await });

        tokio::time::sleep(Duration::from_millis(150)).await;
        cancel.cancel();

        let result = handle.await.unwrap();
        assert!(result.is_ok());

        // No targets means no messages.
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn check_in_channel_closed_returns_error() {
        let (tx, rx) = mpsc::channel(1024);
        let targets = vec![CheckInTarget {
            channel: "email".into(),
            prompt: "check".into(),
        }];
        let svc = HeartbeatService {
            interval: Duration::from_millis(10),
            mode: HeartbeatMode::CheckIn { targets },
            message_tx: tx,
        };

        // Drop receiver to close the channel.
        drop(rx);

        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        let handle = tokio::spawn(async move { svc.start(cancel_clone).await });

        let result = handle.await.unwrap();
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ServiceError::ChannelClosed));
    }

    #[test]
    fn check_in_target_serde_roundtrip() {
        let target = CheckInTarget {
            channel: "email".into(),
            prompt: "Check inbox".into(),
        };
        let json = serde_json::to_string(&target).unwrap();
        let restored: CheckInTarget = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.channel, "email");
        assert_eq!(restored.prompt, "Check inbox");
    }

    #[test]
    fn emit_heartbeat_simple_mode() {
        let (tx, mut rx) = mpsc::channel(1024);
        let svc = HeartbeatService {
            interval: Duration::from_secs(60),
            mode: HeartbeatMode::Simple {
                prompt: "simple check".into(),
            },
            message_tx: tx,
        };

        svc.emit_heartbeat().unwrap();

        let msg = rx.try_recv().unwrap();
        assert_eq!(msg.channel, "heartbeat");
        assert_eq!(msg.content, "simple check");
        assert!(msg.metadata.is_empty());
    }

    #[test]
    fn emit_heartbeat_check_in_mode() {
        let (tx, mut rx) = mpsc::channel(1024);
        let targets = vec![
            CheckInTarget {
                channel: "email".into(),
                prompt: "Email check".into(),
            },
            CheckInTarget {
                channel: "discord".into(),
                prompt: "Discord check".into(),
            },
        ];
        let svc = HeartbeatService {
            interval: Duration::from_secs(60),
            mode: HeartbeatMode::CheckIn { targets },
            message_tx: tx,
        };

        svc.emit_heartbeat().unwrap();

        let msg1 = rx.try_recv().unwrap();
        assert_eq!(msg1.chat_id, "heartbeat:email");
        assert_eq!(msg1.content, "Email check");
        assert_eq!(
            msg1.metadata.get("heartbeat_type"),
            Some(&serde_json::json!("check_in"))
        );
        assert_eq!(
            msg1.metadata.get("target_channel"),
            Some(&serde_json::json!("email"))
        );

        let msg2 = rx.try_recv().unwrap();
        assert_eq!(msg2.chat_id, "heartbeat:discord");
        assert_eq!(msg2.content, "Discord check");
    }
}
