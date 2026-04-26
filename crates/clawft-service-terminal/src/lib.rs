//! WeftOS terminal service — daemon-side PTY allocation and shell
//! hosting.
//!
//! # Why this crate exists
//!
//! Every clawft surface (the VSCode webview, the egui native GUI, future
//! remote SSH) wants a terminal. If each surface allocated its own PTY,
//! sessions would be scoped to a window — close the panel and the shell
//! dies with it. By hosting PTYs in the daemon and publishing output
//! through substrate, the same shell session is observable from any
//! surface and survives surface restarts within the daemon's lifetime.
//!
//! Architecturally this mirrors `clawft-service-whisper`: an external
//! resource (a PTY rather than an HTTP service) gets a thin in-process
//! wrapper, the daemon hosts the wrapper as a tokio task, the wrapper
//! exposes a single in-process manager, and the daemon publishes RPCs
//! that delegate to it. The substrate path the wrapper publishes to is
//! what makes the data multi-surface.
//!
//! # Crate layout
//!
//! - [`session`] — [`TerminalSession`], one struct per live shell.
//!   Owns the PTY master, child handle, writer, and the
//!   `mpsc::UnboundedSender<TerminalEvent>` the reader pushes output
//!   bytes into.
//! - [`TerminalManager`] (this module) — owns a [`DashMap`] of live
//!   sessions keyed by [`SessionId`]. Public surface that
//!   `clawft-weave` glues to its substrate publish path.
//!
//! No substrate or kernel knowledge in this crate. The daemon-side
//! glue (substrate publish, RPC dispatch arms, control-plane wiring)
//! lives in `clawft-weave`'s `daemon.rs`.
//!
//! # Scope cuts
//!
//! - **No ANSI parsing.** Output bytes are forwarded raw; surfaces
//!   render them as UTF-8-lossy. ANSI rendering (colors, cursor
//!   moves) is a follow-up that lands in the surface using `vte`,
//!   not in this crate — keeping bytes raw lets multiple surfaces
//!   each pick their own renderer.
//! - **No scrollback.** Each surface accumulates its own buffer.
//!   Scrollback ring + replay-on-attach lands when a remote SSH
//!   surface forces it.
//! - **Single shell per session.** Multi-tab is a UI concern: spawn
//!   N sessions, one per tab.

#![deny(rust_2018_idioms)]
#![warn(missing_docs)]

pub mod session;

pub use session::{
    SessionId, TerminalError, TerminalEvent, TerminalSession, DEFAULT_COLS, DEFAULT_ROWS,
};

use std::sync::Arc;

use dashmap::DashMap;
use tracing::{debug, info, warn};

/// In-process registry of live PTY sessions, owned by the daemon.
///
/// Construct one at boot, store it in a `OnceLock`, hand RPC handlers a
/// clone of the `Arc`. All methods are `&self` and internally
/// concurrent — fan-out to multiple RPC connections is safe.
#[derive(Debug, Default)]
pub struct TerminalManager {
    sessions: Arc<DashMap<SessionId, Arc<TerminalSession>>>,
}

impl TerminalManager {
    /// Empty manager. Sessions appear via [`Self::spawn`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Allocate a PTY, spawn `shell` (or auto-detect: `$SHELL` →
    /// `/bin/bash` → `/bin/sh`) inside it, and start a reader task that
    /// pushes output bytes into the returned session's event channel.
    ///
    /// Returns the [`SessionId`] of the new session — call
    /// [`Self::session`] / [`Self::write`] / [`Self::resize`] /
    /// [`Self::close`] with this id.
    ///
    /// `cwd` is best-effort: if the path doesn't exist, the shell
    /// inherits the daemon's cwd. We log but don't fail.
    pub fn spawn(
        &self,
        rows: u16,
        cols: u16,
        shell: Option<String>,
        cwd: Option<String>,
    ) -> Result<SessionId, TerminalError> {
        let session = TerminalSession::spawn(rows, cols, shell, cwd)?;
        let id = session.id().clone();
        self.sessions.insert(id.clone(), Arc::new(session));
        info!(session_id = %id, rows, cols, "terminal: session spawned");
        Ok(id)
    }

    /// Look up a live session. Returns `None` once the session has been
    /// closed (or was never spawned).
    pub fn session(&self, id: &SessionId) -> Option<Arc<TerminalSession>> {
        self.sessions.get(id).map(|r| Arc::clone(r.value()))
    }

    /// Write input bytes to a session's PTY (typically what the user
    /// typed in the surface). Returns `Err` if the session id is
    /// unknown or the PTY write fails.
    pub fn write(&self, id: &SessionId, data: &[u8]) -> Result<(), TerminalError> {
        let session = self
            .session(id)
            .ok_or_else(|| TerminalError::UnknownSession(id.clone()))?;
        session.write(data)
    }

    /// Resize a session's PTY (rows × cols, in terminal cells). Surfaces
    /// call this whenever their visible terminal area changes so apps
    /// running in the shell (vim, less) re-flow correctly.
    pub fn resize(&self, id: &SessionId, rows: u16, cols: u16) -> Result<(), TerminalError> {
        let session = self
            .session(id)
            .ok_or_else(|| TerminalError::UnknownSession(id.clone()))?;
        session.resize(rows, cols)
    }

    /// Kill the child shell, drop the PTY master, and forget the
    /// session. Idempotent — closing an already-closed session returns
    /// `Ok(())` so callers don't have to check existence first.
    pub fn close(&self, id: &SessionId) -> Result<(), TerminalError> {
        let Some((_, session)) = self.sessions.remove(id) else {
            debug!(session_id = %id, "terminal: close called on unknown/already-closed session");
            return Ok(());
        };
        match session.close() {
            Ok(()) => {
                info!(session_id = %id, "terminal: session closed");
                Ok(())
            }
            Err(e) => {
                warn!(session_id = %id, error = %e, "terminal: close had errors (session forgotten)");
                Err(e)
            }
        }
    }

    /// Snapshot of currently-live session ids. Useful for
    /// diagnostics / `terminal.list` (deferred — not wired to an RPC
    /// yet, but trivially available if a surface needs it).
    pub fn list(&self) -> Vec<SessionId> {
        self.sessions.iter().map(|r| r.key().clone()).collect()
    }

    /// Number of live sessions. Cheap O(1) on `DashMap`.
    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    /// `true` when no sessions are live.
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn manager_starts_empty() {
        let mgr = TerminalManager::new();
        assert!(mgr.is_empty());
        assert_eq!(mgr.len(), 0);
        assert!(mgr.list().is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn close_unknown_session_is_ok() {
        let mgr = TerminalManager::new();
        // Idempotent: close before spawn returns Ok rather than erroring,
        // so panel teardown doesn't have to remember whether spawn
        // succeeded.
        let result = mgr.close(&SessionId::from("never-existed"));
        assert!(result.is_ok());
    }
}
