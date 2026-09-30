//! Transparent daemon-absent handling for CLI commands (ADR-103 D14).
//!
//! The CLI never falls back to local work without saying which socket it
//! tried and why it was unusable. Read-only commands keep working from local
//! files and print [`local_note`]; state-changing commands refuse with
//! [`refuse_state_change`] instead of quietly changing something the daemon
//! will never see.

use clawft_rpc::probe::probe_default;

/// One-line stderr warning for a read-only local fallback.
pub async fn local_note(detail: &str) -> String {
    let (sock, state) = probe_default().await;
    format!(
        "Warning: no kernel reachable at {} ({}); {detail}",
        sock.display(),
        state.short_reason()
    )
}

/// Error for a state-changing command that needs a running kernel.
pub async fn refuse_state_change(what: &str) -> anyhow::Error {
    let (sock, state) = probe_default().await;
    anyhow::anyhow!(
        "{what} needs a running kernel, and none is reachable at {} ({}).\n  \
         Start one with `weaver kernel start`, or point at another with WEFTOS_RUNTIME_DIR.",
        sock.display(),
        state.short_reason()
    )
}
