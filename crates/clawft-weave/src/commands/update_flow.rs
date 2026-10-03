//! `weaver update` orchestration: discover, decide, verify, install, restart.
//!
//! Every outside effect is a field of [`Ctx`] (release source, doctor
//! environment, daemon host, prompt), so tests run it against a loopback mock
//! release server and fake receipts in temp dirs. Service managers are only
//! reached through [`Host`]; this module never spawns `launchctl`,
//! `systemctl` or `kill` itself.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::bail;
use clawft_rpc::doctor::DoctorEnv;
use clawft_rpc::doctor::channel::{Sources, remedy_for};
use clawft_rpc::doctor::install::scan;
use clawft_rpc::doctor::probe::parse_semver;

use super::daemon_restart::{Host, Inputs, restart_with};
use super::update_install::{self as install, Decision, Inject, Method, Plan};
use super::update_release::{self as release, Release, Source};
use crate::service_units::{LAUNCHD_LABEL, SYSTEMD_UNIT};

/// What the user asked for.
#[derive(Debug, Clone, Copy, Default)]
pub struct Opts {
    /// Report only; install nothing.
    pub check: bool,
    /// Show what would be replaced; download and install nothing.
    pub dry_run: bool,
    /// Reinstall even when already on (or past) the latest release.
    pub force: bool,
    /// Restart the user daemon without asking.
    pub restart: bool,
    /// Never restart or ask; print the command.
    pub no_restart: bool,
}

/// Everything the update reads from the outside world.
pub struct Ctx<'a> {
    pub src: Source,
    pub triple: String,
    pub current_version: String,
    pub current_exe: PathBuf,
    /// The running build is `-dirty` (a source build).
    pub dirty: bool,
    pub env: DoctorEnv,
    /// Daemon paths; `None` skips the restart step.
    pub restart_base: Option<Inputs>,
    pub host: &'a dyn Host,
    /// stdin and stdout are terminals, so a question can be asked.
    pub interactive: bool,
    pub prompt: &'a dyn Fn(&str) -> bool,
    /// Running as root through `sudo`: `HOME` and the uid are root's, not the user's.
    pub sudo_root: bool,
    /// Tests: inject install failures.
    pub inject: Inject,
}

/// How the run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    UpToDate,
    /// `--check` found a newer release.
    Available { version: String },
    DryRun,
    /// Not ours to update; nothing changed.
    Refused { command: String },
    Installed { version: String, daemon_restarted: bool },
}

fn manual_restart_cmd(ctx: &Ctx<'_>, pid: Option<u32>) -> String {
    let (Some(base), Some(pid)) = (&ctx.restart_base, pid) else {
        return "weaver kernel stop && weaver kernel start".into();
    };
    if ctx.host.launchd_main_pid(base.uid) == Some(pid) {
        format!("launchctl kickstart -k gui/{}/{LAUNCHD_LABEL}", base.uid)
    } else if ctx.host.systemd_main_pid() == Some(pid) {
        format!("systemctl --user restart {SYSTEMD_UNIT}")
    } else {
        "weaver kernel stop && weaver kernel start".into()
    }
}

fn live_daemon_pid(ctx: &Ctx<'_>) -> Option<u32> {
    let base = ctx.restart_base.as_ref()?;
    let pid: u32 = std::fs::read_to_string(&base.pid_file).ok()?.split_whitespace().next()?.parse().ok()?;
    (pid > 1 && ctx.host.alive(pid)).then_some(pid)
}

/// Returns whether the daemon was restarted.
fn restart_step(ctx: &Ctx<'_>, opts: &Opts, weaver: &Path, out: &mut dyn Write) -> anyhow::Result<bool> {
    let Some(base) = &ctx.restart_base else {
        writeln!(out, "Restart any running daemon yourself: weaver kernel stop && weaver kernel start")?;
        return Ok(false);
    };
    let pid = live_daemon_pid(ctx);
    let cmd = manual_restart_cmd(ctx, pid);
    let mut inputs = base.clone();
    inputs.installed_exe = weaver.to_path_buf();
    let Some(pid) = pid else {
        for l in restart_with(&inputs, ctx.host).lines(&cmd) {
            writeln!(out, "{l}")?;
        }
        return Ok(false);
    };
    let go = if ctx.sudo_root {
        writeln!(out, "Not restarting: this is running as root via sudo, so the daemon found is root's, not yours.")?;
        false
    } else if opts.no_restart {
        false
    } else if opts.restart {
        true
    } else if ctx.interactive {
        (ctx.prompt)(&format!("The user daemon (pid {pid}) is still running the old build. Restart it now?"))
    } else {
        false
    };
    if !go {
        writeln!(out, "The user daemon (pid {pid}) is still running the old build. Restart it:")?;
        writeln!(out, "  {cmd}")?;
        return Ok(false);
    }
    let report = restart_with(&inputs, ctx.host);
    for l in report.lines(&cmd) {
        writeln!(out, "{l}")?;
    }
    Ok(matches!(report.outcome, super::daemon_restart::Outcome::Restarted { .. }))
}

fn untouched_report(ctx: &Ctx<'_>, plan: &Plan, out: &mut dyn Write) -> anyhow::Result<()> {
    let updated: Vec<PathBuf> = plan
        .dests
        .iter()
        .map(|d| std::fs::canonicalize(&d.path).unwrap_or_else(|_| d.path.clone()))
        .collect();
    let other: Vec<_> = scan(&ctx.env).into_iter().filter(|c| !updated.contains(&c.canonical)).collect();
    if other.is_empty() {
        return Ok(());
    }
    writeln!(out)?;
    writeln!(out, "Other copies on this machine were not touched:")?;
    let sources = Sources::load(&ctx.env);
    for c in other {
        let ch = sources.detect(&c.path, &c.canonical, &c.name, c.dirty);
        writeln!(out, "  {} {}{}", c.path.display(), c.display_version(), if c.winner { " (first on PATH)" } else { "" })?;
        writeln!(out, "    update it with: {}", remedy_for(&ch, &c.name))?;
    }
    Ok(())
}

fn print_plan(plan: &Plan, rel: &Release, out: &mut dyn Write) -> anyhow::Result<()> {
    let how = match &plan.method {
        Method::Receipt(p) => format!("release receipt {}", p.display()),
        Method::Unmanaged => format!("no receipt; binaries next to weaver in {}", plan.dir.display()),
    };
    writeln!(out, "Install method: {how}")?;
    for d in &plan.dests {
        let from = d.installed.as_deref().map_or("not installed".to_string(), |v| format!("v{v}"));
        writeln!(out, "  {} : {from} -> v{}", d.path.display(), rel.version)?;
    }
    for n in &plan.notes {
        writeln!(out, "  note: {n}")?;
    }
    Ok(())
}

/// Run the update.
pub fn execute(ctx: &Ctx<'_>, opts: &Opts, out: &mut dyn Write) -> anyhow::Result<Outcome> {
    let scratch = tempfile::tempdir()?;
    let rel = release::fetch_latest(&ctx.src, &ctx.triple, scratch.path())?;
    writeln!(out, "Current: v{}", ctx.current_version)?;
    writeln!(out, "Latest:  v{}", rel.version)?;
    writeln!(out, "Platform: {}", ctx.triple)?;

    let (cur, new) = (parse_semver(&ctx.current_version), parse_semver(&rel.version));
    let newer = match (&cur, &new) {
        (Some(c), Some(n)) => n > c,
        _ => rel.version != ctx.current_version,
    };
    let names = rel.binaries();
    let decision = install::decide(&ctx.env, &ctx.current_exe, ctx.dirty, &names);

    if !newer && !opts.force {
        let ahead = matches!((&cur, &new), (Some(c), Some(n)) if c > n);
        writeln!(out, "{}", if ahead { "This build is newer than the latest release; not downgrading. Use --force to reinstall." } else { "You are up to date. Use --force to reinstall." })?;
        return Ok(Outcome::UpToDate);
    }
    let plan = match decision {
        Decision::Proceed(p) => p,
        Decision::Refuse { reason, command } => {
            writeln!(out)?;
            writeln!(out, "Not updating: {reason}.")?;
            writeln!(out, "Update it with:\n  {command}")?;
            return Ok(Outcome::Refused { command });
        }
    };
    let ahead = matches!((&cur, &new), (Some(c), Some(n)) if c > n);
    if opts.check {
        writeln!(out, "Update available: v{} -> v{}. Run: weaver update", ctx.current_version, rel.version)?;
        return Ok(Outcome::Available { version: rel.version });
    }
    writeln!(out)?;
    print_plan(&plan, &rel, out)?;
    if opts.dry_run {
        writeln!(out, "Dry run: nothing downloaded or installed.")?;
        return Ok(Outcome::DryRun);
    }
    if ahead {
        writeln!(out, "warning: downgrading from v{} to v{}", ctx.current_version, rel.version)?;
    }
    if plan.method == Method::Unmanaged && !opts.force {
        let q = format!(
            "No install receipt found, so nothing records who installed the binaries in {}. Overwrite them?",
            plan.dir.display()
        );
        if !(ctx.interactive && (ctx.prompt)(&q)) {
            writeln!(out, "Not updating: no install receipt; the binaries in {} may belong to something else.", plan.dir.display())?;
            writeln!(out, "If you installed them from a release archive, re-run:\n  weaver update --force")?;
            return Ok(Outcome::Refused { command: "weaver update --force".into() });
        }
    }
    install::check_writable(&plan.dir)?;
    writeln!(out)?;
    let staging = tempfile::tempdir()?;
    let staged = release::stage(&ctx.src, &rel, staging.path(), out)?;
    for d in &plan.dests {
        if !staged.iter().any(|s| s.name == d.name) {
            bail!("release did not provide {}", d.name);
        }
    }
    let replaced = install::apply(&plan, &staged, ctx.inject)?;
    if let Method::Receipt(p) = &plan.method
        && let Err(e) = install::update_receipt_version(p, &rel.version)
    {
        writeln!(out, "warning: could not record v{} in {}: {e}", rel.version, p.display())?;
    }
    writeln!(out)?;
    for r in &replaced {
        writeln!(out, "  updated {} to v{}", r.path.display(), rel.version)?;
    }
    writeln!(out, "Update complete: {} binaries are now v{}.", replaced.len(), rel.version)?;
    writeln!(out)?;
    let weaver = replaced.iter().find(|r| r.name == "weaver").map(|r| r.path.clone()).unwrap_or_default();
    let daemon_restarted = restart_step(ctx, opts, &weaver, out)?;
    untouched_report(ctx, &plan, out)?;
    Ok(Outcome::Installed { version: rel.version, daemon_restarted })
}
