//! Transparent daemon-absent handling for CLI commands (ADR-103 D14).
//!
//! The CLI never falls back to local work without saying which socket it
//! tried and why it was unusable. Read-only commands keep working from local
//! files and print [`local_note`]; state-changing commands refuse with
//! [`refuse_state_change`] instead of quietly changing something the daemon
//! will never see.

use std::path::PathBuf;

use clawft_rpc::probe::{SocketState, probe_default, probe_socket};

/// Probe the endpoint the CLI would actually dial (honours `--runtime` and
/// `--project`), falling back to the default endpoint when the flags
/// cannot be resolved.
async fn probe_resolved() -> (PathBuf, SocketState) {
    match super::daemon_conn::resolve_current() {
        Ok(res) => {
            let state = probe_socket(&res.socket).await;
            (res.socket, state)
        }
        Err(_) => probe_default().await,
    }
}

/// One-line stderr warning for a read-only local fallback.
pub async fn local_note(detail: &str) -> String {
    let (sock, state) = probe_resolved().await;
    format!(
        "Warning: no kernel reachable at {} ({}); {detail}",
        sock.display(),
        state.short_reason()
    )
}

/// Error for a state-changing command that needs a running kernel.
pub async fn refuse_state_change(what: &str) -> anyhow::Error {
    let (sock, state) = probe_resolved().await;
    anyhow::anyhow!(
        "{what} needs a running kernel, and none is reachable at {} ({}).\n  \
         Start one with `weaver kernel start`, or point at another with WEFTOS_RUNTIME_DIR.",
        sock.display(),
        state.short_reason()
    )
}
