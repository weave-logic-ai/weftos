//! Daemon-backed [`KernelFacadeBackend`] (ADR-102 D6, card api-playground-03).
//!
//! Forwards each facade route's RPC method to the kernel daemon over the
//! `clawft-rpc` line protocol (UDS on unix, named pipe on Windows).
//! Replaces the WEFT-122 [`InMemoryKernelFacade`](super::InMemoryKernelFacade)
//! stub in the gateway; the stub stays for tests.
//!
//! Least privilege: `DaemonClient::call` upgrades an absent `auth` to
//! `admin`, so every request here carries an explicit `read` scope. All
//! gateway tokens are equal today, so mutating routes are disabled (501)
//! until gateway auth can prove a stronger principal (ADR-102 card 04).
//!
//! Failure mapping (bodies are generic; detail goes to `tracing`):
//! - connect failure / connect timeout: `503` with the remedy text
//! - timeout after the request was sent: `504` (the daemon may still act)
//! - method not exposed: `501`
//! - daemon `ok: false`: `403` (`gate_deny`), `504` (`timeout`), else `500`
//!
//! Known gap: SSE `/events` streams only the gateway-local event log,
//! not daemon events.

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

/// Auth scope sent on every daemon request (never `admin`).
const FACADE_AUTH_SCOPE: &str = "read";

/// Read-only daemon methods the facade forwards.
const READ_METHODS: &[&str] = &[
    "kernel.status",
    "kernel.ps",
    "kernel.services",
    "chain.status",
    "chain.tail",
    "ecc.status",
    "ecc.search",
    "ecc.calibrate",
];

/// Mutating methods, disabled until gateway auth distinguishes principals.
const DISABLED_MUTATING: &[&str] = &["agent.spawn", "agent.stop", "custody.attest"];

/// `/chain/events` limits (`?count=`).
const CHAIN_EVENTS_DEFAULT: u64 = 100;
const CHAIN_EVENTS_MAX: u64 = 1000;

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
        tracing::warn!(socket = %self.socket_path().display(), "daemon unavailable");
        FacadeResponse {
            status: 503,
            body: serde_json::json!({
                "error": "daemon unavailable",
                "remedy": "weaver kernel start",
            }),
        }
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
        if DISABLED_MUTATING.contains(&method) {
            return not_implemented(
                "disabled until gateway auth is fixed (ADR-102 card 04)",
            );
        }
        // The route table maps `/chain/events` to `kernel.logs` (the boot
        // log); serve chain events from `chain.tail` instead.
        let (method, params) = if method == "kernel.logs" {
            let count = params
                .get("count")
                .and_then(|v| v.as_u64())
                .filter(|c| *c > 0)
                .map_or(CHAIN_EVENTS_DEFAULT, |c| c.min(CHAIN_EVENTS_MAX));
            ("chain.tail", serde_json::json!({ "count": count }))
        } else {
            (method, params)
        };
        if !READ_METHODS.contains(&method) {
            return not_implemented("route has no daemon RPC method");
        }

        let connect = DaemonClient::connect_path(self.socket_path());
        let Ok(Some(mut client)) = tokio::time::timeout(self.timeout, connect).await else {
            return self.unavailable();
        };
        let request = Request::with_params(method, params).with_auth(FACADE_AUTH_SCOPE);
        match tokio::time::timeout(self.timeout, client.call(request)).await {
            Err(_) => {
                tracing::warn!(method, "daemon call timed out after send");
                FacadeResponse {
                    status: 504,
                    body: serde_json::json!({
                        "error": "daemon timed out; the operation may still have completed",
                    }),
                }
            }
            Ok(Err(e)) => {
                tracing::warn!(method, error = %e, "daemon RPC failed");
                FacadeResponse {
                    status: 502,
                    body: serde_json::json!({ "error": "daemon RPC failed" }),
                }
            }
            Ok(Ok(resp)) if resp.ok => {
                FacadeResponse::ok(resp.result.unwrap_or(serde_json::Value::Null))
            }
            Ok(Ok(resp)) => {
                tracing::warn!(method, error = ?resp.error, kind = ?resp.error_kind, "daemon returned error");
                let (status, msg) = match resp.error_kind.as_deref() {
                    Some("gate_deny") => (403, "daemon denied the request"),
                    Some("timeout") => (504, "daemon operation timed out"),
                    _ => (500, "daemon error"),
                };
                FacadeResponse {
                    status,
                    body: serde_json::json!({ "error": msg }),
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

fn not_implemented(reason: &str) -> FacadeResponse {
    FacadeResponse {
        status: 501,
        body: serde_json::json!({ "error": reason }),
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

    /// Fake daemon that records `(method, auth)` for every request.
    fn recording_daemon(
        path: &std::path::Path,
    ) -> std::sync::Arc<std::sync::Mutex<Vec<(String, Option<String>)>>> {
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let listener = UnixListener::bind(path).unwrap();
        let log = seen.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let log = log.clone();
                tokio::spawn(async move {
                    let (r, mut w) = stream.into_split();
                    let mut line = String::new();
                    BufReader::new(r).read_line(&mut line).await.unwrap();
                    let req: Request = serde_json::from_str(line.trim()).unwrap();
                    log.lock().unwrap().push((req.method.clone(), req.auth.clone()));
                    let mut out =
                        serde_json::to_string(&clawft_rpc::Response::success(serde_json::json!({})))
                            .unwrap();
                    out.push('\n');
                    w.write_all(out.as_bytes()).await.unwrap();
                });
            }
        });
        seen
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
        fn errs(method: &str) -> clawft_rpc::Response {
            match method {
                "kernel.ps" => clawft_rpc::Response::error_with_kind("gate_deny", "secret path /x"),
                _ => clawft_rpc::Response::error("secret detail"),
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("kernel.sock");
        let _srv = fake_daemon(&sock, errs);
        let facade = DaemonKernelFacade::with_socket(&sock);

        let denied = facade.call_rpc("kernel.ps", serde_json::json!({})).await;
        assert_eq!(denied.status, 403);
        let other = facade.call_rpc("kernel.status", serde_json::json!({})).await;
        assert_eq!(other.status, 500);
        assert!(!other.body.to_string().contains("secret"));
    }

    #[tokio::test]
    async fn unreachable_daemon_is_503_without_socket_path() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("absent.sock");
        let facade = DaemonKernelFacade::with_socket(&sock);

        let resp = facade.call_rpc("kernel.ps", serde_json::json!({})).await;
        assert_eq!(resp.status, 503);
        assert_eq!(resp.body["error"], "daemon unavailable");
        assert_eq!(resp.body["remedy"], "weaver kernel start");
        assert!(!resp.body.to_string().contains("absent.sock"));
    }

    #[tokio::test]
    async fn unmapped_and_mutating_routes_are_501_without_contacting_daemon() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("kernel.sock");
        let seen = recording_daemon(&sock);
        let facade = DaemonKernelFacade::with_socket(&sock);

        for m in ["ecc.coherence", "agent.spawn", "agent.stop", "custody.attest"] {
            let resp = facade.call_rpc(m, serde_json::json!({})).await;
            assert_eq!(resp.status, 501, "{m}");
        }
        assert!(seen.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn requests_carry_read_scope_never_admin() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("kernel.sock");
        let seen = recording_daemon(&sock);
        let facade = DaemonKernelFacade::with_socket(&sock);

        for m in ["kernel.status", "kernel.ps", "chain.status", "ecc.status"] {
            assert_eq!(facade.call_rpc(m, serde_json::json!({})).await.status, 200);
        }
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 4);
        assert!(seen.iter().all(|(_, a)| a.as_deref() == Some("read")));
    }

    #[tokio::test]
    async fn chain_events_use_chain_tail_with_capped_count() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("kernel.sock");
        let seen = recording_daemon(&sock);
        let facade = DaemonKernelFacade::with_socket(&sock);

        let r = facade.call_rpc("kernel.logs", serde_json::json!({"count": 5000})).await;
        assert_eq!(r.status, 200);
        assert_eq!(seen.lock().unwrap()[0].0, "chain.tail");
        assert_eq!(super::CHAIN_EVENTS_MAX, 1000);
    }

    #[tokio::test]
    async fn hung_daemon_after_send_is_504_not_503() {
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
        assert_eq!(resp.status, 504);
        assert!(resp.body["error"].as_str().unwrap().contains("may still have completed"));
    }
}
