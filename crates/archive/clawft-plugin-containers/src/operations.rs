//! Container subprocess operations.
//!
//! All commands are constructed programmatically using
//! `tokio::process::Command` to prevent shell injection. User-provided
//! arguments (container names, image names, env vars) are validated
//! before use.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::process::Command;
use tracing::{debug, warn};

use crate::types::{ContainerConfig, ContainerResult, ContainerRuntime};

/// Maximum output size in bytes to capture (1 MB).
const MAX_OUTPUT_BYTES: usize = 1_048_576;

/// Default timeout for container commands (5 minutes).
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);

/// Global concurrency limiter for container operations.
///
/// Tracks the number of in-flight operations. Callers must check
/// `try_acquire` before spawning a command and call `release` when done.
pub struct ConcurrencyLimiter {
    in_flight: AtomicU32,
    max: u32,
}

impl ConcurrencyLimiter {
    /// Create a new limiter with the given maximum concurrent operations.
    pub fn new(max: u32) -> Arc<Self> {
        Arc::new(Self {
            in_flight: AtomicU32::new(0),
            max,
        })
    }

    /// Try to acquire a slot. Returns `true` if acquired, `false` if at capacity.
    pub fn try_acquire(&self) -> bool {
        loop {
            let current = self.in_flight.load(Ordering::Relaxed);
            if current >= self.max {
                return false;
            }
            if self
                .in_flight
                .compare_exchange(current, current + 1, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                return true;
            }
        }
    }

    /// Release a slot after an operation completes.
    pub fn release(&self) {
        self.in_flight.fetch_sub(1, Ordering::Release);
    }

    /// Current number of in-flight operations.
    pub fn current(&self) -> u32 {
        self.in_flight.load(Ordering::Relaxed)
    }
}

/// Execute a container command with validated arguments.
///
/// The command is built entirely programmatically -- no shell interpolation.
pub async fn execute_container(
    runtime: ContainerRuntime,
    args: &[String],
    config: &ContainerConfig,
    limiter: &ConcurrencyLimiter,
) -> Result<ContainerResult, String> {
    if !limiter.try_acquire() {
        return Err(format!(
            "concurrent container operation limit reached ({}/{})",
            limiter.current(),
            config.max_concurrent_ops
        ));
    }

    let result = execute_container_inner(runtime, args).await;

    limiter.release();
    result
}

async fn execute_container_inner(
    runtime: ContainerRuntime,
    args: &[String],
) -> Result<ContainerResult, String> {
    let binary = runtime.binary();
    let mut cmd = Command::new(binary);

    for arg in args {
        cmd.arg(arg);
    }

    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let command_str = format_command(binary, args);
    debug!(command = %command_str, "executing container command");

    let child = cmd
        .spawn()
        .map_err(|e| format!("failed to spawn {binary}: {e}"))?;

    let output = tokio::time::timeout(DEFAULT_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| {
            format!(
                "container command timed out after {}s",
                DEFAULT_TIMEOUT.as_secs()
            )
        })?
        .map_err(|e| format!("container process error: {e}"))?;

    let stdout = truncate_output(&output.stdout);
    let stderr = truncate_output(&output.stderr);
    let exit_code = output.status.code();

    if !output.status.success() {
        warn!(
            command = %command_str,
            exit_code = ?exit_code,
            "container command failed"
        );
    }

    Ok(ContainerResult {
        success: output.status.success(),
        exit_code,
        stdout,
        stderr,
        command: command_str,
    })
}

/// Truncate output to `MAX_OUTPUT_BYTES` and convert to string.
fn truncate_output(bytes: &[u8]) -> String {
    let truncated = if bytes.len() > MAX_OUTPUT_BYTES {
        &bytes[..MAX_OUTPUT_BYTES]
    } else {
        bytes
    };
    String::from_utf8_lossy(truncated).to_string()
}

/// Format the command for display/logging (never for execution).
fn format_command(binary: &str, args: &[String]) -> String {
    let mut parts = vec![binary.to_string()];
    parts.extend(args.iter().cloned());
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_command_basic() {
        let args = vec!["ps".to_string(), "--all".to_string()];
        let cmd = format_command("docker", &args);
        assert_eq!(cmd, "docker ps --all");
    }

    #[test]
    fn truncate_output_short() {
        let data = b"short output";
        let result = truncate_output(data);
        assert_eq!(result, "short output");
    }

    #[test]
    fn truncate_output_long() {
        let data = vec![b'x'; MAX_OUTPUT_BYTES + 100];
        let result = truncate_output(&data);
        assert_eq!(result.len(), MAX_OUTPUT_BYTES);
    }

    #[test]
    fn concurrency_limiter_basic() {
        let limiter = ConcurrencyLimiter::new(2);
        assert!(limiter.try_acquire());
        assert!(limiter.try_acquire());
        assert!(!limiter.try_acquire()); // at capacity
        assert_eq!(limiter.current(), 2);
        limiter.release();
        assert_eq!(limiter.current(), 1);
        assert!(limiter.try_acquire());
    }

    #[test]
    fn concurrency_limiter_zero() {
        let limiter = ConcurrencyLimiter::new(0);
        assert!(!limiter.try_acquire());
    }
}
