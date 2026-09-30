//! Daemon-backed [`KernelFacadeBackend`] (ADR-102 D6, card api-playground-03).
//!
//! Forwards each facade route's RPC method to the kernel daemon over the
//! `clawft-rpc` line protocol (UDS on unix, named pipe on Windows).
//! Replaces the WEFT-122 [`InMemoryKernelFacade`](super::InMemoryKernelFacade)
//! stub in the gateway; the stub stays for tests.
//!
//! Failure mapping:
//! - daemon unreachable / timed out connecting: `503` with the socket path
//!   and the `weaver kernel start` remedy
//! - method with no daemon implementation: `501`
//! - daemon `ok: false`: `403` (`gate_deny`), `504` (`timeout`), else `500`

use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use clawft_kernel::console::KernelEventLog;
use clawft_kernel::http_facade::{
    FacadeResponse, SseMessage, WitnessRequest, WitnessResponse, poll_events,
};
use clawft_rpc::{DaemonClient, Request};

use super::http_facade_api::KernelFacadeBackend;

/// Upper bound for connect + one request/response round trip.
const DAEMON_CALL_TIMEOUT: Duration = Duration::from_secs(5);

/// Facade routes whose RPC method the daemon does not implement.
const UNSUPPORTED_METHODS: &[&str] = &["ecc.coherence"];

/// [`KernelFacadeBackend`] that forwards RPC calls to the kernel daemon.
pub struct DaemonKernelFacade {
    /// Explicit socket path; `None` resolves `clawft_rpc::socket_path()` per call.
    socket: Option<PathBuf>,
    timeout: Duration,
    /// Gateway-local event log feeding the SSE stream.
    event_log: KernelEventLog,
}

impl DaemonKernelFacade {
    /// Facade against the default daemon socket.
    pub fn new() -> Self {
        Self {
            socket: None,
            timeout: DAEMON_CALL_TIMEOUT,
            event_log: KernelEventLog::new(),
        }
    }

    /// Facade against an explicit socket path (hermetic tests).
    pub fn with_socket(path: impl Into<PathBuf>) -> Self {
        Self {
            socket: Some(path.into()),
            ..Self::new()
        }
    }

    /// Override the call timeout (tests).
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    fn socket_path(&self) -> PathBuf {
        self.socket.clone().unwrap_or_else(clawft_rpc::socket_path)
    }

    fn unavailable(&self) -> FacadeResponse {
        FacadeResponse {
            status: 503,
            body: serde_json::json!({
                "error": "daemon unavailable",
                "socket": self.socket_path().display().to_string(),
                "remedy": "weaver kernel start",
            }),
        }
    }

    async fn round_trip(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Option<Result<clawft_rpc::Response, String>> {
        let mut client = DaemonClient::connect_path(self.socket_path()).await?;
        Some(
            client
                .call(Request::with_params(method, params))
                .await
                .map_err(|e| e.to_string()),
        )
    }
}

impl Default for DaemonKernelFacade {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl KernelFacadeBackend for DaemonKernelFacade {
    async fn call_rpc(&self, method: &str, params: serde_json::Value) -> FacadeResponse {
        if UNSUPPORTED_METHODS.contains(&method) {
            return FacadeResponse {
                status: 501,
                body: serde_json::json!({
                    "error": format!("daemon has no `{method}` RPC method"),
                }),
            };
        }
        match tokio::time::timeout(self.timeout, self.round_trip(method, params)).await {
            Err(_) | Ok(None) => self.unavailable(),
            Ok(Some(Err(e))) => {
                tracing::warn!(method, error = %e, "daemon RPC failed");
                FacadeResponse {
                    status: 502,
                    body: serde_json::json!({ "error": format!("daemon RPC failed: {e}") }),
                }
            }
            Ok(Some(Ok(resp))) if resp.ok => {
                FacadeResponse::ok(resp.result.unwrap_or(serde_json::Value::Null))
            }
            Ok(Some(Ok(resp))) => {
                let status = match resp.error_kind.as_deref() {
                    Some("gate_deny") => 403,
                    Some("timeout") => 504,
                    _ => 500,
                };
                FacadeResponse {
                    status,
                    body: serde_json::json!({
                        "error": resp.error.unwrap_or_else(|| "daemon error".into()),
                        "kind": resp.error_kind,
                    }),
                }
            }
        }
    }

    fn poll_events(&self, cursor: usize) -> (Vec<SseMessage>, usize) {
        poll_events(&self.event_log, cursor)
    }

    fn inject_witness(&self, _req: WitnessRequest) -> WitnessResponse {
        WitnessResponse::rejected("daemon has no external witness injection RPC method")
    }

    fn push_info(&self, source: &str, message: &str) {
        self.event_log.info(source, message);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixListener;

    /// Serve `n` connections; each answers one request from `handler`.
    fn fake_daemon(
        path: &std::path::Path,
        handler: fn(&str) -> clawft_rpc::Response,
    ) -> tokio::task::JoinHandle<()> {
        let listener = UnixListener::bind(path).unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else { return };
                tokio::spawn(async move {
                    let (r, mut w) = stream.into_split();
                    let mut line = String::new();
                    BufReader::new(r).read_line(&mut line).await.unwrap();
                    let req: Request = serde_json::from_str(line.trim()).unwrap();
                    let mut out = serde_json::to_string(&handler(&req.method)).unwrap();
                    out.push('\n');
                    w.write_all(out.as_bytes()).await.unwrap();
                });
            }
        })
    }

    fn reply(method: &str) -> clawft_rpc::Response {
        match method {
            "kernel.ps" => clawft_rpc::Response::success(serde_json::json!([{"pid": 7}])),
            "chain.status" => clawft_rpc::Response::success(serde_json::json!({"height": 42})),
            "agent.spawn" => {
                clawft_rpc::Response::error_with_kind("gate_deny", "spawn denied")
            }
            other => clawft_rpc::Response::error(format!("unknown method {other}")),
        }
    }

    #[tokio::test]
    async fn forwards_rpc_to_daemon() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("kernel.sock");
        let _srv = fake_daemon(&sock, reply);
        let facade = DaemonKernelFacade::with_socket(&sock);

        let ps = facade.call_rpc("kernel.ps", serde_json::json!({})).await;
        assert_eq!((ps.status, ps.body), (200, serde_json::json!([{"pid": 7}])));
        let chain = facade.call_rpc("chain.status", serde_json::json!({})).await;
        assert_eq!(chain.body["height"], 42);
    }

    #[tokio::test]
    async fn daemon_errors_map_to_status() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("kernel.sock");
        let _srv = fake_daemon(&sock, reply);
        let facade = DaemonKernelFacade::with_socket(&sock);

        let denied = facade.call_rpc("agent.spawn", serde_json::json!({})).await;
        assert_eq!(denied.status, 403);
        let other = facade.call_rpc("kernel.nope", serde_json::json!({})).await;
        assert_eq!(other.status, 500);
    }

    #[tokio::test]
    async fn unreachable_daemon_is_503_with_socket_path() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("absent.sock");
        let facade = DaemonKernelFacade::with_socket(&sock);

        let resp = facade.call_rpc("kernel.ps", serde_json::json!({})).await;
        assert_eq!(resp.status, 503);
        assert_eq!(resp.body["error"], "daemon unavailable");
        assert_eq!(resp.body["socket"], sock.display().to_string());
        assert_eq!(resp.body["remedy"], "weaver kernel start");
    }

    #[tokio::test]
    async fn method_without_daemon_impl_is_501() {
        let facade = DaemonKernelFacade::with_socket("/nonexistent/x.sock");
        let resp = facade.call_rpc("ecc.coherence", serde_json::json!({})).await;
        assert_eq!(resp.status, 501);
    }

    #[tokio::test]
    async fn hung_daemon_times_out_as_503() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("kernel.sock");
        let listener = UnixListener::bind(&sock).unwrap();
        let _hold = tokio::spawn(async move {
            let _conn = listener.accept().await;
            tokio::time::sleep(Duration::from_secs(30)).await;
        });
        let facade =
            DaemonKernelFacade::with_socket(&sock).with_timeout(Duration::from_millis(200));
        let resp = facade.call_rpc("kernel.ps", serde_json::json!({})).await;
        assert_eq!(resp.status, 503);
    }
}
