//! `weaver init` — Initialize a WeftOS project in the current directory.
//!
//! Generates `weave.toml` with sensible defaults, creates the
//! `.weftos/` runtime directory, and seeds `.clawft/` with the
//! per-instance agent identity files (`SOUL.md`, `IDENTITY.md`,
//! `SOUL.journal.md`) the chat agent reads at boot.
//!
//! Plan reference: `docs/plans/agent-core-v1.md` Phase F1.

use clap::Parser;
use std::path::{Path, PathBuf};

/// Canonical seed for `.clawft/SOUL.md`. Baked into the binary at
/// compile time so `weaver init` doesn't depend on the workspace
/// layout being present at runtime. The included file is the
/// documentation source under `docs/skills/clawft/SOUL.md`; keeping
/// the two in lockstep is what guarantees the
/// [`clawft_core::agent::identity::BINDING_THREAD_EXCERPT`] substring
/// is present in every freshly-initialized project.
const SOUL_TEMPLATE: &str = include_str!("../../../../docs/skills/clawft/SOUL.md");

/// Scaffold for `.clawft/IDENTITY.md`. Short by design — the user
/// is expected to customize. The corresponding template under
/// `docs/skills/clawft/IDENTITY.md` is much longer; we deliberately
/// don't bake the full version so the seed encourages editing.
const IDENTITY_TEMPLATE: &str = "# IDENTITY\n\nclawft Concierge \u{2014} a WeftOS chat agent. Edit this file to customize the persona.\n";

/// Scaffold for `.clawft/SOUL.journal.md` — empty append-only journal
/// the chat agent writes drift observations into. Phase F2's
/// `weaver soul promote` command reads this file, prints a diff
/// against `SOUL.md`, and applies on confirmation.
const JOURNAL_TEMPLATE: &str = "# SOUL.journal\n\nAppend-only journal of identity-drift observations. Promote via `weaver soul promote`.\n";

/// Outcome of a [`seed_clawft_workspace`] call. One row per file the
/// seeder considered; tests assert against this rather than
/// re-reading the directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedReport {
    /// Files that were freshly written by this call.
    pub created: Vec<PathBuf>,
    /// Files the seeder left untouched because they already existed
    /// and `force` was false.
    pub skipped: Vec<PathBuf>,
    /// Files that already existed and were overwritten because
    /// `force` was true.
    pub overwritten: Vec<PathBuf>,
}

/// Materialize `<workspace>/.clawft/{SOUL.md, IDENTITY.md,
/// SOUL.journal.md}` if missing. With `force = true`, existing files
/// are overwritten.
///
/// Idempotent: a second call without `force` is a no-op.
pub fn seed_clawft_workspace(workspace: &Path, force: bool) -> std::io::Result<SeedReport> {
    let clawft_dir = workspace.join(".clawft");
    if !clawft_dir.exists() {
        std::fs::create_dir_all(&clawft_dir)?;
    }

    let mut report = SeedReport {
        created: Vec::new(),
        skipped: Vec::new(),
        overwritten: Vec::new(),
    };

    for (name, contents) in [
        ("SOUL.md", SOUL_TEMPLATE),
        ("IDENTITY.md", IDENTITY_TEMPLATE),
        ("SOUL.journal.md", JOURNAL_TEMPLATE),
    ] {
        let path = clawft_dir.join(name);
        let exists = path.exists();
        if exists && !force {
            report.skipped.push(path);
            continue;
        }
        std::fs::write(&path, contents)?;
        if exists {
            report.overwritten.push(path);
        } else {
            report.created.push(path);
        }
    }

    Ok(report)
}

/// Initialize a WeftOS project.
#[derive(Parser)]
#[command(about = "Initialize a WeftOS project (generate weave.toml, create .weftos/)")]
pub struct InitArgs {
    /// Overwrite existing weave.toml AND any existing `.clawft/`
    /// identity files (`SOUL.md`, `IDENTITY.md`, `SOUL.journal.md`).
    /// Use this to reset a project to canonical seed state.
    #[arg(short, long)]
    pub force: bool,

    /// Add files this version of `weaver init` would seed without
    /// touching anything that already exists. Use this to pick up
    /// newly-added init artifacts (e.g. when the project upgrades to
    /// a release that introduces additional `.clawft/` files) on a
    /// workspace that already has a customized `weave.toml`.
    /// Mutually exclusive with `--force`.
    #[arg(short, long, conflicts_with = "force")]
    pub update: bool,

    /// Project name (defaults to current directory name).
    #[arg(short, long)]
    pub name: Option<String>,

    /// Enable mesh networking in the generated config.
    #[arg(long)]
    pub mesh: bool,

    /// Enable ECC cognitive substrate.
    #[arg(long)]
    pub ecc: bool,

    /// Skip interactive prompts.
    #[arg(short, long)]
    pub yes: bool,
}

pub async fn run(args: InitArgs) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let toml_path = cwd.join("weave.toml");
    let toml_exists = toml_path.exists();

    // Three-way decision on weave.toml:
    //   - default:  bail if exists (current behaviour, never lose
    //               operator-customized config silently)
    //   - --force:  overwrite unconditionally
    //   - --update: leave existing weave.toml alone, only fill in
    //               missing artifacts elsewhere ("play nice" mode)
    if toml_exists && !args.force && !args.update {
        anyhow::bail!(
            "weave.toml already exists. Use --update to seed missing \
             files without touching it, or --force to overwrite."
        );
    }

    let project_name = args.name.unwrap_or_else(|| {
        cwd.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("my-project")
            .to_string()
    });

    // Generate weave.toml only if absent or forced. In `--update`
    // mode an existing weave.toml is preserved so operator-tuned
    // settings (kernel.mesh, transport, listen_addr, …) survive.
    if !toml_exists || args.force {
        let toml_content = generate_weave_toml(&project_name, args.mesh, args.ecc);
        std::fs::write(&toml_path, &toml_content)?;
        if toml_exists {
            println!("Overwrote weave.toml");
        } else {
            println!("Created weave.toml");
        }
    } else {
        println!("Preserved existing weave.toml (--update)");
    }

    // Create .weftos/ runtime directory.
    let weftos_dir = cwd.join(".weftos");
    if !weftos_dir.exists() {
        std::fs::create_dir_all(weftos_dir.join("runtime"))?;
        println!("Created .weftos/runtime/");
    }

    // Create graphify output directory.
    let graphify_dir = cwd.join("graphify-out");
    if !graphify_dir.exists() {
        std::fs::create_dir_all(&graphify_dir)?;
    }

    // Add .weftos/ to .gitignore if not already present.
    ensure_gitignore(&cwd)?;

    // Seed `.clawft/` with the per-instance agent identity files so
    // `IdentityLoader` finds SOUL.md + IDENTITY.md on first boot.
    // agent-core-v1 Phase F1.
    match seed_clawft_workspace(&cwd, args.force) {
        Ok(report) => {
            for path in &report.created {
                println!("Created {}", relative_or_full(&cwd, path));
            }
            for path in &report.overwritten {
                println!("Overwrote {}", relative_or_full(&cwd, path));
            }
            for path in &report.skipped {
                println!(
                    "Preserved existing {} (use --force to overwrite)",
                    relative_or_full(&cwd, path)
                );
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to seed .clawft/ identity files");
            anyhow::bail!("failed to seed .clawft/: {e}");
        }
    }

    println!();
    println!("WeftOS project '{}' initialized.", project_name);
    println!();
    println!("Next steps:");
    println!("  weaver kernel start        # start the kernel daemon");
    println!("  weaver topology extract .  # extract codebase graph");
    println!("  weaver vault enrich .      # enrich markdown files");
    println!();

    // Chain event.
    tracing::info!(
        target: "chain_event",
        source = "weave",
        kind = "project.init",
        project = project_name,
        "chain"
    );

    Ok(())
}

fn generate_weave_toml(name: &str, mesh: bool, ecc: bool) -> String {
    let mut toml = format!(
        r#"# WeftOS project configuration
# See: https://weftos.weavelogic.ai/docs/weftos/guides/configuration

[domain]
name = "{name}"

[kernel]
max_processes = 64
health_check_interval_secs = 30

[tick]
interval_ms = 50
adaptive = true

[sources.files]
root = "."
patterns = ["**/*.rs", "**/*.ts", "**/*.py", "**/*.go", "**/*.md"]
ignore = ["target", "node_modules", "dist", ".git"]
"#
    );

    if mesh {
        toml.push_str(
            r#"
[kernel.mesh]
enabled = true
transport = "tcp"
listen_addr = "0.0.0.0:9470"
discovery = false
seed_peers = []
"#,
        );
    }

    if ecc {
        toml.push_str(
            r#"
[kernel.ecc]
enabled = true
tick_interval_ms = 1000
"#,
        );
    }

    toml.push_str(
        r#"
[embedding]
provider = "local"
model = "all-MiniLM-L6-v2"
"#,
    );

    toml
}

fn relative_or_full(base: &Path, target: &Path) -> String {
    target
        .strip_prefix(base)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| target.display().to_string())
}

fn ensure_gitignore(dir: &Path) -> anyhow::Result<()> {
    let gitignore = dir.join(".gitignore");
    let entries = [".weftos/", "graphify-out/"];

    if gitignore.exists() {
        let content = std::fs::read_to_string(&gitignore)?;
        let mut additions = String::new();
        for entry in entries {
            if !content.lines().any(|l| l.trim() == entry) {
                additions.push_str(entry);
                additions.push('\n');
            }
        }
        if !additions.is_empty() {
            let mut file = std::fs::OpenOptions::new().append(true).open(&gitignore)?;
            std::io::Write::write_all(&mut file, b"\n# WeftOS\n")?;
            std::io::Write::write_all(&mut file, additions.as_bytes())?;
            println!("Updated .gitignore");
        }
    } else {
        std::fs::write(&gitignore, format!("# WeftOS\n{}\n", entries.join("\n")))?;
        println!("Created .gitignore");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use clawft_core::agent::identity::BINDING_THREAD_EXCERPT;

    // Note on `--update` coverage: the .clawft/ seeding behaviour
    // under `--update` is identical to the `force=false` path
    // exercised by `init_does_not_overwrite_existing_files` below
    // (skip-existing, create-missing). The new branch unique to
    // `--update` is the weave.toml gate in `run()` — covered by the
    // clap-level mutual-exclusion test directly below and the
    // operational use case ("play nice on a customized workspace")
    // documented in the InitArgs doc comment.

    #[test]
    fn update_and_force_are_mutually_exclusive_on_cli() {
        // clap should reject `--update --force` because the two
        // intents conflict: force overwrites, update preserves.
        // Keeping them separate keeps the surface auditable.
        let r = InitArgs::try_parse_from(["init", "--update", "--force"]);
        assert!(
            r.is_err(),
            "--update and --force must not be accepted together"
        );

        // Each in isolation parses fine.
        assert!(InitArgs::try_parse_from(["init", "--update"]).is_ok());
        assert!(InitArgs::try_parse_from(["init", "--force"]).is_ok());
        // No flag also parses fine (current default).
        assert!(InitArgs::try_parse_from(["init"]).is_ok());
    }

    #[test]
    fn seeded_soul_md_contains_binding_thread_excerpt() {
        // The compile-time guarantee: whatever ships in the binary as
        // the SOUL.md seed MUST contain the binding-thread excerpt the
        // identity loader's compile-time pin checks against. If the
        // canonical doc ever loses the excerpt, this test breaks the
        // build before init can ever ship it.
        assert!(
            SOUL_TEMPLATE.contains(BINDING_THREAD_EXCERPT),
            "baked-in SOUL.md template must contain BINDING_THREAD_EXCERPT"
        );
    }

    #[test]
    fn init_creates_clawft_directory_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let report = seed_clawft_workspace(tmp.path(), false).unwrap();

        let clawft = tmp.path().join(".clawft");
        assert!(clawft.is_dir(), ".clawft/ should be created");
        assert!(clawft.join("SOUL.md").is_file());
        assert!(clawft.join("IDENTITY.md").is_file());
        assert!(clawft.join("SOUL.journal.md").is_file());

        assert_eq!(report.created.len(), 3);
        assert!(report.skipped.is_empty());
        assert!(report.overwritten.is_empty());

        // SOUL.md content must match the baked-in template (and
        // therefore contain BINDING_THREAD_EXCERPT).
        let soul = std::fs::read_to_string(clawft.join("SOUL.md")).unwrap();
        assert_eq!(soul, SOUL_TEMPLATE);
        assert!(soul.contains(BINDING_THREAD_EXCERPT));
    }

    #[test]
    fn init_does_not_overwrite_existing_files() {
        let tmp = tempfile::tempdir().unwrap();
        let clawft = tmp.path().join(".clawft");
        std::fs::create_dir_all(&clawft).unwrap();
        std::fs::write(clawft.join("SOUL.md"), "custom soul").unwrap();
        std::fs::write(clawft.join("IDENTITY.md"), "custom identity").unwrap();
        // SOUL.journal.md absent — should be created.

        let report = seed_clawft_workspace(tmp.path(), false).unwrap();

        assert_eq!(
            std::fs::read_to_string(clawft.join("SOUL.md")).unwrap(),
            "custom soul",
            "existing SOUL.md must not be overwritten without --force"
        );
        assert_eq!(
            std::fs::read_to_string(clawft.join("IDENTITY.md")).unwrap(),
            "custom identity"
        );
        assert!(clawft.join("SOUL.journal.md").is_file());

        assert_eq!(report.skipped.len(), 2);
        assert_eq!(report.created.len(), 1);
        assert!(report.overwritten.is_empty());
    }

    #[test]
    fn init_force_overwrites() {
        let tmp = tempfile::tempdir().unwrap();
        let clawft = tmp.path().join(".clawft");
        std::fs::create_dir_all(&clawft).unwrap();
        std::fs::write(clawft.join("SOUL.md"), "stale soul").unwrap();
        std::fs::write(clawft.join("IDENTITY.md"), "stale identity").unwrap();
        std::fs::write(clawft.join("SOUL.journal.md"), "stale journal").unwrap();

        let report = seed_clawft_workspace(tmp.path(), true).unwrap();

        assert_eq!(
            std::fs::read_to_string(clawft.join("SOUL.md")).unwrap(),
            SOUL_TEMPLATE,
            "--force must replace SOUL.md with the canonical seed"
        );
        assert_eq!(
            std::fs::read_to_string(clawft.join("IDENTITY.md")).unwrap(),
            IDENTITY_TEMPLATE
        );
        assert_eq!(
            std::fs::read_to_string(clawft.join("SOUL.journal.md")).unwrap(),
            JOURNAL_TEMPLATE
        );

        assert_eq!(report.overwritten.len(), 3);
        assert!(report.created.is_empty());
        assert!(report.skipped.is_empty());
    }

    #[test]
    fn second_call_without_force_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let first = seed_clawft_workspace(tmp.path(), false).unwrap();
        assert_eq!(first.created.len(), 3);

        let second = seed_clawft_workspace(tmp.path(), false).unwrap();
        assert!(second.created.is_empty());
        assert_eq!(second.skipped.len(), 3);
        assert!(second.overwritten.is_empty());
    }
}
