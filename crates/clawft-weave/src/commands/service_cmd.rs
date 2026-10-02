//! `weaver service unit` — print or write a launchd / systemd user unit for
//! the per-user daemon (ADR-103 Phase 1, package H).
//!
//! This command never runs `launchctl` or `systemctl`. It prints the unit,
//! or writes it with `--out`, and prints the install commands as text for
//! the operator to run.

use std::io::Write;
use std::path::{Path, PathBuf};

use clap::{Args, Subcommand, ValueEnum};
use clawft_types::runtime_paths::home_dir;

use crate::service_units::{UnitKind, stable_exe};

/// `weaver service` arguments.
#[derive(Debug, Args)]
pub struct ServiceArgs {
    #[command(subcommand)]
    pub action: ServiceAction,
}

/// Service subcommands.
#[derive(Debug, Subcommand)]
pub enum ServiceAction {
    /// Print a service unit for the per-user daemon (never installs it).
    Unit {
        /// Service manager the unit is for.
        #[arg(long, value_enum)]
        kind: KindArg,
        /// Write the unit to this file instead of printing it.
        #[arg(long, value_name = "FILE")]
        out: Option<PathBuf>,
        /// Overwrite an existing `--out` file.
        #[arg(long, requires = "out")]
        force: bool,
    },
}

/// `--kind` values.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum KindArg {
    Launchd,
    Systemd,
}

impl From<KindArg> for UnitKind {
    fn from(k: KindArg) -> Self {
        match k {
            KindArg::Launchd => UnitKind::Launchd,
            KindArg::Systemd => UnitKind::Systemd,
        }
    }
}

pub async fn run(args: ServiceArgs) -> anyhow::Result<()> {
    match args.action {
        ServiceAction::Unit { kind, out, force } => {
            let home = home_dir().ok_or_else(|| anyhow::anyhow!("cannot determine the home directory"))?;
            let exe = stable_exe(&std::env::current_exe()?, std::env::var_os("PATH").as_deref());
            let mut stdout = std::io::stdout().lock();
            unit(kind.into(), &exe, &home, out.as_deref(), force, &mut stdout)
        }
    }
}

/// Print the unit to `w`, or write it to `out` and print the install
/// commands to `w`. Refuses to overwrite `out` without `force`.
pub fn unit(
    kind: UnitKind,
    exe: &Path,
    home: &Path,
    out: Option<&Path>,
    force: bool,
    w: &mut dyn Write,
) -> anyhow::Result<()> {
    let text = kind.render(exe, home).map_err(|e| anyhow::anyhow!(e))?;
    let Some(path) = out else {
        w.write_all(text.as_bytes())?;
        return Ok(());
    };
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true);
    if force {
        opts.create(true).truncate(true);
    } else {
        opts.create_new(true);
    }
    let mut f = opts.open(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            anyhow::anyhow!("{} exists; pass --force to overwrite", path.display())
        } else {
            anyhow::anyhow!("cannot write {}: {e}", path.display())
        }
    })?;
    f.write_all(text.as_bytes())?;
    writeln!(w, "Wrote {}", path.display())?;
    writeln!(w, "To install, run (this command does not run them):")?;
    for c in kind.install_commands(path, home) {
        writeln!(w, "  {c}")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prints_by_default_and_writes_nothing() {
        let mut buf = Vec::new();
        unit(UnitKind::Systemd, Path::new("/x/weaver"), Path::new("/h"), None, false, &mut buf).unwrap();
        assert!(String::from_utf8(buf).unwrap().contains("ExecStart=\"/x/weaver\""));
    }

    #[test]
    fn out_refuses_overwrite_without_force() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("u.plist");
        std::fs::write(&f, "keep").unwrap();
        let mut buf = Vec::new();
        let err = unit(UnitKind::Launchd, Path::new("/x/weaver"), Path::new("/h"), Some(&f), false, &mut buf)
            .unwrap_err();
        assert!(err.to_string().contains("--force"));
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "keep");
        unit(UnitKind::Launchd, Path::new("/x/weaver"), Path::new("/h"), Some(&f), true, &mut buf).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("launchctl bootstrap gui/$UID"));
        assert!(std::fs::read_to_string(&f).unwrap().contains("ai.weftos.user"));
    }
}
