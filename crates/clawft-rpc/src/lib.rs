//! `clawft-rpc` — RPC client and protocol types for the WeftOS kernel daemon.
//!
//! This crate provides the transport-agnostic protocol types (`Request`,
//! `Response`) and a `DaemonClient` that connects to the running kernel
//! daemon over a Unix domain socket.
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
mod protocol;
pub mod version_check;

pub use client::{DaemonClient, is_daemon_running};
pub use protocol::{
    LOG_FILE_NAME, PID_FILE_NAME, Request, Response, SOCKET_NAME, log_path, pid_path, runtime_dir,
    socket_path,
};

/// Connect to the daemon or bail with a helpful error message.
///
/// This is a convenience for CLI commands that require a running daemon.
pub async fn connect_or_bail() -> anyhow::Result<DaemonClient> {
    DaemonClient::connect().await.ok_or_else(|| {
        anyhow::anyhow!(
            "no kernel daemon running.\n\
             Start the daemon with: weaver kernel start\n\
             Or run: weaver console"
        )
    })
}
