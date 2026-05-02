//! Daemon client — connects to a running kernel daemon.
//!
//! On Unix, connects over a Unix domain socket. On other platforms,
//! `connect()` always returns `None` (daemon transport not yet available).

// ── Unix implementation ──────────────────────────────────────────

#[cfg(unix)]
mod imp {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;

    use crate::protocol::{self, Request, Response};

    /// A client connected to the kernel daemon.
    pub struct DaemonClient {
        stream: UnixStream,
    }

    impl DaemonClient {
        /// Try to connect to the daemon. Returns `None` if no daemon is running.
        pub async fn connect() -> Option<Self> {
            let path = protocol::socket_path();
            let stream = UnixStream::connect(&path).await.ok()?;
            Some(Self { stream })
        }

        /// Send a request and wait for the response.
        ///
        /// WEFT-479: when the request has no `auth` set, the client
        /// transparently attaches an `admin` scope token. The unix-
        /// socket path is filesystem-permission-gated, so any caller
        /// who can connect to the socket is already trusted to admin
        /// the daemon. The capability gate is the load-bearing
        /// defence on the TCP relay path (where bearer auth at the
        /// connection layer is the additional control); on the local
        /// UDS path it would be redundant. A caller that explicitly
        /// passes a non-admin scope (`Request::with_auth("read")`)
        /// keeps that scope; only an absent token gets the implicit
        /// upgrade.
        pub async fn call(&mut self, mut request: Request) -> anyhow::Result<Response> {
            if request.auth.is_none() {
                request.auth = Some("admin".to_string());
            }
            let mut json = serde_json::to_string(&request)?;
            json.push('\n');

            self.stream.write_all(json.as_bytes()).await?;

            let mut reader = BufReader::new(&mut self.stream);
            let mut line = String::new();
            reader.read_line(&mut line).await?;

            if line.trim().is_empty() {
                anyhow::bail!("daemon closed connection without response");
            }

            let response: Response = serde_json::from_str(line.trim())?;
            Ok(response)
        }

        /// Convenience: send a no-params request.
        pub async fn simple_call(&mut self, method: &str) -> anyhow::Result<Response> {
            self.call(Request::new(method)).await
        }
    }
}

// ── Non-Unix stub ────────────────────────────────────────────────

#[cfg(not(unix))]
mod imp {
    use crate::protocol::{Request, Response};

    /// Stub daemon client for non-Unix platforms.
    ///
    /// `connect()` always returns `None`. Windows named-pipe transport
    /// is deferred to 0.8.x (WEFT-483) — the 0.7.0 release matrix in
    /// `cargo-dist.toml` (`[workspace.metadata.dist]`) excludes
    /// `x86_64-pc-windows-msvc` for that reason. When the named-pipe
    /// transport lands, replace this stub with a
    /// `tokio::net::windows::named_pipe::ClientOptions`-based impl
    /// that mirrors the unix `DaemonClient` API and re-add the target.
    pub struct DaemonClient;

    impl DaemonClient {
        /// Always returns `None` on non-Unix platforms.
        pub async fn connect() -> Option<Self> {
            None
        }

        pub async fn call(&mut self, _request: Request) -> anyhow::Result<Response> {
            anyhow::bail!("daemon not available on this platform")
        }

        pub async fn simple_call(&mut self, _method: &str) -> anyhow::Result<Response> {
            anyhow::bail!("daemon not available on this platform")
        }
    }
}

pub use imp::DaemonClient;

/// Check if a daemon is running (socket exists and accepts connections).
pub async fn is_daemon_running() -> bool {
    DaemonClient::connect().await.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn connect_returns_none_when_no_daemon() {
        let client = DaemonClient::connect().await;
        assert!(client.is_none());
    }

    #[tokio::test]
    async fn is_daemon_running_false_when_no_daemon() {
        assert!(!is_daemon_running().await);
    }
}
