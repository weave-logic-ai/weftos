//! Resolved connect + handshake, and the request context.
//!
//! [`DaemonClient::connect_resolved`] dials the endpoint a
//! [`Resolution`] names, runs `kernel.handshake`, and refuses a daemon that
//! is not the one the resolver expected (the stale-socket-to-the-wrong-
//! daemon case). Every failure renders with the exact next command.
//!
//! The request context (`proto`, `project`) lives on each client;
//! [`set_context`] only installs the process default the CLI relies on, so
//! library users holding several clients never share stamps.

use std::fmt;
use std::path::Path;
use std::sync::OnceLock;

use crate::client::DaemonClient;
use crate::handshake::{
    BoundVia, Failure, Handshake, PROTO_MISMATCH_KIND, PROTO_VERSION, ProtoRange, remedy,
    remedy_for,
};
use crate::probe::{SocketState, probe_socket};
use crate::protocol::{Request, Response};
use crate::resolve::{Resolution, ResolveSource};

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

/// Install the process-wide default context (first wins). Per-client
/// contexts ([`DaemonClient::with_context`], `connect_resolved`) take
/// precedence.
pub fn set_context(ctx: ClientContext) -> bool {
    CONTEXT.set(ctx).is_ok()
}

/// Apply the implicit fields to an outgoing request.
///
/// WEFT-479: an absent `auth` becomes `admin` (unix-socket trust,
/// ADR-070). `proto` and `project` come from the client's own context,
/// else the process default, else [`PROTO_VERSION`] and no project.
pub(crate) fn stamp_request(request: &mut Request, client: Option<&ClientContext>) {
    if request.auth.is_none() {
        request.auth = Some("admin".to_string());
    }
    let default = ClientContext::default();
    let ctx = client.or_else(|| CONTEXT.get()).unwrap_or(&default);
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
    /// The daemon does not accept this client's protocol version.
    ProtoMismatch {
        message: String,
        daemon_sha: String,
        client_too_old: bool,
    },
    /// The daemon does not know `kernel.handshake` (an older build) and
    /// the endpoint cannot be trusted without it.
    NoHandshake { detail: String },
    /// The daemon answered the handshake with an error (auth denial,
    /// malformed reply, ...). Never downgraded.
    HandshakeRefused { detail: String },
    /// The daemon serves a different project than the resolver expected.
    ProjectMismatch { expected: String, actual: String },
    /// A project was expected but the daemon is bound to none, and the
    /// endpoint was chosen explicitly (not the user-daemon default).
    ProjectUnbound { expected: String },
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
            Self::ProtoMismatch {
                daemon_sha,
                client_too_old,
                ..
            } => Failure::ProtoMismatch {
                daemon_sha: daemon_sha.clone(),
                client_too_old: *client_too_old,
            },
            Self::ProjectMismatch { expected, actual } => Failure::ProjectMismatch {
                expected: expected.clone(),
                actual: actual.clone(),
            },
            Self::ProjectUnbound { expected } => Failure::ProjectUnbound {
                expected: expected.clone(),
            },
            Self::NodeMismatch { expected, actual } => Failure::NodeMismatch {
                expected: expected.clone(),
                actual: actual.clone(),
            },
            Self::NoHandshake { .. } | Self::HandshakeRefused { .. } | Self::Transport(_) => {
                return None;
            }
        })
    }
}

impl fmt::Display for ConnectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let next = |f: &mut fmt::Formatter<'_>, head: String| match self.failure() {
            Some(fail) => write!(f, "{head}\n  next: {}", remedy_for(&fail)),
            None => write!(f, "{head}"),
        };
        match self {
            Self::Unreachable { resolution, state } => {
                write!(f, "{}", resolution.unreachable_message(state))
            }
            Self::ProtoMismatch { message, .. } => write!(f, "{message}"),
            Self::NoHandshake { detail } => write!(
                f,
                "daemon has no kernel.handshake ({detail}) so the endpoint cannot be verified; {}",
                remedy("unknown", false)
            ),
            Self::HandshakeRefused { detail } => {
                write!(f, "daemon refused the handshake: {detail}")
            }
            Self::ProjectMismatch { expected, actual } => next(
                f,
                format!("wrong daemon: expected project {expected} but the daemon serves {actual}"),
            ),
            Self::ProjectUnbound { expected } => next(
                f,
                format!("wrong daemon: expected project {expected} but the daemon serves none"),
            ),
            Self::NodeMismatch { expected, actual } => next(
                f,
                format!("wrong daemon: expected node {expected} but the daemon is node {actual}"),
            ),
            Self::Transport(e) => write!(f, "daemon connection failed: {e}"),
        }
    }
}

impl std::error::Error for ConnectError {}

/// A verified connection.
pub struct Connected {
    pub client: DaemonClient,
    pub handshake: Handshake,
    /// Non-fatal findings to show the operator (unbound daemon on the
    /// default endpoint, runtime-dir disagreement, old daemon).
    pub warnings: Vec<String>,
}

fn canon(p: &Path) -> std::path::PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}

/// Check a handshake against what the resolver expected; returns warnings.
///
/// Project policy:
/// - expected Some, daemon Some(other): hard error.
/// - expected Some, daemon None: hard error when the endpoint was chosen
///   explicitly (flag, env or manifest), a warning on the default level
///   (the Phase 1 user-daemon case).
pub fn verify_handshake(res: &Resolution, h: &Handshake) -> Result<Vec<String>, ConnectError> {
    let mut warnings = Vec::new();
    match (&res.project_id, &h.project_id) {
        (Some(want), Some(got)) if want != got => {
            return Err(ConnectError::ProjectMismatch {
                expected: want.clone(),
                actual: got.clone(),
            });
        }
        (Some(want), None) if res.source != ResolveSource::Default => {
            return Err(ConnectError::ProjectUnbound {
                expected: want.clone(),
            });
        }
        (Some(want), None) => warnings.push(format!(
            "daemon at {} is not bound to a project; project {want} is not verified",
            res.socket.display()
        )),
        _ => {}
    }
    if let Some(want) = &res.expected_node
        && want != &h.node_id
    {
        return Err(ConnectError::NodeMismatch {
            expected: want.clone(),
            actual: h.node_id.clone(),
        });
    }
    if canon(Path::new(&h.runtime_dir)) != canon(&res.runtime_root) {
        warnings.push(format!(
            "daemon reports runtime dir {} but {} was resolved",
            h.runtime_dir,
            res.runtime_root.display()
        ));
    }
    Ok(warnings)
}

fn parse_handshake(resp: Response) -> Result<Handshake, ConnectError> {
    if !resp.ok {
        let message = resp.error.clone().unwrap_or_else(|| "unknown error".into());
        if resp.error_kind.as_deref() == Some(PROTO_MISMATCH_KIND) {
            let d = resp.data.as_ref().map(|d| &d["daemon"]);
            let daemon_sha = d
                .and_then(|d| d["sha"].as_str())
                .unwrap_or("unknown")
                .to_owned();
            let min = d.and_then(|d| d["min"].as_u64()).unwrap_or(0);
            return Err(ConnectError::ProtoMismatch {
                message,
                daemon_sha,
                client_too_old: u64::from(PROTO_VERSION) < min,
            });
        }
        // Only the daemon's own unknown-method reply means "older build".
        if resp.error_kind.is_none() && message.starts_with("unknown method") {
            return Err(ConnectError::NoHandshake { detail: message });
        }
        return Err(ConnectError::HandshakeRefused { detail: message });
    }
    serde_json::from_value(resp.result.unwrap_or_default()).map_err(|e| {
        ConnectError::HandshakeRefused {
            detail: format!("malformed handshake: {e}"),
        }
    })
}

/// Handshake for a daemon that predates `kernel.handshake`, built from
/// `kernel.status`. Proto range `0..=0` marks it as legacy; node and
/// project are unknown.
fn degraded_handshake(res: &Resolution, status: &serde_json::Value) -> Handshake {
    let build = |k: &str| status["build"][k].as_str().unwrap_or("").to_owned();
    Handshake {
        proto: ProtoRange { current: 0, min: 0 },
        node_id: String::new(),
        user_id: None,
        project_id: None,
        bound_via: BoundVia::None,
        depth: 0,
        parent: None,
        runtime_dir: res.runtime_root.display().to_string(),
        pid: 0,
        version: build("version"),
        sha: build("sha"),
        binary: None,
    }
}

impl DaemonClient {
    /// Use `ctx` for this client's requests instead of the process default.
    pub fn with_context(mut self, ctx: ClientContext) -> Self {
        self.ctx = Some(ctx);
        self
    }

    /// Connect to the endpoint in `res`, handshake, and verify the daemon
    /// is the expected one. The client carries the resolved project id.
    ///
    /// A daemon that answers `unknown method: kernel.handshake` yields a
    /// degraded handshake (proto `0..=0`, unverified) plus a warning, but
    /// only on the default endpoint with no node pin; otherwise it is an
    /// error, since nothing could be verified. Other handshake errors
    /// never downgrade.
    pub async fn connect_resolved(res: &Resolution) -> Result<Connected, ConnectError> {
        let Some(client) = Self::connect_path(&res.socket).await else {
            let state = probe_socket(&res.socket).await;
            return Err(ConnectError::Unreachable {
                resolution: Box::new(res.clone()),
                state,
            });
        };
        let mut client = client.with_context(ClientContext {
            project: res.project_id.clone(),
            proto: PROTO_VERSION,
        });
        let resp = client
            .call(Request::new("kernel.handshake"))
            .await
            .map_err(ConnectError::Transport)?;
        let handshake = match parse_handshake(resp) {
            Ok(h) => h,
            Err(ConnectError::NoHandshake { detail }) => {
                // Nothing can be verified against an old daemon, so only
                // the user-daemon default endpoint may proceed.
                if res.expected_node.is_some() || res.source != ResolveSource::Default {
                    return Err(ConnectError::NoHandshake { detail });
                }
                let status = client
                    .call(Request::new("kernel.status"))
                    .await
                    .map_err(ConnectError::Transport)?
                    .into_result()
                    .map_err(|_| ConnectError::NoHandshake {
                        detail: detail.clone(),
                    })?;
                let handshake = degraded_handshake(res, &status);
                let warnings = vec![format!(
                    "daemon has no kernel.handshake ({detail}); it is an older build and \
                     project/node are unverified: {}",
                    remedy(&handshake.sha, false)
                )];
                return Ok(Connected {
                    client,
                    handshake,
                    warnings,
                });
            }
            Err(e) => return Err(e),
        };
        if !handshake.proto.accepts(PROTO_VERSION) {
            return Err(ConnectError::ProtoMismatch {
                message: format!(
                    "protocol mismatch: client speaks {PROTO_VERSION}, daemon accepts {}..={} \
                     ({}); {}",
                    handshake.proto.min,
                    handshake.proto.current,
                    handshake.sha,
                    remedy_for(&Failure::ProtoMismatch {
                        daemon_sha: handshake.sha.clone(),
                        client_too_old: PROTO_VERSION < handshake.proto.min,
                    })
                ),
                client_too_old: PROTO_VERSION < handshake.proto.min,
                daemon_sha: handshake.sha,
            });
        }
        let warnings = verify_handshake(res, &handshake)?;
        Ok(Connected {
            client,
            handshake,
            warnings,
        })
    }
}

#[cfg(all(test, unix))]
#[path = "connect_tests.rs"]
mod tests;
