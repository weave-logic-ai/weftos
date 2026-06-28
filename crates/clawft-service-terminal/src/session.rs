//! One PTY-backed shell session.
//!
//! [`TerminalSession`] is the unit of state for a single shell. It owns:
//!
//! - The PTY master (a `Box<dyn MasterPty + Send>` from `portable-pty`)
//!   plus the writer half taken off it. The writer is parked behind a
//!   `Mutex` because PTY writes are sync `std::io::Write` and we may
//!   issue them from any tokio task in response to `terminal.write` RPCs.
//! - A handle to the child shell process so [`TerminalSession::close`]
//!   can kill it deterministically rather than waiting for SIGHUP-on-
//!   master-drop to propagate.
//! - The sender half of an `mpsc::UnboundedSender<TerminalEvent>` that
//!   the daemon-side glue subscribes to and republishes as
//!   `substrate/<daemon-node>/derived/terminal/<session>` chunks.
//!
//! ## Reader pump
//!
//! `portable-pty`'s reader is sync `std::io::Read`. We spawn a dedicated
//! `std::thread` for it (NOT a tokio task) — blocking reads on a
//! `Box<dyn Read + Send>` would otherwise stall the runtime. The
//! thread loops:
//!
//! ```text
//! loop {
//!   let n = reader.read(&mut buf)?;
//!   if n == 0 { break }
//!   tx.send(TerminalEvent::Output(buf[..n]))?
//! }
//! ```
//!
//! When the child exits (or `close()` kills it) the master drops the
//! slave-side fd, the read returns `Ok(0)`, the thread emits
//! `TerminalEvent::Exit` and returns. No leaked threads, no orphan
//! children — the receiver side learns about exit through the same
//! channel that carries output.

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use thiserror::Error;
use tokio::sync::mpsc;
use tracing::{debug, warn};

/// Default rows for a freshly-spawned PTY when the surface didn't
/// supply one. Picked to match a typical short panel — surfaces resize
/// immediately on first paint anyway.
pub const DEFAULT_ROWS: u16 = 24;

/// Default cols for a freshly-spawned PTY when the surface didn't
/// supply one. 80 is the unix wire convention; surfaces override.
pub const DEFAULT_COLS: u16 = 80;

/// Newtype around the session id. Cheap to clone (`Arc<str>` would be
/// micro-optimal but `String` is the same big-O and round-trips
/// through serde without help).
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct SessionId(pub String);

impl SessionId {
    /// Generate a fresh, unique id. Short uuid v4 — process-local, never
    /// persisted, collision-free in practice.
    pub fn new() -> Self {
        // First 12 hex chars of a uuid v4 are plenty for in-process
        // disambiguation and short enough to be readable in logs.
        let raw = uuid::Uuid::new_v4().simple().to_string();
        Self(format!("t-{}", &raw[..12]))
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl From<&str> for SessionId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl From<String> for SessionId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Events the reader pump emits onto the session's channel.
///
/// Surfaces (and the daemon's substrate publish task) consume these
/// and translate them to whatever wire shape they need. The variant
/// set is deliberately small — adding ANSI-aware events (cursor pos,
/// title set) is a follow-up that lands inside the surface, not here.
#[derive(Debug, Clone)]
pub enum TerminalEvent {
    /// Bytes the child wrote to its stdout/stderr (PTY merges them on
    /// the master side). Forwarded raw — surfaces decode UTF-8-lossy.
    Output(Vec<u8>),
    /// The child exited or the master EOF'd. After this, no more
    /// `Output` events arrive for this session.
    Exit,
}

/// Errors emitted by terminal session operations.
#[derive(Debug, Error)]
pub enum TerminalError {
    /// PTY allocation or spawn failed at the OS layer.
    #[error("pty: {0}")]
    Pty(String),
    /// Caller referenced a session id the manager doesn't have.
    #[error("unknown session: {0}")]
    UnknownSession(SessionId),
    /// Write to the PTY master failed (typically because the child
    /// already exited and the slave fd is closed).
    #[error("write: {0}")]
    Write(String),
    /// Resize ioctl on the PTY master failed.
    #[error("resize: {0}")]
    Resize(String),
    /// Killing the child process failed. Not fatal to the manager —
    /// we still drop the session; the OS will reap eventually.
    #[error("close: {0}")]
    Close(String),
}

/// One live PTY-backed shell.
///
/// Construct via [`TerminalSession::spawn`]. Hold by `Arc` — cheap to
/// clone, methods are `&self`, internal mutability is parked behind
/// fine-grained locks.
pub struct TerminalSession {
    id: SessionId,
    /// Resolved shell path that was spawned. Echoed back to surfaces
    /// in the spawn response for traceability.
    shell: String,
    /// Resolved cwd that was used. May differ from the requested cwd
    /// when the requested path didn't exist.
    cwd: String,
    /// PTY master kept alive for the session's lifetime. Behind a
    /// `Mutex` because `MasterPty::resize` takes `&mut self`.
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    /// Writer half taken off the master. Behind a `Mutex` because
    /// `Write::write_all` takes `&mut self` and we may write from
    /// concurrent RPC tasks.
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    /// Child shell process. Behind a `Mutex` because `Child::kill`
    /// takes `&mut self`.
    child: Arc<Mutex<Box<dyn portable_pty::Child + Send + Sync>>>,
    /// Output event sender. Held here so callers can resubscribe later
    /// (we hand out a fresh receiver via [`Self::take_events`] exactly
    /// once at spawn time today; future "reattach an existing
    /// session" support adds a multi-receiver fan-out here).
    event_tx: mpsc::UnboundedSender<TerminalEvent>,
    /// `take_events` consumes this. We keep it inside the session
    /// (rather than returning it from `spawn`) so the daemon glue can
    /// take it at its own pace after the session is registered.
    event_rx: Mutex<Option<mpsc::UnboundedReceiver<TerminalEvent>>>,
}

impl std::fmt::Debug for TerminalSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerminalSession")
            .field("id", &self.id)
            .field("shell", &self.shell)
            .field("cwd", &self.cwd)
            .finish_non_exhaustive()
    }
}

impl TerminalSession {
    /// Allocate a PTY, spawn `shell` (or auto-detect), and start the
    /// reader pump. Returns the live session.
    pub fn spawn(
        rows: u16,
        cols: u16,
        shell: Option<String>,
        cwd: Option<String>,
    ) -> Result<Self, TerminalError> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: if rows == 0 { DEFAULT_ROWS } else { rows },
                cols: if cols == 0 { DEFAULT_COLS } else { cols },
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| TerminalError::Pty(format!("openpty: {e}")))?;

        let resolved_shell = resolve_shell(shell);
        let resolved_cwd = resolve_cwd(cwd);

        let mut cmd = CommandBuilder::new(&resolved_shell);
        cmd.cwd(&resolved_cwd);
        // TERM=xterm-256color is the modern default and what most
        // terminals advertise. Without it many TUIs fall back to
        // monochrome and refuse arrow keys.
        cmd.env("TERM", "xterm-256color");

        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| TerminalError::Pty(format!("spawn {resolved_shell}: {e}")))?;

        // Drop the slave fd from this side now that the child holds it
        // — keeping it would prevent the master from EOFing when the
        // child exits.
        drop(pair.slave);

        let writer = pair
            .master
            .take_writer()
            .map_err(|e| TerminalError::Pty(format!("take_writer: {e}")))?;
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| TerminalError::Pty(format!("try_clone_reader: {e}")))?;

        let (event_tx, event_rx) = mpsc::unbounded_channel();

        // Reader pump on a dedicated OS thread (NOT a tokio task —
        // `Read::read` is blocking and would stall the runtime).
        let id = SessionId::new();
        spawn_reader_thread(id.clone(), reader, event_tx.clone());

        Ok(Self {
            id,
            shell: resolved_shell,
            cwd: resolved_cwd,
            master: Arc::new(Mutex::new(pair.master)),
            writer: Arc::new(Mutex::new(writer)),
            child: Arc::new(Mutex::new(child)),
            event_tx,
            event_rx: Mutex::new(Some(event_rx)),
        })
    }

    /// Session id (cheap clone).
    pub fn id(&self) -> &SessionId {
        &self.id
    }

    /// Resolved shell path. Surfaced in the `terminal.spawn` reply.
    pub fn shell(&self) -> &str {
        &self.shell
    }

    /// Resolved cwd. Surfaced in the `terminal.spawn` reply.
    pub fn cwd(&self) -> &str {
        &self.cwd
    }

    /// Take the output event receiver. Returns `None` after the first
    /// successful call — today the daemon glue drains this once per
    /// session and republishes onto substrate. Multi-consumer fan-out
    /// is a follow-up.
    pub fn take_events(&self) -> Option<mpsc::UnboundedReceiver<TerminalEvent>> {
        self.event_rx.lock().ok().and_then(|mut g| g.take())
    }

    /// Write input bytes to the PTY master.
    pub fn write(&self, data: &[u8]) -> Result<(), TerminalError> {
        let mut w = self
            .writer
            .lock()
            .map_err(|_| TerminalError::Write("writer mutex poisoned".into()))?;
        w.write_all(data)
            .map_err(|e| TerminalError::Write(e.to_string()))?;
        w.flush().map_err(|e| TerminalError::Write(e.to_string()))?;
        Ok(())
    }

    /// Resize the PTY (rows × cols, in cells).
    pub fn resize(&self, rows: u16, cols: u16) -> Result<(), TerminalError> {
        let m = self
            .master
            .lock()
            .map_err(|_| TerminalError::Resize("master mutex poisoned".into()))?;
        m.resize(PtySize {
            rows: if rows == 0 { DEFAULT_ROWS } else { rows },
            cols: if cols == 0 { DEFAULT_COLS } else { cols },
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| TerminalError::Resize(e.to_string()))?;
        Ok(())
    }

    /// Kill the child and drop the PTY. Best-effort — if `kill()` fails
    /// because the child already exited we still consider close
    /// successful.
    pub fn close(&self) -> Result<(), TerminalError> {
        let mut child = self
            .child
            .lock()
            .map_err(|_| TerminalError::Close("child mutex poisoned".into()))?;
        // `Child::kill` returns `io::Result<()>`. An ESRCH-equivalent
        // (child already gone) is success from our POV.
        if let Err(e) = child.kill() {
            debug!(session_id = %self.id, error = %e, "terminal: child kill returned error (likely already exited)");
        }
        // The reader pump will EOF as soon as the master drops the
        // slave fd's writer side. We don't wait — the surface treats
        // close as fire-and-forget.
        let _ = self.event_tx.send(TerminalEvent::Exit);
        Ok(())
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        // Defense-in-depth: if `close()` wasn't called explicitly,
        // still kill the child so we don't leak a shell when the
        // manager removes the session via `Drop` of the `Arc`.
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
        }
    }
}

/// Reader-pump thread body. Loops `read` → `Output(...)` → channel,
/// exits cleanly on EOF or a closed receiver.
///
/// 8 KiB is a large-enough chunk that long lines don't fragment into
/// dozens of tiny events but small enough to keep latency low for
/// interactive prompts.
fn spawn_reader_thread(
    id: SessionId,
    mut reader: Box<dyn Read + Send>,
    tx: mpsc::UnboundedSender<TerminalEvent>,
) {
    std::thread::Builder::new()
        .name(format!("weft-pty-reader-{id}"))
        .spawn(move || {
            let mut buf = [0u8; 8 * 1024];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => {
                        debug!(session_id = %id, "terminal: reader EOF");
                        let _ = tx.send(TerminalEvent::Exit);
                        return;
                    }
                    Ok(n) => {
                        if tx.send(TerminalEvent::Output(buf[..n].to_vec())).is_err() {
                            // Receiver dropped — session is gone, nothing
                            // to publish. Stop pumping.
                            debug!(session_id = %id, "terminal: reader receiver dropped");
                            return;
                        }
                    }
                    Err(e) => {
                        warn!(session_id = %id, error = %e, "terminal: reader error");
                        let _ = tx.send(TerminalEvent::Exit);
                        return;
                    }
                }
            }
        })
        .expect("spawn pty reader thread");
}

/// Auto-detect the user's shell. Order: requested → `$SHELL` →
/// `/bin/bash` → `/bin/sh`. The last is an absolute fallback that
/// exists on every POSIX system.
fn resolve_shell(requested: Option<String>) -> String {
    if let Some(s) = requested.filter(|s| !s.is_empty()) {
        return s;
    }
    if let Ok(s) = std::env::var("SHELL")
        && !s.is_empty()
    {
        return s;
    }
    for fallback in ["/bin/bash", "/bin/sh"] {
        if std::path::Path::new(fallback).exists() {
            return fallback.to_string();
        }
    }
    // Last-ditch — let the spawn fail with a useful error if neither
    // exists.
    "/bin/sh".to_string()
}

/// Resolve cwd. Falls back to the daemon's cwd when the requested path
/// doesn't exist (logged but not fatal — better than refusing to spawn).
fn resolve_cwd(requested: Option<String>) -> String {
    let here = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "/".to_string());
    let Some(req) = requested.filter(|s| !s.is_empty()) else {
        return here;
    };
    if std::path::Path::new(&req).is_dir() {
        req
    } else {
        debug!(requested = %req, falling_back_to = %here, "terminal: requested cwd missing — falling back");
        here
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// PTY allocation must succeed and the reader pump must deliver
    /// at least one chunk for `echo hello`.
    ///
    /// Uses `/bin/sh -c "echo hello"` (via `spawn` of `/bin/sh` with
    /// the command then `write`+`exit`) to keep the test
    /// shell-agnostic. Skips on systems without `/bin/sh`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn spawn_write_read_round_trip() {
        if !std::path::Path::new("/bin/sh").exists() {
            eprintln!("skipping: /bin/sh missing");
            return;
        }
        let session = TerminalSession::spawn(
            DEFAULT_ROWS,
            DEFAULT_COLS,
            Some("/bin/sh".to_string()),
            None,
        )
        .expect("spawn");
        assert_eq!(session.shell(), "/bin/sh");
        let mut events = session.take_events().expect("first take_events");

        // Send a command + newline + exit.
        session.write(b"echo hello-from-pty-test\nexit\n").unwrap();

        // Drain events for up to 5s — tests on a loaded CI box can
        // take a moment to flush the shell prompt + the echo + the
        // exit handshake.
        let mut buffered: Vec<u8> = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let timeout = tokio::time::timeout_at(deadline, events.recv()).await;
            match timeout {
                Ok(Some(TerminalEvent::Output(bytes))) => {
                    buffered.extend_from_slice(&bytes);
                    if buffered
                        .windows(20)
                        .any(|w| w == b"hello-from-pty-test\n" || w == b"hello-from-pty-test\r")
                    {
                        // Got the echo'd output — round-trip works.
                        break;
                    }
                }
                Ok(Some(TerminalEvent::Exit)) | Ok(None) => break,
                Err(_) => break,
            }
        }
        let printable: String = String::from_utf8_lossy(&buffered).into_owned();
        assert!(
            printable.contains("hello-from-pty-test"),
            "expected echo output, got: {printable:?}"
        );

        // Close should be Ok and idempotent.
        session.close().unwrap();
        session.close().unwrap();
    }

    #[test]
    fn session_id_is_short_and_prefixed() {
        let id = SessionId::new();
        assert!(id.0.starts_with("t-"));
        assert_eq!(id.0.len(), 14, "t- + 12 hex chars");
        let id2 = SessionId::new();
        assert_ne!(id, id2, "ids must be unique");
    }

    #[test]
    fn resolve_shell_prefers_requested_then_env() {
        // Requested wins.
        assert_eq!(
            resolve_shell(Some("/usr/bin/zsh".to_string())),
            "/usr/bin/zsh"
        );
        // Empty requested → falls through.
        let prior = std::env::var("SHELL").ok();
        // SAFETY: tests run single-threaded under cargo test by
        // default, but `set_var` is unsafe in 2024 edition due to the
        // race semantics. The cfg! guard shows intent.
        unsafe { std::env::set_var("SHELL", "/usr/bin/from-env-shell") };
        assert_eq!(resolve_shell(None), "/usr/bin/from-env-shell");
        // Restore.
        match prior {
            Some(v) => unsafe { std::env::set_var("SHELL", v) },
            None => unsafe { std::env::remove_var("SHELL") },
        }
    }

    #[test]
    fn resolve_cwd_falls_back_when_path_missing() {
        let bogus = "/no/such/dir/anywhere".to_string();
        let resolved = resolve_cwd(Some(bogus));
        assert_ne!(resolved, "/no/such/dir/anywhere");
        // Should be an existing directory.
        assert!(std::path::Path::new(&resolved).is_dir());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn close_releases_pty() {
        if !std::path::Path::new("/bin/sh").exists() {
            return;
        }
        let session = TerminalSession::spawn(
            DEFAULT_ROWS,
            DEFAULT_COLS,
            Some("/bin/sh".to_string()),
            None,
        )
        .unwrap();
        let id = session.id().clone();
        session.close().unwrap();
        // Subsequent close is still Ok.
        session.close().unwrap();
        // ID is still readable after close (struct lives until drop).
        assert_eq!(session.id(), &id);
    }
}
