//! [`DiscordChannel`] -- `Channel` trait implementation for Discord.
//!
//! Uses the Discord Gateway WebSocket protocol to receive events and
//! delivers them to the pipeline through
//! [`ChannelHost::deliver_inbound`](crate::traits::ChannelHost::deliver_inbound).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::RwLock;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use clawft_types::config::DiscordConfig;
use clawft_types::error::ChannelError;
use clawft_types::event::{InboundMessage, OutboundMessage};

use crate::traits::{Channel, ChannelHost, ChannelMetadata, ChannelStatus, MessageId};

use super::api::DiscordApiClient;
use super::events::{
    ConnectionProperties, GatewayPayload, HelloData, IdentifyPayload, MessageCreate, OP_DISPATCH,
    OP_HEARTBEAT, OP_HEARTBEAT_ACK, OP_HELLO, OP_INVALID_SESSION, OP_RECONNECT, OP_RESUME,
    ReadyEvent, ResumePayload,
};

/// Delay before reconnecting after a connection failure.
const RECONNECT_DELAY_SECS: u64 = 5;

/// Maximum message length for Discord (standard limit).
/// Discord allows up to 2000 characters per message (4000 for Nitro).
/// We use 2000 to be universally safe.
const DISCORD_MAX_MESSAGE_LEN: usize = 2000;

/// Split a message into chunks that fit within the Discord character limit.
///
/// Tries to split at line boundaries first, then word boundaries. If a
/// single word exceeds the limit, it is hard-split at the limit.
fn chunk_message(content: &str, max_len: usize) -> Vec<&str> {
    if content.len() <= max_len {
        return vec![content];
    }

    let mut chunks = Vec::new();
    let mut remaining = content;

    while !remaining.is_empty() {
        if remaining.len() <= max_len {
            chunks.push(remaining);
            break;
        }

        // Find the best split point within max_len.
        let search_range = &remaining[..max_len];

        // Prefer splitting at the last newline.
        let split_at = search_range
            .rfind('\n')
            // Fall back to last space.
            .or_else(|| search_range.rfind(' '))
            // Hard-split if no natural boundary.
            .map(|pos| pos + 1)
            .unwrap_or(max_len);

        let (chunk, rest) = remaining.split_at(split_at);
        let chunk = chunk.trim_end();
        if !chunk.is_empty() {
            chunks.push(chunk);
        }
        remaining = rest.trim_start_matches('\n');
        if remaining.is_empty() {
            break;
        }
    }

    if chunks.is_empty() {
        chunks.push(content);
    }

    chunks
}

/// Discord channel implementation using the Gateway WebSocket protocol.
///
/// # Configuration
///
/// Created via [`DiscordChannelFactory`](super::factory::DiscordChannelFactory)
/// from the `DiscordConfig` section of the global config.
pub struct DiscordChannel {
    /// Discord REST API client (for sending messages).
    api: DiscordApiClient,
    /// Current lifecycle status.
    status: Arc<RwLock<ChannelStatus>>,
    /// Parsed configuration.
    config: DiscordConfig,
    /// Last received sequence number for heartbeats and resuming.
    sequence: AtomicU64,
    /// Session ID from the READY event (for resuming).
    session_id: RwLock<Option<String>>,
    /// Resume gateway URL from the READY event.
    resume_url: RwLock<Option<String>>,
}

impl DiscordChannel {
    /// Create a new Discord channel from configuration.
    pub fn new(config: DiscordConfig) -> Self {
        Self {
            api: DiscordApiClient::new(config.token.expose().to_owned()),
            status: Arc::new(RwLock::new(ChannelStatus::Stopped)),
            config,
            sequence: AtomicU64::new(0),
            session_id: RwLock::new(None),
            resume_url: RwLock::new(None),
        }
    }

    /// Create a channel with a custom [`DiscordApiClient`] (for testing).
    #[cfg(test)]
    pub fn with_api(config: DiscordConfig, api: DiscordApiClient) -> Self {
        Self {
            api,
            status: Arc::new(RwLock::new(ChannelStatus::Stopped)),
            config,
            sequence: AtomicU64::new(0),
            session_id: RwLock::new(None),
            resume_url: RwLock::new(None),
        }
    }

    /// Set status under the write lock.
    pub(crate) async fn set_status(&self, status: ChannelStatus) {
        *self.status.write().await = status;
    }

    /// Process a MESSAGE_CREATE event, delivering it to the host.
    pub(crate) async fn process_message_create(
        &self,
        msg: &MessageCreate,
        host: &Arc<dyn ChannelHost>,
    ) -> Result<(), ChannelError> {
        // Skip bot messages to avoid loops.
        if msg.author.bot {
            debug!(
                author = %msg.author.username,
                "skipping bot message"
            );
            return Ok(());
        }

        let sender_id = &msg.author.id;

        if !self.is_allowed(sender_id) {
            warn!(
                sender_id = %sender_id,
                channel_id = %msg.channel_id,
                "message from disallowed user, ignoring"
            );
            return Ok(());
        }

        let mut metadata = HashMap::new();
        metadata.insert(
            "message_id".into(),
            serde_json::Value::String(msg.id.clone()),
        );
        metadata.insert(
            "username".into(),
            serde_json::Value::String(msg.author.username.clone()),
        );
        if let Some(ref guild_id) = msg.guild_id {
            metadata.insert(
                "guild_id".into(),
                serde_json::Value::String(guild_id.clone()),
            );
        }
        if let Some(ref reference) = msg.message_reference
            && let Some(ref ref_id) = reference.message_id
        {
            metadata.insert(
                "reply_to_message_id".into(),
                serde_json::Value::String(ref_id.clone()),
            );
        }

        // Signal that the sender passed the allow_from check so the
        // permission resolver can promote them from zero-trust to user level.
        if !self.config.allow_from.is_empty() {
            metadata.insert("allow_from_match".into(), serde_json::Value::Bool(true));
        }

        let inbound = InboundMessage {
            channel: "discord".into(),
            sender_id: sender_id.clone(),
            chat_id: msg.channel_id.clone(),
            content: msg.content.clone(),
            timestamp: chrono::Utc::now(),
            media: vec![],
            metadata,
        };

        host.deliver_inbound(inbound).await
    }
}

#[async_trait]
impl Channel for DiscordChannel {
    fn name(&self) -> &str {
        "discord"
    }

    fn metadata(&self) -> ChannelMetadata {
        ChannelMetadata {
            name: "discord".into(),
            display_name: "Discord".into(),
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
        self.config.allow_from.is_empty() || self.config.allow_from.iter().any(|id| id == sender_id)
    }

    async fn start(
        &self,
        host: Arc<dyn ChannelHost>,
        cancel: CancellationToken,
    ) -> Result<(), ChannelError> {
        self.set_status(ChannelStatus::Starting).await;

        info!("Discord channel starting");

        // Main reconnection loop.
        loop {
            let gateway_url = {
                let resume = self.resume_url.read().await;
                resume
                    .clone()
                    .unwrap_or_else(|| self.config.gateway_url.clone())
            };

            // Connect to Gateway WebSocket.
            let ws_stream = match tokio_tungstenite::connect_async(&gateway_url).await {
                Ok((stream, _)) => stream,
                Err(e) => {
                    error!(error = %e, "failed to connect Discord Gateway");
                    self.set_status(ChannelStatus::Error(e.to_string())).await;

                    tokio::select! {
                        _ = cancel.cancelled() => break,
                        _ = tokio::time::sleep(
                            std::time::Duration::from_secs(RECONNECT_DELAY_SECS)
                        ) => continue,
                    }
                }
            };

            info!("Discord Gateway connected");

            let (mut ws_write, mut ws_read) = ws_stream.split();

            // Wait for Hello (opcode 10).
            let heartbeat_interval = loop {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        let _ = ws_write.close().await;
                        self.set_status(ChannelStatus::Stopped).await;
                        return Ok(());
                    }
                    msg = ws_read.next() => {
                        match msg {
                            Some(Ok(WsMessage::Text(text))) => {
                                if let Ok(payload) = serde_json::from_str::<GatewayPayload>(&text)
                                    && payload.op == OP_HELLO
                                    && let Some(d) = payload.d
                                    && let Ok(hello) = serde_json::from_value::<HelloData>(d)
                                {
                                    break hello.heartbeat_interval;
                                }
                            }
                            Some(Err(e)) => {
                                error!(error = %e, "WebSocket error waiting for Hello");
                                break 41250; // fallback
                            }
                            None => break 41250,
                            _ => {}
                        }
                    }
                }
            };

            debug!(interval_ms = heartbeat_interval, "received Hello");

            // Decide whether to Resume (OP 6) or Identify (OP 2).
            // If we have a session_id from a previous READY event, attempt
            // to resume the session; otherwise, start fresh with Identify.
            let auth_payload = {
                let session_id_guard = self.session_id.read().await;
                if let Some(ref sid) = *session_id_guard {
                    let seq = self.sequence.load(Ordering::SeqCst);
                    info!(
                        session_id = %sid,
                        seq = seq,
                        "attempting Resume (OP 6)"
                    );
                    GatewayPayload {
                        op: OP_RESUME,
                        d: Some(
                            serde_json::to_value(ResumePayload {
                                token: self.config.token.expose().to_owned(),
                                session_id: sid.clone(),
                                seq,
                            })
                            .unwrap_or_default(),
                        ),
                        s: None,
                        t: None,
                    }
                } else {
                    debug!("no session_id available, sending Identify (OP 2)");
                    GatewayPayload {
                        op: super::events::OP_IDENTIFY,
                        d: Some(
                            serde_json::to_value(IdentifyPayload {
                                token: self.config.token.expose().to_owned(),
                                intents: self.config.intents,
                                properties: ConnectionProperties {
                                    os: std::env::consts::OS.to_owned(),
                                    browser: "clawft".into(),
                                    device: "clawft".into(),
                                },
                            })
                            .unwrap_or_default(),
                        ),
                        s: None,
                        t: None,
                    }
                }
            };

            if let Ok(json) = serde_json::to_string(&auth_payload)
                && let Err(e) = ws_write.send(WsMessage::Text(json)).await
            {
                error!(error = %e, "failed to send Resume/Identify");
                self.set_status(ChannelStatus::Error(e.to_string())).await;

                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = tokio::time::sleep(
                        std::time::Duration::from_secs(RECONNECT_DELAY_SECS)
                    ) => continue,
                }
            }

            self.set_status(ChannelStatus::Running).await;

            // Start heartbeat timer.
            let mut heartbeat_timer =
                tokio::time::interval(std::time::Duration::from_millis(heartbeat_interval));
            // First tick fires immediately; skip it and wait for the
            // first real interval.
            heartbeat_timer.tick().await;

            // Message processing loop.
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        info!("Discord channel received cancellation");
                        let _ = ws_write.close().await;
                        self.set_status(ChannelStatus::Stopped).await;
                        return Ok(());
                    }
                    _ = heartbeat_timer.tick() => {
                        let seq = self.sequence.load(Ordering::SeqCst);
                        let hb = GatewayPayload {
                            op: OP_HEARTBEAT,
                            d: if seq > 0 { Some(serde_json::json!(seq)) } else { None },
                            s: None,
                            t: None,
                        };
                        if let Ok(json) = serde_json::to_string(&hb) {
                            if let Err(e) = ws_write.send(WsMessage::Text(json)).await {
                                warn!(error = %e, "failed to send heartbeat");
                                break;
                            }
                            debug!(seq = seq, "sent heartbeat");
                        }
                    }
                    msg = ws_read.next() => {
                        match msg {
                            Some(Ok(WsMessage::Text(text))) => {
                                match serde_json::from_str::<GatewayPayload>(&text) {
                                    Ok(payload) => {
                                        // Update sequence number.
                                        if let Some(s) = payload.s {
                                            self.sequence.store(s, Ordering::SeqCst);
                                        }

                                        match payload.op {
                                            OP_DISPATCH => {
                                                if let Some(ref event_name) = payload.t {
                                                    match event_name.as_str() {
                                                        "READY" => {
                                                            if let Some(ref d) = payload.d
                                                                && let Ok(ready) = serde_json::from_value::<ReadyEvent>(d.clone())
                                                            {
                                                                info!(
                                                                    bot_id = %ready.user.id,
                                                                    bot_name = %ready.user.username,
                                                                    "Discord bot authenticated"
                                                                );
                                                                *self.session_id.write().await = Some(ready.session_id);
                                                                *self.resume_url.write().await = ready.resume_gateway_url;
                                                            }
                                                        }
                                                        "RESUMED" => {
                                                            info!("session resumed successfully");
                                                        }
                                                        "MESSAGE_CREATE" => {
                                                            if let Some(ref d) = payload.d {
                                                                match serde_json::from_value::<MessageCreate>(d.clone()) {
                                                                    Ok(msg) => {
                                                                        if let Err(e) = self.process_message_create(&msg, &host).await {
                                                                            error!(
                                                                                error = %e,
                                                                                "failed to process MESSAGE_CREATE"
                                                                            );
                                                                        }
                                                                    }
                                                                    Err(e) => {
                                                                        warn!(
                                                                            error = %e,
                                                                            "failed to parse MESSAGE_CREATE"
                                                                        );
                                                                    }
                                                                }
                                                            }
                                                        }
                                                        _ => {
                                                            debug!(
                                                                event = %event_name,
                                                                "unhandled dispatch event"
                                                            );
                                                        }
                                                    }
                                                }
                                            }
                                            OP_HEARTBEAT_ACK => {
                                                debug!("heartbeat acknowledged");
                                            }
                                            OP_HEARTBEAT => {
                                                // Server requesting immediate heartbeat.
                                                let seq = self.sequence.load(Ordering::SeqCst);
                                                let hb = GatewayPayload {
                                                    op: OP_HEARTBEAT,
                                                    d: if seq > 0 { Some(serde_json::json!(seq)) } else { None },
                                                    s: None,
                                                    t: None,
                                                };
                                                if let Ok(json) = serde_json::to_string(&hb) {
                                                    let _ = ws_write.send(WsMessage::Text(json)).await;
                                                }
                                            }
                                            OP_RECONNECT => {
                                                info!("server requested reconnect");
                                                break;
                                            }
                                            OP_INVALID_SESSION => {
                                                // d: true => session is resumable; wait
                                                // with jitter then retry resume.
                                                // d: false => not resumable; clear state
                                                // and fall back to fresh Identify.
                                                let resumable = payload
                                                    .d
                                                    .as_ref()
                                                    .and_then(|v| v.as_bool())
                                                    .unwrap_or(false);

                                                if resumable {
                                                    // Wait 1-5 seconds (random jitter)
                                                    // then reconnect; session state is
                                                    // preserved so the next loop
                                                    // iteration will send Resume.
                                                    let jitter_ms = 1000
                                                        + (std::time::SystemTime::now()
                                                            .duration_since(
                                                                std::time::UNIX_EPOCH,
                                                            )
                                                            .unwrap_or_default()
                                                            .subsec_millis()
                                                            % 4000);
                                                    warn!(
                                                        jitter_ms,
                                                        "invalid session (resumable), retrying"
                                                    );
                                                    tokio::time::sleep(
                                                        std::time::Duration::from_millis(
                                                            jitter_ms.into(),
                                                        ),
                                                    )
                                                    .await;
                                                } else {
                                                    warn!(
                                                        "invalid session (not resumable), \
                                                         clearing state for fresh Identify"
                                                    );
                                                    *self.session_id.write().await = None;
                                                    *self.resume_url.write().await = None;
                                                    self.sequence.store(0, Ordering::SeqCst);
                                                }
                                                break;
                                            }
                                            _ => {
                                                debug!(op = payload.op, "unhandled opcode");
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        warn!(error = %e, "failed to parse gateway payload");
                                    }
                                }
                            }
                            Some(Ok(WsMessage::Close(_))) => {
                                info!("Discord Gateway closed by server");
                                break;
                            }
                            Some(Ok(WsMessage::Ping(data))) => {
                                let _ = ws_write.send(WsMessage::Pong(data)).await;
                            }
                            Some(Err(e)) => {
                                error!(error = %e, "Discord Gateway WebSocket error");
                                break;
                            }
                            None => {
                                info!("Discord Gateway stream ended");
                                break;
                            }
                            _ => {} // Binary, Pong, Frame -- ignore
                        }
                    }
                }
            }

            // Connection dropped. Reconnect unless cancelled.
            self.set_status(ChannelStatus::Error("disconnected".into()))
                .await;

            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = tokio::time::sleep(
                    std::time::Duration::from_secs(RECONNECT_DELAY_SECS)
                ) => {
                    info!("reconnecting Discord Gateway...");
                }
            }
        }

        self.set_status(ChannelStatus::Stopped).await;
        info!("Discord channel stopped");
        Ok(())
    }

    async fn send(&self, msg: &OutboundMessage) -> Result<MessageId, ChannelError> {
        let chunks = chunk_message(&msg.content, DISCORD_MAX_MESSAGE_LEN);
        let mut last_id = String::new();

        for chunk in &chunks {
            last_id = self.api.create_message(&msg.chat_id, chunk).await?;
        }

        Ok(MessageId(last_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_short_message() {
        let msg = "Hello, world!";
        let chunks = chunk_message(msg, 2000);
        assert_eq!(chunks, vec!["Hello, world!"]);
    }

    #[test]
    fn chunk_empty_message() {
        let chunks = chunk_message("", 2000);
        assert_eq!(chunks, vec![""]);
    }

    #[test]
    fn chunk_at_newline_boundary() {
        let line = "x".repeat(900);
        let msg = format!("{line}\n{line}\n{line}");
        let chunks = chunk_message(&msg, 2000);
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].len() <= 2000);
        assert!(chunks[1].len() <= 2000);
    }

    #[test]
    fn chunk_at_space_boundary() {
        // No newlines, but has spaces
        let word = "word ".repeat(500); // ~2500 chars
        let chunks = chunk_message(&word, 2000);
        assert!(chunks.len() >= 2);
        for chunk in &chunks {
            assert!(chunk.len() <= 2000);
        }
    }

    #[test]
    fn chunk_hard_split_no_boundaries() {
        // Single long word with no spaces or newlines
        let long = "a".repeat(5000);
        let chunks = chunk_message(&long, 2000);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].len(), 2000);
        assert_eq!(chunks[1].len(), 2000);
        assert_eq!(chunks[2].len(), 1000);
    }

    #[test]
    fn chunk_exactly_at_limit() {
        let msg = "a".repeat(2000);
        let chunks = chunk_message(&msg, 2000);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), 2000);
    }

    #[test]
    fn chunk_one_over_limit() {
        let msg = "a".repeat(2001);
        let chunks = chunk_message(&msg, 2000);
        assert_eq!(chunks.len(), 2);
    }

    #[test]
    fn chunk_preserves_all_content() {
        let msg = "Hello\nWorld\nThis is a test\nOf message chunking";
        let chunks = chunk_message(msg, 20);
        let reassembled: String = chunks.join("\n");
        // All words should be present
        assert!(reassembled.contains("Hello"));
        assert!(reassembled.contains("World"));
        assert!(reassembled.contains("chunking"));
    }
}
