//! [`SlackChannel`] -- `Channel` trait implementation for Slack.
//!
//! Uses Slack Socket Mode to receive events over a WebSocket connection
//! and delivers them to the pipeline through
//! [`ChannelHost::deliver_inbound`](crate::traits::ChannelHost::deliver_inbound).

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::RwLock;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use clawft_types::config::SlackConfig;
use clawft_types::error::ChannelError;
use clawft_types::event::{InboundMessage, OutboundMessage};

use crate::traits::{Channel, ChannelHost, ChannelMetadata, ChannelStatus, MessageId};

use super::api::SlackApiClient;
use super::events::{SlackAcknowledge, SlackEnvelope};

/// Delay before retrying after a WebSocket connection failure.
const RECONNECT_DELAY_SECS: u64 = 5;

/// Slack channel implementation using Socket Mode.
///
/// # Configuration
///
/// Created via [`SlackChannelFactory`](super::factory::SlackChannelFactory)
/// from the `SlackConfig` section of the global config.
pub struct SlackChannel {
    /// Slack Web API client (for sending messages).
    api: SlackApiClient,
    /// App-level token for Socket Mode connection.
    app_token: String,
    /// Current lifecycle status.
    status: Arc<RwLock<ChannelStatus>>,
    /// Parsed configuration.
    config: SlackConfig,
}

impl SlackChannel {
    /// Create a new Slack channel from configuration.
    pub fn new(config: SlackConfig) -> Self {
        Self {
            api: SlackApiClient::new(config.bot_token.expose().to_owned()),
            app_token: config.app_token.expose().to_owned(),
            status: Arc::new(RwLock::new(ChannelStatus::Stopped)),
            config,
        }
    }

    /// Create a channel with a custom [`SlackApiClient`] (for testing).
    #[cfg(test)]
    pub fn with_api(config: SlackConfig, api: SlackApiClient) -> Self {
        Self {
            api,
            app_token: config.app_token.expose().to_owned(),
            status: Arc::new(RwLock::new(ChannelStatus::Stopped)),
            config,
        }
    }

    /// Set status under the write lock.
    async fn set_status(&self, status: ChannelStatus) {
        *self.status.write().await = status;
    }

    /// Check if a sender is allowed based on the configured policies.
    ///
    /// - For DMs (`channel_type == "im"`): checks `dm.policy` and `dm.allow_from`.
    /// - For group channels: checks `group_policy` and `group_allow_from`.
    /// - Empty allow-lists mean everyone is allowed.
    pub fn check_allowed(&self, sender_id: &str, channel_type: Option<&str>) -> bool {
        let is_dm = channel_type == Some("im");

        if is_dm {
            if !self.config.dm.enabled {
                return false;
            }
            if self.config.dm.policy == "allowlist" {
                return self.config.dm.allow_from.is_empty()
                    || self.config.dm.allow_from.iter().any(|id| id == sender_id);
            }
            // policy == "open"
            return true;
        }

        // Group/channel message
        match self.config.group_policy.as_str() {
            "allowlist" => {
                self.config.group_allow_from.is_empty()
                    || self
                        .config
                        .group_allow_from
                        .iter()
                        .any(|id| id == sender_id)
            }
            "mention" => {
                // For mention policy, any user can trigger via @mention.
                // The event type filtering (app_mention) handles this.
                true
            }
            "open" => true,
            _ => true, // Unknown policy defaults to open.
        }
    }

    /// Process a single Socket Mode envelope, delivering any relevant
    /// event to the host.
    pub(crate) async fn process_envelope(
        &self,
        envelope: &SlackEnvelope,
        host: &Arc<dyn ChannelHost>,
    ) -> Result<(), ChannelError> {
        // Only process events_api envelopes.
        if envelope.envelope_type != "events_api" {
            debug!(
                envelope_type = %envelope.envelope_type,
                "skipping non-events_api envelope"
            );
            return Ok(());
        }

        let Some(ref payload) = envelope.payload else {
            return Ok(());
        };

        let Some(ref event) = payload.event else {
            return Ok(());
        };

        // Skip bot messages to avoid loops.
        if event.bot_id.is_some() {
            debug!("skipping bot message");
            return Ok(());
        }

        // Only process message and app_mention events.
        match event.event_type.as_str() {
            "message" | "app_mention" => {}
            other => {
                debug!(event_type = %other, "skipping unhandled event type");
                return Ok(());
            }
        }

        let Some(ref text) = event.text else {
            return Ok(());
        };

        let sender_id = event.user.as_deref().unwrap_or_default();
        let channel_id = event.channel.as_deref().unwrap_or_default();

        if !self.check_allowed(sender_id, event.channel_type.as_deref()) {
            warn!(
                sender_id = %sender_id,
                channel = %channel_id,
                "message from disallowed user, ignoring"
            );
            return Ok(());
        }

        let mut metadata = HashMap::new();
        if let Some(ref ts) = event.ts {
            metadata.insert("ts".into(), serde_json::Value::String(ts.clone()));
        }
        if let Some(ref thread_ts) = event.thread_ts {
            metadata.insert(
                "thread_ts".into(),
                serde_json::Value::String(thread_ts.clone()),
            );
        }
        metadata.insert(
            "event_type".into(),
            serde_json::Value::String(event.event_type.clone()),
        );
        if let Some(ref ct) = event.channel_type {
            metadata.insert("channel_type".into(), serde_json::Value::String(ct.clone()));
        }

        // WEFT-162: signal that the sender passed an explicit allow_from
        // check so the permission resolver can promote them from
        // zero-trust to user level. Mirrors the Discord channel; only
        // emitted when the relevant allow-list is non-empty AND
        // contains the sender (i.e. a real positive match, not
        // "everyone allowed because the list is empty").
        let is_dm = event.channel_type.as_deref() == Some("im");
        let allow_match = if is_dm {
            !self.config.dm.allow_from.is_empty()
                && self.config.dm.allow_from.iter().any(|id| id == sender_id)
        } else {
            !self.config.group_allow_from.is_empty()
                && self
                    .config
                    .group_allow_from
                    .iter()
                    .any(|id| id == sender_id)
        };
        if allow_match {
            metadata.insert(
                "allow_from_match".into(),
                serde_json::Value::Bool(true),
            );
        }

        let inbound = InboundMessage {
            channel: "slack".into(),
            sender_id: sender_id.to_owned(),
            chat_id: channel_id.to_owned(),
            content: text.clone(),
            timestamp: chrono::Utc::now(),
            media: vec![],
            metadata,
        };

        host.deliver_inbound(inbound).await
    }
}

#[async_trait]
impl Channel for SlackChannel {
    fn name(&self) -> &str {
        "slack"
    }

    fn metadata(&self) -> ChannelMetadata {
        ChannelMetadata {
            name: "slack".into(),
            display_name: "Slack".into(),
            supports_threads: true,
            supports_media: true,
        }
    }

    fn status(&self) -> ChannelStatus {
        self.status
            .try_read()
            .map(|s| s.clone())
            .unwrap_or(ChannelStatus::Stopped)
    }

    fn is_allowed(&self, sender_id: &str) -> bool {
        // Default check: if no specific policy is set, use DM open policy.
        self.check_allowed(sender_id, None)
    }

    async fn start(
        &self,
        host: Arc<dyn ChannelHost>,
        cancel: CancellationToken,
    ) -> Result<(), ChannelError> {
        self.set_status(ChannelStatus::Starting).await;

        info!("Slack channel starting in Socket Mode");

        // Main reconnection loop.
        loop {
            // Obtain a WebSocket URL.
            let ws_url = match self.api.apps_connections_open(&self.app_token).await {
                Ok(url) => url,
                Err(e) => {
                    error!(error = %e, "failed to obtain Slack WebSocket URL");
                    self.set_status(ChannelStatus::Error(e.to_string())).await;

                    tokio::select! {
                        _ = cancel.cancelled() => break,
                        _ = tokio::time::sleep(
                            std::time::Duration::from_secs(RECONNECT_DELAY_SECS)
                        ) => continue,
                    }
                }
            };

            // Connect to WebSocket.
            let ws_stream = match tokio_tungstenite::connect_async(&ws_url).await {
                Ok((stream, _)) => stream,
                Err(e) => {
                    error!(error = %e, "failed to connect Slack WebSocket");
                    self.set_status(ChannelStatus::Error(e.to_string())).await;

                    tokio::select! {
                        _ = cancel.cancelled() => break,
                        _ = tokio::time::sleep(
                            std::time::Duration::from_secs(RECONNECT_DELAY_SECS)
                        ) => continue,
                    }
                }
            };

            self.set_status(ChannelStatus::Running).await;
            info!("Slack WebSocket connected");

            let (mut ws_write, mut ws_read) = ws_stream.split();

            // Message processing loop for this connection.
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        info!("Slack channel received cancellation");
                        // Close the WebSocket gracefully.
                        let _ = ws_write.close().await;
                        self.set_status(ChannelStatus::Stopped).await;
                        return Ok(());
                    }
                    msg = ws_read.next() => {
                        match msg {
                            Some(Ok(WsMessage::Text(text))) => {
                                // Try to parse as an envelope.
                                match serde_json::from_str::<SlackEnvelope>(&text) {
                                    Ok(envelope) => {
                                        // Acknowledge the envelope.
                                        let ack = SlackAcknowledge {
                                            envelope_id: envelope.envelope_id.clone(),
                                            payload: None,
                                        };
                                        if let Ok(ack_json) = serde_json::to_string(&ack)
                                            && let Err(e) = ws_write
                                                .send(WsMessage::Text(ack_json))
                                                .await
                                        {
                                            warn!(
                                                error = %e,
                                                "failed to send acknowledge"
                                            );
                                        }

                                        // Process the event.
                                        if let Err(e) =
                                            self.process_envelope(&envelope, &host).await
                                        {
                                            error!(
                                                error = %e,
                                                "failed to process Slack envelope"
                                            );
                                        }
                                    }
                                    Err(_) => {
                                        // May be a hello or disconnect message.
                                        debug!(raw = %text, "received non-envelope message");
                                    }
                                }
                            }
                            Some(Ok(WsMessage::Close(_))) => {
                                info!("Slack WebSocket closed by server");
                                break;
                            }
                            Some(Ok(WsMessage::Ping(data))) => {
                                let _ = ws_write.send(WsMessage::Pong(data)).await;
                            }
                            Some(Err(e)) => {
                                error!(error = %e, "Slack WebSocket error");
                                break;
                            }
                            None => {
                                info!("Slack WebSocket stream ended");
                                break;
                            }
                            _ => {} // Binary, Pong, Frame -- ignore
                        }
                    }
                }
            }

            // If we get here, the connection dropped. Reconnect unless
            // cancellation was requested.
            self.set_status(ChannelStatus::Error("disconnected".into()))
                .await;

            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = tokio::time::sleep(
                    std::time::Duration::from_secs(RECONNECT_DELAY_SECS)
                ) => {
                    info!("reconnecting Slack WebSocket...");
                }
            }
        }

        self.set_status(ChannelStatus::Stopped).await;
        info!("Slack channel stopped");
        Ok(())
    }

    async fn send(&self, msg: &OutboundMessage) -> Result<MessageId, ChannelError> {
        let thread_ts = msg.metadata.get("thread_ts").and_then(|v| v.as_str());

        let ts = self
            .api
            .chat_post_message(&msg.chat_id, &msg.content, thread_ts)
            .await?;

        Ok(MessageId(ts))
    }
}
