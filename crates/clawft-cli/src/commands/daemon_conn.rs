//! The one way the CLI dials the kernel daemon (ADR-103 D14).
//!
//! Every daemon call site goes through [`connect`] (strict: the error names
//! the endpoint and the exact next command) or [`connect_opt`] (for commands
//! that fall back to local files when no daemon answers). Both resolve the
//! endpoint with `clawft_rpc::resolve` using the global `--runtime` /
//! `--project` flags, verify the daemon's handshake, and print any
//! warnings once per process.

use std::sync::OnceLock;
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

/// [`connect`] for commands that have a local fallback.
///
/// An unreachable endpoint is `None` so the caller falls back (and prints
/// its own note). A reachable but wrong daemon (project, node, protocol
/// mismatch) is reported on stderr, also as `None`: the caller must not
/// believe it reached the right kernel.
pub async fn connect_opt() -> Option<DaemonClient> {
    let res = match resolve_current() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return None;
        }
    };
    match DaemonClient::connect_resolved(&res).await {
        Ok(c) => {
            print_warnings(&c.warnings);
            Some(c.client)
        }
        Err(ConnectError::Unreachable { .. }) => None,
        Err(e) => {
            eprintln!("error: {}", describe(&e));
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
}
