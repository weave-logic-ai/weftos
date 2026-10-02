//! The real [`ParentTransport`]: a project kernel's `project.anchor.submit`
//! over the user daemon's unix socket (ADR-103 A7, Phase 2 packages D, G, H).
//!
//! Blocking `std` I/O with a hard deadline for the whole exchange: connect,
//! write and the read of the answer each honour it, so a wedged or
//! trickling parent costs at most the deadline and then reads as
//! [`AnchorSubmitError::Unreachable`], which the anchor backend turns into
//! a pending statement and a retry. The request names its project in
//! `Request.project` and carries the explicit literal scope `read` as
//! `auth`: the route is authenticated by the statement's signature, and a
//! child must never send the implicit `admin` that `DaemonClient` would
//! attach to a request without `auth`.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use clawft_kernel::chain_anchor::{AnchorAck, AnchorSubmitError, ParentTransport};
use clawft_rpc::{PROTO_VERSION, Request, Response};
use clawft_types::project::canon::hex_decode;
use clawft_types::project::cert::ProjectAnchorStmt;
use serde_json::Value;

/// Default bound on one submission.
pub const SUBMIT_DEADLINE: Duration = Duration::from_secs(8);
/// Largest answer accepted.
const MAX_REPLY: usize = 1 << 20;

/// Submits anchor statements to the user daemon at `socket`.
#[derive(Debug, Clone)]
pub struct UnixParentTransport {
    socket: PathBuf,
    project_id: String,
    deadline: Duration,
}

impl UnixParentTransport {
    /// A transport for `project_id` to the parent at `socket`.
    pub fn new(socket: PathBuf, project_id: String) -> Self {
        Self { socket, project_id, deadline: SUBMIT_DEADLINE }
    }

    /// Replace the deadline (tests).
    pub fn with_deadline(mut self, deadline: Duration) -> Self {
        self.deadline = deadline;
        self
    }

    fn exchange(&self, line: &[u8]) -> Result<Vec<u8>, String> {
        let end = Instant::now() + self.deadline;
        let left = |end: Instant| end.checked_duration_since(Instant::now()).filter(|d| !d.is_zero());
        let mut s = UnixStream::connect(&self.socket).map_err(|e| format!("connect {}: {e}", self.socket.display()))?;
        // macOS refuses `setsockopt` with EINVAL once the peer has closed its
        // end; by then the data (or EOF) is already readable, so a failure to
        // set a timeout is not fatal. The deadline is also checked by hand
        // before every read.
        let _ = s.set_write_timeout(left(end));
        s.write_all(line).map_err(|e| format!("write: {e}"))?;
        let mut out = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            let remaining = left(end).ok_or("timed out waiting for the user daemon")?;
            let _ = s.set_read_timeout(Some(remaining));
            match s.read(&mut buf) {
                Ok(0) => return Err("the user daemon closed the connection without an answer".into()),
                Ok(n) => {
                    out.extend_from_slice(&buf[..n]);
                    if out.contains(&b'\n') {
                        return Ok(out);
                    }
                    if out.len() > MAX_REPLY {
                        return Err("answer too large".into());
                    }
                }
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
                    return Err("timed out waiting for the user daemon".into());
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(format!("read: {e}")),
            }
        }
    }
}

impl ParentTransport for UnixParentTransport {
    fn submit(&self, stmt: &ProjectAnchorStmt) -> Result<AnchorAck, AnchorSubmitError> {
        let unreachable = |m: String| AnchorSubmitError::Unreachable(m);
        let params = serde_json::to_value(stmt).map_err(|e| unreachable(e.to_string()))?;
        let mut req = Request::with_params("project.anchor.submit", params);
        req.project = Some(self.project_id.clone());
        req.proto = Some(PROTO_VERSION);
        req.auth = Some("read".to_owned());
        let mut line = serde_json::to_vec(&req).map_err(|e| unreachable(e.to_string()))?;
        line.push(b'\n');
        let raw = self.exchange(&line).map_err(unreachable)?;
        let text = String::from_utf8_lossy(raw.split(|b| *b == b'\n').next().unwrap_or_default());
        let resp: Response = serde_json::from_str(&text).map_err(|e| unreachable(format!("malformed answer: {e}")))?;
        if resp.ok {
            return resp
                .result
                .as_ref()
                .and_then(crate::anchor_rpc::parse_ack)
                .ok_or_else(|| unreachable("the acknowledgement is malformed".into()));
        }
        let data = resp.data.unwrap_or(Value::Null);
        let last = data.get("last").and_then(|l| {
            let st: ProjectAnchorStmt = serde_json::from_value(l.get("statement")?.clone()).ok()?;
            let ack = crate::anchor_rpc::parse_ack(l)?;
            Some(Box::new((st, ack)))
        });
        let key_history = data
            .get("key_history")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|h| h.as_str().and_then(hex_decode::<32>)).collect())
            .unwrap_or_default();
        Err(AnchorSubmitError::Rejected {
            kind: resp.error_kind.unwrap_or_else(|| "anchor_error".into()),
            message: resp.error.unwrap_or_default(),
            last,
            key_history,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader};
    use std::os::unix::net::UnixListener;
    use std::sync::{Arc, Mutex};

    use super::*;
    use serde_json::json;

    const ID: &str = "01JB8Z3Q0V6X9KQ4M2N7T5R1WD";

    fn stmt(seq: u64) -> ProjectAnchorStmt {
        ProjectAnchorStmt {
            project_id: ID.into(),
            project_key_id: "ab".repeat(16),
            cert_serial: 1,
            seq,
            chain_id: 0,
            head_hash: "cd".repeat(32),
            head_seq: 7,
            rule_hash: "ef".repeat(32),
            at: "2026-10-01T10:00:00Z".into(),
            prev_anchor: None,
            sig: "00".repeat(64),
        }
    }

    /// One-shot server: records the request, replies with `reply` (`None`
    /// means stay silent).
    fn server(dir: &std::path::Path, reply: Option<Value>) -> (PathBuf, Arc<Mutex<Option<Value>>>) {
        let sock = dir.join("kernel.sock");
        let l = UnixListener::bind(&sock).unwrap();
        let got = Arc::new(Mutex::new(None));
        let got2 = got.clone();
        std::thread::spawn(move || {
            let (s, _) = l.accept().unwrap();
            let mut line = String::new();
            BufReader::new(s.try_clone().unwrap()).read_line(&mut line).unwrap();
            *got2.lock().unwrap() = serde_json::from_str(&line).ok();
            match reply {
                Some(r) => {
                    let mut s = s;
                    writeln!(s, "{r}").unwrap();
                }
                None => std::thread::sleep(Duration::from_secs(5)),
            }
        });
        (sock, got)
    }

    fn tmp() -> tempfile::TempDir {
        tempfile::Builder::new().prefix("ptr").tempdir_in("/tmp").unwrap()
    }

    #[test]
    fn success_returns_the_ack_and_the_request_is_not_admin() {
        let d = tmp();
        let (sock, got) = server(d.path(), Some(json!({"ok": true, "result": {"user_seq": 9, "user_event_hash": "aa"}})));
        let ack = UnixParentTransport::new(sock, ID.into()).submit(&stmt(1)).unwrap();
        assert_eq!((ack.user_seq, ack.user_event_hash.as_str()), (9, "aa"));
        let req = got.lock().unwrap().clone().unwrap();
        assert_eq!(req["method"], "project.anchor.submit");
        assert_eq!(req["project"], ID, "the child names its project");
        assert_eq!(req["auth"], "read", "never the implicit admin");
        assert_eq!(req["params"]["seq"], 1);
    }

    #[test]
    fn a_refusal_carries_kind_and_resync_data() {
        let d = tmp();
        let last = stmt(4);
        let (sock, _) = server(
            d.path(),
            Some(json!({"ok": false, "error": "seq", "error_kind": "anchor_seq",
                "data": {"last": {"statement": last, "user_seq": 3, "user_event_hash": "bb", "rec_sig": "x"},
                         "key_history": ["11".repeat(32)]}})),
        );
        match UnixParentTransport::new(sock, ID.into()).submit(&stmt(9)) {
            Err(AnchorSubmitError::Rejected { kind, last: Some(l), key_history, .. }) => {
                assert_eq!(kind, "anchor_seq");
                assert_eq!(l.0.seq, 4);
                assert_eq!(l.1.user_seq, 3);
                assert_eq!(key_history, vec![[0x11u8; 32]]);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_silent_parent_is_unreachable_within_the_deadline() {
        let d = tmp();
        let (sock, _) = server(d.path(), None);
        let t = Instant::now();
        let r = UnixParentTransport::new(sock, ID.into())
            .with_deadline(Duration::from_millis(300))
            .submit(&stmt(1));
        assert!(matches!(r, Err(AnchorSubmitError::Unreachable(ref m)) if m.contains("timed out")), "{r:?}");
        assert!(t.elapsed() < Duration::from_secs(2), "bounded by the deadline");
    }

    #[test]
    fn no_socket_and_garbage_are_unreachable() {
        let d = tmp();
        let r = UnixParentTransport::new(d.path().join("none.sock"), ID.into()).submit(&stmt(1));
        assert!(matches!(r, Err(AnchorSubmitError::Unreachable(_))));
        let (sock, _) = server(d.path(), Some(json!("not a response")));
        let r = UnixParentTransport::new(sock, ID.into()).submit(&stmt(1));
        assert!(matches!(r, Err(AnchorSubmitError::Unreachable(ref m)) if m.contains("malformed")), "{r:?}");
    }
}
