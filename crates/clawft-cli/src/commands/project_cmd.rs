//! `weft project` -- project identity (ADR-103 D5/D14).
//!
//! `init` gives the current tree a ULID (`.weftos/project.toml`) and registers
//! it in the user-level manifest index; `list` / `show` read that index
//! directly (no daemon needed); `seed` imports `~/.clawft/workspaces.json`.
//!
//! Clone policy: a same-machine copy of a registered tree is refused with
//! `RootConflict`; `weft project init --fork` gives the copy its own identity.
//!
//! The manifests dir is `~/.weftos/projects`, overridable with
//! `WEFTOS_MANIFESTS_DIR` (used by the tests; also handy for scratch indexes).

use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use clap::{Args, Subcommand};
use comfy_table::{Table, presets::UTF8_FULL};

use clawft_rpc::DaemonClient;
use clawft_rpc::resolve::{ResolveFlags, resolve};
use clawft_types::project::{
    PROJECT_DIR, ProjectManifest, ProjectState, SeedReport, adopt_or_init, find_by_id,
    find_project_toml, list_manifests, reinit_fork, seed_from_workspaces,
};

/// Env override for the manifests dir.
pub const MANIFESTS_DIR_ENV: &str = "WEFTOS_MANIFESTS_DIR";

/// Arguments for `weft project`.
#[derive(Args)]
pub struct ProjectArgs {
    #[command(subcommand)]
    pub action: ProjectAction,
}

/// `weft project` subcommands.
#[derive(Subcommand)]
pub enum ProjectAction {
    /// Give the current tree a project identity and register it.
    Init {
        /// Project name (defaults to the directory name).
        #[arg(long)]
        name: Option<String>,

        /// This tree is a copy of another project: mint a new identity for it.
        #[arg(long)]
        fork: bool,

        /// With --fork: also re-identify a tree that is the registered home
        /// of its id (archives that manifest).
        #[arg(long, requires = "fork")]
        force: bool,
    },

    /// List registered projects.
    List {
        /// Machine-readable output.
        #[arg(long)]
        json: bool,
    },

    /// Show one project (default: the one containing the current directory).
    Show {
        /// Project ULID, name, or `.` for the current directory.
        target: Option<String>,

        /// Show the project containing the current directory.
        #[arg(long, conflicts_with = "target")]
        here: bool,

        /// Machine-readable output.
        #[arg(long)]
        json: bool,
    },

    /// Import ~/.clawft/workspaces.json into the project index.
    Seed,
}

/// Where everything lives, injected so tests never touch the real home.
pub struct Env {
    pub home: PathBuf,
    pub manifests_dir: PathBuf,
    pub cwd: PathBuf,
}

impl Env {
    /// Read HOME, the manifests override and the cwd from the process.
    pub fn from_process() -> anyhow::Result<Self> {
        let home = clawft_types::runtime_paths::home_dir()
            .context("cannot determine the home directory")?;
        let manifests_dir = match std::env::var_os(MANIFESTS_DIR_ENV).filter(|v| !v.is_empty()) {
            Some(p) => PathBuf::from(p),
            None => clawft_rpc::resolve::manifests_dir(&home),
        };
        Ok(Self {
            home,
            manifests_dir,
            cwd: std::env::current_dir().context("cannot read the current directory")?,
        })
    }
}

/// Run `weft project ...`.
pub async fn run(args: ProjectArgs) -> anyhow::Result<()> {
    let env = Env::from_process()?;
    let out = match args.action {
        ProjectAction::Init { name, fork, force } => init(&env, name.as_deref(), fork, force)?,
        ProjectAction::List { json } => list(&env, json)?,
        ProjectAction::Show { target, here, json } => {
            let m = lookup(&env, target.as_deref(), here)?;
            let live = live_handshake(&m.id).await;
            render_show(&m, live, json)?
        }
        ProjectAction::Seed => seed(&env)?,
    };
    println!("{out}");
    Ok(())
}

fn canon(p: &Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}

/// Why `root` can never be a project root, if so. A project rooted at
/// `$HOME`, above it, or in the tool's own state directories would claim
/// trees that are not one project.
fn forbidden_root(root: &Path, home: &Path) -> Option<String> {
    let why = if root.parent().is_none() {
        "the filesystem root"
    } else if root == home {
        "your home directory"
    } else if home.starts_with(root) {
        "a parent of your home directory"
    } else if root.starts_with(home.join(".weftos")) || root.starts_with(home.join(".clawft")) {
        "inside the WeftOS state directories (~/.weftos, ~/.clawft)"
    } else {
        return None;
    };
    Some(format!(
        "refusing to create a project at {} ({why}); cd into a project directory first",
        root.display()
    ))
}

/// Root of the project containing `cwd`, or `cwd` itself when none exists,
/// plus a note when that root is an ancestor of the cwd.
fn project_root(env: &Env) -> anyhow::Result<(PathBuf, Option<String>)> {
    let home = canon(&env.home);
    let cwd = canon(&env.cwd);
    let found = find_project_toml(&env.cwd, Some(&home));
    let root = found.clone().map(|r| canon(&r)).unwrap_or_else(|| cwd.clone());
    if let Some(msg) = forbidden_root(&root, &home) {
        bail!("{msg}");
    }
    let note = (root != cwd).then(|| {
        format!(
            "note: acting on the enclosing project root {} (not the current directory)",
            root.display()
        )
    });
    Ok((root, note))
}

/// `weft project init`.
pub fn init(env: &Env, name: Option<&str>, fork: bool, force: bool) -> anyhow::Result<String> {
    let (root, note) = project_root(env)?;
    if let Some(n) = note {
        eprintln!("{n}");
    }
    let m = if fork {
        reinit_fork(&root, &env.manifests_dir, name, force)
    } else {
        adopt_or_init(&root, &env.manifests_dir, name)
    }
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    let ignored = match update_gitignore(&m.root) {
        Ok(added) => added,
        Err(e) => {
            eprintln!("warning: could not update .gitignore: {e}");
            Vec::new()
        }
    };
    let mut out = format!(
        "project {} ({})\n  id:       {}\n  root:     {}\n  identity: {}\n  manifest: {}",
        m.name,
        if fork { "forked" } else { "initialised" },
        m.id,
        m.root.display(),
        m.root.join(PROJECT_DIR).join("project.toml").display(),
        env.manifests_dir.join(format!("{}.toml", m.id)).display(),
    );
    if !ignored.is_empty() {
        out.push_str(&format!("\n  .gitignore: added {}", ignored.join(", ")));
    }
    Ok(out)
}

/// Append the project-kernel files that must never be committed (chain,
/// key, certificate mirror, child state) to an existing `.gitignore` (never
/// creates one). `overlay.toml` is meant to be committed and is not listed.
/// Returns the lines added.
fn update_gitignore(root: &Path) -> Result<Vec<String>, String> {
    let path = root.join(".gitignore");
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let missing: Vec<String> = [
        ".weftos/chain/",
        ".weftos/project.key",
        ".weftos/project.cert.json",
        ".weftos/state/",
    ]
        .into_iter()
        .filter(|l| !text.lines().any(|t| t.trim() == *l))
        .map(String::from)
        .collect();
    if missing.is_empty() {
        return Ok(missing);
    }
    let mut next = text;
    if !next.ends_with('\n') && !next.is_empty() {
        next.push('\n');
    }
    for l in &missing {
        next.push_str(l);
        next.push('\n');
    }
    std::fs::write(&path, next).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(missing)
}

fn root_exists(m: &ProjectManifest) -> bool {
    m.root.exists()
}

fn state_label(m: &ProjectManifest) -> &'static str {
    match m.state {
        ProjectState::Archived => "archived",
        ProjectState::Missing => "missing",
        ProjectState::Active if !root_exists(m) => "missing",
        ProjectState::Active => "active",
    }
}

/// `weft project list`.
pub fn list(env: &Env, json: bool) -> anyhow::Result<String> {
    let listing = list_manifests(&env.manifests_dir).map_err(|e| anyhow::anyhow!("{e}"))?;
    for (path, why) in &listing.skipped {
        eprintln!("warning: skipped {}: {why}", path.display());
    }
    if json {
        let rows: Vec<_> = listing
            .manifests
            .iter()
            .map(|m| {
                let mut v = serde_json::to_value(m)?;
                v["root_exists"] = root_exists(m).into();
                v["status"] = state_label(m).into();
                Ok(v)
            })
            .collect::<anyhow::Result<_>>()?;
        return Ok(serde_json::to_string_pretty(&rows)?);
    }
    if listing.manifests.is_empty() {
        return Ok(
            "No projects registered.\n  `weft project init` registers the current \
                   directory; `weft project seed` imports ~/.clawft/workspaces.json."
                .into(),
        );
    }
    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_header(["ID", "NAME", "STATUS", "ROOT"]);
    for m in &listing.manifests {
        table.add_row([
            m.id.clone(),
            m.name.clone(),
            state_label(m).to_owned(),
            m.root.display().to_string(),
        ]);
    }
    Ok(format!("{table}\n  {} project(s)", listing.manifests.len()))
}

/// Find the manifest `weft project show` was asked about.
pub fn lookup(env: &Env, target: Option<&str>, here: bool) -> anyhow::Result<ProjectManifest> {
    let by_cwd = |env: &Env| -> anyhow::Result<ProjectManifest> {
        let root = find_project_toml(&env.cwd, Some(&canon(&env.home))).ok_or_else(|| {
            anyhow::anyhow!(
                "{} is not inside a project; run `weft project init` or name one \
                 (see `weft project list`)",
                env.cwd.display()
            )
        })?;
        clawft_types::project::find_by_root(&env.manifests_dir, &root)
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "{} has a project.toml but is not registered; run `weft project init`",
                    root.display()
                )
            })
    };
    let target = match (target, here) {
        (_, true) | (None, _) | (Some("."), _) => return by_cwd(env),
        (Some(t), false) => t,
    };
    if clawft_types::project::validate_id(target).is_ok()
        && let Some(m) =
            find_by_id(&env.manifests_dir, target).map_err(|e| anyhow::anyhow!("{e}"))?
    {
        return Ok(m);
    }
    let all = list_manifests(&env.manifests_dir).map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut hits: Vec<_> = all
        .manifests
        .into_iter()
        .filter(|m| m.name == target)
        .collect();
    match hits.len() {
        0 => bail!("no project with id or name {target:?} (see `weft project list`)"),
        1 => Ok(hits.remove(0)),
        _ => bail!(
            "name {target:?} matches {} projects; use the id instead:\n{}",
            hits.len(),
            hits.iter()
                .map(|m| format!("  {}  {}", m.id, m.root.display()))
                .collect::<Vec<_>>()
                .join("\n")
        ),
    }
}

/// Ask the daemon for its handshake, as seen by a client scoped to `id`.
/// `Err` carries the operator-facing reason (including what was tried).
async fn live_handshake(id: &str) -> Result<clawft_rpc::Handshake, String> {
    let g = super::daemon_conn::flags();
    let flags = ResolveFlags {
        runtime: g.runtime,
        project: Some(id.to_owned()),
    };
    let res = resolve(&flags).map_err(|e| e.to_string())?;
    match DaemonClient::connect_resolved(&res).await {
        Ok(c) => Ok(c.handshake),
        Err(e) => Err(super::daemon_conn::describe(&e)),
    }
}

/// Render `show` output. `live` is the handshake or why there is none.
pub fn render_show(
    m: &ProjectManifest,
    live: Result<clawft_rpc::Handshake, String>,
    json: bool,
) -> anyhow::Result<String> {
    if json {
        let mut v = serde_json::to_value(m)?;
        v["root_exists"] = root_exists(m).into();
        v["status"] = state_label(m).into();
        v["daemon"] = match &live {
            Ok(h) => serde_json::to_value(h)?,
            Err(why) => serde_json::json!({ "error": why }),
        };
        return Ok(serde_json::to_string_pretty(&v)?);
    }
    let mut out = format!(
        "{name}\n  id:       {id}\n  root:     {root}\n  status:   {status}\n  created:  {created}\n  \
         chain:    {chain}\n  identity: {ident}",
        name = m.name,
        id = m.id,
        root = m.root.display(),
        status = state_label(m),
        created = m.created.to_rfc3339(),
        chain = m.chain_dir().display(),
        ident = match m.project_toml {
            clawft_types::project::ProjectTomlPresence::Present => "project.toml present",
            clawft_types::project::ProjectTomlPresence::Pending =>
                "pending (run `weft project init` in the root to write project.toml)",
        },
    );
    if let Some(s) = &m.seed {
        out.push_str(&format!("\n  seeded:   {}", s.source));
    }
    match live {
        Ok(h) => out.push_str(&format!(
            "\n  daemon:   pid {} node {} sha {} runtime {}\n            serves project: {}",
            h.pid,
            h.node_id,
            h.sha,
            h.runtime_dir,
            h.project_id.as_deref().unwrap_or("none"),
        )),
        Err(why) => out.push_str(&format!("\n  daemon:   not verified\n{}", indent(&why))),
    }
    Ok(out)
}

fn indent(s: &str) -> String {
    s.lines()
        .map(|l| format!("            {l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Render a [`SeedReport`].
pub fn render_seed(r: &SeedReport) -> String {
    let mut out = format!(
        "seeded: {} created, {} adopted, {} missing, {} unchanged, {} skipped",
        r.created.len(),
        r.adopted.len(),
        r.missing.len(),
        r.unchanged.len(),
        r.skipped.len()
    );
    for (path, why) in &r.skipped {
        out.push_str(&format!("\n  skipped {}: {why}", path.display()));
    }
    out
}

/// `weft project seed`.
pub fn seed(env: &Env) -> anyhow::Result<String> {
    let src = env.home.join(".clawft").join("workspaces.json");
    let report =
        seed_from_workspaces(&src, &env.manifests_dir).map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(format!("from {}\n{}", src.display(), render_seed(&report)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forbidden_roots() {
        let home = Path::new("/Users/me");
        for (root, frag) in [
            ("/", "filesystem root"),
            ("/Users/me", "home directory"),
            ("/Users", "parent of your home"),
            ("/Users/me/.weftos", "state directories"),
            ("/Users/me/.weftos/projects/x", "state directories"),
            ("/Users/me/.clawft/ws", "state directories"),
        ] {
            let m = forbidden_root(Path::new(root), home).unwrap_or_else(|| panic!("{root}"));
            assert!(m.contains(frag), "{root}: {m}");
        }
        for ok in ["/Users/me/dev/x", "/Users/me/.config/x", "/srv/x", "/Users/meh"] {
            assert!(forbidden_root(Path::new(ok), home).is_none(), "{ok}");
        }
    }
}
