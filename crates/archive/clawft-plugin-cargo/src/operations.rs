//! Cargo subprocess operations.
//!
//! All commands are constructed programmatically using
//! `tokio::process::Command` to prevent shell injection. User-provided
//! arguments (package names, feature flags) are validated before use.

use std::path::Path;
use std::time::Duration;

use tokio::process::Command;
use tracing::{debug, warn};

use crate::types::{CargoConfig, CargoFlags, CargoResult, CargoSubcommand};

/// Maximum output size in bytes to capture (1 MB).
const MAX_OUTPUT_BYTES: usize = 1_048_576;

/// Default timeout for cargo commands (5 minutes).
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);

/// Execute a cargo subcommand with the given flags.
///
/// The command is built entirely programmatically -- no shell interpolation.
/// Arguments are validated by [`CargoFlags`] before reaching this function.
pub async fn execute_cargo(
    subcommand: CargoSubcommand,
    flags: &CargoFlags,
    config: &CargoConfig,
) -> Result<CargoResult, String> {
    let mut cmd = Command::new(&config.cargo_binary);
    cmd.arg(subcommand.as_str());

    // Apply validated flags
    for arg in flags.to_args() {
        cmd.arg(&arg);
    }

    // Set working directory if configured
    if let Some(ref dir) = config.working_dir {
        let path = Path::new(dir);
        if !path.is_dir() {
            return Err(format!("working directory does not exist: {dir}"));
        }
        cmd.current_dir(path);
    }

    // Capture stdout and stderr
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    // Build command string for logging/result
    let command_str = format_command(&config.cargo_binary, subcommand, flags);
    debug!(command = %command_str, "executing cargo command");

    // Spawn and wait with timeout
    let child = cmd
        .spawn()
        .map_err(|e| format!("failed to spawn cargo: {e}"))?;

    let output = tokio::time::timeout(DEFAULT_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| format!("cargo command timed out after {}s", DEFAULT_TIMEOUT.as_secs()))?
        .map_err(|e| format!("cargo process error: {e}"))?;

    let stdout = truncate_output(&output.stdout);
    let stderr = truncate_output(&output.stderr);
    let exit_code = output.status.code();

    if !output.status.success() {
        warn!(
            command = %command_str,
            exit_code = ?exit_code,
            "cargo command failed"
        );
    }

    Ok(CargoResult {
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
fn format_command(binary: &str, subcommand: CargoSubcommand, flags: &CargoFlags) -> String {
    let mut parts = vec![binary.to_string(), subcommand.as_str().to_string()];
    parts.extend(flags.to_args());
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_command_basic() {
        let flags = CargoFlags::default();
        let cmd = format_command("cargo", CargoSubcommand::Build, &flags);
        assert_eq!(cmd, "cargo build");
    }

    #[test]
    fn format_command_with_flags() {
        let flags = CargoFlags {
            release: true,
            workspace: true,
            package: None,
            json_output: false,
            extra_args: vec![],
        };
        let cmd = format_command("cargo", CargoSubcommand::Test, &flags);
        assert_eq!(cmd, "cargo test --release --workspace");
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
}
