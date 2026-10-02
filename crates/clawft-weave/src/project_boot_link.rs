//! The child's wire to the user daemon: bounded-time JSON-lines calls and
//! the real [`ParentTransport`] for package D's anchor (ADR-103 A6, Phase 2
//! package H).
//!
//! Every call has a deadline for connect, write and read (the parent may be
//! down, wedged or replaced by a squatter), reads are size-capped, and no
//! call attaches an implicit scope: `DaemonClient::call` would add `admin`
//! to a request without `auth`, which a child must never present.
//!
//! The parent socket must be an actual unix socket owned by this uid
//! ([`verify_parent_socket`]); a socket owned by someone else is refused
//! before a byte is sent. A same-uid squatter is out of Phase 2's reach
//! (peer credentials are Phase 3), but it cannot forge a registration
//! acknowledgement (user-key signature over child-chosen randomness) or a
//! certificate, so it cannot get a child to run.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::os::unix::net::UnixStream as StdUnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use clawft_kernel::chain_anchor::{AnchorAck, AnchorSubmitError, ParentTransport};
use clawft_rpc::Response;
use clawft_types::project::canon::hex_decode;
use clawft_types::project::cert::ProjectAnchorStmt;
use serde_json::{Value, json};

use crate::anchor_rpc::{Accepted, parse_ack};

/// Largest response line accepted from the parent.
const MAX_RESPONSE_BYTES: u64 = 1 << 20;

/// Why a call to the parent did not produce a success result.
#[derive(Debug, Clone)]
pub enum LinkError {
    /// Down, wrong owner, timed out, malformed: nothing was decided.
    Unavailable(String),
    /// The parent answered and refused.
    Refused {
        /// Daemon `error_kind`.
        kind: String,
        /// Human message.
        message: String,
        /// Structured detail, if any.
        data: Option<Value>,
    },
}

impl std::fmt::Display for LinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(m) => write!(f, "user daemon unavailable: {m}"),
            Self::Refused { kind, message, .. } => {
                write!(f, "user daemon refused ({kind}): {message}")
            }
        }
    }
}

impl std::error::Error for LinkError {}

/// The parent socket must be a socket, not a symlink, owned by this uid.
pub fn verify_parent_socket(path: &Path) -> Result<(), String> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !meta.file_type().is_socket() {
        return Err(format!("{} is not a unix socket", path.display()));
    }
    let me = nix::unistd::geteuid().as_raw();
    if meta.uid() != me {
        return Err(format!(
            "{} is owned by uid {}, not this user (uid {me}); refusing to talk to it",
            path.display(),
            meta.uid()
        ));
    }
    Ok(())
}

fn request_line(method: &str, params: Value, project: Option<&str>) -> String {
    let mut req = json!({ "id": "mesh-local", "method": method, "params": params });
    if let Some(p) = project {
        req["project"] = json!(p);
    }
    format!("{req}\n")
}

fn into_result(resp: Response) -> Result<Value, LinkError> {
    if resp.ok {
        Ok(resp.result.unwrap_or(Value::Null))
    } else {
        Err(LinkError::Refused {
            kind: resp.error_kind.unwrap_or_else(|| "parent_error".into()),
            message: resp.error.unwrap_or_default(),
            data: resp.data,
        })
    }
}

/// One call, bounded by `timeout` for the whole exchange (async).
pub async fn call_async(
    socket: &Path,
    method: &str,
    params: Value,
    project: Option<&str>,
    timeout: Duration,
) -> Result<Value, LinkError> {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader as TBufReader};
    verify_parent_socket(socket).map_err(LinkError::Unavailable)?;
    let line = request_line(method, params, project);
    let exchange = async {
        let stream = tokio::net::UnixStream::connect(socket).await?;
        let (r, mut w) = stream.into_split();
        w.write_all(line.as_bytes()).await?;
        let mut out = String::new();
        TBufReader::new(r.take(MAX_RESPONSE_BYTES))
            .read_line(&mut out)
            .await?;
        Ok::<_, std::io::Error>(out)
    };
    let out = tokio::time::timeout(timeout, exchange)
        .await
        .map_err(|_| LinkError::Unavailable(format!("{method} timed out after {timeout:?}")))?
        .map_err(|e| LinkError::Unavailable(format!("{method}: {e}")))?;
    parse_response(method, &out).and_then(into_result)
}

fn parse_response(method: &str, line: &str) -> Result<Response, LinkError> {
    if line.trim().is_empty() {
        return Err(LinkError::Unavailable(format!(
            "{method}: the daemon closed the connection"
        )));
    }
    serde_json::from_str(line.trim())
        .map_err(|e| LinkError::Unavailable(format!("{method}: malformed response: {e}")))
}

/// One call with read and write deadlines (blocking; for worker threads).
/// `connect` itself is not interruptible on every platform; callers run this
/// on a thread they can abandon (package D's contract).
pub fn call_blocking(
    socket: &Path,
    method: &str,
    params: Value,
    project: Option<&str>,
    timeout: Duration,
) -> Result<Response, LinkError> {
    verify_parent_socket(socket).map_err(LinkError::Unavailable)?;
    let mut stream = StdUnixStream::connect(socket)
        .map_err(|e| LinkError::Unavailable(format!("{}: {e}", socket.display())))?;
    let io = |e: std::io::Error| LinkError::Unavailable(format!("{method}: {e}"));
    stream.set_write_timeout(Some(timeout)).map_err(io)?;
    stream.set_read_timeout(Some(timeout)).map_err(io)?;
    stream
        .write_all(request_line(method, params, project).as_bytes())
        .map_err(io)?;
    let mut out = String::new();
    BufReader::new((&stream).take(MAX_RESPONSE_BYTES))
        .read_line(&mut out)
        .map_err(io)?;
    parse_response(method, &out)
}

/// `project.anchor.submit` over the parent socket (package D's transport).
///
/// Carries `Request.project` so the user daemon's audit names the project;
/// authentication is the statement's signature, as the route documents.
#[derive(Debug, Clone)]
pub struct RpcParentTransport {
    socket: PathBuf,
    project_id: String,
    timeout: Duration,
}

impl RpcParentTransport {
    /// A transport to `socket` for `project_id` with a per-call deadline.
    pub fn new(socket: PathBuf, project_id: String, timeout: Duration) -> Self {
        Self {
            socket,
            project_id,
            timeout,
        }
    }
}

impl ParentTransport for RpcParentTransport {
    fn submit(&self, stmt: &ProjectAnchorStmt) -> Result<AnchorAck, AnchorSubmitError> {
        let params = serde_json::to_value(stmt)
            .map_err(|e| AnchorSubmitError::Unreachable(format!("encode statement: {e}")))?;
        let resp = call_blocking(
            &self.socket,
            "project.anchor.submit",
            params,
            Some(&self.project_id),
            self.timeout,
        )
        .map_err(|e| AnchorSubmitError::Unreachable(e.to_string()))?;
        if resp.ok {
            return resp
                .result
                .as_ref()
                .and_then(parse_ack)
                .ok_or_else(|| AnchorSubmitError::Unreachable("malformed acknowledgement".into()));
        }
        let data = resp.data.as_ref();
        let last = data
            .and_then(|d| d.get("last"))
            .and_then(|l| serde_json::from_value::<Accepted>(l.clone()).ok())
            .map(|a| {
                let ack = AnchorAck {
                    user_seq: a.user_seq,
                    user_event_hash: a.user_event_hash.clone(),
                };
                Box::new((a.statement, ack))
            });
        let key_history = data
            .and_then(|d| d.get("key_history"))
            .and_then(Value::as_array)
            .map(|v| {
                v.iter()
                    .filter_map(|k| k.as_str().and_then(hex_decode::<32>))
                    .collect()
            })
            .unwrap_or_default();
        Err(AnchorSubmitError::Rejected {
            kind: resp.error_kind.unwrap_or_else(|| "parent_error".into()),
            message: resp.error.unwrap_or_default(),
            last,
            key_history,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    fn serve_once(path: &Path, reply: &'static str) -> std::thread::JoinHandle<String> {
        let l = UnixListener::bind(path).unwrap();
        std::thread::spawn(move || {
            let (s, _) = l.accept().unwrap();
            let mut line = String::new();
            BufReader::new(&s).read_line(&mut line).unwrap();
            (&s).write_all(reply.as_bytes()).unwrap();
            line
        })
    }

    #[test]
    fn blocking_call_round_trips_and_carries_the_project() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("s");
        let h = serve_once(
            &p,
            "{\"ok\":true,\"result\":{\"user_seq\":3,\"user_event_hash\":\"ab\"}}\n",
        );
        let r = call_blocking(
            &p,
            "project.anchor.submit",
            json!({}),
            Some("PID"),
            Duration::from_secs(2),
        )
        .unwrap();
        assert!(r.ok);
        let sent: Value = serde_json::from_str(&h.join().unwrap()).unwrap();
        assert_eq!(sent["project"], "PID");
        assert!(sent.get("auth").is_none(), "no implicit scope");
    }

    #[test]
    fn a_silent_parent_times_out() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("s");
        let l = UnixListener::bind(&p).unwrap();
        let h = std::thread::spawn(move || {
            let (_s, _) = l.accept().unwrap();
            std::thread::sleep(Duration::from_millis(600));
        });
        let start = std::time::Instant::now();
        let e = call_blocking(&p, "m", json!({}), None, Duration::from_millis(150)).unwrap_err();
        assert!(matches!(e, LinkError::Unavailable(_)), "{e}");
        assert!(start.elapsed() < Duration::from_millis(550));
        h.join().unwrap();
    }

    #[tokio::test]
    async fn async_call_times_out_on_a_silent_parent() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("s");
        let _l = tokio::net::UnixListener::bind(&p).unwrap();
        let e = call_async(&p, "m", json!({}), None, Duration::from_millis(150))
            .await
            .unwrap_err();
        assert!(matches!(e, LinkError::Unavailable(m) if m.contains("timed out")));
    }

    #[test]
    fn not_a_socket_is_refused_before_connecting() {
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("plain");
        std::fs::write(&f, "x").unwrap();
        assert!(
            verify_parent_socket(&f)
                .unwrap_err()
                .contains("not a unix socket")
        );
        let link = t.path().join("link");
        let real = t.path().join("real");
        let _l = UnixListener::bind(&real).unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(verify_parent_socket(&link).is_err(), "symlinks are refused");
        assert!(verify_parent_socket(&real).is_ok());
    }

    #[test]
    fn rejection_carries_kind_and_resync_detail() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("s");
        let _h = serve_once(
            &p,
            "{\"ok\":false,\"error\":\"nope\",\"error_kind\":\"anchor_seq\"}\n",
        );
        let tr = RpcParentTransport::new(p, "PID".into(), Duration::from_secs(2));
        let stmt = ProjectAnchorStmt {
            project_id: "PID".into(),
            project_key_id: "k".into(),
            cert_serial: 1,
            seq: 1,
            chain_id: 0,
            head_hash: "00".repeat(32),
            head_seq: 1,
            rule_hash: "00".repeat(32),
            at: "2026-01-01T00:00:00Z".into(),
            prev_anchor: None,
            sig: String::new(),
        };
        match tr.submit(&stmt) {
            Err(AnchorSubmitError::Rejected { kind, last, .. }) => {
                assert_eq!(kind, "anchor_seq");
                assert!(last.is_none());
            }
            other => panic!("{other:?}"),
        }
    }
}
