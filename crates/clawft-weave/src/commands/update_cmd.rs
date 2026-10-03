//! `weaver update` / `weft update` — verified, receipt-aware self-update.
//!
//! Fetches the latest GitHub Release, verifies every archive against its
//! published sha256 and `dist-manifest.json`, then replaces every binary of
//! the release set together, with rollback. It refuses to touch Homebrew,
//! `cargo install` and source-build copies and prints their own update
//! command instead. See [`super::update_flow`] for the sequence and
//! `docs/guides/updating.md` for the user-facing behaviour.

use std::io::{IsTerminal, Write};

use clap::{Args, Subcommand};
use clawft_rpc::doctor::DoctorEnv;

use super::daemon_restart::{self, RealHost};
use super::update_flow::{Ctx, Opts, Outcome, execute};
use super::update_release::Source;

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Flags shared by `weaver update` and `weaver update install`.
#[derive(Debug, Clone, Copy, Default, Args)]
pub struct UpdateFlags {
    /// Report whether an update exists and how it would be applied; install nothing.
    #[arg(long)]
    pub check: bool,
    /// Show which binaries would be replaced; download and install nothing.
    #[arg(long)]
    pub dry_run: bool,
    /// Reinstall even if already on the latest release.
    #[arg(long)]
    pub force: bool,
    /// Restart the per-user daemon after installing, without asking.
    #[arg(long, conflicts_with = "no_restart")]
    pub restart: bool,
    /// Never restart or ask; print the restart command.
    #[arg(long)]
    pub no_restart: bool,
}

/// `weaver update` arguments.
#[derive(Debug, Args)]
pub struct UpdateArgs {
    #[command(subcommand)]
    pub cmd: Option<UpdateCmd>,
    #[command(flatten)]
    pub flags: UpdateFlags,
}

/// Update subcommands (kept for compatibility; the flags work without them).
#[derive(Debug, Subcommand)]
pub enum UpdateCmd {
    /// Same as `weaver update --check`.
    Check,
    /// Same as `weaver update`.
    Install {
        #[command(flatten)]
        flags: UpdateFlags,
    },
}

pub async fn run(args: UpdateArgs) -> anyhow::Result<()> {
    let flags = match args.cmd {
        Some(UpdateCmd::Check) => UpdateFlags { check: true, ..args.flags },
        Some(UpdateCmd::Install { flags }) => flags,
        None => args.flags,
    };
    run_with(flags)
}

fn ask(question: &str) -> bool {
    print!("{question} [y/N] ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).is_ok() && matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// root reached through `sudo` (`SUDO_USER` set): HOME and uid are root's.
fn running_as_sudo_root() -> bool {
    #[cfg(unix)]
    {
        nix::unistd::geteuid().is_root() && std::env::var_os("SUDO_USER").is_some()
    }
    #[cfg(not(unix))]
    {
        false
    }
}

fn run_with(flags: UpdateFlags) -> anyhow::Result<()> {
    let exe = std::env::current_exe()?;
    let ctx = Ctx {
        src: Source::github(),
        triple: detect_target_triple().to_string(),
        current_version: CURRENT_VERSION.to_string(),
        dirty: option_env!("BUILD_VERSION").is_some_and(|v| v.contains("-dirty")),
        env: DoctorEnv::detect(),
        restart_base: daemon_restart::user_inputs(exe.clone()),
        current_exe: exe,
        host: &RealHost,
        interactive: std::io::stdin().is_terminal() && std::io::stdout().is_terminal(),
        prompt: &ask,
        sudo_root: running_as_sudo_root(),
        inject: Default::default(),
    };
    let opts = Opts {
        check: flags.check,
        dry_run: flags.dry_run,
        force: flags.force,
        restart: flags.restart,
        no_restart: flags.no_restart,
    };
    let outcome = execute(&ctx, &opts, &mut std::io::stdout())?;
    match outcome {
        Outcome::Refused { .. } => anyhow::bail!("update refused: this install is managed elsewhere"),
        Outcome::Installed { .. } => print_service_update_lines(),
        _ => {}
    }
    Ok(())
}

fn detect_target_triple() -> &'static str {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        // Check if musl or glibc
        if is_musl() {
            "x86_64-unknown-linux-musl"
        } else {
            "x86_64-unknown-linux-gnu"
        }
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        if is_musl() {
            "aarch64-unknown-linux-musl"
        } else {
            "aarch64-unknown-linux-gnu"
        }
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        "x86_64-apple-darwin"
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        "aarch64-apple-darwin"
    }
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        "x86_64-pc-windows-msvc"
    }
    #[cfg(not(any(
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "windows", target_arch = "x86_64"),
    )))]
    {
        "unknown"
    }
}

#[cfg(target_os = "linux")]
fn is_musl() -> bool {
    // Check if current binary is statically linked (musl)
    std::process::Command::new("ldd")
        .arg(std::env::current_exe().unwrap_or_default())
        .output()
        .map(|o| {
            let out = String::from_utf8_lossy(&o.stdout);
            out.contains("musl") || !o.status.success()
        })
        .unwrap_or(false)
}

/// Machine mesh service: print (never run) the sudo lines when its build
/// differs from this binary's. The service is root-owned and restarted by an
/// administrator; `weaver update` has no path to signal it.
fn print_service_update_lines() {
    #[cfg(all(unix, feature = "mesh"))]
    {
        use crate::install_tiers::{service_update_lines, Manager, ServiceObserved};
        let record = std::path::Path::new(crate::service_units_system::RUN_DIR).join("service.json");
        let observed = clawft_mesh_local::proto::ServiceRecord::load(&record)
            .ok()
            .map(|r| ServiceObserved { build_sha: r.build_sha });
        let exe = std::env::current_exe().unwrap_or_default();
        let lines = service_update_lines(Manager::host(), &exe, env!("BUILD_VERSION"), observed.as_ref());
        if !lines.is_empty() {
            println!();
            for l in lines {
                println!("{l}");
            }
        }
    }
}
