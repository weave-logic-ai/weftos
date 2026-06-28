//! WebSocket handler for real-time topic-based events.
//!
//! Clients connect to `/ws` and can subscribe to named topics (e.g.
//! `"agents"`, `"sessions:abc123"`). When a producer publishes to a topic
//! via [`TopicBroadcaster`], the message is forwarded to all connected
//! clients subscribed to that topic.
//!
//! # Protocol
//!
//! Clients send JSON commands:
//!
//! - `{"type":"subscribe","topic":"<name>"}` -- subscribe to a topic
//! - `{"type":"unsubscribe","topic":"<name>"}` -- unsubscribe from a topic
//! - `{"type":"ping"}` -- keepalive; server responds with `{"type":"pong"}`
//! - `{"type":"pong"}` -- client-side reply to a server-initiated heartbeat
//!
//! The server sends JSON events:
//!
//! - `{"type":"connected","message":"..."}` -- on initial connection
//! - `{"type":"subscribed","topic":"<name>"}` -- ack after subscribe
//! - `{"type":"unsubscribed","topic":"<name>"}` -- ack after unsubscribe
//! - `{"type":"event","topic":"<name>","data":{...}}` -- broadcast event
//! - `{"type":"pong"}` -- keepalive response (when client sent ping)
//! - `{"type":"ping"}` -- server-initiated heartbeat (client must reply
//!   with `{"type":"pong"}` within [`HEARTBEAT_TIMEOUT`] or be evicted)
//!
//! # Heartbeat (WEFT-300)
//!
//! The server pings each socket every [`HEARTBEAT_INTERVAL`]. If a client
//! fails to send a `pong` within [`HEARTBEAT_TIMEOUT`] (i.e. misses two
//! consecutive pings) the connection is closed and all per-topic
//! subscription forwarding tasks are aborted. This prevents dead
//! connections from leaking broadcaster fan-out slots.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use axum::{
    extract::{
        State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::IntoResponse,
};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use super::ApiState;

/// How often the server sends a heartbeat ping to each socket.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);

/// Maximum time the server waits for a pong before evicting the
/// connection. Two missed pings (60s) trips eviction.
pub const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(60);

/// Heartbeat tuning. Default values match production constants but can
/// be overridden in tests via [`handle_socket_with_heartbeat`].
#[derive(Clone, Copy)]
pub struct HeartbeatConfig {
    /// Interval between server-initiated pings.
    pub interval: Duration,
    /// Maximum age of the most recent pong before the socket is evicted.
    pub timeout: Duration,
}

impl Default for HeartbeatConfig {
    fn default() -> Self {
        Self {
            interval: HEARTBEAT_INTERVAL,
            timeout: HEARTBEAT_TIMEOUT,
        }
    }
}

/// WebSocket upgrade handler.
pub async fn ws_handler(ws: WebSocketUpgrade, State(state): State<ApiState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

/// Handle a single WebSocket connection with the production heartbeat
/// configuration.
pub async fn handle_socket(socket: WebSocket, state: ApiState) {
    handle_socket_with_heartbeat(socket, state, HeartbeatConfig::default()).await
}

/// Handle a single WebSocket connection.
///
/// Splits the socket into a sender and receiver. The sender is wrapped in
/// `Arc<Mutex<_>>` so that subscription forwarding tasks (and the
/// heartbeat task) can write to it concurrently. Each subscription
/// spawns a task that reads from the broadcast receiver and forwards
/// events to the client. A separate heartbeat task pings the client at
/// `cfg.interval` and signals shutdown if no pong arrives within
/// `cfg.timeout`.
pub async fn handle_socket_with_heartbeat(
    socket: WebSocket,
    state: ApiState,
    cfg: HeartbeatConfig,
) {
    let (ws_sender, mut ws_receiver) = socket.split();

    // Wrap sender in Arc<Mutex> so subscription tasks can send messages.
    let sender = Arc::new(Mutex::new(ws_sender));

    // Track active subscription forwarding tasks per topic.
    let mut subscriptions: HashMap<String, JoinHandle<()>> = HashMap::new();

    // Send welcome message.
    {
        let welcome = serde_json::json!({
            "type": "connected",
            "message": "ClawFT WebSocket connected"
        });
        let mut s = sender.lock().await;
        if s.send(Message::Text(welcome.to_string().into()))
            .await
            .is_err()
        {
            return;
        }
    }

    // Heartbeat state. `last_pong_ms` is the unix-ms timestamp of the
    // most recent pong (or, at start, the connect time so the first
    // interval tick has a sane baseline). `eviction_signal` flips to
    // `true` when the heartbeat task decides the socket is dead; the
    // main loop polls it on each iteration so it can exit promptly.
    let last_pong_ms = Arc::new(AtomicU64::new(now_ms()));
    let eviction_signal = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let heartbeat_handle = spawn_heartbeat_task(
        sender.clone(),
        last_pong_ms.clone(),
        eviction_signal.clone(),
        cfg,
    );

    // Main message loop -- process client commands, exit on close /
    // eviction / receive error.
    loop {
        // Bail out as soon as the heartbeat task signals eviction. This
        // is checked between every read so a stalled client (no
        // inbound messages, no pongs) is dropped within `cfg.timeout`
        // of its last activity rather than waiting forever for the
        // next inbound frame.
        if eviction_signal.load(Ordering::Relaxed) {
            break;
        }

        // Wake up at least every interval so eviction can trigger even
        // when the client is silent. `tokio::select!` races the
        // receiver against a timeout; whichever wins runs first.
        let next = tokio::select! {
            msg = ws_receiver.next() => msg,
            _ = tokio::time::sleep(cfg.interval) => continue,
        };

        let msg = match next {
            Some(Ok(m)) => m,
            // Receive error or stream end -> client gone.
            _ => break,
        };

        match msg {
            Message::Text(text) => {
                let cmd = match serde_json::from_str::<serde_json::Value>(&text) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let cmd_type = cmd.get("type").and_then(|v| v.as_str()).unwrap_or("");

                match cmd_type {
                    "subscribe" => {
                        let topic = cmd
                            .get("topic")
                            .and_then(|v| v.as_str())
                            .unwrap_or("*")
                            .to_string();

                        // Don't double-subscribe; just re-ack.
                        if subscriptions.contains_key(&topic) {
                            let ack = serde_json::json!({"type": "subscribed", "topic": &topic});
                            let mut s = sender.lock().await;
                            let _ = s.send(Message::Text(ack.to_string().into())).await;
                            continue;
                        }

                        // Subscribe to the broadcast channel.
                        let mut rx = state.broadcaster.subscribe(&topic).await;
                        let sender_clone = sender.clone();
                        let topic_clone = topic.clone();

                        // Spawn a forwarding task that reads from the broadcast
                        // channel and writes events to this client's WebSocket.
                        let handle = tokio::spawn(async move {
                            loop {
                                match rx.recv().await {
                                    Ok(msg) => {
                                        // Parse the message as JSON, falling
                                        // back to a plain string value.
                                        let data = serde_json::from_str::<serde_json::Value>(&msg)
                                            .unwrap_or(serde_json::Value::String(msg));

                                        let event = serde_json::json!({
                                            "type": "event",
                                            "topic": &topic_clone,
                                            "data": data
                                        });
                                        let mut s = sender_clone.lock().await;
                                        if s.send(Message::Text(event.to_string().into()))
                                            .await
                                            .is_err()
                                        {
                                            break; // Client disconnected.
                                        }
                                    }
                                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                                        // Slow consumer; skip missed messages.
                                        continue;
                                    }
                                    Err(_) => {
                                        // Channel closed.
                                        break;
                                    }
                                }
                            }
                        });

                        subscriptions.insert(topic.clone(), handle);

                        // Send subscription acknowledgement.
                        let ack = serde_json::json!({"type": "subscribed", "topic": &topic});
                        let mut s = sender.lock().await;
                        let _ = s.send(Message::Text(ack.to_string().into())).await;
                    }

                    "unsubscribe" => {
                        let topic = cmd.get("topic").and_then(|v| v.as_str()).unwrap_or("*");

                        // Abort the forwarding task if it exists.
                        if let Some(handle) = subscriptions.remove(topic) {
                            handle.abort();
                        }

                        let ack = serde_json::json!({"type": "unsubscribed", "topic": topic});
                        let mut s = sender.lock().await;
                        let _ = s.send(Message::Text(ack.to_string().into())).await;
                    }

                    "ping" => {
                        // Client-initiated keepalive. Respond with pong
                        // and (since the client is clearly alive) refresh
                        // the heartbeat clock so we don't evict it.
                        last_pong_ms.store(now_ms(), Ordering::Relaxed);
                        let pong = serde_json::json!({"type": "pong"});
                        let mut s = sender.lock().await;
                        let _ = s.send(Message::Text(pong.to_string().into())).await;
                    }

                    "pong" => {
                        // Server-initiated heartbeat reply.
                        last_pong_ms.store(now_ms(), Ordering::Relaxed);
                    }

                    _ => {}
                }
            }
            // Native WebSocket pong frames also count as liveness so we
            // don't evict clients that respond at the protocol level
            // rather than via JSON.
            Message::Pong(_) => {
                last_pong_ms.store(now_ms(), Ordering::Relaxed);
            }
            Message::Ping(_) => {
                // axum auto-replies to ping frames; nothing to do here
                // beyond noting that the client is alive.
                last_pong_ms.store(now_ms(), Ordering::Relaxed);
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    // Cleanup: stop the heartbeat task and abort all subscription
    // forwarding tasks. Aborting the forwarding tasks releases this
    // socket's clones of the broadcaster receivers, which lets the
    // broadcaster drop the subscriber from its topic map on the next
    // publish (broadcast::Sender prunes dead receivers automatically).
    heartbeat_handle.abort();
    for (_, handle) in subscriptions {
        handle.abort();
    }
}

/// Current unix time in milliseconds. Saturates at 0 if the clock is
/// somehow before the epoch.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Spawn the heartbeat task for a single socket.
///
/// The task pings the client every `cfg.interval`. Before each ping it
/// checks whether the most recent pong is older than `cfg.timeout`; if
/// so it flips `eviction_signal` so the main loop can exit, then
/// terminates itself. The send-mutex is shared with the subscription
/// forwarders, so a slow forwarder won't block the heartbeat
/// indefinitely (it just queues behind on the next tick).
fn spawn_heartbeat_task(
    sender: Arc<Mutex<futures_util::stream::SplitSink<axum::extract::ws::WebSocket, Message>>>,
    last_pong_ms: Arc<AtomicU64>,
    eviction_signal: Arc<std::sync::atomic::AtomicBool>,
    cfg: HeartbeatConfig,
) -> JoinHandle<()> {
    let timeout_ms = cfg.timeout.as_millis() as u64;
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(cfg.interval);
        // Skip the immediate first tick; we just opened the socket.
        ticker.tick().await;
        loop {
            ticker.tick().await;

            // Eviction check: too long since the last pong?
            let last = last_pong_ms.load(Ordering::Relaxed);
            let now = now_ms();
            if now.saturating_sub(last) > timeout_ms {
                eviction_signal.store(true, Ordering::Relaxed);
                // Best-effort close; ignore errors since the peer may
                // already be gone.
                let mut s = sender.lock().await;
                let _ = s.send(Message::Close(None)).await;
                break;
            }

            // Send the next heartbeat ping. If the send fails the
            // client is gone -- flip the eviction signal so the main
            // loop exits.
            let ping = serde_json::json!({"type": "ping"});
            let mut s = sender.lock().await;
            if s.send(Message::Text(ping.to_string().into()))
                .await
                .is_err()
            {
                eviction_signal.store(true, Ordering::Relaxed);
                break;
            }
        }
    })
}

#[cfg(test)]
mod tests {
    //! WEFT-300: heartbeat + dead-connection cleanup.
    //!
    //! These tests boot a real `ws_handler` against an in-process axum
    //! router and use [`tokio_tungstenite`] as a black-box client to
    //! verify both the live (pong-replying) and dead (silent) paths.
    //!
    //! The production heartbeat interval is 30s; for the tests we
    //! plumb in a 50ms interval / 200ms timeout via
    //! [`handle_socket_with_heartbeat`] so the assertions run in
    //! roughly half a second.
    use super::*;
    use crate::api::auth::TokenStore;
    use crate::api::broadcaster::TopicBroadcaster;
    use crate::api::{
        AgentAccess, AgentInfo, ApiState, BusAccess, ChannelAccess, ChannelStatusInfo,
        ConfigAccess, MemoryAccess, MemoryEntryInfo, SessionAccess, SessionDetail, SessionInfo,
        SkillAccess, SkillInfo, ToolInfo, ToolRegistryAccess, TtsProviderInfo, VoiceAccess,
        VoiceSettingsInfo, VoiceSettingsUpdate, VoiceStatusInfo,
    };
    use std::sync::Arc;

    // ── Stub access traits ─────────────────────────────────────
    // These are the smallest possible no-op implementations of
    // every trait required to materialise an `ApiState`. We only
    // exercise the WS path so none of these methods are called.

    struct Stub;
    impl ToolRegistryAccess for Stub {
        fn list_tools(&self) -> Vec<ToolInfo> {
            vec![]
        }
        fn tool_schema(&self, _: &str) -> Option<serde_json::Value> {
            None
        }
    }
    impl SessionAccess for Stub {
        fn list_sessions(&self) -> Vec<SessionInfo> {
            vec![]
        }
        fn get_session(&self, _: &str) -> Option<SessionDetail> {
            None
        }
        fn delete_session(&self, _: &str) -> bool {
            false
        }
    }
    impl AgentAccess for Stub {
        fn list_agents(&self) -> Vec<AgentInfo> {
            vec![]
        }
        fn get_agent(&self, _: &str) -> Option<AgentInfo> {
            None
        }
    }
    impl BusAccess for Stub {
        fn send_message(&self, _: &str, _: &str, _: &str) {}
    }
    impl SkillAccess for Stub {
        fn list_skills(&self) -> Vec<SkillInfo> {
            vec![]
        }
        fn install_skill(&self, _: &str) -> Result<(), String> {
            Err("stub".into())
        }
        fn uninstall_skill(&self, _: &str) -> Result<(), String> {
            Err("stub".into())
        }
    }
    impl MemoryAccess for Stub {
        fn list_entries(&self) -> Vec<MemoryEntryInfo> {
            vec![]
        }
        fn search(&self, _: &str, _: usize) -> Vec<MemoryEntryInfo> {
            vec![]
        }
        fn store(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: &[String],
        ) -> Result<MemoryEntryInfo, String> {
            Err("stub".into())
        }
        fn delete(&self, _: &str) -> bool {
            false
        }
    }
    impl ConfigAccess for Stub {
        fn get_config(&self) -> serde_json::Value {
            serde_json::Value::Null
        }
        fn save_config(&self, _: serde_json::Value) -> Result<(), String> {
            Err("stub".into())
        }
    }
    impl ChannelAccess for Stub {
        fn list_channels(&self) -> Vec<ChannelStatusInfo> {
            vec![]
        }
    }
    impl VoiceAccess for Stub {
        fn get_status(&self) -> VoiceStatusInfo {
            VoiceStatusInfo {
                state: "idle".into(),
                talk_mode_active: false,
                wake_word_enabled: false,
            }
        }
        fn get_settings(&self) -> VoiceSettingsInfo {
            VoiceSettingsInfo {
                enabled: false,
                wake_word_enabled: false,
                language: "en-US".into(),
                echo_cancel: false,
                noise_suppression: false,
                push_to_talk: false,
            }
        }
        fn update_settings(&self, _: VoiceSettingsUpdate) -> Result<(), String> {
            Ok(())
        }
        fn get_tts_config(&self) -> TtsProviderInfo {
            TtsProviderInfo {
                provider: "browser".into(),
                model: String::new(),
                voice: String::new(),
                speed: 1.0,
                api_key: String::new(),
                api_base: None,
            }
        }
    }

    fn stub_state() -> ApiState {
        let stub: Arc<Stub> = Arc::new(Stub);
        ApiState {
            tools: stub.clone(),
            sessions: stub.clone(),
            agents: stub.clone(),
            bus: stub.clone(),
            auth: Arc::new(TokenStore::new()),
            skills: stub.clone(),
            memory: stub.clone(),
            config: stub.clone(),
            channels: stub.clone(),
            voice: stub.clone(),
            broadcaster: Arc::new(TopicBroadcaster::new()),
        }
    }

    /// Heartbeat config injected into the test handler via a thread-
    /// local static. Each test sets this before binding the listener
    /// and the handler reads it on the upgrade. Avoids the closure-
    /// type acrobatics of a generic test fixture.
    use std::sync::OnceLock;
    static TEST_CFG: OnceLock<std::sync::Mutex<HeartbeatConfig>> = OnceLock::new();

    fn set_test_cfg(cfg: HeartbeatConfig) {
        let slot = TEST_CFG.get_or_init(|| std::sync::Mutex::new(HeartbeatConfig::default()));
        *slot.lock().unwrap() = cfg;
    }

    fn get_test_cfg() -> HeartbeatConfig {
        TEST_CFG
            .get()
            .map(|m| *m.lock().unwrap())
            .unwrap_or_default()
    }

    async fn test_ws_handler(
        ws: axum::extract::WebSocketUpgrade,
        axum::extract::State(state): axum::extract::State<ApiState>,
    ) -> impl axum::response::IntoResponse {
        let cfg = get_test_cfg();
        ws.on_upgrade(move |socket| handle_socket_with_heartbeat(socket, state, cfg))
    }

    /// Bind a localhost listener serving the WS handler with the
    /// supplied heartbeat config and return the bound address.
    async fn spawn_test_ws(cfg: HeartbeatConfig) -> std::net::SocketAddr {
        use axum::Router;
        use axum::routing::get;

        set_test_cfg(cfg);

        let state = stub_state();
        let app = Router::new()
            .route("/ws", get(test_ws_handler))
            .with_state(state);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.ok();
        });
        addr
    }

    /// A silent client (never sends pong) must be evicted within
    /// roughly `cfg.timeout` of connect.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stalled_client_is_evicted_within_timeout() {
        use tokio_tungstenite::connect_async;
        use tokio_tungstenite::tungstenite::Message as TMessage;

        let cfg = HeartbeatConfig {
            interval: Duration::from_millis(50),
            timeout: Duration::from_millis(200),
        };

        let addr = spawn_test_ws(cfg).await;

        let url = format!("ws://{}/ws", addr);
        let (ws_stream, _) = connect_async(&url).await.expect("connect");
        let (_write, mut read) = futures_util::StreamExt::split(ws_stream);

        // Drain incoming frames -- we never reply to ping. The server
        // should close the connection within ~timeout (200ms) plus
        // one interval (50ms) of slack.
        let start = std::time::Instant::now();
        let deadline = Duration::from_secs(2);
        let mut closed = false;
        while start.elapsed() < deadline {
            match tokio::time::timeout(Duration::from_millis(500), read.next()).await {
                Ok(Some(Ok(TMessage::Close(_)))) => {
                    closed = true;
                    break;
                }
                Ok(None) | Err(_) => {
                    // Stream ended (server dropped) -- also acceptable.
                    closed = true;
                    break;
                }
                _ => continue,
            }
        }
        assert!(
            closed,
            "stalled client must be evicted within {:?}",
            deadline
        );
    }

    /// A client that promptly replies to every server ping must NOT
    /// be evicted within several heartbeat intervals.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_client_is_not_evicted() {
        use futures_util::SinkExt;
        use tokio_tungstenite::connect_async;
        use tokio_tungstenite::tungstenite::Message as TMessage;

        let cfg = HeartbeatConfig {
            interval: Duration::from_millis(50),
            timeout: Duration::from_millis(200),
        };

        let addr = spawn_test_ws(cfg).await;

        let url = format!("ws://{}/ws", addr);
        let (ws_stream, _) = connect_async(&url).await.expect("connect");
        let (mut write, mut read) = futures_util::StreamExt::split(ws_stream);

        // Reply to every server ping with a JSON pong, for ~600ms (well
        // past `timeout` but never starving). Track that we got at
        // least two server pings (proving the heartbeat task fires).
        let mut server_pings = 0;
        let deadline = std::time::Instant::now() + Duration::from_millis(600);
        while std::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(150), read.next()).await {
                Ok(Some(Ok(TMessage::Text(t)))) => {
                    let s = t.to_string();
                    if s.contains("\"ping\"") {
                        server_pings += 1;
                        write
                            .send(TMessage::Text("{\"type\":\"pong\"}".into()))
                            .await
                            .unwrap();
                    }
                }
                Ok(Some(Ok(TMessage::Close(_)))) => {
                    panic!("live client must not be evicted");
                }
                Ok(None) => panic!("live client connection ended unexpectedly"),
                _ => continue,
            }
        }
        assert!(
            server_pings >= 2,
            "expected >=2 server-initiated pings in 600ms, got {server_pings}"
        );
    }
}
