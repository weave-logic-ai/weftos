//! Daemon client — connects to a running kernel daemon.
//!
//! - **Unix**: Unix domain socket under the runtime directory.
//! - **Windows**: named pipe derived from the same logical path (WEFT-11).
//! - **Other**: `connect()` returns `None` (no local transport).

// ── Unix implementation ──────────────────────────────────────────

#[cfg(unix)]
mod imp {
    use std::path::Path;

    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;
    use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

    use crate::protocol::{self, Request, Response};

    /// A client connected to the kernel daemon.
    pub struct DaemonClient {
        stream: UnixStream,
        /// Per-client request context; overrides the process default.
        pub(crate) ctx: Option<crate::connect::ClientContext>,
    }

    /// Streaming subscription returned by [`DaemonClient::open_stream`].
    ///
    /// After the initial ack `Response`, the daemon owns the write half
    /// of the connection and pushes line-delimited JSON frames. This
    /// reader yields those frames until the stream ends (daemon
    /// disconnect or client drop). The write half is retained so the
    /// socket stays open for the forwarder (WEFT-434).
    pub struct StreamSession {
        reader: BufReader<OwnedReadHalf>,
        _writer: OwnedWriteHalf,
    }

    impl StreamSession {
        /// Read the next newline-terminated frame.
        ///
        /// Returns `Ok(None)` on clean EOF (daemon closed the stream).
        pub async fn next_line(&mut self) -> anyhow::Result<Option<String>> {
            let mut line = String::new();
            let n = self.reader.read_line(&mut line).await?;
            if n == 0 {
                return Ok(None);
            }
            Ok(Some(line))
        }
    }

    impl DaemonClient {
        /// Try to connect to the daemon at the default socket path.
        /// Returns `None` if no daemon is running.
        pub async fn connect() -> Option<Self> {
            Self::connect_path(protocol::socket_path()).await
        }

        /// Try to connect to a daemon at an explicit socket path.
        ///
        /// Used by hermetic tests (and callers that already know the
        /// runtime dir) so they do not probe the shared default path
        /// that a live developer daemon may own (WEFT-645).
        pub async fn connect_path(path: impl AsRef<Path>) -> Option<Self> {
            let stream = UnixStream::connect(path.as_ref()).await.ok()?;
            Some(Self { stream, ctx: None })
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
            crate::connect::stamp_request(&mut request, self.ctx.as_ref());
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

        /// Open a streaming RPC (WEFT-434).
        ///
        /// Sends `request`, reads a single ack [`Response`], then
        /// returns a [`StreamSession`] for subsequent line-delimited
        /// frames. Consumes the client — the connection is owned by
        /// the stream until dropped.
        ///
        /// Used by `kernel.logs_stream`, `ipc.subscribe_stream`, and
        /// `substrate.subscribe`.
        pub async fn open_stream(
            mut self,
            mut request: Request,
        ) -> anyhow::Result<(Response, StreamSession)> {
            crate::connect::stamp_request(&mut request, self.ctx.as_ref());
            let mut json = serde_json::to_string(&request)?;
            json.push('\n');
            self.stream.write_all(json.as_bytes()).await?;

            let (reader, writer) = self.stream.into_split();
            let mut reader = BufReader::new(reader);
            let mut line = String::new();
            reader.read_line(&mut line).await?;
            if line.trim().is_empty() {
                anyhow::bail!("daemon closed connection without stream ack");
            }
            let response: Response = serde_json::from_str(line.trim())?;
            Ok((
                response,
                StreamSession {
                    reader,
                    _writer: writer,
                },
            ))
        }
    }
}

// ── Windows named-pipe implementation (WEFT-11) ──────────────────

#[cfg(windows)]
mod imp {
    use std::path::Path;
    use std::time::Duration;

    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, ReadHalf, WriteHalf};
    use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};

    use crate::protocol::{self, Request, Response};

    /// A client connected to the kernel daemon over a Windows named pipe.
    pub struct DaemonClient {
        stream: NamedPipeClient,
        /// Per-client request context; overrides the process default.
        pub(crate) ctx: Option<crate::connect::ClientContext>,
    }

    /// Streaming subscription returned by [`DaemonClient::open_stream`].
    pub struct StreamSession {
        reader: BufReader<ReadHalf<NamedPipeClient>>,
        _writer: WriteHalf<NamedPipeClient>,
    }

    impl StreamSession {
        /// Read the next newline-terminated frame.
        ///
        /// Returns `Ok(None)` on clean EOF (daemon closed the stream).
        pub async fn next_line(&mut self) -> anyhow::Result<Option<String>> {
            let mut line = String::new();
            let n = self.reader.read_line(&mut line).await?;
            if n == 0 {
                return Ok(None);
            }
            Ok(Some(line))
        }
    }

    /// `ERROR_PIPE_BUSY` — all instances are currently busy.
    const ERROR_PIPE_BUSY: i32 = 231;
    /// `ERROR_FILE_NOT_FOUND` — no pipe server is listening.
    const ERROR_FILE_NOT_FOUND: i32 = 2;

    impl DaemonClient {
        /// Try to connect to the daemon at the default logical socket path.
        ///
        /// Maps the path to a named pipe via [`protocol::pipe_name_for_path`].
        /// Returns `None` if no daemon is listening.
        pub async fn connect() -> Option<Self> {
            Self::connect_path(protocol::socket_path()).await
        }

        /// Try to connect to a daemon for an explicit logical socket path.
        ///
        /// The path is converted to a Windows named-pipe name. Used by
        /// hermetic tests (WEFT-645) and by callers that already know the
        /// runtime dir. Returns `None` when nothing is listening.
        pub async fn connect_path(path: impl AsRef<Path>) -> Option<Self> {
            let pipe = protocol::pipe_name_for_path(path.as_ref());
            connect_pipe(&pipe).await.map(|stream| Self { stream, ctx: None })
        }

        /// Send a request and wait for the response.
        ///
        /// WEFT-479: when the request has no `auth` set, the client
        /// attaches an `admin` scope token. On Windows the named pipe is
        /// ACL-gated by the creating process (same trust model as UDS
        /// filesystem permissions on Unix).
        pub async fn call(&mut self, mut request: Request) -> anyhow::Result<Response> {
            crate::connect::stamp_request(&mut request, self.ctx.as_ref());
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

        /// Open a streaming RPC (WEFT-434).
        ///
        /// Sends `request`, reads a single ack [`Response`], then
        /// returns a [`StreamSession`] for subsequent line-delimited
        /// frames. Consumes the client — the connection is owned by
        /// the stream until dropped.
        pub async fn open_stream(
            mut self,
            mut request: Request,
        ) -> anyhow::Result<(Response, StreamSession)> {
            crate::connect::stamp_request(&mut request, self.ctx.as_ref());
            let mut json = serde_json::to_string(&request)?;
            json.push('\n');
            self.stream.write_all(json.as_bytes()).await?;

            let (reader, writer) = tokio::io::split(self.stream);
            let mut reader = BufReader::new(reader);
            let mut line = String::new();
            reader.read_line(&mut line).await?;
            if line.trim().is_empty() {
                anyhow::bail!("daemon closed connection without stream ack");
            }
            let response: Response = serde_json::from_str(line.trim())?;
            Ok((
                response,
                StreamSession {
                    reader,
                    _writer: writer,
                },
            ))
        }
    }

    /// Open a named pipe, retrying briefly on `ERROR_PIPE_BUSY`.
    ///
    /// Returns `None` when no server is listening (`ERROR_FILE_NOT_FOUND`
    /// and similar dial failures).
    async fn connect_pipe(pipe: &str) -> Option<NamedPipeClient> {
        // A few short retries cover the common "server re-creating the
        // next instance" window without blocking CLI tools for long.
        for attempt in 0..10u32 {
            match ClientOptions::new().open(pipe) {
                Ok(client) => return Some(client),
                Err(e) => {
                    let code = e.raw_os_error();
                    if code == Some(ERROR_PIPE_BUSY) {
                        tokio::time::sleep(Duration::from_millis(20 + 10 * u64::from(attempt)))
                            .await;
                        continue;
                    }
                    if code == Some(ERROR_FILE_NOT_FOUND) {
                        return None;
                    }
                    // Other errors (access denied, invalid name, etc.) —
                    // treat as "no usable daemon" so callers keep the
                    // Option-based connect contract.
                    return None;
                }
            }
        }
        None
    }
}

// ── Non-Unix / non-Windows stub ──────────────────────────────────

#[cfg(not(any(unix, windows)))]
mod imp {
    use std::path::Path;

    use crate::protocol::{Request, Response};

    /// Stub daemon client for platforms without a local transport
    /// (e.g. pure WASM). Prefer the Unix UDS or Windows named-pipe
    /// implementations on native targets.
    pub struct DaemonClient {
        pub(crate) ctx: Option<crate::connect::ClientContext>,
    }

    /// Stub stream session (never constructible on this platform).
    pub struct StreamSession;

    impl StreamSession {
        /// Always errors — no local daemon transport on this platform.
        pub async fn next_line(&mut self) -> anyhow::Result<Option<String>> {
            anyhow::bail!(
                "daemon not available on this platform \
                 (local transport requires Unix UDS or Windows named pipes; WEFT-11)"
            )
        }
    }

    impl DaemonClient {
        /// Always returns `None` — no local daemon transport.
        pub async fn connect() -> Option<Self> {
            None
        }

        /// Always returns `None` — no local daemon transport.
        pub async fn connect_path(_path: impl AsRef<Path>) -> Option<Self> {
            None
        }

        pub async fn call(&mut self, _request: Request) -> anyhow::Result<Response> {
            anyhow::bail!(
                "daemon not available on this platform \
                 (local transport requires Unix UDS or Windows named pipes; WEFT-11)"
            )
        }

        pub async fn simple_call(&mut self, _method: &str) -> anyhow::Result<Response> {
            anyhow::bail!(
                "daemon not available on this platform \
                 (local transport requires Unix UDS or Windows named pipes; WEFT-11)"
            )
        }

        pub async fn open_stream(
            self,
            _request: Request,
        ) -> anyhow::Result<(Response, StreamSession)> {
            anyhow::bail!(
                "daemon not available on this platform \
                 (local transport requires Unix UDS or Windows named pipes; WEFT-11)"
            )
        }
    }
}

pub use imp::{DaemonClient, StreamSession};

/// Check if a daemon is running at the default socket path
/// (socket/pipe exists and accepts connections).
pub async fn is_daemon_running() -> bool {
    DaemonClient::connect().await.is_some()
}

/// Check if a daemon is accepting connections at `path`.
///
/// Prefer this over [`is_daemon_running`] in tests so a live developer
/// daemon on the shared default path cannot flip the result (WEFT-645).
/// On Windows `path` is the logical socket path (mapped to a named pipe).
pub async fn is_daemon_running_at(path: impl AsRef<std::path::Path>) -> bool {
    DaemonClient::connect_path(path).await.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Isolated socket path under a unique temp dir. Nothing listens
    /// there, and a live weaver on the default path cannot affect it.
    fn isolated_socket() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("temp runtime dir");
        let sock = dir.path().join(crate::SOCKET_NAME);
        (dir, sock)
    }

    #[tokio::test]
    async fn connect_returns_none_when_no_daemon() {
        let (_dir, sock) = isolated_socket();
        let client = DaemonClient::connect_path(&sock).await;
        assert!(
            client.is_none(),
            "connect_path should return None for an isolated empty runtime dir"
        );
    }

    #[tokio::test]
    async fn is_daemon_running_false_when_no_daemon() {
        let (_dir, sock) = isolated_socket();
        assert!(
            !is_daemon_running_at(&sock).await,
            "is_daemon_running_at should be false for an isolated empty runtime dir"
        );
    }

    /// Windows: named-pipe client/server line-delimited JSON roundtrip.
    ///
    /// Compiles and runs only on Windows hosts (or Windows CI). Proves
    /// `DaemonClient::connect_path` + `call` against the server helpers
    /// in [`crate::named_pipe`].
    #[cfg(windows)]
    #[tokio::test]
    async fn windows_named_pipe_roundtrip() {
        use crate::named_pipe::{create_listener, create_listener_next};
        use crate::protocol::{Request, Response};
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        let dir = tempfile::tempdir().expect("temp runtime dir");
        let sock = dir.path().join(crate::SOCKET_NAME);

        let server = create_listener(&sock).expect("create first pipe instance");
        let sock_for_client = sock.clone();

        let client_task = tokio::spawn(async move {
            // Retry briefly — server may still be wiring connect().
            for _ in 0..50 {
                if let Some(mut client) = DaemonClient::connect_path(&sock_for_client).await {
                    let resp = client
                        .call(Request::new("kernel.ping"))
                        .await
                        .expect("call");
                    return resp;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            panic!("client failed to connect to named pipe");
        });

        server.connect().await.expect("server connect");
        // Keep a next instance ready (daemon accept-loop pattern).
        let _next = create_listener_next(&sock).expect("create next instance");

        let mut reader = BufReader::new(&mut server);
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .await
            .expect("read request line");
        let req: Request = serde_json::from_str(line.trim()).expect("parse request");
        assert_eq!(req.method, "kernel.ping");
        assert_eq!(req.auth.as_deref(), Some("admin"));

        let mut resp = Response::success(serde_json::json!({"pong": true}));
        resp.id = req.id;
        let mut out = serde_json::to_string(&resp).expect("serialize");
        out.push('\n');
        reader
            .get_mut()
            .write_all(out.as_bytes())
            .await
            .expect("write response");

        let resp = client_task.await.expect("client task");
        assert!(resp.ok);
        assert_eq!(resp.result.unwrap()["pong"], true);
    }
}
