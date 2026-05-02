//! `weaver console` -- interactive kernel REPL.
//!
//! Boots or attaches to a kernel and provides an interactive command shell
//! that accepts both `weaver` and `weft` style commands.

use std::io::{self, BufRead, Write};

use clap::Args;

use crate::client::DaemonClient;
use crate::protocol::{Request, Response};

#[derive(Args)]
pub struct ConsoleArgs {
    /// Attach to an already-running kernel instead of booting one.
    #[arg(long)]
    pub attach: bool,

    /// Show boot log of running kernel and exit (no REPL).
    #[arg(long)]
    pub replay_boot: bool,

    /// Config file path override.
    #[arg(short, long)]
    pub config: Option<String>,
}

pub async fn run(args: ConsoleArgs) -> anyhow::Result<()> {
    // replay-boot mode: just show boot log and exit
    if args.replay_boot {
        let mut client = connect_or_fail().await?;
        replay_boot_log(&mut client).await?;
        return Ok(());
    }

    // If --attach, connect to existing daemon; otherwise boot one first
    if args.attach {
        let mut client = connect_or_fail().await?;
        println!("{}", clawft_kernel::console::boot_banner());
        println!("  Attached to running kernel");
        println!();
        replay_boot_log(&mut client).await?;
        println!();
        run_repl(&mut client).await?;
    } else {
        // Boot the daemon in the background, then attach
        // Check if already running
        if DaemonClient::connect().await.is_some() {
            println!("Kernel already running -- attaching.");
        } else {
            // Start the daemon in the background
            crate::daemon::daemonize(args.config.as_deref())?;
            // Wait briefly for it to start
            for _ in 0..20 {
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                if DaemonClient::connect().await.is_some() {
                    break;
                }
            }
        }
        let mut client = connect_or_fail().await?;
        println!("{}", clawft_kernel::console::boot_banner());
        replay_boot_log(&mut client).await?;
        println!();
        run_repl(&mut client).await?;
    }

    Ok(())
}

async fn connect_or_fail() -> anyhow::Result<DaemonClient> {
    DaemonClient::connect()
        .await
        .ok_or_else(|| anyhow::anyhow!("no daemon running -- start with 'weaver kernel start'"))
}

async fn replay_boot_log(client: &mut DaemonClient) -> anyhow::Result<()> {
    let params = serde_json::json!({"count": 0}); // 0 = all
    let resp = client
        .call(Request::with_params("kernel.logs", params))
        .await?;
    if !resp.ok {
        println!("  (boot log unavailable)");
        return Ok(());
    }
    if let Some(result) = resp.result
        && let Some(entries) = result.as_array() {
            for entry in entries {
                let phase = entry.get("phase").and_then(|v| v.as_str()).unwrap_or("?");
                let message = entry.get("message").and_then(|v| v.as_str()).unwrap_or("");
                let level = entry.get("level").and_then(|v| v.as_str()).unwrap_or("info");
                if level == "debug" {
                    continue;
                }
                println!("  [{phase:<10}] {message}");
            }
        }
    Ok(())
}

/// Run the interactive REPL loop.
async fn run_repl(client: &mut DaemonClient) -> anyhow::Result<()> {
    let stdin = io::stdin();
    let mut reader = stdin.lock();

    loop {
        print!("weftos> ");
        io::stdout().flush()?;

        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break, // EOF
            Ok(_) => {}
            Err(e) => {
                eprintln!("read error: {e}");
                break;
            }
        }

        let input = line.trim();
        if input.is_empty() {
            continue;
        }

        match input {
            "exit" | "quit" | "q" => break,
            "help" | "?" => print_help(),
            "boot-log" => {
                if let Err(e) = replay_boot_log(client).await {
                    eprintln!("error: {e}");
                }
            }
            "status" => dispatch(client, "kernel.status").await,
            "ps" => dispatch(client, "kernel.ps").await,
            "services" => dispatch(client, "kernel.services").await,
            "logs" => dispatch(client, "kernel.logs").await,
            "chain status" => dispatch(client, "chain.status").await,
            "chain local" => {
                dispatch_params(
                    client,
                    "chain.local",
                    serde_json::json!({"count": 20}),
                )
                .await;
            }
            "chain verify" => dispatch(client, "chain.verify").await,
            "tree stats" => dispatch(client, "resource.stats").await,
            "ecc status" => dispatch(client, "ecc.status").await,
            "ecc tick" => dispatch(client, "ecc.tick").await,
            "ecc calibrate" => dispatch(client, "ecc.calibrate").await,
            _ => {
                // Try to parse as "namespace.method" RPC call
                if let Some((ns, cmd)) = input.split_once(' ') {
                    let method = format!("{ns}.{cmd}");
                    dispatch(client, &method).await;
                } else if input.contains('.') {
                    dispatch(client, input).await;
                } else {
                    eprintln!("unknown command: {input} (type 'help' for commands)");
                }
            }
        }
    }

    println!("Goodbye.");
    Ok(())
}

async fn dispatch(client: &mut DaemonClient, method: &str) {
    match client.simple_call(method).await {
        Ok(resp) => print_response(&resp),
        Err(e) => eprintln!("error: {e}"),
    }
}

async fn dispatch_params(client: &mut DaemonClient, method: &str, params: serde_json::Value) {
    match client.call(Request::with_params(method, params)).await {
        Ok(resp) => print_response(&resp),
        Err(e) => eprintln!("error: {e}"),
    }
}

fn print_response(resp: &Response) {
    if !resp.ok {
        eprintln!("error: {}", resp.error.as_deref().unwrap_or("unknown"));
        return;
    }
    if let Some(ref result) = resp.result
        && let Ok(pretty) = serde_json::to_string_pretty(result) {
            println!("{pretty}");
        }
}

fn print_help() {
    println!("WeftOS Console Commands:");
    println!();
    println!("  Kernel:");
    println!("    status          Show kernel state, uptime, counts");
    println!("    ps              List process table");
    println!("    services        List registered services");
    println!("    logs            Show kernel event log");
    println!("    boot-log        Replay boot events");
    println!();
    println!("  Chain:");
    println!("    chain status    Show chain status");
    println!("    chain local     List recent chain events");
    println!("    chain verify    Verify chain integrity");
    println!();
    println!("  Resources:");
    println!("    tree stats      Show resource tree statistics");
    println!();
    println!("  ECC:");
    println!("    ecc status      Show ECC subsystem status");
    println!("    ecc tick        Show cognitive tick statistics");
    println!("    ecc calibrate   Re-run boot calibration");
    println!();
    println!("  General:");
    println!("    help            Show this help");
    println!("    exit / quit     Exit console");
    println!();
    println!("  Any RPC method can be called directly: <method>");
    println!("    Example: kernel.status, agent.list, chain.status");
}
