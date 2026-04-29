//! Signal channel adapter implementation.
//!
//! Implements [`ChannelAdapter`] for Signal messaging via the
//! `signal-cli` daemon. The daemon is spawned with a TCP JSON-RPC
//! listener; this adapter opens a single [`tokio::net::TcpStream`]
//! to it, reads newline-delimited JSON-RPC events for inbound
//! `MessageReceived` notifications, and writes JSON-RPC `send`
//! requests for outbound messages.
//!
//! # Lifecycle
//!
//! - [`SignalChannelAdapter::start`] spawns `signal-cli daemon
//!   --tcp <bind>` (with `kill_on_drop`), polls until the listener
//!   is reachable, opens the socket, and runs a read-loop until the
//!   provided [`CancellationToken`] is fired. JSON-RPC responses are
//!   correlated to outstanding `send` calls by id; JSON-RPC
//!   notifications with `method == "receive"` are decoded and
//!   forwarded via [`ChannelAdapterHost::deliver_inbound`].
//! - [`SignalChannelAdapter::send`] issues a `send` request over the
//!   shared socket, awaits the matching response, and returns the
//!   `timestamp` field as the message id.
//! - On cancellation the writer half is dropped (which signals
//!   end-of-input to the daemon), the [`Child`] is dropped (kill on
//!   drop terminates `signal-cli`), and the read-loop returns.
//!
//! # Argument sanitization
//!
//! All values that flow into `signal-cli`'s argv (binary path, phone
//! number, data directory, bind address) pass through
//! [`sanitize_argument`] before [`tokio::process::Command`] is built.
//! `signal-cli` is invoked directly with `Command::new(...).arg(...)`
//! — never via a shell — so shell-metacharacter rejection is a
//! defense-in-depth check, not the primary boundary.

use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::TcpStream;
use tokio::process::{Child, Command};
use tokio::sync::{oneshot, Mutex};
use tokio::time::{sleep, timeout};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use clawft_plugin::error::PluginError;
use clawft_plugin::message::MessagePayload;
use clawft_plugin::traits::{ChannelAdapter, ChannelAdapterHost};

use super::types::{sanitize_argument, SignalAdapterConfig};

/// Pending JSON-RPC request waiters keyed by request id.
type PendingMap = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>;

/// Shared write half guarded by a [`Mutex`] so [`SignalChannelAdapter::send`]
/// can be called concurrently with the read-loop.
type SharedWriter = Arc<Mutex<Option<OwnedWriteHalf>>>;

/// Signal channel adapter using the `signal-cli` JSON-RPC daemon.
pub struct SignalChannelAdapter {
    config: SignalAdapterConfig,

    /// Monotonic JSON-RPC request id counter.
    next_id: AtomicU64,

    /// Outstanding requests awaiting a response.
    pending: PendingMap,

    /// Shared TCP writer; populated by `start()` and consumed by `send()`.
    writer: SharedWriter,
}

impl SignalChannelAdapter {
    /// Create a new Signal channel adapter.
    pub fn new(config: SignalAdapterConfig) -> Self {
        Self {
            config,
            next_id: AtomicU64::new(1),
            pending: Arc::new(Mutex::new(HashMap::new())),
            writer: Arc::new(Mutex::new(None)),
        }
    }

    /// Check if a phone number is in the allow list.
    pub fn is_number_allowed(&self, number: &str) -> bool {
        if self.config.allowed_numbers.is_empty() {
            return true;
        }
        self.config.allowed_numbers.iter().any(|n| n == number)
    }

    /// Validate the adapter configuration.
    fn validate_config(&self) -> Result<(), PluginError> {
        if self.config.phone_number.is_empty() {
            return Err(PluginError::LoadFailed(
                "signal adapter: phone_number is required".into(),
            ));
        }
        sanitize_argument(&self.config.phone_number).map_err(|e| {
            PluginError::LoadFailed(format!(
                "signal adapter: invalid phone_number: {e}"
            ))
        })?;
        sanitize_argument(&self.config.signal_cli_path).map_err(|e| {
            PluginError::LoadFailed(format!(
                "signal adapter: invalid signal_cli_path: {e}"
            ))
        })?;
        sanitize_argument(&self.config.daemon_bind_addr).map_err(|e| {
            PluginError::LoadFailed(format!(
                "signal adapter: invalid daemon_bind_addr: {e}"
            ))
        })?;
        if let Some(dir) = self.config.data_dir.as_deref() {
            sanitize_argument(dir).map_err(|e| {
                PluginError::LoadFailed(format!(
                    "signal adapter: invalid data_dir: {e}"
                ))
            })?;
        }
        Ok(())
    }

    /// Spawn `signal-cli daemon --tcp <bind>` with `kill_on_drop`.
    fn spawn_daemon(&self) -> Result<Child, PluginError> {
        let mut cmd = Command::new(&self.config.signal_cli_path);
        if let Some(dir) = self.config.data_dir.as_deref() {
            cmd.arg("--config").arg(dir);
        }
        cmd.arg("-a").arg(&self.config.phone_number);
        cmd.arg("daemon")
            .arg("--tcp")
            .arg(&self.config.daemon_bind_addr);
        cmd.kill_on_drop(true);
        // Inherit stdio for now; the JSON-RPC channel is on TCP, so
        // stdout/stderr is just human-readable logging.
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::null());
        cmd.spawn().map_err(|e| {
            PluginError::ExecutionFailed(format!(
                "signal: failed to spawn `{}`: {e}",
                self.config.signal_cli_path
            ))
        })
    }

    /// Poll [`TcpStream::connect`] until it succeeds or we exhaust the
    /// timeout budget.
    async fn await_listener(
        &self,
    ) -> Result<TcpStream, PluginError> {
        let deadline = Duration::from_secs(self.config.timeout_secs);
        let poll_every = Duration::from_millis(100);
        let result = timeout(deadline, async {
            loop {
                match TcpStream::connect(&self.config.daemon_bind_addr).await {
                    Ok(stream) => return Ok::<_, io::Error>(stream),
                    Err(_) => sleep(poll_every).await,
                }
            }
        })
        .await;
        match result {
            Ok(Ok(stream)) => Ok(stream),
            Ok(Err(e)) => Err(PluginError::ExecutionFailed(format!(
                "signal: daemon listener unreachable: {e}"
            ))),
            Err(_) => Err(PluginError::ExecutionFailed(format!(
                "signal: daemon listener at {} not reachable within {}s",
                self.config.daemon_bind_addr, self.config.timeout_secs
            ))),
        }
    }

    /// Drive the JSON-RPC read-loop until `cancel` fires or EOF.
    ///
    /// Each newline-delimited frame is parsed as JSON. Frames carrying
    /// a numeric `"id"` are matched against [`Self::pending`]; frames
    /// with `"method": "receive"` are decoded into a [`MessagePayload`]
    /// and forwarded to [`ChannelAdapterHost::deliver_inbound`].
    async fn read_loop(
        reader: OwnedReadHalf,
        host: Arc<dyn ChannelAdapterHost>,
        pending: PendingMap,
        cancel: CancellationToken,
    ) {
        let mut lines = BufReader::new(reader).lines();
        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    debug!("signal: read-loop cancelled");
                    return;
                }
                next = lines.next_line() => {
                    match next {
                        Ok(Some(line)) => {
                            if line.trim().is_empty() {
                                continue;
                            }
                            if let Err(e) = handle_frame(
                                &line,
                                host.as_ref(),
                                &pending,
                            )
                            .await
                            {
                                warn!(error = %e, "signal: bad JSON-RPC frame");
                            }
                        }
                        Ok(None) => {
                            debug!("signal: read-loop EOF");
                            return;
                        }
                        Err(e) => {
                            error!(error = %e, "signal: read-loop I/O error");
                            return;
                        }
                    }
                }
            }
        }
    }

    /// Fail every outstanding request with a shutdown error so callers
    /// don't hang once the read-loop exits.
    async fn drain_pending(pending: &PendingMap, reason: &str) {
        let mut guard = pending.lock().await;
        for (_id, tx) in guard.drain() {
            let _ = tx.send(Err(reason.to_string()));
        }
    }
}

/// Decode a single JSON-RPC frame and dispatch it.
///
/// Returns `Err` only on outright parse failure; missing optional
/// fields are tolerated (signal-cli's schema evolves).
async fn handle_frame(
    line: &str,
    host: &dyn ChannelAdapterHost,
    pending: &PendingMap,
) -> Result<(), String> {
    let v: Value = serde_json::from_str(line)
        .map_err(|e| format!("invalid JSON: {e}"))?;
    let obj = v.as_object().ok_or_else(|| "frame is not an object".to_string())?;

    // Response: id present, no method.
    if let Some(id_val) = obj.get("id") {
        if let Some(id) = id_val.as_u64() {
            let mut guard = pending.lock().await;
            if let Some(tx) = guard.remove(&id) {
                if let Some(err) = obj.get("error") {
                    let _ = tx.send(Err(err.to_string()));
                } else if let Some(result) = obj.get("result") {
                    let _ = tx.send(Ok(result.clone()));
                } else {
                    let _ = tx.send(Ok(Value::Null));
                }
            }
            return Ok(());
        }
    }

    // Notification: method present.
    if let Some(method) = obj.get("method").and_then(Value::as_str) {
        if method == "receive" {
            let params = obj
                .get("params")
                .ok_or_else(|| "receive: missing params".to_string())?;
            forward_inbound(host, params).await?;
            return Ok(());
        }
        debug!(method, "signal: ignoring JSON-RPC notification");
    }
    Ok(())
}

/// Translate a `MessageReceived`-shape JSON-RPC params object into a
/// [`MessagePayload`] and deliver it.
async fn forward_inbound(
    host: &dyn ChannelAdapterHost,
    params: &Value,
) -> Result<(), String> {
    let envelope = params.get("envelope").unwrap_or(params);
    let source = envelope
        .get("source")
        .or_else(|| envelope.get("sourceNumber"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let data_msg = envelope
        .get("dataMessage")
        .or_else(|| envelope.get("syncMessage").and_then(|s| s.get("sentMessage")));
    let body = data_msg
        .and_then(|m| m.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if body.is_empty() {
        debug!("signal: empty inbound body, skipping");
        return Ok(());
    }
    let chat_id = data_msg
        .and_then(|m| m.get("groupInfo"))
        .and_then(|g| g.get("groupId"))
        .and_then(Value::as_str)
        .map(|s| s.to_string())
        .unwrap_or_else(|| source.clone());

    let mut metadata: HashMap<String, Value> = HashMap::new();
    if let Some(ts) = envelope.get("timestamp") {
        metadata.insert("timestamp".into(), ts.clone());
    }

    host.deliver_inbound(
        "signal",
        &source,
        &chat_id,
        MessagePayload::text(body),
        metadata,
    )
    .await
    .map_err(|e| format!("deliver_inbound failed: {e}"))
}

#[async_trait]
impl ChannelAdapter for SignalChannelAdapter {
    fn name(&self) -> &str {
        "signal"
    }

    fn display_name(&self) -> &str {
        "Signal"
    }

    fn supports_threads(&self) -> bool {
        false
    }

    fn supports_media(&self) -> bool {
        false
    }

    async fn start(
        &self,
        host: Arc<dyn ChannelAdapterHost>,
        cancel: CancellationToken,
    ) -> Result<(), PluginError> {
        info!(
            phone = %self.config.phone_number,
            cli = %self.config.signal_cli_path,
            bind = %self.config.daemon_bind_addr,
            "Signal channel adapter starting"
        );

        self.validate_config()?;

        // Spawn the daemon. `kill_on_drop` ties its lifetime to this
        // `Child` handle so we don't leak processes on cancellation.
        let mut child = self.spawn_daemon()?;

        // Wait for the TCP listener to come up.
        let stream = match self.await_listener().await {
            Ok(s) => s,
            Err(e) => {
                let _ = child.kill().await;
                return Err(e);
            }
        };
        // Disable Nagle's so JSON-RPC requests flush promptly.
        let _ = stream.set_nodelay(true);
        let (reader, writer) = stream.into_split();

        {
            let mut w = self.writer.lock().await;
            *w = Some(writer);
        }

        let read_handle = {
            let pending = self.pending.clone();
            let cancel = cancel.clone();
            tokio::spawn(SignalChannelAdapter::read_loop(
                reader, host, pending, cancel,
            ))
        };

        // Block until cancelled, then tear down.
        cancel.cancelled().await;
        info!("Signal channel adapter shutting down");

        // Drop the writer half (signals EOF to the daemon), then
        // terminate the daemon child.
        {
            let mut w = self.writer.lock().await;
            if let Some(mut w) = w.take() {
                let _ = w.shutdown().await;
            }
        }
        let _ = child.start_kill();
        let _ = timeout(Duration::from_secs(5), child.wait()).await;

        // Read-loop should now exit on EOF / cancellation; wait briefly.
        let _ = timeout(Duration::from_secs(2), read_handle).await;

        SignalChannelAdapter::drain_pending(
            &self.pending,
            "signal channel shut down",
        )
        .await;
        Ok(())
    }

    async fn send(
        &self,
        target: &str,
        payload: &MessagePayload,
    ) -> Result<String, PluginError> {
        let content = payload.as_text().ok_or_else(|| {
            PluginError::ExecutionFailed(
                "signal: only text payloads supported".into(),
            )
        })?;

        if self.config.phone_number.is_empty() {
            return Err(PluginError::ExecutionFailed(
                "signal: phone_number not configured".into(),
            ));
        }

        // Sanitize the target before passing it to the daemon.
        sanitize_argument(target).map_err(|e| {
            PluginError::ExecutionFailed(format!(
                "signal: invalid target number: {e}"
            ))
        })?;

        // Build JSON-RPC request.
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "send",
            "params": {
                "account": self.config.phone_number,
                "recipient": [target],
                "message": content,
            },
            "id": id,
        });
        let mut frame = serde_json::to_vec(&request).map_err(|e| {
            PluginError::ExecutionFailed(format!(
                "signal: failed to encode request: {e}"
            ))
        })?;
        frame.push(b'\n');

        // Register response slot before writing.
        let (tx, rx) = oneshot::channel();
        {
            let mut guard = self.pending.lock().await;
            guard.insert(id, tx);
        }

        // Write to the shared socket.
        {
            let mut w = self.writer.lock().await;
            let writer = w.as_mut().ok_or_else(|| {
                PluginError::ExecutionFailed(
                    "signal: not started; no JSON-RPC connection".into(),
                )
            })?;
            if let Err(e) = writer.write_all(&frame).await {
                // Drop the pending entry so no future response leaks.
                let mut g = self.pending.lock().await;
                g.remove(&id);
                return Err(PluginError::ExecutionFailed(format!(
                    "signal: socket write failed: {e}"
                )));
            }
            if let Err(e) = writer.flush().await {
                let mut g = self.pending.lock().await;
                g.remove(&id);
                return Err(PluginError::ExecutionFailed(format!(
                    "signal: socket flush failed: {e}"
                )));
            }
        }

        // Await response with a timeout.
        let response = timeout(
            Duration::from_secs(self.config.timeout_secs),
            rx,
        )
        .await;
        let value = match response {
            Ok(Ok(Ok(v))) => v,
            Ok(Ok(Err(e))) => {
                return Err(PluginError::ExecutionFailed(format!(
                    "signal: daemon error: {e}"
                )))
            }
            Ok(Err(_)) => {
                return Err(PluginError::ExecutionFailed(
                    "signal: response channel closed".into(),
                ))
            }
            Err(_) => {
                let mut g = self.pending.lock().await;
                g.remove(&id);
                return Err(PluginError::ExecutionFailed(format!(
                    "signal: response timeout after {}s",
                    self.config.timeout_secs
                )));
            }
        };

        // signal-cli replies with `{"timestamp": <i64>, ...}` on success.
        let ts = value
            .get("timestamp")
            .and_then(Value::as_i64)
            .ok_or_else(|| {
                PluginError::ExecutionFailed(format!(
                    "signal: response missing timestamp: {value}"
                ))
            })?;
        debug!(to = %target, content_len = content.len(), id = ts, "signal: send ok");
        Ok(ts.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::Mutex as StdMutex;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpListener;

    fn make_config() -> SignalAdapterConfig {
        SignalAdapterConfig {
            phone_number: "+15551234567".into(),
            ..Default::default()
        }
    }

    #[test]
    fn name_is_signal() {
        let adapter = SignalChannelAdapter::new(make_config());
        assert_eq!(adapter.name(), "signal");
    }

    #[test]
    fn display_name() {
        let adapter = SignalChannelAdapter::new(make_config());
        assert_eq!(adapter.display_name(), "Signal");
    }

    #[test]
    fn no_threads_or_media() {
        let adapter = SignalChannelAdapter::new(make_config());
        assert!(!adapter.supports_threads());
        assert!(!adapter.supports_media());
    }

    #[test]
    fn number_filtering() {
        let mut config = make_config();
        config.allowed_numbers = vec!["+1234567890".into()];
        let adapter = SignalChannelAdapter::new(config);

        assert!(adapter.is_number_allowed("+1234567890"));
        assert!(!adapter.is_number_allowed("+9876543210"));
    }

    #[test]
    fn empty_allow_list_allows_all() {
        let adapter = SignalChannelAdapter::new(make_config());
        assert!(adapter.is_number_allowed("+anyone"));
    }

    #[test]
    fn validate_config_success() {
        let adapter = SignalChannelAdapter::new(make_config());
        assert!(adapter.validate_config().is_ok());
    }

    #[test]
    fn validate_config_empty_phone() {
        let mut config = make_config();
        config.phone_number = String::new();
        let adapter = SignalChannelAdapter::new(config);
        let err = adapter.validate_config().unwrap_err();
        assert!(err.to_string().contains("phone_number"));
    }

    #[test]
    fn validate_config_bad_phone_number() {
        let mut config = make_config();
        config.phone_number = "+1234; rm -rf /".into();
        let adapter = SignalChannelAdapter::new(config);
        let err = adapter.validate_config().unwrap_err();
        assert!(err.to_string().contains("phone_number"));
    }

    #[test]
    fn validate_config_bad_cli_path() {
        let mut config = make_config();
        config.signal_cli_path = "/bin/evil; cat /etc/passwd".into();
        let adapter = SignalChannelAdapter::new(config);
        let err = adapter.validate_config().unwrap_err();
        assert!(err.to_string().contains("signal_cli_path"));
    }

    #[test]
    fn validate_config_bad_bind_addr() {
        let mut config = make_config();
        config.daemon_bind_addr = "127.0.0.1:7583; cat /etc/passwd".into();
        let adapter = SignalChannelAdapter::new(config);
        let err = adapter.validate_config().unwrap_err();
        assert!(err.to_string().contains("daemon_bind_addr"));
    }

    #[test]
    fn validate_config_bad_data_dir() {
        let mut config = make_config();
        config.data_dir = Some("/var/lib; rm -rf /".into());
        let adapter = SignalChannelAdapter::new(config);
        let err = adapter.validate_config().unwrap_err();
        assert!(err.to_string().contains("data_dir"));
    }

    #[tokio::test]
    async fn send_non_text_fails() {
        let adapter = SignalChannelAdapter::new(make_config());
        let payload = MessagePayload::structured(serde_json::json!({}));
        let result = adapter.send("+1234567890", &payload).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn send_to_unsafe_target_fails() {
        let adapter = SignalChannelAdapter::new(make_config());
        let payload = MessagePayload::text("test");
        let result = adapter.send("+1234; rm -rf /", &payload).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("target"));
    }

    #[tokio::test]
    async fn send_without_start_errors() {
        let adapter = SignalChannelAdapter::new(make_config());
        let payload = MessagePayload::text("hi");
        let err = adapter.send("+15550000001", &payload).await.unwrap_err();
        assert!(err.to_string().contains("not started"));
    }

    /// Mock host that records every inbound delivery for assertion.
    #[derive(Default)]
    struct RecordingHost {
        inbox: Arc<StdMutex<Vec<RecordedInbound>>>,
    }

    #[derive(Debug, Clone)]
    struct RecordedInbound {
        sender: String,
        chat_id: String,
        body: String,
    }

    #[async_trait]
    impl ChannelAdapterHost for RecordingHost {
        async fn deliver_inbound(
            &self,
            _channel: &str,
            sender_id: &str,
            chat_id: &str,
            payload: MessagePayload,
            _metadata: HashMap<String, Value>,
        ) -> Result<(), PluginError> {
            self.inbox.lock().unwrap().push(RecordedInbound {
                sender: sender_id.to_string(),
                chat_id: chat_id.to_string(),
                body: payload.as_text().unwrap_or("").to_string(),
            });
            Ok(())
        }
    }

    /// Drive the JSON-RPC framing helpers directly.
    #[tokio::test]
    async fn handle_frame_routes_response() {
        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        let host = RecordingHost::default();
        let (tx, rx) = oneshot::channel();
        pending.lock().await.insert(7, tx);

        handle_frame(
            r#"{"jsonrpc":"2.0","id":7,"result":{"timestamp":99}}"#,
            &host,
            &pending,
        )
        .await
        .unwrap();

        let result = rx.await.unwrap().unwrap();
        assert_eq!(result["timestamp"], 99);
    }

    #[tokio::test]
    async fn handle_frame_routes_error_response() {
        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        let host = RecordingHost::default();
        let (tx, rx) = oneshot::channel();
        pending.lock().await.insert(8, tx);

        handle_frame(
            r#"{"jsonrpc":"2.0","id":8,"error":{"code":-32000,"message":"bad"}}"#,
            &host,
            &pending,
        )
        .await
        .unwrap();

        let err = rx.await.unwrap().unwrap_err();
        assert!(err.contains("bad"));
    }

    #[tokio::test]
    async fn handle_frame_dispatches_receive_notification() {
        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        let host = RecordingHost::default();

        let frame = json!({
            "jsonrpc": "2.0",
            "method": "receive",
            "params": {
                "envelope": {
                    "source": "+15550000007",
                    "timestamp": 1700000000000_i64,
                    "dataMessage": {
                        "message": "hello bot",
                    }
                }
            }
        });

        handle_frame(&frame.to_string(), &host, &pending)
            .await
            .unwrap();

        let recorded = host.inbox.lock().unwrap().clone();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].sender, "+15550000007");
        assert_eq!(recorded[0].chat_id, "+15550000007");
        assert_eq!(recorded[0].body, "hello bot");
    }

    #[tokio::test]
    async fn handle_frame_skips_empty_body() {
        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        let host = RecordingHost::default();
        let frame = json!({
            "jsonrpc": "2.0",
            "method": "receive",
            "params": {
                "envelope": {
                    "source": "+15550000007",
                    "dataMessage": {"message": ""}
                }
            }
        });
        handle_frame(&frame.to_string(), &host, &pending)
            .await
            .unwrap();
        assert!(host.inbox.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn handle_frame_rejects_invalid_json() {
        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        let host = RecordingHost::default();
        let err = handle_frame("not json", &host, &pending).await.unwrap_err();
        assert!(err.contains("invalid JSON"));
    }

    /// Loopback mock daemon that:
    ///   1. for each newline-delimited request, parses it,
    ///   2. responds with `{"timestamp": <next_ts>}` echoing the id,
    ///   3. before its first response, pushes one `receive` notification.
    /// Returns the bound address.
    async fn spawn_mock_daemon(notify_first: bool) -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let handle = tokio::spawn(async move {
            let (sock, _) = listener.accept().await.unwrap();
            let (read, mut write) = sock.into_split();
            if notify_first {
                let notif = json!({
                    "jsonrpc": "2.0",
                    "method": "receive",
                    "params": {
                        "envelope": {
                            "source": "+15550000099",
                            "timestamp": 1_700_000_000_000_i64,
                            "dataMessage": {"message": "ping"}
                        }
                    }
                });
                let mut line = serde_json::to_vec(&notif).unwrap();
                line.push(b'\n');
                let _ = write.write_all(&line).await;
                let _ = write.flush().await;
            }
            let mut lines = BufReader::new(read).lines();
            let mut next_ts: i64 = 1_700_000_001_000;
            while let Ok(Some(line)) = lines.next_line().await {
                if line.trim().is_empty() {
                    continue;
                }
                let v: Value = match serde_json::from_str(&line) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let id = v.get("id").cloned().unwrap_or(Value::Null);
                let resp = json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {"timestamp": next_ts}
                });
                next_ts += 1;
                let mut out = serde_json::to_vec(&resp).unwrap();
                out.push(b'\n');
                if write.write_all(&out).await.is_err() {
                    break;
                }
                let _ = write.flush().await;
            }
        });
        (addr, handle)
    }

    /// Manually drive the read-loop and writer against a mock TCP
    /// daemon (no `signal-cli` subprocess). Exercises send + receive.
    #[tokio::test]
    async fn send_and_receive_against_mock_daemon() {
        let (addr, mock) = spawn_mock_daemon(true).await;

        let mut config = make_config();
        config.daemon_bind_addr = addr.clone();
        config.timeout_secs = 5;
        let adapter = Arc::new(SignalChannelAdapter::new(config));

        // Connect manually so we don't need to spawn signal-cli.
        let stream = TcpStream::connect(&addr).await.unwrap();
        let _ = stream.set_nodelay(true);
        let (reader, writer) = stream.into_split();
        {
            let mut w = adapter.writer.lock().await;
            *w = Some(writer);
        }

        let host: Arc<dyn ChannelAdapterHost> =
            Arc::new(RecordingHost::default());
        let host_ref = host.clone();
        let pending = adapter.pending.clone();
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();
        let read_task = tokio::spawn(async move {
            SignalChannelAdapter::read_loop(
                reader,
                host_ref,
                pending,
                cancel_clone,
            )
            .await;
        });

        // First, send and confirm we get a real timestamp back.
        let payload = MessagePayload::text("hello");
        let id = adapter.send("+15550000001", &payload).await.unwrap();
        assert_eq!(id, "1700000001000");

        // Give the read loop a moment to deliver the inbound notification.
        for _ in 0..20 {
            tokio::time::sleep(Duration::from_millis(20)).await;
            // We can't easily downcast `host`, but the same Arc points
            // to a `RecordingHost`, so we can re-cast via a trick: build
            // a fresh recording host? Instead, just confirm the second
            // send still works (proves the read-loop is alive).
            break;
        }

        // Second send to prove ids are monotonic + matched.
        let id2 = adapter.send("+15550000002", &payload).await.unwrap();
        assert_eq!(id2, "1700000001001");

        cancel.cancel();
        // Drop the writer to close the socket (so mock daemon exits).
        {
            let mut w = adapter.writer.lock().await;
            w.take();
        }
        let _ = timeout(Duration::from_secs(2), read_task).await;
        let _ = timeout(Duration::from_secs(2), mock).await;
    }

    /// Verify the read-loop forwards a `receive` notification to the
    /// host even when no outbound requests are made.
    #[tokio::test]
    async fn receive_only_forwards_to_host() {
        let (addr, mock) = spawn_mock_daemon(true).await;

        let stream = TcpStream::connect(&addr).await.unwrap();
        let (reader, _writer) = stream.into_split();
        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        let host = Arc::new(RecordingHost::default());
        let inbox = host.inbox.clone();
        let host_dyn: Arc<dyn ChannelAdapterHost> = host;
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        let read_task = tokio::spawn(async move {
            SignalChannelAdapter::read_loop(
                reader,
                host_dyn,
                pending,
                cancel_clone,
            )
            .await;
        });

        // Wait until the notification arrives.
        let mut got = false;
        for _ in 0..50 {
            if !inbox.lock().unwrap().is_empty() {
                got = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(got, "expected one inbound notification");
        let recorded = inbox.lock().unwrap().clone();
        assert_eq!(recorded[0].sender, "+15550000099");
        assert_eq!(recorded[0].body, "ping");

        cancel.cancel();
        let _ = timeout(Duration::from_secs(2), read_task).await;
        let _ = timeout(Duration::from_secs(2), mock).await;
    }

    #[tokio::test]
    async fn send_times_out_when_daemon_silent() {
        // Mock daemon that accepts the connection but never replies.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let mock = tokio::spawn(async move {
            let (_sock, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(60)).await;
        });

        let mut config = make_config();
        config.daemon_bind_addr = addr.clone();
        config.timeout_secs = 1;
        let adapter = Arc::new(SignalChannelAdapter::new(config));

        let stream = TcpStream::connect(&addr).await.unwrap();
        let (_reader, writer) = stream.into_split();
        {
            let mut w = adapter.writer.lock().await;
            *w = Some(writer);
        }

        let payload = MessagePayload::text("hello");
        let err = adapter.send("+15550000001", &payload).await.unwrap_err();
        assert!(
            err.to_string().contains("timeout"),
            "unexpected error: {err}"
        );

        mock.abort();
    }

    #[tokio::test]
    async fn start_validates_phone_number() {
        let mut config = make_config();
        config.phone_number = String::new();
        let adapter = SignalChannelAdapter::new(config);

        let host: Arc<dyn ChannelAdapterHost> =
            Arc::new(RecordingHost::default());
        let cancel = CancellationToken::new();
        let result = adapter.start(host, cancel).await;
        assert!(result.is_err());
    }
}
