//! The one way the CLI dials the kernel daemon (ADR-103 D14).
//!
//! Every daemon call site goes through [`connect`] (strict: the error names
//! the endpoint and the exact next command) or [`connect_opt`] (for commands
//! that fall back to local files when no daemon answers). Both resolve the
//! endpoint with `clawft_rpc::resolve` using the global `--runtime` /
//! `--project` flags, verify the daemon's handshake, and print any
//! warnings once per process.

use std::sync::{Mutex, OnceLock};
use std::sync::atomic::{AtomicBool, Ordering};

use clawft_rpc::handshake::remedy_for;
use clawft_rpc::resolve::{Resolution, ResolveError, ResolveFlags, resolve};
use clawft_rpc::{ConnectError, Connected, DaemonClient};

static FLAGS: OnceLock<ResolveFlags> = OnceLock::new();
static WARNED: AtomicBool = AtomicBool::new(false);

/// Record the global `--runtime` / `--project` flags (first call wins).
pub fn set_flags(flags: ResolveFlags) {
    let _ = FLAGS.set(flags);
}

/// The flags recorded by [`set_flags`], or empty ones.
pub fn flags() -> ResolveFlags {
    FLAGS.get().cloned().unwrap_or_default()
}

/// Resolve the endpoint from flags, environment and the working directory.
pub fn resolve_current() -> Result<Resolution, ResolveError> {
    resolve(&flags())
}

/// The exact next command for a connect failure, when one is known.
pub fn remedy_text(err: &ConnectError) -> Option<String> {
    err.failure().map(|f| remedy_for(&f))
}

/// Operator-facing text: what failed plus the exact next command.
///
/// `ConnectError`'s `Display` already ends in the remedy for every class
/// that has one; classes without one (transport, refused handshake) get the
/// endpoint and a pointer at `--runtime`.
pub fn describe(err: &ConnectError) -> String {
    match remedy_text(err) {
        Some(_) => err.to_string(),
        None => match err {
            ConnectError::Transport(_) | ConnectError::HandshakeRefused { .. } => format!(
                "{err}\n  next: check the daemon with `weaver kernel status`, or point at \
                 another with --runtime / WEFTOS_RUNTIME_DIR"
            ),
            _ => err.to_string(),
        },
    }
}

fn print_warnings(warnings: &[String]) {
    if warnings.is_empty() || WARNED.swap(true, Ordering::Relaxed) {
        return;
    }
    for w in warnings {
        eprintln!("warning: {w}");
    }
}

/// What dialing the resolved endpoint found.
pub enum Outcome {
    /// The right daemon answered.
    Reachable(Box<Connected>),
    /// Nothing answers at the endpoint; a local fallback is legitimate.
    Unreachable,
    /// Something answered (or the request was invalid) but it is not the
    /// target: never fall back to local work. Carries the operator text,
    /// ending in the remedy.
    Wrong(String),
}

/// Dial `res` and classify the result.
pub async fn outcome_for(res: &Resolution) -> Outcome {
    match DaemonClient::connect_resolved(res).await {
        Ok(c) => {
            print_warnings(&c.warnings);
            Outcome::Reachable(Box::new(c))
        }
        Err(ConnectError::Unreachable { .. }) => Outcome::Unreachable,
        Err(e) => Outcome::Wrong(describe(&e)),
    }
}

/// Resolve from flags and environment, then [`outcome_for`].
pub async fn outcome() -> Outcome {
    match resolve_current() {
        Ok(res) => outcome_for(&res).await,
        Err(e) => Outcome::Wrong(e.to_string()),
    }
}

/// Resolve, connect and verify. Errors carry the remedy.
pub async fn connect() -> anyhow::Result<Connected> {
    let res = resolve_current().map_err(|e| anyhow::anyhow!("{e}"))?;
    match DaemonClient::connect_resolved(&res).await {
        Ok(c) => {
            print_warnings(&c.warnings);
            Ok(c)
        }
        Err(e) => Err(anyhow::anyhow!("{}", describe(&e))),
    }
}

/// [`connect`] for one-shot commands that have a local fallback.
///
/// `None` means nothing is listening, so the caller may fall back. A daemon
/// that is the wrong one (project, node or protocol mismatch, refused
/// handshake) or an invalid `--project` is a hard error: the message and its
/// remedy go to stderr and the process exits 1, so no caller can run local
/// work against the wrong target.
pub async fn connect_opt() -> Option<DaemonClient> {
    match outcome().await {
        Outcome::Reachable(c) => Some(c.client),
        Outcome::Unreachable => None,
        Outcome::Wrong(text) => {
            eprintln!("error: {text}");
            std::process::exit(1);
        }
    }
}

static LAST_WRONG: Mutex<Option<String>> = Mutex::new(None);

/// [`connect_opt`] for reconnect loops: a wrong daemon is reported once per
/// distinct error and yields `None` (the loop's own give-up path).
pub async fn connect_retry() -> Option<DaemonClient> {
    match outcome().await {
        Outcome::Reachable(c) => Some(c.client),
        Outcome::Unreachable => None,
        Outcome::Wrong(text) => {
            let mut last = LAST_WRONG.lock().unwrap_or_else(|p| p.into_inner());
            if last.as_deref() != Some(text.as_str()) {
                eprintln!("error: {text}");
                *last = Some(text);
            }
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clawft_rpc::probe::SocketState;
    use clawft_rpc::resolve::{ResolveInputs, resolve_with};

    fn res() -> Resolution {
        let dir = tempfile::tempdir().unwrap();
        resolve_with(&ResolveInputs {
            home: Some(dir.path().to_path_buf()),
            cwd: Some(dir.path().to_path_buf()),
            ..Default::default()
        })
        .unwrap()
    }

    #[test]
    fn unreachable_names_endpoint_and_start_command() {
        let e = ConnectError::Unreachable {
            resolution: Box::new(res()),
            state: SocketState::NoSocketFile,
        };
        let t = describe(&e);
        assert!(t.contains("weaver kernel start"), "{t}");
        assert!(t.contains("--runtime"), "{t}");
        assert_eq!(
            remedy_text(&e).as_deref(),
            Some("start a kernel with `weaver kernel start`")
        );
    }

    #[test]
    fn stale_and_permission_remedies_differ() {
        let stale = ConnectError::Unreachable {
            resolution: Box::new(res()),
            state: SocketState::Stale,
        };
        assert!(remedy_text(&stale).unwrap().contains("stale socket"));
        let denied = ConnectError::Unreachable {
            resolution: Box::new(res()),
            state: SocketState::PermissionDenied,
        };
        assert!(remedy_text(&denied).unwrap().contains("WEFTOS_RUNTIME_DIR"));
    }

    #[test]
    fn project_errors_name_both_ids_and_the_fix() {
        let m = ConnectError::ProjectMismatch {
            expected: "AAA".into(),
            actual: "BBB".into(),
        };
        let t = describe(&m);
        assert!(t.contains("AAA") && t.contains("BBB"), "{t}");
        assert!(t.contains("--project AAA"), "{t}");
        let u = ConnectError::ProjectUnbound {
            expected: "AAA".into(),
        };
        assert!(describe(&u).contains("serves none"));
        assert!(describe(&u).contains("weaver kernel start"));
    }

    #[test]
    fn node_mismatch_and_proto_mismatch_have_remedies() {
        let n = ConnectError::NodeMismatch {
            expected: "n1".into(),
            actual: "n2".into(),
        };
        assert!(remedy_text(&n).unwrap().contains("n2"));
        let p = ConnectError::ProtoMismatch {
            message: "protocol mismatch".into(),
            daemon_sha: "abc12345".into(),
            client_too_old: true,
        };
        assert!(remedy_text(&p).is_some());
    }

    #[test]
    fn no_handshake_points_at_restart_and_transport_gets_a_next_step() {
        let n = ConnectError::NoHandshake {
            detail: "unknown method".into(),
        };
        assert!(describe(&n).contains("unknown method"));
        let t = ConnectError::Transport(anyhow::anyhow!("broken pipe"));
        let text = describe(&t);
        assert!(
            text.contains("broken pipe") && text.contains("next:"),
            "{text}"
        );
        let r = ConnectError::HandshakeRefused {
            detail: "denied".into(),
        };
        assert!(describe(&r).contains("next:"));
    }

    // ---- fake daemons: Outcome classification ----

    use clawft_rpc::handshake::{
        BoundVia, DaemonBuild, ProtoRange, handshake_value, proto_mismatch_response,
    };
    use clawft_rpc::{Handshake, Request, Response};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixListener;

    const ID_A: &str = "01J0000000000000000000000A";
    const ID_B: &str = "01J0000000000000000000000B";

    /// Short path: unix socket paths are limited to ~104 bytes.
    fn short_dir() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("wc")
            .tempdir_in(std::env::temp_dir())
            .unwrap()
    }

    fn hs(project: Option<&str>) -> Handshake {
        Handshake {
            proto: ProtoRange::supported(),
            node_id: "n".into(),
            user_id: None,
            project_id: project.map(String::from),
            bound_via: if project.is_some() { BoundVia::Project } else { BoundVia::None },
            depth: 0,
            parent: None,
            runtime_dir: "/x".into(),
            pid: 1,
            version: "0.8.1".into(),
            sha: "abcd1234".into(),
            binary: None,
            mesh: None,
            user_key_id: None,
            profile: None,
            roles: Vec::new(),
        }
    }

    fn serve(dir: &std::path::Path, reply: impl Fn(&Request) -> Response + Send + 'static) {
        let l = UnixListener::bind(dir.join("kernel.sock")).unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((s, _)) = l.accept().await else { return };
                let (r, mut w) = s.into_split();
                let mut lines = BufReader::new(r).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let req: Request = serde_json::from_str(&line).unwrap();
                    let mut out = serde_json::to_string(&reply(&req)).unwrap();
                    out.push('\n');
                    if w.write_all(out.as_bytes()).await.is_err() {
                        return;
                    }
                }
            }
        });
    }

    fn resolution_for(dir: &std::path::Path, project: Option<&str>) -> Resolution {
        resolve_with(&ResolveInputs {
            flags: clawft_rpc::resolve::ResolveFlags {
                runtime: Some(dir.to_path_buf()),
                project: project.map(String::from),
            },
            ..Default::default()
        })
        .unwrap()
    }

    fn wrong_text(o: Outcome) -> String {
        match o {
            Outcome::Wrong(t) => t,
            Outcome::Reachable(_) => panic!("expected Wrong, got Reachable"),
            Outcome::Unreachable => panic!("expected Wrong, got Unreachable"),
        }
    }

    #[tokio::test]
    async fn nothing_listening_is_unreachable() {
        let d = short_dir();
        let o = outcome_for(&resolution_for(d.path(), Some(ID_A))).await;
        assert!(matches!(o, Outcome::Unreachable));
    }

    #[tokio::test]
    async fn right_daemon_is_reachable() {
        let d = short_dir();
        serve(d.path(), |_| Response::success(handshake_value(&hs(Some(ID_A)))));
        let o = outcome_for(&resolution_for(d.path(), Some(ID_A))).await;
        assert!(matches!(o, Outcome::Reachable(_)));
    }

    #[tokio::test]
    async fn wrong_project_daemon_is_wrong_not_unreachable() {
        let d = short_dir();
        serve(d.path(), |_| Response::success(handshake_value(&hs(Some(ID_B)))));
        let t = wrong_text(outcome_for(&resolution_for(d.path(), Some(ID_A))).await);
        assert!(t.contains(ID_A) && t.contains(ID_B), "{t}");
        assert!(t.contains(&format!("--project {ID_A}")), "{t}");
        assert!(!t.contains("no kernel reachable"), "{t}");
    }

    #[tokio::test]
    async fn unbound_daemon_on_explicit_endpoint_is_wrong() {
        let d = short_dir();
        serve(d.path(), |_| Response::success(handshake_value(&hs(None))));
        let t = wrong_text(outcome_for(&resolution_for(d.path(), Some(ID_A))).await);
        assert!(t.contains("serves none"), "{t}");
    }

    #[tokio::test]
    async fn proto_mismatch_no_handshake_and_refused_are_wrong() {
        let d = short_dir();
        serve(d.path(), |_| {
            proto_mismatch_response(9, DaemonBuild { sha: "abcd1234", version: "0.8.1" })
        });
        let t = wrong_text(outcome_for(&resolution_for(d.path(), None)).await);
        assert!(t.contains("protocol mismatch"), "{t}");

        let d = short_dir();
        serve(d.path(), |r| Response::error(format!("unknown method: {}", r.method)));
        let t = wrong_text(outcome_for(&resolution_for(d.path(), None)).await);
        assert!(t.contains("kernel.handshake"), "{t}");

        let d = short_dir();
        serve(d.path(), |_| Response::error_with_kind("denied", "no"));
        let t = wrong_text(outcome_for(&resolution_for(d.path(), None)).await);
        assert!(t.contains("refused the handshake"), "{t}");
    }
}
