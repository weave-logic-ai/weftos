//! What the supervisor asks a running child over its socket: the handshake
//! (is this really the project's kernel, which pid) and a graceful
//! `kernel.shutdown`. A trait so tests can stand in for a child that cannot
//! speak JSON-RPC (a shell script).

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use clawft_rpc::handshake::Handshake;
use clawft_rpc::{DaemonClient, Request};
use ed25519_dalek::SigningKey;

use crate::project_forward::stamp_forward;

/// Bound on every call to a child: a wedged child must never wedge the
/// supervisor.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(5);

/// A child's identity as it reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildHandshake {
    /// The project the kernel says it serves.
    pub project_id: Option<String>,
    /// The kernel's own pid.
    pub pid: u32,
    /// The build stamp the kernel reports (`BUILD_GIT_HASH`); empty from a
    /// kernel too old to say.
    pub sha: String,
    /// The crate version the kernel reports.
    pub version: String,
}

/// Calls to a child kernel.
#[async_trait]
pub trait ChildIo: Send + Sync {
    /// Full challenge-bound Wasmtime identity; unsupported transports fail closed.
    async fn wasm_handshake(&self, _socket: &Path, _challenge: &str) -> Option<serde_json::Value> {
        None
    }

    /// `kernel.handshake` at `socket`; `None` when nothing answers.
    async fn handshake(&self, socket: &Path) -> Option<ChildHandshake>;
    /// Ask the kernel at `socket` (project `project_id`) to shut down
    /// gracefully. Errors are ignored: the caller escalates to a signal.
    async fn shutdown(&self, socket: &Path, project_id: &str);
}

/// The real thing, over the child's unix socket.
pub struct RpcChildIo {
    user_key: SigningKey,
    manifests_dir: PathBuf,
}

impl RpcChildIo {
    /// Calls signed with `user_key`; the child's key id is read from
    /// `<manifests_dir>/<id>.cert.json` for the forward header.
    pub fn new(user_key: SigningKey, manifests_dir: PathBuf) -> Self {
        Self {
            user_key,
            manifests_dir,
        }
    }

    fn target_key_id(&self, id: &str) -> Option<String> {
        let text = std::fs::read_to_string(self.manifests_dir.join(format!("{id}.cert.json"))).ok()?;
        let v: serde_json::Value = serde_json::from_str(&text).ok()?;
        v.get("project_key_id")?.as_str().map(str::to_owned)
    }
}

#[async_trait]
impl ChildIo for RpcChildIo {
    async fn wasm_handshake(&self, socket: &Path, challenge: &str) -> Option<serde_json::Value> {
        let call = async {
            let mut client = DaemonClient::connect_path(socket).await?;
            let reply = client
                .call(Request::with_params(
                    "kernel.handshake",
                    serde_json::json!({"challenge":challenge}),
                ))
                .await
                .ok()?;
            if !reply.ok {
                return None;
            }
            reply.result
        };
        tokio::time::timeout(CALL_TIMEOUT, call).await.ok().flatten()
    }

    async fn handshake(&self, socket: &Path) -> Option<ChildHandshake> {
        let call = async {
            let mut client = DaemonClient::connect_path(socket).await?;
            let resp = client.call(Request::new("kernel.handshake")).await.ok()?;
            let h: Handshake = serde_json::from_value(resp.result?).ok()?;
            Some(ChildHandshake {
                project_id: h.project_id,
                pid: h.pid,
                sha: h.sha,
                version: h.version,
            })
        };
        tokio::time::timeout(CALL_TIMEOUT, call).await.ok().flatten()
    }

    async fn shutdown(&self, socket: &Path, project_id: &str) {
        let mut req = Request::new("kernel.shutdown");
        // The forward header is the LAST step: it binds method, params and
        // the target child key (only known once the child is certified).
        if let Some(target) = self.target_key_id(project_id) {
            let now_ms = crate::project_supervisor::state::now_unix() * 1000;
            let _ = stamp_forward(&mut req, &self.user_key, project_id, &target, now_ms);
        }
        let call = async {
            let mut client = DaemonClient::connect_path(socket).await?;
            client.call(req).await.ok()
        };
        let _ = tokio::time::timeout(CALL_TIMEOUT, call).await;
    }
}
