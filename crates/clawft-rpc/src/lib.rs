//! `clawft-rpc` — RPC client and protocol types for the WeftOS kernel daemon.
//!
//! This crate provides the transport-agnostic protocol types (`Request`,
//! `Response`) and a `DaemonClient` that connects to the running kernel
//! daemon over a **local transport**:
//!
//! - **Unix**: Unix domain socket (`kernel.sock` under the runtime dir)
//! - **Windows**: named pipe derived from the same logical path (WEFT-11)
//!
//! Server-side named-pipe helpers live in [`named_pipe`]. The weave
//! daemon accept loop binds them under `cfg(windows)` (WEFT-559).
//!
//! # Usage
//!
//! ```rust,no_run
//! use clawft_rpc::{DaemonClient, Request};
//!
//! # async fn example() -> anyhow::Result<()> {
//! let mut client = DaemonClient::connect()
//!     .await
//!     .ok_or_else(|| anyhow::anyhow!("no daemon running"))?;
//!
//! let resp = client.simple_call("kernel.status").await?;
//! println!("{:?}", resp.result);
//! # Ok(())
//! # }
//! ```

mod client;
pub mod named_pipe;
pub mod probe;
mod protocol;
pub mod version_check;

pub use client::{DaemonClient, StreamSession, is_daemon_running, is_daemon_running_at};
pub use protocol::{
    LOG_FILE_NAME, PID_FILE_NAME, PIPE_NAME_PREFIX, Request, Response, SOCKET_NAME,
    default_pipe_name, log_path, pid_path, pipe_name_for_path, runtime_dir, runtime_paths, socket_path,
};

/// Connect to the daemon or bail with a helpful error message.
///
/// This is a convenience for CLI commands that require a running daemon.
pub async fn connect_or_bail() -> anyhow::Result<DaemonClient> {
    match DaemonClient::connect().await {
        Some(client) => Ok(client),
        None => Err(anyhow::anyhow!(probe::unreachable_message().await)),
    }
}
