//! `weaver kernel` subcommand implementation.
//!
//! Provides kernel lifecycle and introspection commands:
//! - `weaver kernel start`         -- start the kernel daemon (backgrounds by default)
//! - `weaver kernel start --foreground` -- start in foreground (blocking)
//! - `weaver kernel stop`          -- stop a running daemon
//! - `weaver kernel status`        -- kernel state, uptime, PID, process/service counts
//! - `weaver kernel services`      -- list registered services with health
//! - `weaver kernel ps`            -- list process table entries
//! - `weaver kernel attach`        -- stream kernel logs in real time
//! - `weaver kernel logs`          -- show kernel event log snapshot
//!
//! Query commands (`status`, `services`, `ps`) connect to a running daemon
//! first. If no daemon is running, they boot an ephemeral kernel, display
//! results, and exit.

use std::sync::Arc;

use clap::{Parser, Subcommand};
use comfy_table::{Table, presets};

use clawft_kernel::{Kernel, KernelState};
use clawft_platform::NativePlatform;

use crate::client::DaemonClient;
use crate::protocol;

/// Kernel management subcommand.
#[derive(Parser)]
#[command(about = "WeftOS kernel management (start, stop, status, services, processes)")]
pub struct KernelArgs {
    /// Kernel subcommand.
    #[command(subcommand)]
    pub action: KernelAction,

    /// Config file path (overrides auto-discovery).
    #[arg(short, long, global = true)]
    pub config: Option<String>,
}

/// Kernel subcommands.
#[derive(Subcommand)]
pub enum KernelAction {
    /// Start the kernel daemon (backgrounds by default).
    Start {
        /// Run in foreground instead of backgrounding.
        #[arg(long)]
        foreground: bool,

        /// Start a fresh chain at the resolved path instead of adopting the
        /// legacy `~/.clawft` chain (the legacy chain is left untouched).
        #[arg(long)]
        new_chain: bool,
    },

    /// Stop a running kernel daemon (sends SIGTERM).
    Stop {
        /// Force-kill with SIGKILL if graceful shutdown times out.
        #[arg(long)]
        force: bool,
    },

    /// Restart a running kernel daemon (sends SIGHUP for re-exec).
    Restart,

    /// Show kernel state, uptime, process count, service count.
    Status,

    /// List registered services with name, type, health status.
    Services,

    /// List process table entries.
    Ps,

    /// Attach to a running daemon and stream logs in real time.
    Attach {
        /// Number of recent entries to show before streaming (default: 20).
        #[arg(short = 'n', long, default_value = "20")]
        tail: usize,

        /// Minimum log level: debug, info, warn, error.
        #[arg(short, long)]
        level: Option<String>,
    },

    /// Show kernel event log.
    Logs {
        /// Number of recent entries to show (default: 50, 0 = all).
        #[arg(short = 'n', long, default_value = "50")]
        count: usize,

        /// Minimum log level: debug, info, warn, error.
        #[arg(short, long)]
        level: Option<String>,
    },

    /// Rotate the node Ed25519 identity key (S10 / WEFT-107).
    ///
    /// Generates a new keypair, dual-signs an `identity.key_rotation`
    /// ExoChain event with both old and new keys, appends it to the local
    /// chain export, and replaces `<runtime>/node.key`.
    #[command(name = "rotate-key")]
    RotateKey {
        /// Grace period in seconds during which the old key remains trusted.
        #[arg(long, default_value_t = 86_400)]
        grace_period_secs: u64,

        /// Dry-run: build and verify the dual-signed payload without writing.
        #[arg(long)]
        dry_run: bool,
    },
}

/// Run the kernel subcommand.
pub async fn run(args: KernelArgs) -> anyhow::Result<()> {
    // Platforms without a local daemon transport (neither Unix UDS nor
    // Windows named pipes) cannot host kernel start/stop/restart.
    #[cfg(not(any(unix, windows)))]
    {
        match args.action {
            KernelAction::Start { .. } | KernelAction::Stop { .. } | KernelAction::Restart => {
                anyhow::bail!(
                    "kernel daemon server is not available on this platform.\n\
                     Local transport requires Unix domain sockets or Windows named pipes (WEFT-559)."
                );
            }
            _ => {}
        }
    }

    match args.action {
        #[cfg(any(unix, windows))]
        KernelAction::Start {
            foreground,
            new_chain,
        } => {
            if foreground {
                // Run in foreground (blocking)
                let platform = NativePlatform::new();
                let loaded =
                    super::load_config_layered(&platform, args.config.as_deref()).await?;
                let kernel_config = loaded.config.kernel.clone();
                clawft_kernel::chain_storage::request_new_chain(new_chain);
                crate::daemon::run(
                    loaded.config,
                    kernel_config,
                    loaded.global_routing,
                    loaded.workspace_routing,
                )
                .await?;
            } else {
                // Background (default) — spawn detached child
                crate::daemon::daemonize(args.config.as_deref(), new_chain)?;
            }
        }
        #[cfg(unix)]
        KernelAction::Stop { force } => {
            let pid = read_daemon_pid()?;
            let nix_pid = nix::unistd::Pid::from_raw(pid);

            // Send SIGTERM for graceful shutdown
            nix::sys::signal::kill(nix_pid, nix::sys::signal::Signal::SIGTERM)
                .map_err(|e| anyhow::anyhow!("failed to send SIGTERM to PID {pid}: {e}"))?;
            println!("SIGTERM sent to PID {pid}, waiting for exit...");

            if wait_for_exit(pid, std::time::Duration::from_secs(10)) {
                println!("Daemon stopped.");
            } else if force {
                println!("Graceful shutdown timed out — sending SIGKILL.");
                let _ = nix::sys::signal::kill(nix_pid, nix::sys::signal::Signal::SIGKILL);
                std::thread::sleep(std::time::Duration::from_millis(500));
                println!("Daemon killed.");
            } else {
                eprintln!("Daemon still running after 10s. Use --force to SIGKILL.");
            }

            // Clean up stale files
            cleanup_runtime_files();
        }
        // Windows: prefer RPC `kernel.shutdown` over named pipe; fall back
        // to taskkill using the PID file (WEFT-559).
        #[cfg(windows)]
        KernelAction::Stop { force } => {
            stop_windows(force).await?;
        }
        #[cfg(unix)]
        KernelAction::Restart => {
            let pid = read_daemon_pid()?;
            let nix_pid = nix::unistd::Pid::from_raw(pid);

            nix::sys::signal::kill(nix_pid, nix::sys::signal::Signal::SIGHUP)
                .map_err(|e| anyhow::anyhow!("failed to send SIGHUP to PID {pid}: {e}"))?;
            println!("SIGHUP sent to PID {pid} — daemon will restart.");
        }
        // Windows has no SIGHUP re-exec path — stop then start.
        #[cfg(windows)]
        KernelAction::Restart => {
            println!("Restarting daemon (stop + start)...");
            let _ = stop_windows(true).await;
            crate::daemon::daemonize(args.config.as_deref(), false)?;
        }
        #[cfg(not(any(unix, windows)))]
        KernelAction::Start { .. } | KernelAction::Stop { .. } | KernelAction::Restart => {
            unreachable!("handled above");
        }
        KernelAction::Status => {
            if let Some(mut client) = DaemonClient::connect().await {
                let resp = client.simple_call("kernel.status").await?;
                if resp.ok {
                    let result: protocol::KernelStatusResult =
                        serde_json::from_value(resp.result.unwrap())?;
                    // Read PID from PID file if available
                    let pid = protocol::pid_path()
                        .exists()
                        .then(|| std::fs::read_to_string(protocol::pid_path()).ok())
                        .flatten()
                        .and_then(|s| s.trim().parse::<u32>().ok());
                    print_daemon_status(&result, pid);
                    print_cluster_summary(&mut client).await;
                } else {
                    let msg = resp.error.unwrap_or_else(|| "unknown error".into());
                    eprintln!("daemon error: {msg}");
                }
            } else {
                eprintln!("(no daemon running — booting ephemeral kernel)\n");
                let platform = NativePlatform::new();
                let config = super::load_config(&platform, args.config.as_deref()).await?;
                let kernel = boot_or_exit(config.clone(), config.kernel.clone(), platform).await;
                print_status(&kernel);
            }
        }
        KernelAction::Services => {
            if let Some(mut client) = DaemonClient::connect().await {
                let resp = client.simple_call("kernel.services").await?;
                if resp.ok {
                    let infos: Vec<protocol::ServiceInfo> =
                        serde_json::from_value(resp.result.unwrap())?;
                    print_daemon_services(&infos);
                } else {
                    let msg = resp.error.unwrap_or_else(|| "unknown error".into());
                    eprintln!("daemon error: {msg}");
                }
            } else {
                eprintln!("(no daemon running — booting ephemeral kernel)\n");
                let platform = NativePlatform::new();
                let config = super::load_config(&platform, args.config.as_deref()).await?;
                let kernel = boot_or_exit(config.clone(), config.kernel.clone(), platform).await;
                print_services(&kernel).await;
            }
        }
        KernelAction::Ps => {
            if let Some(mut client) = DaemonClient::connect().await {
                let resp = client.simple_call("kernel.ps").await?;
                if resp.ok {
                    let entries: Vec<protocol::ProcessInfo> =
                        serde_json::from_value(resp.result.unwrap())?;
                    print_daemon_ps(&entries);
                } else {
                    let msg = resp.error.unwrap_or_else(|| "unknown error".into());
                    eprintln!("daemon error: {msg}");
                }
            } else {
                eprintln!("(no daemon running — booting ephemeral kernel)\n");
                let platform = NativePlatform::new();
                let config = super::load_config(&platform, args.config.as_deref()).await?;
                let kernel = boot_or_exit(config.clone(), config.kernel.clone(), platform).await;
                print_ps(&kernel);
            }
        }
        KernelAction::RotateKey {
            grace_period_secs,
            dry_run,
        } => {
            run_rotate_key(grace_period_secs, dry_run)?;
        }
        KernelAction::Attach { tail, level } => {
            let mut client = clawft_rpc::connect_or_bail().await?;

            // Get the total event count to seed our cursor, then show recent tail
            let all_params = protocol::LogsParams {
                count: 0, // 0 = all
                level: level.clone(),
            };
            let all_req =
                protocol::Request::with_params("kernel.logs", serde_json::to_value(&all_params)?);
            let all_resp = client.call(all_req).await?;
            let total_events = if all_resp.ok {
                let entries: Vec<protocol::LogEntry> =
                    serde_json::from_value(all_resp.result.unwrap())?;
                let total = entries.len();
                // Show the last `tail` entries
                let show_from = total.saturating_sub(tail);
                let shown = &entries[show_from..];
                if !shown.is_empty() {
                    println!(
                        "--- Recent kernel logs ({} of {} entries) ---",
                        shown.len(),
                        total
                    );
                    print_daemon_logs(shown);
                    println!("--- Streaming (Ctrl+C to detach) ---");
                } else {
                    println!("--- No logs yet — streaming (Ctrl+C to detach) ---");
                }
                total
            } else {
                println!("--- Streaming (Ctrl+C to detach) ---");
                0
            };

            // Poll for new logs at interval, only printing entries beyond our cursor
            let poll_interval = std::time::Duration::from_secs(1);
            let mut last_count = total_events;
            loop {
                tokio::time::sleep(poll_interval).await;

                // Reconnect each poll (connection may be closed after response)
                let mut poll_client = match DaemonClient::connect().await {
                    Some(c) => c,
                    None => {
                        println!("\n[daemon disconnected]");
                        break;
                    }
                };

                let params = protocol::LogsParams {
                    count: 0, // all
                    level: level.clone(),
                };
                let req =
                    protocol::Request::with_params("kernel.logs", serde_json::to_value(&params)?);
                match poll_client.call(req).await {
                    Ok(resp) if resp.ok => {
                        let entries: Vec<protocol::LogEntry> =
                            serde_json::from_value(resp.result.unwrap())?;
                        let current_count = entries.len();
                        if current_count > last_count {
                            // Print only new entries (those beyond our cursor)
                            for entry in &entries[last_count..] {
                                let ts = &entry.timestamp[11..19];
                                let level_tag = match entry.level.as_str() {
                                    "error" => "ERR ",
                                    "warn" => "WARN",
                                    "debug" => "DBG ",
                                    _ => "INFO",
                                };
                                println!("{ts} [{level_tag}] {}", entry.message);
                            }
                        }
                        last_count = current_count;
                    }
                    Ok(_) => {} // non-ok response, keep polling
                    Err(_) => {
                        println!("\n[daemon disconnected]");
                        break;
                    }
                }
            }
        }
        KernelAction::Logs { count, level } => {
            if let Some(mut client) = DaemonClient::connect().await {
                let params = protocol::LogsParams {
                    count,
                    level: level.clone(),
                };
                let req =
                    protocol::Request::with_params("kernel.logs", serde_json::to_value(params)?);
                let resp = client.call(req).await?;
                if resp.ok {
                    let entries: Vec<protocol::LogEntry> =
                        serde_json::from_value(resp.result.unwrap())?;
                    print_daemon_logs(&entries);
                } else {
                    let msg = resp.error.unwrap_or_else(|| "unknown error".into());
                    eprintln!("daemon error: {msg}");
                }
            } else {
                eprintln!("(no daemon running — booting ephemeral kernel)\n");
                let platform = NativePlatform::new();
                let config = super::load_config(&platform, args.config.as_deref()).await?;
                let kernel = boot_or_exit(config.clone(), config.kernel.clone(), platform).await;
                print_event_log(&kernel, count, level.as_deref());
            }
        }
    }

    Ok(())
}

// ── S10 key rotation (WEFT-107) ───────────────────────────────────

/// Rotate the node identity key: dual-sign chain event + rewrite node.key.
fn run_rotate_key(grace_period_secs: u64, dry_run: bool) -> anyhow::Result<()> {
    use clawft_kernel::{
        ChainManager, DEFAULT_GRACE_PERIOD_SECS, format_rotation_summary,
        rotate_chain_signing_key_now, verify_key_rotation_event,
    };
    use ed25519_dalek::SigningKey;
    use std::fs;
    use std::path::PathBuf;

    let grace = if grace_period_secs == 0 {
        DEFAULT_GRACE_PERIOD_SECS
    } else {
        grace_period_secs
    };

    let paths = protocol::runtime_paths();
    fs::create_dir_all(paths.root())?;
    let key_path: PathBuf = paths.node_key();

    // Load or generate current identity (same layout as node_identity).
    let old_key = if key_path.exists() {
        let bytes = fs::read(&key_path)?;
        if bytes.len() != 32 {
            anyhow::bail!(
                "node.key at {} is malformed (expected 32 bytes, got {})",
                key_path.display(),
                bytes.len()
            );
        }
        let mut seed = [0u8; 32];
        seed.copy_from_slice(&bytes);
        SigningKey::from_bytes(&seed)
    } else {
        eprintln!(
            "(no {} — generating a temporary identity for rotation)",
            key_path.display()
        );
        let mut seed = [0u8; 32];
        use rand::RngCore;
        rand::rngs::OsRng.fill_bytes(&mut seed);
        SigningKey::from_bytes(&seed)
    };

    let mut chain = ChainManager::new(0, 1000).with_signing_key(old_key);
    let (event, new_key) = rotate_chain_signing_key_now(&mut chain, grace)
        .map_err(|e| anyhow::anyhow!("key rotation failed: {e}"))?;
    let payload = verify_key_rotation_event(&event)
        .map_err(|e| anyhow::anyhow!("post-rotate verify failed: {e}"))?;

    let new_pk = new_key.verifying_key().to_bytes();
    let new_node_id = clawft_kernel::node_id_from_pubkey(&new_pk);
    println!("{}", format_rotation_summary(&event, &payload, &new_node_id));

    if dry_run {
        println!("dry-run: no files written");
        return Ok(());
    }

    // Persist new key with 0600 perms.
    fs::write(&key_path, new_key.to_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&key_path)?.permissions();
        perms.set_mode(0o600);
        fs::set_permissions(&key_path, perms)?;
    }

    // Append rotation event to local chain export (best-effort).
    let chain_dir = paths.chain_dir();
    fs::create_dir_all(&chain_dir)?;
    let chain_path = chain_dir.join("local.jsonl");
    if let Err(e) = chain.save_to_file(&chain_path) {
        eprintln!("warning: failed to save chain export: {e}");
    } else {
        println!("chain export: {}", chain_path.display());
    }
    println!("new key written: {}", key_path.display());
    println!(
        "note: restart the daemon (`weaver kernel restart`) so peers pick up the new identity"
    );
    Ok(())
}

// ── Daemon-mode display (from protocol types) ─────────────────────

/// Print kernel status from a daemon response.
fn print_daemon_status(result: &protocol::KernelStatusResult, pid: Option<u32>) {
    let uptime_str = format_uptime(result.uptime_secs);

    println!("WeftOS Kernel Status (daemon)");
    println!("-----------------------------");
    println!("State:      {}", result.state);
    if let Some(p) = pid {
        println!("PID:        {p}");
    }
    println!("Uptime:     {uptime_str}");
    println!("Processes:  {}", result.process_count);
    println!("Services:   {}", result.service_count);
    println!("Max procs:  {}", result.max_processes);
    println!("Health chk: {}s", result.health_check_interval_secs);
    if !result.build.sha.is_empty() {
        println!(
            "Build:      {} ({} {})",
            result.build.version, result.build.sha, result.build.timestamp
        );
    }
    println!("Socket:     {}", protocol::socket_path().display());
    println!("Log:        {}", protocol::log_path().display());

    // Show cluster info if available (via separate RPC call)
    // This is best-effort; errors are silently ignored.
}

/// Fetch and print cluster info from daemon (appended to status output).
async fn print_cluster_summary(client: &mut DaemonClient) {
    let resp = match client.simple_call("cluster.nodes").await {
        Ok(r) => r,
        Err(_) => return,
    };
    if !resp.ok {
        return;
    }
    let nodes: Vec<protocol::ClusterNodeInfo> =
        match serde_json::from_value(resp.result.unwrap_or_default()) {
            Ok(n) => n,
            Err(_) => return,
        };
    if !nodes.is_empty() {
        println!("Cluster:    {} nodes", nodes.len());
    }
}

/// Print services from a daemon response.
fn print_daemon_services(infos: &[protocol::ServiceInfo]) {
    if infos.is_empty() {
        println!("No services registered.");
        return;
    }

    let mut table = Table::new();
    table.load_preset(presets::UTF8_FULL_CONDENSED);
    // WEFT-333: Detail column carries agent.chat metrics (and any
    // other SystemService::health_detail payload).
    table.set_header(vec!["Name", "Type", "Health", "Detail"]);

    for info in infos {
        let detail = info.detail.as_deref().unwrap_or("-");
        table.add_row(vec![
            info.name.as_str(),
            info.service_type.as_str(),
            info.health.as_str(),
            detail,
        ]);
    }

    println!("{table}");
}

/// Print process table from a daemon response.
fn print_daemon_ps(entries: &[protocol::ProcessInfo]) {
    if entries.is_empty() {
        println!("No agents running.");
        return;
    }

    let mut table = Table::new();
    table.load_preset(presets::UTF8_FULL_CONDENSED);
    table.set_header(vec!["PID", "Agent", "State", "Mem", "CPU", "Parent"]);

    for entry in entries {
        let mem = format_bytes(entry.memory_bytes);
        let cpu = format!("{:.1}s", entry.cpu_time_ms as f64 / 1000.0);
        let parent = entry
            .parent_pid
            .map(|p| p.to_string())
            .unwrap_or_else(|| "-".into());

        table.add_row(vec![
            &entry.pid.to_string(),
            &entry.agent_id,
            &entry.state,
            &mem,
            &cpu,
            &parent,
        ]);
    }

    println!("{table}");
}

/// Print log entries from a daemon response.
fn print_daemon_logs(entries: &[protocol::LogEntry]) {
    if entries.is_empty() {
        println!("No log entries.");
        return;
    }

    for entry in entries {
        let ts = &entry.timestamp[11..19]; // HH:MM:SS from ISO timestamp
        let level_tag = match entry.level.as_str() {
            "error" => "ERR ",
            "warn" => "WARN",
            "debug" => "DBG ",
            _ => "INFO",
        };
        println!("{ts} [{level_tag}] {}", entry.message);
    }
    println!("({} entries)", entries.len());
}

// ── Ephemeral-mode display (from Kernel<P>) ───────────────────────

/// Boot the kernel or exit with an error message.
async fn boot_or_exit(
    config: clawft_types::config::Config,
    kernel_config: clawft_types::config::KernelConfig,
    platform: NativePlatform,
) -> Kernel<NativePlatform> {
    match Kernel::boot(config, kernel_config, Arc::new(platform)).await {
        Ok(kernel) => kernel,
        Err(e) => {
            eprintln!("kernel boot failed: {e}");
            std::process::exit(1);
        }
    }
}

/// Print kernel status from an ephemeral kernel.
fn print_status<P: clawft_platform::Platform>(kernel: &Kernel<P>) {
    let state_str = match kernel.state() {
        KernelState::Booting => "booting",
        KernelState::Running => "running",
        KernelState::ShuttingDown => "shutting down",
        KernelState::Halted => "halted",
        _ => "unknown",
    };

    let uptime_str = format_uptime(kernel.uptime().as_secs_f64());

    println!("WeftOS Kernel Status (ephemeral)");
    println!("--------------------------------");
    println!("State:      {state_str}");
    println!("Uptime:     {uptime_str}");
    println!("Processes:  {}", kernel.process_table().len());
    println!("Services:   {}", kernel.services().len());
    println!("Max procs:  {}", kernel.kernel_config().max_processes);
    println!(
        "Health chk: {}s",
        kernel.kernel_config().health_check_interval_secs
    );

    // Cluster membership
    let membership = kernel.cluster_membership();
    let peer_count = membership.len();
    if peer_count > 0 {
        let active = membership.active_peers().len();
        println!("Cluster:    {peer_count} nodes ({active} active)");
    }
}

/// Print services table from an ephemeral kernel.
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

/// Print process table from an ephemeral kernel.
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

/// Print event log from an ephemeral kernel.
fn print_event_log<P: clawft_platform::Platform>(
    kernel: &Kernel<P>,
    count: usize,
    level: Option<&str>,
) {
    let event_log = kernel.event_log();

    let events = if let Some(level_str) = level {
        let min_level = match level_str {
            "debug" => clawft_kernel::LogLevel::Debug,
            "warn" | "warning" => clawft_kernel::LogLevel::Warn,
            "error" => clawft_kernel::LogLevel::Error,
            _ => clawft_kernel::LogLevel::Info,
        };
        event_log.filter_level(&min_level, count)
    } else {
        event_log.tail(count)
    };

    if events.is_empty() {
        println!("No log entries.");
        return;
    }

    for event in &events {
        let ts = &event.timestamp.format("%H:%M:%S").to_string();
        let level_tag = match event.level {
            clawft_kernel::LogLevel::Error => "ERR ",
            clawft_kernel::LogLevel::Warn => "WARN",
            clawft_kernel::LogLevel::Debug => "DBG ",
            clawft_kernel::LogLevel::Info => "INFO",
            _ => "??? ",
        };
        println!("{ts} [{level_tag}] {}", event.message);
    }
    println!("({} entries)", events.len());
}

// ── Signal / PID helpers ────────────────────────────────────────

#[cfg(unix)]
/// Read the daemon PID from `<runtime>/kernel.pid` and validate the process exists.
fn read_daemon_pid() -> anyhow::Result<i32> {
    let pid_path = protocol::pid_path();
    let pid_str = std::fs::read_to_string(&pid_path)
        .map_err(|_| anyhow::anyhow!("no PID file found — is the daemon running?"))?;
    let pid: i32 = pid_str
        .trim()
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid PID in {}", pid_path.display()))?;

    // Verify process is alive (kill -0)
    let nix_pid = nix::unistd::Pid::from_raw(pid);
    nix::sys::signal::kill(nix_pid, None)
        .map_err(|_| anyhow::anyhow!("PID {pid} is not running (stale PID file)"))?;

    Ok(pid)
}

#[cfg(unix)]
/// Poll until the given PID exits, returning `true` if it exited within timeout.
fn wait_for_exit(pid: i32, timeout: std::time::Duration) -> bool {
    let nix_pid = nix::unistd::Pid::from_raw(pid);
    let start = std::time::Instant::now();
    let poll_interval = std::time::Duration::from_millis(200);

    while start.elapsed() < timeout {
        // kill -0: returns Err if process is gone
        if nix::sys::signal::kill(nix_pid, None).is_err() {
            return true;
        }
        std::thread::sleep(poll_interval);
    }
    false
}

#[cfg(unix)]
/// Remove stale PID and socket files if the daemon is no longer running.
fn cleanup_runtime_files() {
    let pid_path = protocol::pid_path();
    if pid_path.exists() {
        let _ = std::fs::remove_file(&pid_path);
    }
    let sock_path = protocol::socket_path();
    if sock_path.exists() {
        let _ = std::fs::remove_file(&sock_path);
    }
}

/// Windows stop path (WEFT-559): RPC `kernel.shutdown` first, then taskkill.
#[cfg(windows)]
async fn stop_windows(force: bool) -> anyhow::Result<()> {
    use std::process::Command;
    use std::time::Duration;

    // Prefer graceful RPC shutdown over the named pipe.
    if let Some(mut client) = DaemonClient::connect().await {
        match client.simple_call("kernel.shutdown").await {
            Ok(resp) if resp.ok => {
                println!("kernel.shutdown accepted — waiting for exit...");
            }
            Ok(resp) => {
                let msg = resp.error.unwrap_or_else(|| "unknown error".into());
                eprintln!("kernel.shutdown error: {msg}");
            }
            Err(e) => {
                eprintln!("kernel.shutdown RPC failed: {e}");
            }
        }

        // Wait for the pipe to disappear / client dials to fail.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            if DaemonClient::connect().await.is_none() {
                println!("Daemon stopped.");
                cleanup_runtime_files_windows();
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }

        if !force {
            anyhow::bail!("Daemon still running after 10s. Use --force to taskkill.");
        }
        println!("Graceful shutdown timed out — taskkill /F.");
    } else {
        println!("No live named-pipe daemon; trying PID file...");
    }

    // Force path: kill via PID file if present.
    let pid_path = protocol::pid_path();
    if pid_path.exists() {
        if let Ok(pid_str) = std::fs::read_to_string(&pid_path) {
            if let Ok(pid) = pid_str.trim().parse::<u32>() {
                let mut cmd = Command::new("taskkill");
                cmd.args(["/PID", &pid.to_string()]);
                if force {
                    cmd.arg("/F");
                }
                match cmd.status() {
                    Ok(status) if status.success() => {
                        println!("taskkill sent to PID {pid}.");
                    }
                    Ok(status) => {
                        eprintln!("taskkill exited with {status}");
                    }
                    Err(e) => {
                        eprintln!("taskkill failed: {e}");
                    }
                }
            }
        }
    } else if DaemonClient::connect().await.is_none() {
        anyhow::bail!("no PID file found and no daemon answering — is the daemon running?");
    }

    cleanup_runtime_files_windows();
    Ok(())
}

#[cfg(windows)]
fn cleanup_runtime_files_windows() {
    let pid_path = protocol::pid_path();
    if pid_path.exists() {
        let _ = std::fs::remove_file(&pid_path);
    }
    // Named pipes have no filesystem node to remove.
}

// ── Shared helpers ────────────────────────────────────────────────

/// Format an uptime in seconds as a human-readable string.
fn format_uptime(secs: f64) -> String {
    let total_secs = secs as u64;
    if total_secs > 3600 {
        format!(
            "{}h {}m {}s",
            total_secs / 3600,
            (total_secs % 3600) / 60,
            total_secs % 60
        )
    } else if total_secs > 60 {
        format!("{}m {}s", total_secs / 60, total_secs % 60)
    } else {
        format!("{:.1}s", secs)
    }
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
    fn format_uptime_units() {
        assert_eq!(format_uptime(0.5), "0.5s");
        assert_eq!(format_uptime(42.0), "42.0s");
        assert_eq!(format_uptime(90.0), "1m 30s");
        assert_eq!(format_uptime(3661.0), "1h 1m 1s");
    }

    #[test]
    fn kernel_args_parses() {
        use clap::CommandFactory;
        KernelArgs::command().debug_assert();
    }
}
