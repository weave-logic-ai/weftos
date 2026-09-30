//! Resolved connect + handshake, and the per-process request context.
//!
//! [`DaemonClient::connect_resolved`] dials the endpoint a
//! [`Resolution`] names, runs `kernel.handshake`, and refuses a daemon that
//! is not the one the resolver expected (the stale-socket-to-the-wrong-
//! daemon case). Every failure renders with the exact next command.

use std::fmt;
use std::sync::OnceLock;

use crate::client::DaemonClient;
use crate::handshake::{
    Failure, Handshake, PROTO_MISMATCH_KIND, PROTO_VERSION, remedy, remedy_for,
};
use crate::probe::{SocketState, probe_socket};
use crate::protocol::{Request, Response};
use crate::resolve::Resolution;

/// Fields stamped onto every request that does not set them itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientContext {
    /// Project ULID to scope requests to.
    pub project: Option<String>,
    /// Protocol version to announce.
    pub proto: u32,
}

impl Default for ClientContext {
    fn default() -> Self {
        Self {
            project: None,
            proto: PROTO_VERSION,
        }
    }
}

static CONTEXT: OnceLock<ClientContext> = OnceLock::new();

/// Install the process-wide request context. Returns `false` when one was
/// already installed (the first wins; it is process-global by design so
/// call sites do not change).
pub fn set_context(ctx: ClientContext) -> bool {
    CONTEXT.set(ctx).is_ok()
}

/// Apply the implicit fields to an outgoing request.
///
/// WEFT-479: an absent `auth` becomes `admin` (unix-socket trust,
/// ADR-070). `proto` and `project` come from [`set_context`], with
/// `proto` defaulting to [`PROTO_VERSION`].
pub(crate) fn stamp_request(request: &mut Request) {
    if request.auth.is_none() {
        request.auth = Some("admin".to_string());
    }
    let default = ClientContext::default();
    let ctx = CONTEXT.get().unwrap_or(&default);
    if request.proto.is_none() {
        request.proto = Some(ctx.proto);
    }
    if request.project.is_none() {
        request.project.clone_from(&ctx.project);
    }
}

/// Why [`DaemonClient::connect_resolved`] failed.
#[derive(Debug)]
pub enum ConnectError {
    /// Nothing usable at the endpoint.
    Unreachable {
        resolution: Box<Resolution>,
        state: SocketState,
    },
    /// The daemon refused this client's protocol version.
    ProtoMismatch { message: String, daemon_sha: String },
    /// The daemon does not answer `kernel.handshake` (older build) or the
    /// reply was unusable.
    NoHandshake { detail: String },
    /// The daemon serves a different project than the resolver expected.
    ProjectMismatch { expected: String, actual: String },
    /// The daemon's node id is not the expected one.
    NodeMismatch { expected: String, actual: String },
    /// Transport failure after connecting.
    Transport(anyhow::Error),
}

impl ConnectError {
    /// The failure class, for [`remedy_for`].
    pub fn failure(&self) -> Option<Failure> {
        Some(match self {
            Self::Unreachable { state, .. } => match state {
                SocketState::Stale => Failure::StaleSocket,
                SocketState::PermissionDenied => Failure::PermissionDenied,
                _ => Failure::NoSocket,
            },
            Self::ProtoMismatch { daemon_sha, .. } => Failure::ProtoMismatch {
                daemon_sha: daemon_sha.clone(),
                client_too_old: false,
            },
            Self::ProjectMismatch { expected, actual } => Failure::ProjectMismatch {
                expected: expected.clone(),
                actual: actual.clone(),
            },
            Self::NodeMismatch { expected, actual } => Failure::NodeMismatch {
                expected: expected.clone(),
                actual: actual.clone(),
            },
            Self::NoHandshake { .. } | Self::Transport(_) => return None,
        })
    }
}

impl fmt::Display for ConnectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreachable { resolution, state } => {
                write!(f, "{}", resolution.unreachable_message(state))
            }
            Self::ProtoMismatch { message, .. } => write!(f, "{message}"),
            Self::NoHandshake { detail } => write!(
                f,
                "daemon did not complete the handshake ({detail}); it is probably an older \
                 build: {}",
                remedy("unknown", false)
            ),
            Self::ProjectMismatch { expected, actual } => write!(
                f,
                "wrong daemon: expected project {expected} but the daemon serves {actual}\n  next: {}",
                remedy_for(&Failure::ProjectMismatch {
                    expected: expected.clone(),
                    actual: actual.clone()
                })
            ),
            Self::NodeMismatch { expected, actual } => write!(
                f,
                "wrong daemon: expected node {expected} but the daemon is node {actual}\n  next: {}",
                remedy_for(&Failure::NodeMismatch {
                    expected: expected.clone(),
                    actual: actual.clone()
                })
            ),
            Self::Transport(e) => write!(f, "daemon connection failed: {e}"),
        }
    }
}

impl std::error::Error for ConnectError {}

/// Check a handshake against what the resolver expected.
///
/// A daemon that reports no project (the user-level daemon in Phase 1)
/// never conflicts; only a reported id that differs does.
pub fn verify_handshake(res: &Resolution, h: &Handshake) -> Result<(), ConnectError> {
    if let (Some(want), Some(got)) = (&res.project_id, &h.project_id)
        && want != got
    {
        return Err(ConnectError::ProjectMismatch {
            expected: want.clone(),
            actual: got.clone(),
        });
    }
    if let Some(want) = &res.expected_node
        && want != &h.node_id
    {
        return Err(ConnectError::NodeMismatch {
            expected: want.clone(),
            actual: h.node_id.clone(),
        });
    }
    Ok(())
}

fn parse_handshake(resp: Response) -> Result<Handshake, ConnectError> {
    if !resp.ok {
        let message = resp.error.clone().unwrap_or_else(|| "unknown error".into());
        if resp.error_kind.as_deref() == Some(PROTO_MISMATCH_KIND) {
            let daemon_sha = resp
                .data
                .as_ref()
                .and_then(|d| d["daemon"]["sha"].as_str())
                .unwrap_or("unknown")
                .to_owned();
            return Err(ConnectError::ProtoMismatch {
                message,
                daemon_sha,
            });
        }
        return Err(ConnectError::NoHandshake { detail: message });
    }
    serde_json::from_value(resp.result.unwrap_or_default()).map_err(|e| {
        ConnectError::NoHandshake {
            detail: format!("malformed handshake: {e}"),
        }
    })
}

impl DaemonClient {
    /// Connect to the endpoint in `res`, handshake, and verify the daemon
    /// is the expected one. Also installs the request context (project id
    /// and protocol) when none is set yet.
    pub async fn connect_resolved(res: &Resolution) -> Result<(Self, Handshake), ConnectError> {
        let Some(mut client) = Self::connect_path(&res.socket).await else {
            let state = probe_socket(&res.socket).await;
            return Err(ConnectError::Unreachable {
                resolution: Box::new(res.clone()),
                state,
            });
        };
        let _ = set_context(ClientContext {
            project: res.project_id.clone(),
            proto: PROTO_VERSION,
        });
        let resp = client
            .call(Request::new("kernel.handshake"))
            .await
            .map_err(ConnectError::Transport)?;
        let handshake = parse_handshake(resp)?;
        verify_handshake(res, &handshake)?;
        Ok((client, handshake))
    }
}

#[cfg(all(test, unix))]
#[path = "connect_tests.rs"]
mod tests;
