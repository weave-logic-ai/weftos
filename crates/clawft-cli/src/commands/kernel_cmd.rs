//! `weft kernel` subcommand implementation.
//!
//! Provides kernel introspection commands:
//! - `weft kernel status` -- kernel state, uptime, process/service counts
//! - `weft kernel services` -- list registered services with health
//! - `weft kernel ps` -- list process table entries

use std::sync::Arc;

use clap::{Parser, Subcommand};
use comfy_table::{presets, Table};

use clawft_kernel::{Kernel, KernelState};
use clawft_platform::NativePlatform;

/// Kernel management subcommand.
#[derive(Parser)]
pub struct KernelArgs {
    /// Kernel subcommand.
    #[command(subcommand)]
    pub action: KernelAction,

    /// Config file path (overrides auto-discovery).
    #[arg(short, long, global = true)]
    pub config: Option<String>,

    /// Print daemon results as raw JSON instead of text.
    #[arg(long, global = true)]
    pub json: bool,
}

/// Kernel subcommands.
#[derive(Subcommand)]
pub enum KernelAction {
    /// Show kernel state, uptime, process count, service count.
    Status,

    /// List registered services with name, type, health status.
    Services,

    /// List process table entries.
    Ps,

    /// Boot the kernel (non-interactive, foreground).
    Boot {
        /// Run in foreground with log output (no REPL).
        #[arg(long)]
        foreground: bool,
    },
}

/// Render a daemon JSON result as text: an object becomes `key: value`
/// lines, an array of objects becomes a table, anything else prints as-is.
fn render_json_text(value: &serde_json::Value) -> String {
    use serde_json::Value;
    fn cell(v: &Value) -> String {
        match v {
            Value::String(s) => s.clone(),
            Value::Null => "-".into(),
            other => other.to_string(),
        }
    }
    match value {
        Value::Object(map) => map
            .iter()
            .map(|(k, v)| format!("{k}: {}", cell(v)))
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Array(rows) if rows.iter().all(Value::is_object) && !rows.is_empty() => {
            let cols: Vec<String> = rows[0].as_object().unwrap().keys().cloned().collect();
            let mut table = Table::new();
            table.load_preset(presets::NOTHING);
            table.set_header(cols.clone());
            for row in rows {
                table.add_row(cols.iter().map(|c| cell(&row[c.as_str()])).collect::<Vec<_>>());
            }
            table.to_string()
        }
        other => cell(other),
    }
}

/// Ask a running daemon (if any) for `method` and print its result.
///
/// Returns `Ok(true)` when a daemon answered successfully, so the caller
/// skips the local ephemeral boot; `Ok(false)` when no daemon is running.
/// A daemon that answers with an error is a failure: the message goes to
/// stderr and the process exits non-zero.
async fn print_from_daemon(method: &str, json: bool) -> anyhow::Result<bool> {
    let Some(mut client) = clawft_rpc::DaemonClient::connect().await else {
        return Ok(false);
    };
    let resp = client.simple_call(method).await?;
    if !resp.ok {
        eprintln!(
            "daemon error: {}",
            resp.error.unwrap_or_else(|| "unknown error".into())
        );
        std::process::exit(1);
    }
    let value = resp.result.unwrap_or(serde_json::Value::Null);
    if json {
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        println!("(daemon)\n{}", render_json_text(&value));
    }
    Ok(true)
}

/// Boot an ephemeral kernel for inspection: no listeners are bound.
async fn boot_for_inspection(
    config: clawft_types::config::Config,
    kernel_config: clawft_types::config::KernelConfig,
    platform: NativePlatform,
) -> Kernel<NativePlatform> {
    match Kernel::boot(config, kernel_config.for_inspection(), Arc::new(platform)).await {
        Ok(kernel) => kernel,
        Err(e) => {
            eprintln!("kernel boot failed: {e}");
            std::process::exit(1);
        }
    }
}

/// Run the kernel subcommand.
pub async fn run(args: KernelArgs) -> anyhow::Result<()> {
    let platform = NativePlatform::new();
    let config = super::load_config(&platform, args.config.as_deref()).await?;
    let kernel_config = config.kernel.clone();

    match args.action {
        KernelAction::Status => {
            if print_from_daemon("kernel.status", args.json).await? {
                return Ok(());
            }
            let kernel = boot_for_inspection(config, kernel_config, platform).await;
            print_status(&kernel);
        }
        KernelAction::Services => {
            if print_from_daemon("kernel.services", args.json).await? {
                return Ok(());
            }
            let kernel = boot_for_inspection(config, kernel_config, platform).await;
            print_services(&kernel).await;
        }
        KernelAction::Ps => {
            if print_from_daemon("kernel.ps", args.json).await? {
                return Ok(());
            }
            let kernel = boot_for_inspection(config, kernel_config, platform).await;
            print_ps(&kernel);
        }
        KernelAction::Boot { foreground } => {
            let kernel = if foreground {
                // A long-lived kernel: full config, and the same persisted
                // node.key the daemon uses so its node id is stable.
                // A second kernel would share the daemon's node id and key.
                if clawft_rpc::is_daemon_running().await {
                    eprintln!(
                        "a daemon is already running for this runtime dir; refusing to boot a \
                         second kernel with the same node identity (stop it first, or set \
                         WEFTOS_RUNTIME_DIR to an isolated directory)"
                    );
                    std::process::exit(1);
                }
                let runtime_dir = clawft_rpc::socket_path()
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| std::path::PathBuf::from("."));
                let key = clawft_kernel::load_or_generate_node_key(&runtime_dir)
                    .map_err(|e| anyhow::anyhow!("node key bootstrap: {e}"))?;
                Kernel::boot_with_node_key(
                    config,
                    kernel_config,
                    Arc::new(platform),
                    Some(key.to_bytes()),
                )
                .await
            } else {
                Ok(boot_for_inspection(config, kernel_config, platform).await)
            };
            match kernel {
                Ok(kernel) => {
                    // Print boot log
                    print!("{}", clawft_kernel::console::boot_banner());
                    print!("{}", kernel.boot_log().format_all());

                    if foreground {
                        println!("\nKernel running in foreground. Press Ctrl+C to stop.");
                        // Wait indefinitely until interrupted
                        tokio::signal::ctrl_c().await?;
                        println!("\nShutting down...");
                    }
                }
                Err(e) => {
                    eprintln!("kernel boot failed: {e}");
                    std::process::exit(1);
                }
            }
        }
    }

    Ok(())
}

/// Print kernel status summary.
fn print_status<P: clawft_platform::Platform>(kernel: &Kernel<P>) {
    let state_str = match kernel.state() {
        KernelState::Booting => "booting",
        KernelState::Running => "running",
        KernelState::ShuttingDown => "shutting down",
        KernelState::Halted => "halted",
    };

    let uptime = kernel.uptime();
    let uptime_str = if uptime.as_secs() > 3600 {
        format!(
            "{}h {}m {}s",
            uptime.as_secs() / 3600,
            (uptime.as_secs() % 3600) / 60,
            uptime.as_secs() % 60
        )
    } else if uptime.as_secs() > 60 {
        format!(
            "{}m {}s",
            uptime.as_secs() / 60,
            uptime.as_secs() % 60
        )
    } else {
        format!("{:.1}s", uptime.as_secs_f64())
    };

    println!("WeftOS Kernel Status");
    println!("--------------------");
    println!("State:      {state_str}");
    println!("Uptime:     {uptime_str}");
    println!("Processes:  {}", kernel.process_table().len());
    println!("Services:   {}", kernel.services().len());
    println!(
        "Max procs:  {}",
        kernel.kernel_config().max_processes
    );
    println!(
        "Health chk: {}s",
        kernel.kernel_config().health_check_interval_secs
    );
}

/// Print services table.
async fn print_services<P: clawft_platform::Platform>(kernel: &Kernel<P>) {
    let services = kernel.services().list();
    if services.is_empty() {
        println!("No services registered.");
        return;
    }

    let health_results = kernel.services().health_all().await;

    let mut table = Table::new();
    table.load_preset(presets::UTF8_FULL_CONDENSED);
    table.set_header(vec!["Name", "Type", "Health"]);

    for (name, stype) in &services {
        let health = health_results
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, h)| h.to_string())
            .unwrap_or_else(|| "unknown".into());

        table.add_row(vec![name.as_str(), &stype.to_string(), &health]);
    }

    println!("{table}");
}

/// Print process table.
fn print_ps<P: clawft_platform::Platform>(kernel: &Kernel<P>) {
    let entries = kernel.process_table().list();
    if entries.is_empty() {
        println!("No agents running.");
        return;
    }

    let mut table = Table::new();
    table.load_preset(presets::UTF8_FULL_CONDENSED);
    table.set_header(vec!["PID", "Agent", "State", "Mem", "CPU", "Parent"]);

    let mut entries = entries;
    entries.sort_by_key(|e| e.pid);

    for entry in &entries {
        let mem = format_bytes(entry.resource_usage.memory_bytes);
        let cpu = format!("{:.1}s", entry.resource_usage.cpu_time_ms as f64 / 1000.0);
        let parent = entry
            .parent_pid
            .map(|p| p.to_string())
            .unwrap_or_else(|| "-".into());

        table.add_row(vec![
            &entry.pid.to_string(),
            &entry.agent_id,
            &entry.state.to_string(),
            &mem,
            &cpu,
            &parent,
        ]);
    }

    println!("{table}");
}

/// Format a byte count as a human-readable string.
fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.1}GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    } else if bytes >= 1024 * 1024 {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes}B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_bytes_units() {
        assert_eq!(format_bytes(0), "0B");
        assert_eq!(format_bytes(512), "512B");
        assert_eq!(format_bytes(1024), "1.0KB");
        assert_eq!(format_bytes(1024 * 1024), "1.0MB");
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.0GB");
    }

    #[test]
    fn kernel_args_parses() {
        use clap::CommandFactory;
        KernelArgs::command().debug_assert();
    }
}
