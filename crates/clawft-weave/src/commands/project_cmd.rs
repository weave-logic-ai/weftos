//! `weaver project` (ADR-103 A6, Phase 2 package G): operator commands for
//! per-project kernels. `weft project` keeps the registration commands.

use clap::{Parser, Subcommand};

use crate::project_migrate::{self, Action, MigrateError};

/// Project operator subcommand.
#[derive(Parser)]
#[command(about = "Per-project kernel operations (migrate-kernel)")]
pub struct ProjectArgs {
    /// Project subcommand.
    #[command(subcommand)]
    pub action: ProjectAction,
}

/// Project subcommands.
#[derive(Subcommand)]
pub enum ProjectAction {
    /// Opt one project into a supervised child kernel (or back out).
    ///
    /// Refuses while the project's own daemon runs (it is never signalled),
    /// copies workloads.json and apps.json into <root>/.weftos/state/,
    /// sets `[serve] via = "child-kernel"` and prints the rollback line.
    #[command(name = "migrate-kernel")]
    MigrateKernel {
        /// Project id or unique registered name.
        project: String,
        /// Print what would change; change nothing.
        #[arg(long)]
        dry_run: bool,
        /// Set the project back to the user daemon (copies are left).
        #[arg(long)]
        revert: bool,
    },
}

/// Run the project subcommand.
pub async fn run(args: ProjectArgs) -> anyhow::Result<()> {
    match args.action {
        ProjectAction::MigrateKernel { project, dry_run, revert } => migrate(&project, dry_run, revert),
    }
}

fn migrate(project: &str, dry_run: bool, revert: bool) -> anyhow::Result<()> {
    let home = clawft_types::runtime_paths::home_dir()
        .ok_or_else(|| anyhow::anyhow!("cannot determine the home directory"))?;
    let mdir = crate::user_daemon::manifests_dir(&home);
    let explain = |e: MigrateError| anyhow::anyhow!("{e}");
    if revert {
        let m = project_migrate::revert(&home, &mdir, project, dry_run).map_err(explain)?;
        println!(
            "{}project {} ({}): [serve] via = \"user-daemon\"",
            if dry_run { "(dry run) would set " } else { "set " },
            m.id,
            m.name
        );
        return Ok(());
    }
    let plan = project_migrate::plan(&mdir, project).map_err(explain)?;
    let m = &plan.manifest;
    println!("project {} ({}) at {}", m.id, m.name, m.root.display());
    for a in &plan.actions {
        match a {
            Action::Copy { from, to } => println!("  copy {} -> {}", from.display(), to.display()),
            Action::KeepExisting { to } => {
                println!("  keep {} (exists and differs; not overwritten)", to.display());
            }
            Action::SetVia(v) => println!("  set [serve] via = {v:?} in the manifest"),
        }
    }
    if dry_run {
        println!("(dry run: nothing changed)");
        return Ok(());
    }
    project_migrate::apply(&mdir, &plan).map_err(explain)?;
    println!("done. start it with `weaver kernel start --project {}`", m.id);
    println!("{}", project_migrate::rollback_line(m));
    Ok(())
}
