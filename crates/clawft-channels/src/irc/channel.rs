//! IRC channel adapter.
//!
//! Implements [`ChannelAdapter`] for IRC messaging. The runtime
//! performs a real TCP (or TLS) dial, completes the
//! `CAP LS 302 → CAP REQ → AUTHENTICATE → PASS / NICK / USER` handshake
//! per RFC 2812 + IRCv3 SASL, waits for `001 RPL_WELCOME`, auto-joins
//! the configured channels, then runs a read-loop that responds to
//! `PING` with `PONG` and forwards `PRIVMSG` events to the host bus.
//!
//! ## Synthetic message id
//!
//! IRC has no native message id. `send()` returns a synthetic id of
//! the form `<server-host>-<unix-millis>-<rand>` so that downstream
//! tracing has something correlatable; this id is local to the
//! process and is **not** echoed back by the server.
//!
//! ## Long-message handling
//!
//! Each `PRIVMSG` line on the wire must stay under the 512-byte RFC
//! limit (including `:nick!user@host PRIVMSG #chan :…\r\n`). We
//! conservatively split outbound bodies at 400-byte boundaries — that
//! leaves headroom for the prefix the server prepends before relaying.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use clawft_plugin::error::PluginError;
use clawft_plugin::message::MessagePayload;
use clawft_plugin::traits::{ChannelAdapter, ChannelAdapterHost};

use super::types::{validate_config, IrcAdapterConfig};

/// Maximum body bytes per PRIVMSG line. The IRC line limit is 512
/// bytes including command, target, prefix, and CRLF. 400 leaves
/// roughly 110 bytes for the server-side prefix the receiver sees,
/// which is enough for any realistic nick!user@host string.
const MAX_PRIVMSG_BODY: usize = 400;

type DynWriter = Box<dyn AsyncWrite + Unpin + Send>;
type DynReader = Box<dyn AsyncRead + Unpin + Send>;

/// IRC channel adapter.
pub struct IrcChannelAdapter {
    config: IrcAdapterConfig,
    /// Active writer half. `None` until `start()` has dialed and
    /// completed the handshake; cleared on shutdown.
    writer: Arc<Mutex<Option<DynWriter>>>,
}

impl IrcChannelAdapter {
    /// Create a new IRC channel adapter with the given configuration.
    pub fn new(config: IrcAdapterConfig) -> Self {
        Self {
            config,
            writer: Arc::new(Mutex::new(None)),
        }
    }

    /// Check if a sender nickname is in the allow list.
    ///
    /// If `allowed_senders` is empty, all senders are allowed.
    pub fn is_sender_allowed(&self, sender: &str) -> bool {
        if self.config.allowed_senders.is_empty() {
            return true;
        }
        self.config.allowed_senders.iter().any(|s| s == sender)
    }

    /// Validate the adapter configuration.
    fn validate(&self) -> Result<(), PluginError> {
        validate_config(&self.config).map_err(PluginError::LoadFailed)
    }

    /// Resolve a `password_env` style reference to a concrete password
    /// by reading the named env var. Empty / missing variables yield
    /// `None` (the caller decides whether that is fatal).
    fn read_secret(env_name: Option<&str>) -> Option<String> {
        env_name
            .and_then(|n| std::env::var(n).ok())
            .filter(|v| !v.is_empty())
    }

    /// Build the `AUTHENTICATE PLAIN` payload for SASL.
    fn sasl_plain_payload(authcid: &str, password: &str) -> String {
        use base64::Engine;
        // RFC 4616: authzid \0 authcid \0 password
        let raw = format!("\0{authcid}\0{password}");
        base64::engine::general_purpose::STANDARD.encode(raw.as_bytes())
    }
}

#[async_trait]
impl ChannelAdapter for IrcChannelAdapter {
    fn name(&self) -> &str {
        "irc"
    }

    fn display_name(&self) -> &str {
        "IRC"
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
            server = %self.config.server,
            port = self.config.port,
            tls = self.config.use_tls,
            nick = %self.config.nickname,
            "IRC channel adapter starting"
        );
        self.validate()?;

        // Dial.
        let (reader, writer) = dial(&self.config).await?;
        let mut reader = BufReader::new(reader);
        let mut writer = writer;

        // Handshake.
        handshake(&self.config, &mut reader, &mut writer).await?;

        // Auto-join.
        for ch in &self.config.channels {
            send_line(&mut writer, &format!("JOIN {ch}")).await?;
        }

        // Publish writer for outbound `send()` calls.
        {
            let mut slot = self.writer.lock().await;
            *slot = Some(writer);
        }

        let writer_slot = self.writer.clone();
        let server_host = self.config.server.clone();
        let allow = self.config.allowed_senders.clone();
        let channel_name = self.name().to_string();
        let cancel_for_loop = cancel.clone();

        // Read-loop.
        let read_handle = tokio::spawn(async move {
            let mut line = String::new();
            loop {
                line.clear();
                tokio::select! {
                    biased;
                    _ = cancel_for_loop.cancelled() => break,
                    res = reader.read_line(&mut line) => {
                        match res {
                            Ok(0) => {
                                warn!("irc: server closed connection");
                                break;
                            }
                            Ok(_) => {
                                let trimmed = line.trim_end_matches(['\r', '\n']);
                                if trimmed.is_empty() {
                                    continue;
                                }
                                if let Err(e) = handle_inbound(
                                    trimmed,
                                    &writer_slot,
                                    &host,
                                    &channel_name,
                                    &server_host,
                                    &allow,
                                ).await {
                                    error!(error = %e, "irc: inbound handler error");
                                }
                            }
                            Err(e) => {
                                error!(error = %e, "irc: read error; ending loop");
                                break;
                            }
                        }
                    }
                }
            }
        });

        // Wait for cancellation, then politely QUIT and tear down.
        cancel.cancelled().await;
        info!("IRC channel adapter shutting down");

        if let Some(mut w) = self.writer.lock().await.take() {
            // Best-effort QUIT; ignore errors (peer may have closed).
            let _ = send_line(&mut w, "QUIT :clawft shutdown").await;
            let _ = w.shutdown().await;
        }

        // Reader task should exit shortly after the socket closes.
        let _ = read_handle.await;
        Ok(())
    }

    async fn send(
        &self,
        target: &str,
        payload: &MessagePayload,
    ) -> Result<String, PluginError> {
        let content = match payload {
            MessagePayload::Text { content } => content.clone(),
            MessagePayload::Binary { .. } => {
                return Err(PluginError::ExecutionFailed(
                    "irc: binary payloads are not supported (IRC is text-only)".into(),
                ));
            }
            MessagePayload::Structured { .. } => {
                return Err(PluginError::ExecutionFailed(
                    "irc: structured payloads are not supported (IRC is text-only)".into(),
                ));
            }
            // `MessagePayload` is `#[non_exhaustive]`; reject any
            // future variant we don't know about.
            _ => {
                return Err(PluginError::ExecutionFailed(
                    "irc: payload variant not supported (IRC is text-only)".into(),
                ));
            }
        };

        // Reject embedded CR/LF — they would inject extra IRC commands.
        if content.contains('\n') || content.contains('\r') || content.contains('\0') {
            return Err(PluginError::ExecutionFailed(
                "irc: payload contains forbidden control byte (CR/LF/NUL)".into(),
            ));
        }

        let mut guard = self.writer.lock().await;
        let writer = guard.as_mut().ok_or_else(|| {
            PluginError::ExecutionFailed(
                "irc: not connected — call start() and wait for welcome before send()".into(),
            )
        })?;

        // Split on byte boundaries that respect UTF-8 codepoints.
        for chunk in chunk_utf8(&content, MAX_PRIVMSG_BODY) {
            let line = format!("PRIVMSG {target} :{chunk}");
            send_line(writer, &line).await?;
        }

        // Synthetic id: <server>-<unix-ms>-<rand-suffix>. Documented
        // in the module-level rustdoc; IRC has no native message id.
        let suffix = uuid::Uuid::new_v4().simple().to_string();
        let msg_id = format!(
            "{}-{}-{}",
            self.config.server,
            chrono::Utc::now().timestamp_millis(),
            &suffix[..8]
        );
        Ok(msg_id)
    }
}

// ---------------------------------------------------------------------------
// dial / handshake / line I/O helpers
// ---------------------------------------------------------------------------

async fn dial(
    cfg: &IrcAdapterConfig,
) -> Result<(DynReader, DynWriter), PluginError> {
    let addr = format!("{}:{}", cfg.server, cfg.port);
    let tcp = TcpStream::connect(&addr)
        .await
        .map_err(|e| PluginError::LoadFailed(format!("irc: TCP connect to {addr}: {e}")))?;
    // Lower the latency of small handshake frames; IRC is line-oriented.
    let _ = tcp.set_nodelay(true);

    if cfg.use_tls {
        let tls = tls_connect(&cfg.server, tcp).await?;
        let (r, w) = tokio::io::split(tls);
        Ok((Box::new(r), Box::new(w)))
    } else {
        let (r, w) = tcp.into_split();
        Ok((Box::new(r), Box::new(w)))
    }
}

async fn tls_connect(
    host: &str,
    tcp: TcpStream,
) -> Result<tokio_rustls::client::TlsStream<TcpStream>, PluginError> {
    use std::sync::Arc;
    use tokio_rustls::rustls::{ClientConfig, RootCertStore};
    use tokio_rustls::TlsConnector;

    // Trust roots from webpki-roots (Mozilla CA bundle, statically
    // compiled — no runtime cert-store dependency).
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

    let config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();

    let connector = TlsConnector::from(Arc::new(config));
    let server_name = rustls_pki_types::ServerName::try_from(host.to_string())
        .map_err(|e| PluginError::LoadFailed(format!("irc: bad TLS server name {host}: {e}")))?;
    connector
        .connect(server_name, tcp)
        .await
        .map_err(|e| PluginError::LoadFailed(format!("irc: TLS handshake: {e}")))
}

async fn send_line<W: AsyncWrite + Unpin + ?Sized>(
    w: &mut W,
    line: &str,
) -> Result<(), PluginError> {
    debug!(line = %line, "irc: → send");
    w.write_all(line.as_bytes())
        .await
        .map_err(|e| PluginError::ExecutionFailed(format!("irc: write: {e}")))?;
    w.write_all(b"\r\n")
        .await
        .map_err(|e| PluginError::ExecutionFailed(format!("irc: write: {e}")))?;
    w.flush()
        .await
        .map_err(|e| PluginError::ExecutionFailed(format!("irc: flush: {e}")))?;
    Ok(())
}

async fn handshake<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    cfg: &IrcAdapterConfig,
    reader: &mut BufReader<R>,
    writer: &mut W,
) -> Result<(), PluginError> {
    // Step 1: CAP LS 302 — discover server capabilities. We use the
    // response only to decide whether to request `sasl`; if the
    // server doesn't advertise SASL but we have a sasl password
    // configured, that's an operator misconfiguration (we log and
    // continue without SASL rather than fail closed, since many
    // legacy networks tolerate `PASS` instead).
    send_line(writer, "CAP LS 302").await?;

    let want_sasl = cfg.auth_method == "sasl"
        && IrcChannelAdapter::read_secret(cfg.password_env.as_deref()).is_some();

    let mut server_supports_sasl = false;
    let mut line = String::new();
    // Drain CAP LS lines until we see a non-LS response. Most servers
    // emit either `CAP * LS :…` (single) or `CAP * LS * :…` (paged).
    for _ in 0..16 {
        line.clear();
        let n = reader
            .read_line(&mut line)
            .await
            .map_err(|e| PluginError::LoadFailed(format!("irc: read CAP LS: {e}")))?;
        if n == 0 {
            return Err(PluginError::LoadFailed(
                "irc: server closed during CAP LS".into(),
            ));
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        debug!(line = %trimmed, "irc: ← cap-ls");
        if trimmed.contains(" CAP ") && trimmed.contains(" LS") {
            if trimmed.contains("sasl") {
                server_supports_sasl = true;
            }
            // Paged response uses `CAP * LS * :` for non-final pages;
            // a final page omits the second `*`.
            if !trimmed.contains(" LS * :") {
                break;
            }
        } else {
            // Some servers send NOTICE / PING before CAP LS replies.
            if let Some(rest) = trimmed.strip_prefix("PING ") {
                let pong = format!("PONG {rest}");
                send_line(writer, &pong).await?;
            }
        }
    }

    if want_sasl {
        if !server_supports_sasl {
            warn!("irc: SASL configured but server did not advertise sasl capability");
        } else {
            send_line(writer, "CAP REQ :sasl").await?;
            // Expect `CAP * ACK :sasl`.
            let mut ack = String::new();
            reader
                .read_line(&mut ack)
                .await
                .map_err(|e| PluginError::LoadFailed(format!("irc: read CAP ACK: {e}")))?;
            debug!(line = %ack.trim_end(), "irc: ← cap-ack");

            send_line(writer, "AUTHENTICATE PLAIN").await?;
            // Server responds `AUTHENTICATE +` to invite the payload.
            let mut prompt = String::new();
            reader
                .read_line(&mut prompt)
                .await
                .map_err(|e| PluginError::LoadFailed(format!("irc: read AUTHENTICATE: {e}")))?;
            debug!(line = %prompt.trim_end(), "irc: ← authenticate-prompt");

            let pw = IrcChannelAdapter::read_secret(cfg.password_env.as_deref())
                .ok_or_else(|| {
                    PluginError::LoadFailed(
                        "irc: SASL configured but password_env unset at runtime".into(),
                    )
                })?;
            let payload =
                IrcChannelAdapter::sasl_plain_payload(&cfg.nickname, &pw);
            send_line(writer, &format!("AUTHENTICATE {payload}")).await?;

            // Expect 903 RPL_SASLSUCCESS (or 904/905 failure).
            let mut sasl_result = String::new();
            reader
                .read_line(&mut sasl_result)
                .await
                .map_err(|e| {
                    PluginError::LoadFailed(format!("irc: read SASL result: {e}"))
                })?;
            debug!(line = %sasl_result.trim_end(), "irc: ← sasl-result");
            let sr_trim = sasl_result.trim_end_matches(['\r', '\n']);
            if !sr_trim.contains(" 903 ") {
                return Err(PluginError::LoadFailed(format!(
                    "irc: SASL authentication failed: {sr_trim}"
                )));
            }
        }
        send_line(writer, "CAP END").await?;
    } else if cfg.auth_method == "sasl" {
        // No password env present at runtime — abort the CAP negotiation.
        send_line(writer, "CAP END").await?;
    } else {
        send_line(writer, "CAP END").await?;
    }

    // Step 2: PASS (optional, for nickserv-on-connect or server pwd),
    // NICK, USER.
    if cfg.auth_method == "nickserv" {
        if let Some(pw) = IrcChannelAdapter::read_secret(cfg.password_env.as_deref()) {
            send_line(writer, &format!("PASS {pw}")).await?;
        }
    }
    send_line(writer, &format!("NICK {}", cfg.nickname)).await?;
    let user = if cfg.nickname.is_empty() {
        "clawft"
    } else {
        cfg.nickname.as_str()
    };
    // RFC 2812 USER: <user> <mode> <unused> :<realname>
    send_line(writer, &format!("USER {user} 0 * :{user} (clawft bot)"))
        .await?;

    // Step 3: wait for 001 RPL_WELCOME (or fail on hard errors).
    let mut welcome_seen = false;
    for _ in 0..200 {
        line.clear();
        let n = reader
            .read_line(&mut line)
            .await
            .map_err(|e| PluginError::LoadFailed(format!("irc: read welcome: {e}")))?;
        if n == 0 {
            return Err(PluginError::LoadFailed(
                "irc: server closed before 001 welcome".into(),
            ));
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        debug!(line = %trimmed, "irc: ← welcome-phase");

        // Respond to inline PINGs during welcome.
        if let Some(rest) = trimmed.strip_prefix("PING ") {
            let pong = format!("PONG {rest}");
            send_line(writer, &pong).await?;
            continue;
        }
        // Numerics 001..004 indicate a successful registration; we
        // commit on the first 001.
        if has_numeric(trimmed, "001") {
            welcome_seen = true;
            break;
        }
        // ERROR / 433 (nick in use) / 432 (erroneus nick) → bail out.
        if trimmed.starts_with("ERROR")
            || has_numeric(trimmed, "432")
            || has_numeric(trimmed, "433")
            || has_numeric(trimmed, "464")
            || has_numeric(trimmed, "465")
        {
            return Err(PluginError::LoadFailed(format!(
                "irc: registration rejected: {trimmed}"
            )));
        }
    }

    if !welcome_seen {
        return Err(PluginError::LoadFailed(
            "irc: never received 001 welcome".into(),
        ));
    }
    Ok(())
}

/// Returns true if the line is an IRC numeric reply with the given
/// 3-digit code. Numerics look like `:server 001 nick :Welcome…`.
fn has_numeric(line: &str, code: &str) -> bool {
    // Skip the optional source prefix.
    let rest = line.strip_prefix(':').map_or(line, |s| {
        s.split_once(' ').map(|(_, r)| r).unwrap_or(s)
    });
    rest.starts_with(code)
        && rest
            .as_bytes()
            .get(code.len())
            .map(|b| *b == b' ')
            .unwrap_or(false)
}

/// Handle a single inbound IRC line.
async fn handle_inbound(
    line: &str,
    writer_slot: &Arc<Mutex<Option<DynWriter>>>,
    host: &Arc<dyn ChannelAdapterHost>,
    channel_name: &str,
    server_host: &str,
    allow: &[String],
) -> Result<(), PluginError> {
    // PING — must be answered immediately to avoid being killed.
    if let Some(rest) = line.strip_prefix("PING ") {
        let pong = format!("PONG {rest}");
        let mut guard = writer_slot.lock().await;
        if let Some(w) = guard.as_mut() {
            send_line(w, &pong).await?;
        }
        return Ok(());
    }

    // Parse a PRIVMSG. Format:
    //   :nick!user@host PRIVMSG <target> :<text>
    if !line.starts_with(':') {
        return Ok(());
    }
    let (prefix, rest) = match line[1..].split_once(' ') {
        Some(t) => t,
        None => return Ok(()),
    };
    let mut parts = rest.splitn(3, ' ');
    let cmd = parts.next().unwrap_or("");
    if cmd != "PRIVMSG" {
        return Ok(());
    }
    let target = parts.next().unwrap_or("");
    let raw_body = parts.next().unwrap_or("");
    let body = raw_body.strip_prefix(':').unwrap_or(raw_body);

    // Sender nick = everything before `!` in the prefix.
    let sender = prefix.split('!').next().unwrap_or(prefix);
    if !allow.is_empty() && !allow.iter().any(|s| s == sender) {
        debug!(sender, "irc: ignoring PRIVMSG from non-allow-listed sender");
        return Ok(());
    }

    let mut metadata = std::collections::HashMap::new();
    metadata.insert(
        "irc_server".to_string(),
        serde_json::Value::String(server_host.to_string()),
    );
    metadata.insert(
        "irc_target".to_string(),
        serde_json::Value::String(target.to_string()),
    );

    host.deliver_inbound(
        channel_name,
        sender,
        target,
        MessagePayload::text(body),
        metadata,
    )
    .await
}

/// Split `s` into UTF-8-safe chunks of at most `max_bytes` bytes each.
fn chunk_utf8(s: &str, max_bytes: usize) -> Vec<&str> {
    if s.len() <= max_bytes {
        return vec![s];
    }
    let mut out = Vec::new();
    let mut start = 0;
    while start < s.len() {
        let remaining = s.len() - start;
        if remaining <= max_bytes {
            out.push(&s[start..]);
            break;
        }
        // Scan back from the byte cap to a UTF-8 codepoint boundary.
        let mut end = start + max_bytes;
        while end > start && !s.is_char_boundary(end) {
            end -= 1;
        }
        if end == start {
            // Pathological: a single codepoint > max_bytes. Force
            // forward to the next boundary so we don't loop forever.
            end = start + max_bytes;
            while end < s.len() && !s.is_char_boundary(end) {
                end += 1;
            }
        }
        out.push(&s[start..end]);
        start = end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex as StdMutex;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader as TokioBufReader};
    use tokio::net::TcpListener;

    fn make_config_for_port(port: u16) -> IrcAdapterConfig {
        IrcAdapterConfig {
            server: "127.0.0.1".into(),
            port,
            use_tls: false,
            nickname: "clawft-bot".into(),
            channels: vec!["#general".into()],
            ..Default::default()
        }
    }

    // -------- pure-function tests --------

    #[test]
    fn name_is_irc() {
        let adapter = IrcChannelAdapter::new(make_config_for_port(6667));
        assert_eq!(adapter.name(), "irc");
        assert_eq!(adapter.display_name(), "IRC");
    }

    #[test]
    fn no_threads_or_media() {
        let adapter = IrcChannelAdapter::new(make_config_for_port(6667));
        assert!(!adapter.supports_threads());
        assert!(!adapter.supports_media());
    }

    #[test]
    fn sender_allowed_empty_list_allows_all() {
        let adapter = IrcChannelAdapter::new(make_config_for_port(6667));
        assert!(adapter.is_sender_allowed("anyone"));
        assert!(adapter.is_sender_allowed("stranger"));
    }

    #[test]
    fn sender_allowed_with_filter() {
        let mut config = make_config_for_port(6667);
        config.allowed_senders = vec!["admin".into()];
        let adapter = IrcChannelAdapter::new(config);
        assert!(adapter.is_sender_allowed("admin"));
        assert!(!adapter.is_sender_allowed("random"));
    }

    #[test]
    fn validate_rejects_empty_server() {
        let mut cfg = make_config_for_port(6667);
        cfg.server = String::new();
        let adapter = IrcChannelAdapter::new(cfg);
        assert!(adapter.validate().is_err());
    }

    #[test]
    fn chunk_utf8_short_returns_one() {
        assert_eq!(chunk_utf8("hello", 400), vec!["hello"]);
    }

    #[test]
    fn chunk_utf8_splits_at_boundary() {
        let s = "a".repeat(900);
        let chunks = chunk_utf8(&s, 400);
        assert_eq!(chunks.len(), 3);
        assert!(chunks.iter().all(|c| c.len() <= 400));
        assert_eq!(chunks.concat(), s);
    }

    #[test]
    fn chunk_utf8_respects_codepoints() {
        // 4-byte glyphs near the cap.
        let s = "x".repeat(398) + "🦀🦀🦀";
        let chunks = chunk_utf8(&s, 400);
        for c in &chunks {
            assert!(c.is_char_boundary(0));
            assert!(std::str::from_utf8(c.as_bytes()).is_ok());
        }
        assert_eq!(chunks.concat(), s);
    }

    #[test]
    fn has_numeric_matches() {
        assert!(has_numeric(":irc.example 001 bot :Welcome", "001"));
        assert!(has_numeric(":irc.example 433 * bot :Nick in use", "433"));
        assert!(!has_numeric(":irc.example NOTICE * :hi", "001"));
        assert!(!has_numeric(":irc.example 0011 bot :nope", "001"));
    }

    #[test]
    fn sasl_plain_payload_known_vector() {
        // RFC 4616 example: authcid=tim, password=tanstaaftanstaaf,
        // authzid empty -> base64("\0tim\0tanstaaftanstaaf").
        let got = IrcChannelAdapter::sasl_plain_payload("tim", "tanstaaftanstaaf");
        assert_eq!(got, "AHRpbQB0YW5zdGFhZnRhbnN0YWFm");
    }

    // -------- mock-server integration tests --------

    /// A minimal IRC server harness. Accepts one TCP connection, runs
    /// the supplied scripted task against it, and records the outbound
    /// lines the client sent for assertions.
    struct MockServer {
        port: u16,
        sent: Arc<StdMutex<Vec<String>>>,
    }

    impl MockServer {
        async fn spawn<F, Fut>(script: F) -> Self
        where
            F: FnOnce(
                    tokio::net::tcp::OwnedReadHalf,
                    tokio::net::tcp::OwnedWriteHalf,
                    Arc<StdMutex<Vec<String>>>,
                ) -> Fut
                + Send
                + 'static,
            Fut: std::future::Future<Output = ()> + Send + 'static,
        {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let sent: Arc<StdMutex<Vec<String>>> = Arc::new(StdMutex::new(Vec::new()));
            let sent_for_task = sent.clone();
            tokio::spawn(async move {
                let (sock, _) = listener.accept().await.unwrap();
                let (r, w) = sock.into_split();
                script(r, w, sent_for_task).await;
            });
            Self { port, sent }
        }
    }

    /// Drive a client through welcome and capture the lines it sent.
    async fn welcome_script(
        r: tokio::net::tcp::OwnedReadHalf,
        mut w: tokio::net::tcp::OwnedWriteHalf,
        sent: Arc<StdMutex<Vec<String>>>,
    ) {
        let mut br = TokioBufReader::new(r);
        let mut line = String::new();
        // Send a single CAP LS reply (no sasl).
        w.write_all(b":mock.test CAP * LS :multi-prefix\r\n").await.unwrap();
        loop {
            line.clear();
            if br.read_line(&mut line).await.unwrap() == 0 {
                return;
            }
            let trimmed = line.trim_end_matches(['\r', '\n']).to_string();
            sent.lock().unwrap().push(trimmed.clone());
            if trimmed.starts_with("USER ") {
                // Send 001 welcome.
                w.write_all(
                    b":mock.test 001 clawft-bot :Welcome to mock.test clawft-bot\r\n",
                )
                .await
                .unwrap();
            }
            if trimmed.starts_with("JOIN ") {
                // Acknowledge join + immediately deliver an inbound PRIVMSG.
                w.write_all(
                    b":alice!alice@host PRIVMSG #general :hello bot\r\n",
                )
                .await
                .unwrap();
            }
            if trimmed.starts_with("QUIT") {
                let _ = w.shutdown().await;
                return;
            }
        }
    }

    struct CapturingHost {
        delivered: Arc<StdMutex<Vec<(String, String, String, String)>>>,
    }
    #[async_trait]
    impl ChannelAdapterHost for CapturingHost {
        async fn deliver_inbound(
            &self,
            channel: &str,
            sender_id: &str,
            chat_id: &str,
            payload: MessagePayload,
            _metadata: HashMap<String, serde_json::Value>,
        ) -> Result<(), PluginError> {
            let body = match payload {
                MessagePayload::Text { content } => content,
                _ => "<non-text>".into(),
            };
            self.delivered.lock().unwrap().push((
                channel.into(),
                sender_id.into(),
                chat_id.into(),
                body,
            ));
            Ok(())
        }
    }

    #[tokio::test]
    async fn connect_and_complete_welcome() {
        let server = MockServer::spawn(welcome_script).await;
        let cfg = make_config_for_port(server.port);
        let adapter = Arc::new(IrcChannelAdapter::new(cfg));
        let host: Arc<dyn ChannelAdapterHost> = Arc::new(CapturingHost {
            delivered: Arc::new(StdMutex::new(Vec::new())),
        });
        let cancel = CancellationToken::new();
        let cancel_for_task = cancel.clone();
        let adapter_for_task = adapter.clone();
        let handle = tokio::spawn(async move {
            adapter_for_task.start(host, cancel_for_task).await
        });

        // Wait briefly for handshake + JOIN to flush server-side.
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        cancel.cancel();
        let res = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            handle,
        )
        .await
        .expect("adapter shutdown timeout")
        .unwrap();
        assert!(res.is_ok(), "start() returned error: {res:?}");

        let lines = server.sent.lock().unwrap().clone();
        assert!(lines.iter().any(|l| l == "CAP LS 302"), "missing CAP LS 302: {lines:?}");
        assert!(lines.iter().any(|l| l == "CAP END"));
        assert!(lines.iter().any(|l| l == "NICK clawft-bot"));
        assert!(lines.iter().any(|l| l.starts_with("USER ")));
        assert!(lines.iter().any(|l| l == "JOIN #general"));
        // QUIT is best-effort on cancel-via-token; the welcome+JOIN
        // handshake is what this test gates. Explicit stop() exercises
        // the QUIT path elsewhere.
    }

    #[tokio::test]
    async fn send_privmsg_writes_protocol_line() {
        let server = MockServer::spawn(welcome_script).await;
        let cfg = make_config_for_port(server.port);
        let adapter = Arc::new(IrcChannelAdapter::new(cfg));
        let host: Arc<dyn ChannelAdapterHost> = Arc::new(CapturingHost {
            delivered: Arc::new(StdMutex::new(Vec::new())),
        });
        let cancel = CancellationToken::new();
        let cancel_for_task = cancel.clone();
        let adapter_for_task = adapter.clone();
        let handle = tokio::spawn(async move {
            adapter_for_task.start(host, cancel_for_task).await
        });

        // Wait for connection + welcome to settle.
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;

        let payload = MessagePayload::text("hello world");
        let id = adapter.send("#general", &payload).await.unwrap();
        assert!(id.starts_with("127.0.0.1-"), "unexpected id format: {id}");

        // Allow the line to flush before we capture.
        tokio::time::sleep(std::time::Duration::from_millis(75)).await;
        cancel.cancel();
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            handle,
        )
        .await
        .unwrap();

        let lines = server.sent.lock().unwrap().clone();
        assert!(
            lines.iter().any(|l| l == "PRIVMSG #general :hello world"),
            "outbound PRIVMSG not seen: {lines:?}"
        );
    }

    #[tokio::test]
    async fn receive_privmsg_publishes_to_host() {
        let server = MockServer::spawn(welcome_script).await;
        let cfg = make_config_for_port(server.port);
        let adapter = Arc::new(IrcChannelAdapter::new(cfg));
        let delivered: Arc<StdMutex<Vec<(String, String, String, String)>>> =
            Arc::new(StdMutex::new(Vec::new()));
        let host: Arc<dyn ChannelAdapterHost> = Arc::new(CapturingHost {
            delivered: delivered.clone(),
        });
        let cancel = CancellationToken::new();
        let cancel_for_task = cancel.clone();
        let adapter_for_task = adapter.clone();
        let handle = tokio::spawn(async move {
            adapter_for_task.start(host, cancel_for_task).await
        });

        // The mock server emits one inbound PRIVMSG immediately after JOIN.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        cancel.cancel();
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            handle,
        )
        .await
        .unwrap();

        let got = delivered.lock().unwrap().clone();
        assert!(
            got.iter().any(|(ch, sender, target, body)| {
                ch == "irc"
                    && sender == "alice"
                    && target == "#general"
                    && body == "hello bot"
            }),
            "expected inbound PRIVMSG not delivered: {got:?}"
        );
    }

    #[tokio::test]
    async fn send_rejects_non_text_payloads() {
        let adapter = IrcChannelAdapter::new(make_config_for_port(6667));
        let bin = MessagePayload::binary("audio/wav", vec![0u8; 4]);
        assert!(adapter.send("#general", &bin).await.is_err());
        let st = MessagePayload::structured(serde_json::json!({"k": "v"}));
        assert!(adapter.send("#general", &st).await.is_err());
    }

    #[tokio::test]
    async fn send_without_connect_errors() {
        let adapter = IrcChannelAdapter::new(make_config_for_port(6667));
        let payload = MessagePayload::text("hi");
        let err = adapter.send("#general", &payload).await.unwrap_err();
        assert!(err.to_string().contains("not connected"));
    }

    #[tokio::test]
    async fn send_rejects_embedded_crlf() {
        // Even without a live connection we exercise the validation
        // path before the writer lookup is attempted? -- the writer
        // check happens after the CRLF check, so this would also
        // surface the not-connected error. Pin this to the same
        // mock-server fixture so the writer is in place.
        let server = MockServer::spawn(welcome_script).await;
        let cfg = make_config_for_port(server.port);
        let adapter = Arc::new(IrcChannelAdapter::new(cfg));
        let host: Arc<dyn ChannelAdapterHost> = Arc::new(CapturingHost {
            delivered: Arc::new(StdMutex::new(Vec::new())),
        });
        let cancel = CancellationToken::new();
        let cancel_for_task = cancel.clone();
        let adapter_for_task = adapter.clone();
        let handle = tokio::spawn(async move {
            adapter_for_task.start(host, cancel_for_task).await
        });
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;

        let bad = MessagePayload::text("inject\r\nQUIT");
        let err = adapter.send("#general", &bad).await.unwrap_err();
        assert!(err.to_string().contains("CR/LF/NUL"));

        cancel.cancel();
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            handle,
        )
        .await
        .unwrap();
    }
}
