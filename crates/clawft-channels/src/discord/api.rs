//! Discord REST API client.
//!
//! [`DiscordApiClient`] provides typed methods for the subset of the
//! Discord REST API used by the channel plugin: sending and editing
//! messages.

use reqwest::Client;
use tracing::{debug, warn};

use clawft_types::error::ChannelError;

use super::events::RateLimitInfo;

/// Base URL for the Discord REST API v10.
const DISCORD_API_BASE: &str = "https://discord.com/api/v10";

/// Response from creating or editing a message.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct DiscordMessage {
    /// Unique message ID.
    pub id: String,

    /// Channel where the message lives.
    pub channel_id: String,

    /// Message content.
    pub content: String,
}

/// HTTP client for the Discord REST API.
///
/// Wraps a [`reqwest::Client`] with Bot token authentication and
/// basic rate limit tracking.
pub struct DiscordApiClient {
    /// Shared HTTP client.
    http: Client,
    /// Bot token for API authorization.
    token: String,
    /// Base URL for API calls.
    base_url: String,
}

impl DiscordApiClient {
    /// Create a new client with the given bot token.
    pub fn new(token: String) -> Self {
        Self {
            http: Client::new(),
            token,
            base_url: DISCORD_API_BASE.to_owned(),
        }
    }

    /// Create a client pointing at a custom base URL (for testing).
    #[cfg(test)]
    pub fn with_base_url(token: String, base_url: String) -> Self {
        Self {
            http: Client::new(),
            token,
            base_url,
        }
    }

    /// Return the base URL used for API requests.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Send a message to a channel.
    ///
    /// Returns the message ID on success.
    pub async fn create_message(
        &self,
        channel_id: &str,
        content: &str,
    ) -> Result<String, ChannelError> {
        let url = format!("{}/channels/{channel_id}/messages", self.base_url);

        let body = serde_json::json!({
            "content": content,
        });

        debug!(channel_id = %channel_id, "creating message");

        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Bot {}", self.token))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| ChannelError::SendFailed(e.to_string()))?;

        // Check rate limit headers.
        let rate_limit = RateLimitInfo::from_headers(resp.headers());
        if rate_limit.is_limited() {
            let wait_ms = rate_limit.retry_after_ms().unwrap_or(1000);
            warn!(
                wait_ms = wait_ms,
                "Discord rate limit reached, waiting before retry"
            );
            tokio::time::sleep(std::time::Duration::from_millis(wait_ms)).await;
        }

        let status = resp.status();
        if !status.is_success() {
            let err_body = resp.text().await.unwrap_or_else(|_| "unknown error".into());
            return Err(ChannelError::SendFailed(format!(
                "Discord API returned {status}: {err_body}"
            )));
        }

        let msg: DiscordMessage = resp
            .json()
            .await
            .map_err(|e| ChannelError::SendFailed(e.to_string()))?;

        Ok(msg.id)
    }

    /// Edit an existing message.
    ///
    /// On a 429 / rate-limited response the call sleeps for the
    /// `retry_after_ms` window indicated by the Discord rate-limit
    /// headers (or the response body's `retry_after`) and retries the
    /// PATCH exactly once. If the second attempt also returns an error
    /// status, the error is propagated to the caller.
    pub async fn edit_message(
        &self,
        channel_id: &str,
        message_id: &str,
        content: &str,
    ) -> Result<(), ChannelError> {
        let url = format!(
            "{}/channels/{channel_id}/messages/{message_id}",
            self.base_url
        );

        let body = serde_json::json!({
            "content": content,
        });

        debug!(
            channel_id = %channel_id,
            message_id = %message_id,
            "editing message"
        );

        let resp = self
            .http
            .patch(&url)
            .header("Authorization", format!("Bot {}", self.token))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| ChannelError::SendFailed(e.to_string()))?;

        // If rate-limited, sleep for the indicated window and retry once.
        // Discord signals rate-limiting either by returning HTTP 429 or
        // by setting `x-ratelimit-remaining: 0` on a normal response.
        let rate_limit = RateLimitInfo::from_headers(resp.headers());
        let status = resp.status();
        let is_429 = status.as_u16() == 429;
        if is_429 || rate_limit.is_limited() {
            let wait_ms = rate_limit.retry_after_ms().unwrap_or(1000);
            warn!(
                wait_ms = wait_ms,
                status = %status,
                "Discord rate limit reached on edit_message, sleeping before single retry"
            );
            tokio::time::sleep(std::time::Duration::from_millis(wait_ms)).await;

            let retry = self
                .http
                .patch(&url)
                .header("Authorization", format!("Bot {}", self.token))
                .header("Content-Type", "application/json")
                .json(&body)
                .send()
                .await
                .map_err(|e| ChannelError::SendFailed(e.to_string()))?;

            let retry_status = retry.status();
            if !retry_status.is_success() {
                let err_body = retry
                    .text()
                    .await
                    .unwrap_or_else(|_| "unknown error".into());
                return Err(ChannelError::SendFailed(format!(
                    "Discord API returned {retry_status} after rate-limit retry: {err_body}"
                )));
            }
            return Ok(());
        }

        if !status.is_success() {
            let err_body = resp.text().await.unwrap_or_else(|_| "unknown error".into());
            return Err(ChannelError::SendFailed(format!(
                "Discord API returned {status}: {err_body}"
            )));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn default_base_url() {
        let client = DiscordApiClient::new("test-token".into());
        assert_eq!(client.base_url(), "https://discord.com/api/v10");
    }

    #[test]
    fn custom_base_url() {
        let client =
            DiscordApiClient::with_base_url("test-token".into(), "http://localhost:9999".into());
        assert_eq!(client.base_url(), "http://localhost:9999");
    }

    /// Read a single HTTP request from a stream until the body has
    /// arrived. Returns once the request line, headers, and exactly
    /// `Content-Length` body bytes (or 0) have been consumed. Headers
    /// are not parsed beyond `Content-Length`.
    async fn read_request(stream: &mut tokio::net::TcpStream) {
        let mut buf = [0u8; 4096];
        let mut total = Vec::with_capacity(1024);
        let mut content_length: Option<usize> = None;
        let mut header_end: Option<usize> = None;

        loop {
            let n = stream.read(&mut buf).await.unwrap_or(0);
            if n == 0 {
                return;
            }
            total.extend_from_slice(&buf[..n]);

            if header_end.is_none() {
                if let Some(idx) = total.windows(4).position(|w| w == b"\r\n\r\n") {
                    header_end = Some(idx + 4);
                    let header_str = String::from_utf8_lossy(&total[..idx]);
                    for line in header_str.split("\r\n") {
                        if let Some((k, v)) = line.split_once(':') {
                            if k.trim().eq_ignore_ascii_case("content-length") {
                                content_length = v.trim().parse::<usize>().ok();
                            }
                        }
                    }
                }
            }

            if let (Some(end), len) = (header_end, content_length.unwrap_or(0))
                && total.len() >= end + len
            {
                return;
            }
            if header_end.is_some() && content_length.is_none() {
                // No body expected.
                return;
            }
        }
    }

    /// WEFT-161: a 429 response on `edit_message` must cause the
    /// client to sleep for `retry_after_ms` and retry the PATCH.
    /// The mock server returns 429 the first time and 204 No Content
    /// the second time; the test asserts both that the call succeeds
    /// (proving the retry happened) and that wall-clock elapsed at
    /// least the rate-limit window (proving the sleep happened).
    #[tokio::test]
    async fn edit_message_sleeps_and_retries_on_rate_limit() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_clone = calls.clone();

        // Spawn a tiny HTTP mock that:
        //   1st request → 429 + x-ratelimit-remaining: 0 + x-ratelimit-reset-after: 0.05
        //   2nd request → 204 No Content
        let server = tokio::spawn(async move {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                read_request(&mut stream).await;
                let n = calls_clone.fetch_add(1, Ordering::SeqCst);
                let response: &[u8] = if n == 0 {
                    // 429: use a 50ms reset window to keep the test fast.
                    b"HTTP/1.1 429 Too Many Requests\r\n\
                      x-ratelimit-remaining: 0\r\n\
                      x-ratelimit-reset-after: 0.05\r\n\
                      content-length: 0\r\n\
                      connection: close\r\n\r\n"
                } else {
                    b"HTTP/1.1 204 No Content\r\n\
                      content-length: 0\r\n\
                      connection: close\r\n\r\n"
                };
                stream.write_all(response).await.unwrap();
                stream.flush().await.unwrap();
                let _ = stream.shutdown().await;
            }
        });

        let client = DiscordApiClient::with_base_url(
            "test-token".into(),
            format!("http://127.0.0.1:{port}"),
        );

        let started = std::time::Instant::now();
        let result = client.edit_message("chan-1", "msg-1", "hello").await;
        let elapsed = started.elapsed();

        server.await.unwrap();

        assert!(
            result.is_ok(),
            "edit_message should succeed after retry: {result:?}"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "edit_message must retry the PATCH exactly once after a 429"
        );
        assert!(
            elapsed >= std::time::Duration::from_millis(50),
            "edit_message must sleep for the rate-limit window before retrying \
             (elapsed = {elapsed:?})"
        );
    }

    /// Negative companion: when the server returns a non-rate-limit
    /// error, the client must NOT retry and must surface the error.
    #[tokio::test]
    async fn edit_message_does_not_retry_on_non_429_failure() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_clone = calls.clone();

        let server = tokio::spawn(async move {
            // Accept one connection only; if the client retries it will
            // hang on accept and the test will fail with a count of 2
            // never reached, but only the single response is sent.
            let (mut stream, _) = listener.accept().await.unwrap();
            read_request(&mut stream).await;
            calls_clone.fetch_add(1, Ordering::SeqCst);
            let response: &[u8] = b"HTTP/1.1 500 Internal Server Error\r\n\
                                    content-length: 0\r\n\
                                    connection: close\r\n\r\n";
            stream.write_all(response).await.unwrap();
            stream.flush().await.unwrap();
            let _ = stream.shutdown().await;
        });

        let client = DiscordApiClient::with_base_url(
            "test-token".into(),
            format!("http://127.0.0.1:{port}"),
        );

        let result = client.edit_message("chan-1", "msg-1", "hello").await;
        server.await.unwrap();

        assert!(result.is_err(), "500 must propagate as an error");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "non-rate-limit failures must not trigger a retry"
        );
    }
}
