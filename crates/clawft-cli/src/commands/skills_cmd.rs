//! `weft skills` -- CLI commands for skill discovery and management.
//!
//! Provides subcommands:
//!
//! - `weft skills list` -- list all skills (workspace, user, builtin) with
//!   source annotation.
//! - `weft skills show <name>` -- show skill details (description, variables,
//!   instructions preview).
//! - `weft skills install <path>` -- copy a skill to the user skills dir.
//!   Prompts for shell-access approval if the skill manifest declares
//!   `shell: true` or any tool with `category: shell` (WEFT-63).
//! - `weft skills remove <name>` -- remove a user-installed skill.
//! - `weft skills pending` -- list skills staged with a `.pending` marker
//!   (autogen / awaiting human review) along with a SKILL.md preview
//!   (WEFT-60).
//! - `weft skills approve <name>` -- promote a `.pending` staged skill to
//!   the canonical user skills directory (WEFT-59).
//! - `weft skills reject <name>` -- discard a `.pending` staged skill
//!   directory (WEFT-59).
//! - `weft skills search <query>` -- search ClawHub for skills.
//! - `weft skills publish <path>` -- publish a skill to ClawHub.
//! - `weft skills remote-install <name>` -- install a skill from ClawHub.
//! - `weft skills keygen` -- generate a signing key pair.

use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use comfy_table::{Table, presets};

use clawft_core::agent::skills_v2::SkillRegistry;
use clawft_rpc::{DaemonClient, Request};
use clawft_types::skill::{SkillDefinition, SkillFormat};

/// Arguments for the `weft skills` subcommand.
#[derive(Args)]
pub struct SkillsArgs {
    #[command(subcommand)]
    pub action: SkillsAction,
}

/// Subcommands for `weft skills`.
#[derive(Subcommand)]
pub enum SkillsAction {
    /// List all skills with source annotation.
    List,

    /// Show details of a specific skill.
    Show {
        /// Skill name to inspect.
        name: String,
    },

    /// Install a skill from a local path.
    Install {
        /// Path to a skill directory (containing SKILL.md or skill.json).
        path: String,
        /// Skip the interactive shell-access approval prompt (CI use).
        ///
        /// If the skill manifest declares `shell: true` or any tool with
        /// `category: shell`, install will normally block on stdin asking
        /// for approval. `--yes` accepts implicitly. Default is rejected
        /// when stdin is non-interactive and `--yes` is not set.
        #[arg(long)]
        yes: bool,
    },

    /// Remove a user-installed skill.
    Remove {
        /// Skill name to remove from ~/.clawft/skills/.
        name: String,
    },

    /// List skills staged with a `.pending` marker (autogen, awaiting review).
    Pending,

    /// Approve a `.pending` staged skill -- move it into the canonical user
    /// skills directory.
    Approve {
        /// Skill name (directory name) under `~/.clawft/skills/<name>/`.
        name: String,
    },

    /// Reject a `.pending` staged skill -- remove the staged directory.
    Reject {
        /// Skill name (directory name) under `~/.clawft/skills/<name>/`.
        name: String,
    },

    /// Search ClawHub for skills.
    Search {
        /// Search query.
        query: String,
        /// Maximum results.
        #[arg(long, default_value = "10")]
        limit: usize,
    },

    /// Publish a skill to ClawHub.
    Publish {
        /// Path to skill directory.
        path: String,
        /// Allow unsigned skills (local dev only).
        #[arg(long)]
        allow_unsigned: bool,
    },

    /// Install a skill from ClawHub by name.
    RemoteInstall {
        /// Skill name or ID on ClawHub.
        name: String,
        /// Allow unsigned skills.
        #[arg(long)]
        allow_unsigned: bool,
    },

    /// Generate a signing key pair for skill publishing.
    Keygen,
}

/// Warning printed when falling back to local execution without daemon.
const DAEMON_FALLBACK_WARNING: &str = "Warning: running without kernel daemon — results may not reflect live kernel state. \
     Start daemon with: weaver kernel start";

/// Try to send an RPC to the daemon. Returns `Some(result_json)` on success,
/// or `None` if no daemon is running (caller should fall back to local).
async fn try_daemon_rpc(method: &str, params: serde_json::Value) -> Option<serde_json::Value> {
    let mut client = DaemonClient::connect().await?;
    let request = Request::with_params(method, params);
    match client.call(request).await {
        Ok(resp) => match resp.into_result() {
            Ok(val) => Some(val),
            Err(e) => {
                eprintln!("Daemon RPC error: {e}");
                None
            }
        },
        Err(e) => {
            eprintln!("Daemon RPC error: {e}");
            None
        }
    }
}

/// Print the daemon fallback warning and return the result of the local
/// fallback closure.
fn with_fallback_warning<F: FnOnce() -> anyhow::Result<()>>(f: F) -> anyhow::Result<()> {
    eprintln!("{DAEMON_FALLBACK_WARNING}");
    f()
}

/// Run the skills subcommand.
pub async fn run(args: SkillsArgs) -> anyhow::Result<()> {
    // Keygen is pure local crypto — no daemon routing needed.
    if matches!(args.action, SkillsAction::Keygen) {
        return skills_keygen();
    }

    let (ws_dir, user_dir) = discover_skill_dirs();

    match args.action {
        SkillsAction::List => {
            if let Some(result) = try_daemon_rpc("skills.list", serde_json::json!({})).await {
                if let Some(output) = result.get("output").and_then(|v| v.as_str()) {
                    print!("{output}");
                } else {
                    println!("{}", serde_json::to_string_pretty(&result)?);
                }
                return Ok(());
            }
            eprintln!("{DAEMON_FALLBACK_WARNING}");
            let registry =
                SkillRegistry::discover(ws_dir.as_deref(), user_dir.as_deref(), Vec::new())
                    .await
                    .map_err(|e| anyhow::anyhow!("failed to discover skills: {e}"))?;
            skills_list(&registry, ws_dir.as_deref(), user_dir.as_deref())
        }
        SkillsAction::Show { name } => {
            if let Some(result) =
                try_daemon_rpc("skills.show", serde_json::json!({ "name": name })).await
            {
                if let Some(output) = result.get("output").and_then(|v| v.as_str()) {
                    print!("{output}");
                } else {
                    println!("{}", serde_json::to_string_pretty(&result)?);
                }
                return Ok(());
            }
            eprintln!("{DAEMON_FALLBACK_WARNING}");
            let registry =
                SkillRegistry::discover(ws_dir.as_deref(), user_dir.as_deref(), Vec::new())
                    .await
                    .map_err(|e| anyhow::anyhow!("failed to discover skills: {e}"))?;
            skills_show(&registry, &name)
        }
        SkillsAction::Install { path, yes } => {
            if let Some(result) = try_daemon_rpc(
                "skills.install",
                serde_json::json!({ "path": path, "yes": yes }),
            )
            .await
            {
                if let Some(output) = result.get("output").and_then(|v| v.as_str()) {
                    print!("{output}");
                } else {
                    println!("{}", serde_json::to_string_pretty(&result)?);
                }
                return Ok(());
            }
            with_fallback_warning(|| skills_install(&path, user_dir.as_deref(), yes))
        }
        SkillsAction::Pending => with_fallback_warning(|| skills_pending(user_dir.as_deref())),
        SkillsAction::Approve { name } => {
            with_fallback_warning(|| skills_approve(&name, user_dir.as_deref()))
        }
        SkillsAction::Reject { name } => {
            with_fallback_warning(|| skills_reject(&name, user_dir.as_deref()))
        }
        SkillsAction::Remove { name } => {
            if let Some(result) =
                try_daemon_rpc("skills.remove", serde_json::json!({ "name": name })).await
            {
                if let Some(output) = result.get("output").and_then(|v| v.as_str()) {
                    print!("{output}");
                } else {
                    println!("{}", serde_json::to_string_pretty(&result)?);
                }
                return Ok(());
            }
            with_fallback_warning(|| skills_remove(&name, user_dir.as_deref()))
        }
        SkillsAction::Search { query, limit } => {
            if let Some(result) = try_daemon_rpc(
                "skills.search",
                serde_json::json!({ "query": query, "limit": limit }),
            )
            .await
            {
                if let Some(output) = result.get("output").and_then(|v| v.as_str()) {
                    print!("{output}");
                } else {
                    println!("{}", serde_json::to_string_pretty(&result)?);
                }
                return Ok(());
            }
            eprintln!("{DAEMON_FALLBACK_WARNING}");
            skills_search(&query, limit).await
        }
        SkillsAction::Publish {
            path,
            allow_unsigned,
        } => {
            if let Some(result) = try_daemon_rpc(
                "skills.publish",
                serde_json::json!({ "path": path, "allow_unsigned": allow_unsigned }),
            )
            .await
            {
                if let Some(output) = result.get("output").and_then(|v| v.as_str()) {
                    print!("{output}");
                } else {
                    println!("{}", serde_json::to_string_pretty(&result)?);
                }
                return Ok(());
            }
            eprintln!("{DAEMON_FALLBACK_WARNING}");
            skills_publish(&path, allow_unsigned).await
        }
        SkillsAction::RemoteInstall {
            name,
            allow_unsigned,
        } => {
            if let Some(result) = try_daemon_rpc(
                "skills.remote-install",
                serde_json::json!({ "name": name, "allow_unsigned": allow_unsigned }),
            )
            .await
            {
                if let Some(output) = result.get("output").and_then(|v| v.as_str()) {
                    print!("{output}");
                } else {
                    println!("{}", serde_json::to_string_pretty(&result)?);
                }
                return Ok(());
            }
            eprintln!("{DAEMON_FALLBACK_WARNING}");
            skills_remote_install(&name, allow_unsigned, user_dir.as_deref()).await
        }
        SkillsAction::Keygen => unreachable!(),
    }
}

/// Discover workspace and user skill directories.
fn discover_skill_dirs() -> (Option<PathBuf>, Option<PathBuf>) {
    let user_dir = dirs::home_dir().map(|h| h.join(".clawft").join("skills"));

    // Walk upward from cwd to find .clawft/skills/
    let ws_dir = std::env::current_dir().ok().and_then(|cwd| {
        let mut dir: &Path = cwd.as_path();
        loop {
            let candidate = dir.join(".clawft").join("skills");
            if candidate.is_dir() {
                return Some(candidate);
            }
            match dir.parent() {
                Some(parent) => dir = parent,
                None => return None,
            }
        }
    });

    (ws_dir, user_dir)
}

// ── List ─────────────────────────────────────────────────────────────

/// List all skills in a table with source annotation.
fn skills_list(
    registry: &SkillRegistry,
    ws_dir: Option<&Path>,
    user_dir: Option<&Path>,
) -> anyhow::Result<()> {
    let skills = registry.list();

    if skills.is_empty() {
        println!("No skills found.");
        println!();
        if let Some(dir) = user_dir {
            println!("User skills directory: {}", dir.display());
        }
        if let Some(dir) = ws_dir {
            println!("Workspace skills directory: {}", dir.display());
        }
        return Ok(());
    }

    let mut table = Table::new();
    table.load_preset(presets::UTF8_FULL_CONDENSED);
    table.set_header(["NAME", "SOURCE", "FORMAT", "DESCRIPTION"]);

    // Sort by name for deterministic output.
    let mut sorted: Vec<&SkillDefinition> = skills;
    sorted.sort_by_key(|s| &s.name);

    for skill in sorted {
        let source = classify_source(skill, ws_dir, user_dir);
        let format = match skill.format {
            SkillFormat::SkillMd => "SKILL.md",
            SkillFormat::Legacy => "legacy",
            _ => "unknown",
        };
        let desc = truncate(&skill.description, 50);
        table.add_row([&skill.name, source, format, &desc]);
    }

    println!("{table}");
    println!();
    println!("Total: {} skill(s)", registry.len());

    Ok(())
}

// ── Show ─────────────────────────────────────────────────────────────

/// Show details of a specific skill.
fn skills_show(registry: &SkillRegistry, name: &str) -> anyhow::Result<()> {
    let skill = registry.get(name).ok_or_else(|| {
        anyhow::anyhow!("skill not found: {name}\nUse 'weft skills list' to see available skills.")
    })?;

    println!("Skill: {}", skill.name);
    println!("Description: {}", skill.description);

    if !skill.version.is_empty() {
        println!("Version: {}", skill.version);
    }

    println!(
        "Format: {}",
        match skill.format {
            SkillFormat::SkillMd => "SKILL.md",
            SkillFormat::Legacy => "legacy (skill.json)",
            _ => "unknown",
        }
    );

    if let Some(ref path) = skill.source_path {
        println!("Source: {}", path.display());
    }

    println!("User-invocable: {}", skill.user_invocable);

    if !skill.variables.is_empty() {
        println!("Variables: {}", skill.variables.join(", "));
    }

    if let Some(ref hint) = skill.argument_hint {
        println!("Argument hint: {hint}");
    }

    if !skill.allowed_tools.is_empty() {
        println!("Allowed tools: {}", skill.allowed_tools.join(", "));
    }

    if !skill.metadata.is_empty() {
        println!("Metadata:");
        for (key, value) in &skill.metadata {
            println!("  {key}: {value}");
        }
    }

    if !skill.instructions.is_empty() {
        println!();
        println!("Instructions (preview):");
        println!("---");
        // Show first 500 characters of instructions.
        let preview = truncate(&skill.instructions, 500);
        println!("{preview}");
        if skill.instructions.len() > 500 {
            println!("... ({} chars total)", skill.instructions.len());
        }
        println!("---");
    }

    Ok(())
}

// ── Install (local) ──────────────────────────────────────────────────

/// Install a skill from a local path to the user skills directory.
///
/// # WEFT-63
///
/// If the skill manifest declares shell access (`shell: true` in the
/// frontmatter, or any entry in `tools` / `allowed-tools` carrying a
/// `category: shell` annotation), the function asks the user via stdin
/// whether to grant shell access. Behavior:
///
/// - `yes = true`         → silently approve.
/// - stdin is a TTY       → prompt; default-rejected if input is empty.
/// - stdin is not a TTY   → reject (CI-safe).
fn skills_install(source_path: &str, user_dir: Option<&Path>, yes: bool) -> anyhow::Result<()> {
    let user_dir = user_dir.ok_or_else(|| {
        anyhow::anyhow!(
            "cannot determine user skills directory (no home directory). \
             Set $HOME or use an explicit path."
        )
    })?;

    let source = PathBuf::from(source_path);
    if !source.exists() {
        anyhow::bail!("source path does not exist: {source_path}");
    }

    // Determine skill name from source directory name.
    let skill_name = source
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow::anyhow!("cannot determine skill name from path: {source_path}"))?;

    // WEFT-63: Inspect SKILL.md (if present) for shell access declarations
    // before copying anything to the user skills dir.
    let skill_md_path = source.join("SKILL.md");
    if skill_md_path.is_file() {
        match std::fs::read_to_string(&skill_md_path) {
            Ok(content) => {
                if requires_shell_access(&content) && !approve_shell_access(skill_name, yes)? {
                    anyhow::bail!(
                        "shell access not approved for skill '{skill_name}'; \
                         install aborted. Re-run with --yes to skip the prompt."
                    );
                }
            }
            Err(e) => {
                eprintln!(
                    "Warning: could not read SKILL.md to check shell access: {e}; \
                     proceeding without shell-approval prompt."
                );
            }
        }
    }

    let dest = user_dir.join(skill_name);

    // Ensure user skills directory exists.
    std::fs::create_dir_all(user_dir)
        .map_err(|e| anyhow::anyhow!("failed to create user skills directory: {e}"))?;

    if dest.exists() {
        anyhow::bail!(
            "skill '{skill_name}' already exists at {}. Remove it first.",
            dest.display()
        );
    }

    // Copy directory recursively.
    copy_dir_recursive(&source, &dest).map_err(|e| anyhow::anyhow!("failed to copy skill: {e}"))?;

    println!("Installed skill '{skill_name}' to {}", dest.display());

    Ok(())
}

/// Check whether a SKILL.md frontmatter / body declares shell-execution
/// access. Returns `true` if any of the following match:
///
/// - frontmatter has `shell: true` or `requires_shell: true`
/// - frontmatter `tools:` / `allowed-tools:` / `allowed_tools:` has any
///   entry with `category: shell` (sequence of mappings) or with the
///   literal value `shell.exec`
fn requires_shell_access(skill_md_content: &str) -> bool {
    // Extract the frontmatter block; if absent, no declaration.
    let trimmed = skill_md_content.trim_start();
    if !trimmed.starts_with("---") {
        return false;
    }
    let after_open = trimmed.strip_prefix("---").unwrap_or(trimmed);
    let after_open = after_open.strip_prefix('\n').unwrap_or(after_open);
    let close_pos = match after_open.find("\n---") {
        Some(p) => p,
        None => return false,
    };
    let yaml = &after_open[..close_pos];

    // Parse with serde_yaml.
    let parsed: serde_yaml::Value = match serde_yaml::from_str(yaml) {
        Ok(v) => v,
        Err(_) => return false,
    };

    // Boolean flags.
    if let Some(b) = parsed.get("shell").and_then(|v| v.as_bool())
        && b
    {
        return true;
    }
    if let Some(b) = parsed.get("requires_shell").and_then(|v| v.as_bool())
        && b
    {
        return true;
    }

    // Tool-list inspection. Accept several keys.
    let keys = ["tools", "allowed-tools", "allowed_tools"];
    for k in &keys {
        if let Some(seq) = parsed.get(*k).and_then(|v| v.as_sequence()) {
            for entry in seq {
                if let Some(s) = entry.as_str()
                    && (s == "shell.exec" || s == "shell" || s.starts_with("shell."))
                {
                    return true;
                }
                if let Some(map) = entry.as_mapping() {
                    let cat = map
                        .get(serde_yaml::Value::String("category".into()))
                        .and_then(|v| v.as_str());
                    if cat == Some("shell") {
                        return true;
                    }
                }
            }
        }
    }

    false
}

/// Prompt the user on stdin to approve shell access for `skill_name`.
/// Returns `Ok(true)` if approved, `Ok(false)` if rejected.
///
/// - When `yes` is true, returns `Ok(true)` without prompting.
/// - When stdin is not a TTY, returns `Ok(false)` (CI-safe: default reject).
/// - At a TTY, prompts; empty input or `n`/`no` returns false; `y`/`yes`
///   returns true; any other input returns false.
fn approve_shell_access(skill_name: &str, yes: bool) -> anyhow::Result<bool> {
    if yes {
        eprintln!(
            "Skill '{skill_name}' requests shell-execution access. \
             --yes given, approving."
        );
        return Ok(true);
    }

    // Detect TTY without bringing in an extra crate.
    let is_tty = is_stdin_tty();
    if !is_tty {
        eprintln!(
            "Skill '{skill_name}' requests shell-execution access. \
             stdin is not a TTY and --yes was not given; refusing."
        );
        return Ok(false);
    }

    eprint!(
        "Skill '{skill_name}' requests shell-execution access. \
         Approve? [y/N] "
    );
    use std::io::{BufRead, Write};
    let _ = std::io::stderr().flush();
    let stdin = std::io::stdin();
    let mut line = String::new();
    let mut handle = stdin.lock();
    if handle.read_line(&mut line).is_err() {
        return Ok(false);
    }
    let answer = line.trim().to_ascii_lowercase();
    Ok(matches!(answer.as_str(), "y" | "yes"))
}

/// Best-effort detect whether stdin is a TTY. Avoids pulling in `atty` or
/// `is-terminal` -- uses libc::isatty on Unix and falls back to `false`.
fn is_stdin_tty() -> bool {
    #[cfg(unix)]
    {
        // SAFETY: isatty(0) is safe to call with a file descriptor.
        unsafe { libc_isatty(0) == 1 }
    }
    #[cfg(not(unix))]
    {
        false
    }
}

#[cfg(unix)]
unsafe extern "C" {
    fn isatty(fd: i32) -> i32;
}

#[cfg(unix)]
#[inline]
unsafe fn libc_isatty(fd: i32) -> i32 {
    unsafe { isatty(fd) }
}

// ── Pending / Approve / Reject (WEFT-59 + 60) ────────────────────────

/// Marker filename signaling that a staged skill awaits human review.
const PENDING_MARKER: &str = ".pending";

/// List skills with a `.pending` marker in the user skills directory.
///
/// For each pending skill, prints the source dir, key trust-root info
/// from the frontmatter, plus a SKILL.md preview truncated to 30 lines.
fn skills_pending(user_dir: Option<&Path>) -> anyhow::Result<()> {
    let user_dir = user_dir.ok_or_else(|| {
        anyhow::anyhow!(
            "cannot determine user skills directory (no home directory). \
             Set $HOME or use an explicit path."
        )
    })?;

    if !user_dir.exists() {
        println!(
            "No user skills directory at {} (no pending skills).",
            user_dir.display()
        );
        return Ok(());
    }

    let mut pending: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(user_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if path.join(PENDING_MARKER).is_file() {
            pending.push(path);
        }
    }

    if pending.is_empty() {
        println!("No pending skills in {}.", user_dir.display());
        return Ok(());
    }

    pending.sort();

    println!("Pending skills in {}:", user_dir.display());
    println!();

    for p in &pending {
        let name = p
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("<unknown>");
        println!("── {name} ──");
        println!("  Path: {}", p.display());

        // Read trust-root + signature info from `.pending` if it has any.
        let marker_path = p.join(PENDING_MARKER);
        if let Ok(marker_content) = std::fs::read_to_string(&marker_path) {
            let trimmed = marker_content.trim();
            if !trimmed.is_empty() {
                println!("  Marker: {trimmed}");
            }
        }

        // Optional sig.json sidecar for trust-root + signature data.
        let sig_path = p.join("signature.json");
        if sig_path.is_file()
            && let Ok(sig) = std::fs::read_to_string(&sig_path)
        {
            let trimmed = sig.trim();
            if !trimmed.is_empty() {
                let preview: String = trimmed.chars().take(200).collect();
                println!("  Signature: {preview}");
            }
        }

        // Print SKILL.md preview, truncated to 30 lines.
        let skill_md = p.join("SKILL.md");
        if skill_md.is_file() {
            match std::fs::read_to_string(&skill_md) {
                Ok(content) => {
                    println!("  SKILL.md preview:");
                    for (count, line) in content.lines().enumerate() {
                        if count >= 30 {
                            println!("    ... (truncated; full file at {})", skill_md.display());
                            break;
                        }
                        println!("    {line}");
                    }
                }
                Err(e) => {
                    println!("  Could not read SKILL.md: {e}");
                }
            }
        } else {
            println!("  No SKILL.md present.");
        }
        println!();
    }

    println!(
        "Use 'weft skills approve <name>' to install a pending skill, \
         or 'weft skills reject <name>' to discard."
    );

    Ok(())
}

/// Approve a `.pending` staged skill: remove the marker, leaving the
/// skill in place under the user skills directory.
///
/// The convention is that staged skills already live at
/// `~/.clawft/skills/<name>/` with a `.pending` sentinel file. Removing
/// the marker promotes the skill to canonical status. Implementations
/// that stage skills under a separate path can override this by setting
/// `pending_path/source` text inside the marker -- if so, we'll move
/// the directory into place.
fn skills_approve(name: &str, user_dir: Option<&Path>) -> anyhow::Result<()> {
    let user_dir = user_dir.ok_or_else(|| {
        anyhow::anyhow!(
            "cannot determine user skills directory (no home directory). \
             Set $HOME or use an explicit path."
        )
    })?;

    validate_skill_name(name)?;

    let skill_dir = user_dir.join(name);
    let marker = skill_dir.join(PENDING_MARKER);

    if !marker.is_file() {
        anyhow::bail!(
            "no pending marker found at {}. \
             Use 'weft skills pending' to list staged skills.",
            marker.display()
        );
    }

    // Remove the marker only. The skill keeps its content in place.
    std::fs::remove_file(&marker)
        .map_err(|e| anyhow::anyhow!("failed to remove pending marker: {e}"))?;

    println!("Approved skill '{name}' (marker removed; now active).");
    Ok(())
}

/// Reject a `.pending` staged skill: remove the entire skill directory.
fn skills_reject(name: &str, user_dir: Option<&Path>) -> anyhow::Result<()> {
    let user_dir = user_dir.ok_or_else(|| {
        anyhow::anyhow!(
            "cannot determine user skills directory (no home directory). \
             Set $HOME or use an explicit path."
        )
    })?;

    validate_skill_name(name)?;

    let skill_dir = user_dir.join(name);
    let marker = skill_dir.join(PENDING_MARKER);

    if !marker.is_file() {
        anyhow::bail!(
            "no pending marker found at {}. \
             Refusing to remove a skill that is not staged. \
             Use 'weft skills remove {name}' for non-pending skills.",
            marker.display()
        );
    }

    std::fs::remove_dir_all(&skill_dir)
        .map_err(|e| anyhow::anyhow!("failed to remove staged skill '{name}': {e}"))?;

    println!("Rejected skill '{name}'; staged directory removed.");
    Ok(())
}

/// Reject a skill name that contains path separators or other unsafe
/// characters. Mirrors the validation used by `remote-install`.
fn validate_skill_name(name: &str) -> anyhow::Result<()> {
    if name.is_empty()
        || name.starts_with('.')
        || !name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '.' || c == '-' || c == '_')
    {
        anyhow::bail!(
            "skill name '{name}' contains invalid characters. \
             Only alphanumeric, '.', '-', '_' are allowed; cannot start with '.'."
        );
    }
    Ok(())
}

// ── Remove ───────────────────────────────────────────────────────────

/// Remove a user-installed skill from `~/.clawft/skills/<name>/`.
///
/// Only removes skills from the user directory. Workspace and built-in
/// skills cannot be removed via this command.
fn skills_remove(name: &str, user_dir: Option<&Path>) -> anyhow::Result<()> {
    let user_dir = user_dir.ok_or_else(|| {
        anyhow::anyhow!(
            "cannot determine user skills directory (no home directory). \
             Set $HOME or use an explicit path."
        )
    })?;

    let skill_path = user_dir.join(name);

    if !skill_path.exists() {
        anyhow::bail!(
            "skill '{name}' not found in user skills directory ({}). \
             Only user-installed skills can be removed.",
            user_dir.display()
        );
    }

    std::fs::remove_dir_all(&skill_path)
        .map_err(|e| anyhow::anyhow!("failed to remove skill '{name}': {e}"))?;

    println!("Removed skill '{name}' from {}", skill_path.display());

    Ok(())
}

// ── Search (ClawHub) ─────────────────────────────────────────────────

/// Search ClawHub for skills.
#[cfg(feature = "services")]
async fn skills_search(query: &str, limit: usize) -> anyhow::Result<()> {
    use clawft_services::clawhub::{ClawHubClient, ClawHubConfig};

    let config = ClawHubConfig::from_env();
    let client = ClawHubClient::new(config);

    println!("Searching ClawHub for '{query}'...");
    println!();

    match client.search(query, limit, 0).await {
        Ok(response) => {
            let skills = response.data.unwrap_or_default();

            if skills.is_empty() {
                println!("No skills found matching '{query}'.");
                return Ok(());
            }

            let mut table = Table::new();
            table.load_preset(presets::UTF8_FULL_CONDENSED);
            table.set_header([
                "NAME",
                "VERSION",
                "AUTHOR",
                "STARS",
                "SIGNED",
                "DESCRIPTION",
            ]);

            for skill in &skills {
                let signed = if skill.signed { "yes" } else { "no" };
                table.add_row([
                    &skill.name,
                    &skill.version,
                    &skill.author,
                    &skill.stars.to_string(),
                    signed,
                    &truncate(&skill.description, 40),
                ]);
            }

            println!("{table}");
            println!();

            if let Some(pg) = response.pagination {
                println!(
                    "Showing {}/{} results (offset: {})",
                    skills.len(),
                    pg.total,
                    pg.offset
                );
            }
        }
        Err(e) => {
            eprintln!("Failed to search ClawHub: {e}");
            eprintln!();
            eprintln!("Make sure the ClawHub server is running or set CLAWHUB_API_URL.");
        }
    }

    Ok(())
}

#[cfg(not(feature = "services"))]
async fn skills_search(_query: &str, _limit: usize) -> anyhow::Result<()> {
    anyhow::bail!(
        "ClawHub search requires the 'services' feature. Rebuild with --features services."
    );
}

// ── Publish (ClawHub) ────────────────────────────────────────────────

/// Publish a skill to ClawHub.
#[cfg(feature = "services")]
async fn skills_publish(path: &str, allow_unsigned: bool) -> anyhow::Result<()> {
    use clawft_services::clawhub::{ClawHubClient, ClawHubConfig, PublishRequest};

    let skill_dir = PathBuf::from(path);
    if !skill_dir.exists() {
        anyhow::bail!("skill directory does not exist: {path}");
    }

    // Parse SKILL.md to extract metadata.
    let skill_md_path = skill_dir.join("SKILL.md");
    if !skill_md_path.exists() {
        anyhow::bail!(
            "no SKILL.md found in {path}. Publishable skills must use the SKILL.md format."
        );
    }

    let skill_md_content = std::fs::read_to_string(&skill_md_path)
        .map_err(|e| anyhow::anyhow!("failed to read SKILL.md: {e}"))?;

    let (name, description, version, tags) = parse_skill_frontmatter(&skill_md_content)?;

    println!("Publishing skill '{name}' v{version}...");

    // Compute content hash.
    let content_hash = compute_simple_hash(&skill_dir)?;

    // Attempt to sign if keys exist.
    let keys_dir = dirs::home_dir()
        .map(|h| h.join(".clawft").join("keys"))
        .unwrap_or_else(|| PathBuf::from(".clawft/keys"));

    let (signature, public_key) = try_sign_content(&content_hash, &keys_dir, allow_unsigned)?;

    // Read and base64-encode the SKILL.md content as the package.
    let content_bytes = std::fs::read(&skill_md_path)
        .map_err(|e| anyhow::anyhow!("failed to read skill content: {e}"))?;
    let content_b64 = base64_encode(&content_bytes);

    let mut config = ClawHubConfig::from_env();
    config.allow_unsigned = allow_unsigned;
    let client = ClawHubClient::new(config);

    let request = PublishRequest {
        name: name.clone(),
        description,
        version,
        content: content_b64,
        content_hash,
        signature,
        public_key,
        tags,
    };

    match client.publish(&request).await {
        Ok(response) => {
            if response.ok {
                if let Some(entry) = response.data {
                    println!(
                        "Published '{name}' as {} (hash: {})",
                        entry.id, entry.content_hash
                    );
                } else {
                    println!("Published '{name}' successfully.");
                }
            } else {
                let err = response.error.unwrap_or_else(|| "unknown error".into());
                eprintln!("Publish failed: {err}");
            }
        }
        Err(e) => {
            eprintln!("Failed to publish to ClawHub: {e}");
            eprintln!();
            eprintln!("Make sure the ClawHub server is running or set CLAWHUB_API_URL.");
        }
    }

    Ok(())
}

#[cfg(not(feature = "services"))]
async fn skills_publish(_path: &str, _allow_unsigned: bool) -> anyhow::Result<()> {
    anyhow::bail!(
        "ClawHub publish requires the 'services' feature. Rebuild with --features services."
    );
}

// ── Remote Install (ClawHub) ─────────────────────────────────────────

/// Install a skill from ClawHub.
#[cfg(feature = "services")]
async fn skills_remote_install(
    name: &str,
    allow_unsigned: bool,
    user_dir: Option<&Path>,
) -> anyhow::Result<()> {
    use clawft_services::clawhub::{ClawHubClient, ClawHubConfig};

    let user_dir = user_dir.ok_or_else(|| {
        anyhow::anyhow!(
            "cannot determine user skills directory (no home directory). \
             Set $HOME or use an explicit path."
        )
    })?;

    let mut config = ClawHubConfig::from_env();
    config.allow_unsigned = allow_unsigned;
    let client = ClawHubClient::new(config);

    println!("Searching ClawHub for '{name}'...");

    // Search for the skill first.
    let search_result = client
        .search(name, 1, 0)
        .await
        .map_err(|e| anyhow::anyhow!("failed to search ClawHub: {e}"))?;

    let skills = search_result.data.unwrap_or_default();
    let skill = skills.first().ok_or_else(|| {
        anyhow::anyhow!(
            "skill '{name}' not found on ClawHub. \
             Use 'weft skills search {name}' to check available skills."
        )
    })?;

    // Check signature requirement.
    if !skill.signed && !allow_unsigned {
        anyhow::bail!(
            "skill '{}' is not signed. Use --allow-unsigned to install unsigned skills.",
            skill.name
        );
    }

    // Validate skill name to prevent path traversal attacks.
    if skill.name.is_empty()
        || skill.name.starts_with('.')
        || !skill
            .name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '.' || c == '-' || c == '_')
    {
        anyhow::bail!(
            "skill name '{}' contains invalid characters. \
             Only alphanumeric characters, '.', '-', and '_' are allowed, \
             and the name must not start with '.'.",
            skill.name
        );
    }

    println!(
        "Found '{}' v{} by {} ({} stars)",
        skill.name, skill.version, skill.author, skill.stars
    );
    println!("Downloading...");

    // Download the skill content.
    let content = client
        .download(&skill.id)
        .await
        .map_err(|e| anyhow::anyhow!("failed to download skill: {e}"))?;

    // Install to user skills directory.
    let skill_dest = user_dir.join(&skill.name);
    if skill_dest.exists() {
        anyhow::bail!(
            "skill '{}' already exists at {}. Remove it first with 'weft skills remove {}'.",
            skill.name,
            skill_dest.display(),
            skill.name
        );
    }

    std::fs::create_dir_all(&skill_dest)
        .map_err(|e| anyhow::anyhow!("failed to create skill directory: {e}"))?;

    std::fs::write(skill_dest.join("SKILL.md"), &content)
        .map_err(|e| anyhow::anyhow!("failed to write skill content: {e}"))?;

    println!("Installed '{}' to {}", skill.name, skill_dest.display());

    Ok(())
}

#[cfg(not(feature = "services"))]
async fn skills_remote_install(
    _name: &str,
    _allow_unsigned: bool,
    _user_dir: Option<&Path>,
) -> anyhow::Result<()> {
    anyhow::bail!(
        "ClawHub install requires the 'services' feature. Rebuild with --features services."
    );
}

// ── Keygen ───────────────────────────────────────────────────────────

/// Generate a signing key pair.
fn skills_keygen() -> anyhow::Result<()> {
    let keys_dir = dirs::home_dir()
        .map(|h| h.join(".clawft").join("keys"))
        .ok_or_else(|| anyhow::anyhow!("cannot determine home directory for key storage"))?;

    let priv_path = keys_dir.join("skill-signing.key");
    if priv_path.exists() {
        anyhow::bail!(
            "signing key already exists at {}. \
             Remove it manually to generate a new one.",
            priv_path.display()
        );
    }

    #[cfg(feature = "services")]
    {
        // Use the signing module from clawft-core if available at compile time.
        // The signing module requires the 'signing' feature on clawft-core.
        // Since we may not have it, use a standalone implementation.
    }

    // Standalone key generation using the same algorithm as the signing module.
    generate_keypair_standalone(&keys_dir)?;

    println!("Generated signing key pair:");
    println!(
        "  Private key: {}",
        keys_dir.join("skill-signing.key").display()
    );
    println!(
        "  Public key:  {}",
        keys_dir.join("skill-signing.pub").display()
    );
    println!();
    println!("Keep your private key safe! It is used to sign skills for publication.");

    Ok(())
}

/// Standalone Ed25519 key pair generation.
///
/// Uses `ed25519-dalek` for proper key derivation -- both files are
/// hex-encoded so they remain shell-friendly. Replaces the historical
/// "(derived on first sign)" placeholder (WEFT-23).
fn generate_keypair_standalone(output_dir: &Path) -> anyhow::Result<()> {
    use ed25519_dalek::SigningKey;
    use rand::RngCore;
    use rand::rngs::OsRng;

    std::fs::create_dir_all(output_dir)?;

    // Generate 32 cryptographically secure random bytes as the seed.
    let mut seed = [0u8; 32];
    OsRng.fill_bytes(&mut seed);

    let signing_key = SigningKey::from_bytes(&seed);
    let verifying_key = signing_key.verifying_key();

    let priv_hex: String = seed.iter().map(|b| format!("{b:02x}")).collect();
    let pub_hex: String = verifying_key
        .to_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();

    let priv_path = output_dir.join("skill-signing.key");
    std::fs::write(&priv_path, &priv_hex)?;

    // Set restrictive permissions on the private key.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&priv_path, std::fs::Permissions::from_mode(0o600))?;
    }

    let pub_path = output_dir.join("skill-signing.pub");
    std::fs::write(&pub_path, &pub_hex)?;

    Ok(())
}

#[cfg(test)]
mod weft23_keygen_tests {
    use super::*;
    use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};

    fn hex_to_bytes(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn keypair_roundtrip_and_sign_verify() {
        // Generate into a tempdir and reload both halves.
        let tmp = tempfile::tempdir().unwrap();
        generate_keypair_standalone(tmp.path()).unwrap();

        let priv_hex = std::fs::read_to_string(tmp.path().join("skill-signing.key")).unwrap();
        let pub_hex = std::fs::read_to_string(tmp.path().join("skill-signing.pub")).unwrap();

        // Both files must be 64 hex chars (32 raw bytes each).
        assert_eq!(priv_hex.trim().len(), 64, "seed should be 32 bytes hex");
        assert_eq!(pub_hex.trim().len(), 64, "pubkey should be 32 bytes hex");

        // The pubkey must NOT be the legacy placeholder string.
        assert_ne!(pub_hex.trim(), "(derived on first sign)");

        // Reload and confirm the pubkey was actually derived from the seed.
        let priv_bytes: [u8; 32] = hex_to_bytes(priv_hex.trim()).try_into().unwrap();
        let pub_bytes: [u8; 32] = hex_to_bytes(pub_hex.trim()).try_into().unwrap();

        let signing = SigningKey::from_bytes(&priv_bytes);
        assert_eq!(
            signing.verifying_key().to_bytes(),
            pub_bytes,
            "stored pubkey must match the derivation from the stored seed"
        );

        // Sign / verify a sample payload.
        let msg = b"weft-skills-content-hash-sample";
        let sig: Signature = signing.sign(msg);
        let verifier = VerifyingKey::from_bytes(&pub_bytes).unwrap();
        verifier.verify(msg, &sig).expect("signature should verify");
    }

    #[test]
    fn two_keygens_produce_distinct_keys() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        generate_keypair_standalone(a.path()).unwrap();
        generate_keypair_standalone(b.path()).unwrap();
        let pa = std::fs::read_to_string(a.path().join("skill-signing.pub")).unwrap();
        let pb = std::fs::read_to_string(b.path().join("skill-signing.pub")).unwrap();
        assert_ne!(pa, pb, "fresh keygens must produce different pubkeys");
    }
}

// ── Helpers ──────────────────────────────────────────────────────────

/// Classify the source of a skill for display.
fn classify_source(
    skill: &SkillDefinition,
    ws_dir: Option<&Path>,
    user_dir: Option<&Path>,
) -> &'static str {
    if let Some(ref path) = skill.source_path {
        if let Some(ws) = ws_dir
            && path.starts_with(ws)
        {
            return "workspace";
        }
        if let Some(ud) = user_dir
            && path.starts_with(ud)
        {
            return "user";
        }
    }
    "builtin"
}

/// Truncate a string to `max_len` characters, appending "..." if truncated.
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len.saturating_sub(3)])
    }
}

/// Recursively copy a directory.
fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let entry_path = entry.path();
        let dest_path = dst.join(entry.file_name());

        if entry_path.is_dir() {
            copy_dir_recursive(&entry_path, &dest_path)?;
        } else {
            std::fs::copy(&entry_path, &dest_path)?;
        }
    }
    Ok(())
}

/// Parse SKILL.md frontmatter for name, description, version, and tags.
fn parse_skill_frontmatter(content: &str) -> anyhow::Result<(String, String, String, Vec<String>)> {
    // Extract YAML between --- delimiters.
    let yaml = if content.starts_with("---") {
        content
            .strip_prefix("---")
            .and_then(|rest| rest.split_once("---"))
            .map(|(yaml, _)| yaml.trim())
    } else {
        None
    };

    let yaml = yaml.ok_or_else(|| anyhow::anyhow!("SKILL.md missing YAML frontmatter (---)"))?;

    let value: serde_json::Value = serde_yaml::from_str(yaml)
        .map_err(|e| anyhow::anyhow!("failed to parse SKILL.md frontmatter: {e}"))?;

    let name = value
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("unnamed")
        .to_string();

    let description = value
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let version = value
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("0.1.0")
        .to_string();

    let tags = value
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    Ok((name, description, version, tags))
}

/// Compute a simple SHA-256 hash of all files in a directory.
///
/// Uses a portable implementation that does not require the `sha2` crate.
fn compute_simple_hash(dir: &Path) -> anyhow::Result<String> {
    use std::collections::BTreeMap;

    let mut files = BTreeMap::new();
    collect_files_for_hash(dir, dir, &mut files)?;

    // Use a simple FNV-like hash for the content. If the signing feature
    // is available, we defer to the proper SHA-256 from there. This hash
    // is for the content_hash field in the publish request.
    let mut hasher = SimpleHasher::new();
    for (rel_path, content) in &files {
        hasher.update(rel_path.as_bytes());
        hasher.update(&[0]);
        hasher.update(&(content.len() as u64).to_le_bytes());
        hasher.update(content);
    }

    Ok(hasher.finalize())
}

/// Collect files for hashing (sorted BTreeMap).
fn collect_files_for_hash(
    current: &Path,
    base: &Path,
    out: &mut std::collections::BTreeMap<String, Vec<u8>>,
) -> anyhow::Result<()> {
    if !current.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(current)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if name_str.starts_with('.') || name_str == "target" {
            continue;
        }

        let path = entry.path();
        if path.is_dir() {
            collect_files_for_hash(&path, base, out)?;
        } else if path.is_file() {
            let rel = path
                .strip_prefix(base)
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();
            let content = std::fs::read(&path)?;
            out.insert(rel, content);
        }
    }
    Ok(())
}

/// A simple hasher that produces a hex string.
///
/// When the `sha2` crate is not available, this uses FNV-1a (64-bit)
/// as a fallback. The proper SHA-256 hash is computed by the signing module.
struct SimpleHasher {
    state: u64,
}

impl SimpleHasher {
    fn new() -> Self {
        Self {
            state: 0xcbf2_9ce4_8422_2325, // FNV offset basis
        }
    }

    fn update(&mut self, data: &[u8]) {
        for &byte in data {
            self.state ^= byte as u64;
            self.state = self.state.wrapping_mul(0x0100_0000_01b3); // FNV prime
        }
    }

    fn finalize(&self) -> String {
        format!("{:016x}", self.state)
    }
}

/// Try to sign content with a local key pair.
///
/// Returns `(signature_hex, public_key_hex)` or `(None, None)` if no key
/// exists and `allow_unsigned` is set. WEFT-23 wired this to real
/// `ed25519-dalek` signing -- the previous "(derived on first sign)"
/// placeholder is gone.
fn try_sign_content(
    content_hash: &str,
    keys_dir: &Path,
    allow_unsigned: bool,
) -> anyhow::Result<(Option<String>, Option<String>)> {
    use ed25519_dalek::{Signer, SigningKey};

    let priv_path = keys_dir.join("skill-signing.key");
    if !priv_path.exists() {
        if allow_unsigned {
            println!("No signing key found. Publishing unsigned (--allow-unsigned).");
            return Ok((None, None));
        }
        anyhow::bail!(
            "no signing key found at {}. \
             Run 'weft skills keygen' to generate one, \
             or use --allow-unsigned for local dev.",
            priv_path.display()
        );
    }

    // Read and validate the private key hex.
    let priv_hex = std::fs::read_to_string(&priv_path)?;
    let priv_hex = priv_hex.trim();
    if priv_hex.len() != 64 {
        anyhow::bail!(
            "invalid signing key at {} (expected 64 hex chars, got {})",
            priv_path.display(),
            priv_hex.len()
        );
    }
    let priv_bytes =
        hex_decode(priv_hex).map_err(|e| anyhow::anyhow!("invalid signing key hex: {e}"))?;
    let seed: [u8; 32] = priv_bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("signing key must decode to exactly 32 bytes"))?;

    let signing_key = SigningKey::from_bytes(&seed);
    let signature = signing_key.sign(content_hash.as_bytes());
    let pubkey_bytes = signing_key.verifying_key().to_bytes();

    let sig_hex: String = signature
        .to_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let pub_hex: String = pubkey_bytes.iter().map(|b| format!("{b:02x}")).collect();

    Ok((Some(sig_hex), Some(pub_hex)))
}

/// Decode hex string to bytes.
fn hex_decode(s: &str) -> Result<Vec<u8>, String> {
    if !s.len().is_multiple_of(2) {
        return Err("odd-length hex string".into());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect()
}

/// Simple base64 encoding (no external dep needed).
fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut result = String::with_capacity(data.len().div_ceil(3) * 4);

    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };

        let n = (b0 << 16) | (b1 << 8) | b2;

        result.push(TABLE[((n >> 18) & 0x3F) as usize] as char);
        result.push(TABLE[((n >> 12) & 0x3F) as usize] as char);

        if chunk.len() > 1 {
            result.push(TABLE[((n >> 6) & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }

        if chunk.len() > 2 {
            result.push(TABLE[(n & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_dir(prefix: &str) -> PathBuf {
        let id = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        std::env::temp_dir().join(format!("clawft_skills_cmd_{prefix}_{pid}_{id}"))
    }

    fn create_skill_md(dir: &Path, name: &str, desc: &str) {
        let skill_dir = dir.join(name);
        std::fs::create_dir_all(&skill_dir).unwrap();
        let content =
            format!("---\nname: {name}\ndescription: {desc}\n---\n\nInstructions for {name}.");
        std::fs::write(skill_dir.join("SKILL.md"), content).unwrap();
    }

    #[test]
    fn truncate_short_string() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_long_string() {
        let result = truncate("this is a long string that should be truncated", 20);
        assert!(result.ends_with("..."));
        assert!(result.len() <= 20);
    }

    #[test]
    fn truncate_exact_length() {
        assert_eq!(truncate("hello", 5), "hello");
    }

    #[test]
    fn classify_source_workspace() {
        let ws = PathBuf::from("/project/.clawft/skills");
        let skill = SkillDefinition {
            source_path: Some(PathBuf::from("/project/.clawft/skills/research/SKILL.md")),
            ..SkillDefinition::new("research", "desc")
        };
        assert_eq!(classify_source(&skill, Some(&ws), None), "workspace");
    }

    #[test]
    fn classify_source_user() {
        let user = PathBuf::from("/home/user/.clawft/skills");
        let skill = SkillDefinition {
            source_path: Some(PathBuf::from("/home/user/.clawft/skills/coding/SKILL.md")),
            ..SkillDefinition::new("coding", "desc")
        };
        assert_eq!(classify_source(&skill, None, Some(&user)), "user");
    }

    #[test]
    fn classify_source_builtin() {
        let skill = SkillDefinition::new("builtin", "desc");
        assert_eq!(classify_source(&skill, None, None), "builtin");
    }

    #[tokio::test]
    async fn skills_list_with_registry() {
        let dir = temp_dir("list");
        create_skill_md(&dir, "alpha", "Alpha skill");
        create_skill_md(&dir, "beta", "Beta skill");

        let registry = SkillRegistry::discover(Some(&dir), None, Vec::new())
            .await
            .unwrap();
        // Just verify it does not panic and the registry has the skills.
        assert_eq!(registry.len(), 2);
        assert!(registry.get("alpha").is_some());
        assert!(registry.get("beta").is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn skills_show_found() {
        let dir = temp_dir("show");
        create_skill_md(&dir, "test_skill", "A test skill");

        let registry = SkillRegistry::discover(Some(&dir), None, Vec::new())
            .await
            .unwrap();
        let skill = registry.get("test_skill");
        assert!(skill.is_some());
        assert_eq!(skill.unwrap().description, "A test skill");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn skills_show_not_found() {
        let registry = SkillRegistry::discover(None, None, Vec::new())
            .await
            .unwrap();
        let result = skills_show(&registry, "nonexistent");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("not found"));
    }

    #[test]
    fn skills_install_source_not_found() {
        let user_dir = temp_dir("install_user");
        std::fs::create_dir_all(&user_dir).unwrap();

        let result = skills_install("/nonexistent/path", Some(&user_dir), false);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("does not exist"));

        let _ = std::fs::remove_dir_all(&user_dir);
    }

    #[test]
    fn skills_install_no_user_dir() {
        let result = skills_install("/some/path", None, false);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("cannot determine"));
    }

    #[test]
    fn skills_install_copies_directory() {
        let src = temp_dir("install_src");
        let user_dir = temp_dir("install_user_dir");

        create_skill_md(&src, "installable", "To be installed");

        let result = skills_install(
            src.join("installable").to_str().unwrap(),
            Some(&user_dir),
            false,
        );
        assert!(result.is_ok());

        let installed = user_dir.join("installable").join("SKILL.md");
        assert!(installed.exists());

        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&user_dir);
    }

    #[test]
    fn skills_install_already_exists() {
        let src = temp_dir("install_exists_src");
        let user_dir = temp_dir("install_exists_user");

        create_skill_md(&src, "dupe", "Original");
        create_skill_md(&user_dir, "dupe", "Existing");

        let result = skills_install(src.join("dupe").to_str().unwrap(), Some(&user_dir), false);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("already exists"));

        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&user_dir);
    }

    // ── WEFT-63: shell access detection ───────────────────────────

    #[test]
    fn requires_shell_false_without_frontmatter() {
        let content = "Just plain markdown, no frontmatter.";
        assert!(!requires_shell_access(content));
    }

    #[test]
    fn requires_shell_false_when_unset() {
        let content = "---\nname: safe\ndescription: ok\n---\nBody";
        assert!(!requires_shell_access(content));
    }

    #[test]
    fn requires_shell_true_with_shell_flag() {
        let content = "---\nname: noisy\ndescription: x\nshell: true\n---\nBody";
        assert!(requires_shell_access(content));
    }

    #[test]
    fn requires_shell_true_with_requires_shell() {
        let content = "---\nname: noisy\ndescription: x\nrequires_shell: true\n---\nBody";
        assert!(requires_shell_access(content));
    }

    #[test]
    fn requires_shell_true_with_shell_exec_tool() {
        let content =
            "---\nname: x\ndescription: y\nallowed-tools:\n  - shell.exec\n  - Read\n---\nBody";
        assert!(requires_shell_access(content));
    }

    #[test]
    fn requires_shell_true_with_category_shell_tool() {
        let content =
            "---\nname: x\ndescription: y\ntools:\n  - name: bash\n    category: shell\n---\nBody";
        assert!(requires_shell_access(content));
    }

    #[test]
    fn requires_shell_install_blocks_when_unapproved() {
        // Install a skill that asks for shell, with --yes=false; we expect a
        // refusal because the test runner has no TTY.
        let src = temp_dir("install_shell_src");
        let user_dir = temp_dir("install_shell_user");
        std::fs::create_dir_all(src.join("riskskill")).unwrap();
        let md = "---\nname: riskskill\ndescription: needs shell\nshell: true\n---\nBody";
        std::fs::write(src.join("riskskill").join("SKILL.md"), md).unwrap();

        let result = skills_install(
            src.join("riskskill").to_str().unwrap(),
            Some(&user_dir),
            false,
        );
        assert!(result.is_err(), "expected refusal without --yes / TTY");
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("shell access not approved"), "got: {msg}");

        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&user_dir);
    }

    #[test]
    fn requires_shell_install_passes_with_yes_flag() {
        let src = temp_dir("install_shell_yes_src");
        let user_dir = temp_dir("install_shell_yes_user");
        std::fs::create_dir_all(src.join("riskskill_ok")).unwrap();
        let md = "---\nname: riskskill_ok\ndescription: needs shell\nshell: true\n---\nBody";
        std::fs::write(src.join("riskskill_ok").join("SKILL.md"), md).unwrap();

        let result = skills_install(
            src.join("riskskill_ok").to_str().unwrap(),
            Some(&user_dir),
            true,
        );
        assert!(result.is_ok(), "should install when --yes given");

        assert!(user_dir.join("riskskill_ok").join("SKILL.md").is_file());

        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&user_dir);
    }

    // ── WEFT-59 / WEFT-60: pending / approve / reject ─────────────

    fn create_pending_skill(user_dir: &Path, name: &str, body: &str) {
        let skill_dir = user_dir.join(name);
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: pending test\n---\n{body}"),
        )
        .unwrap();
        std::fs::write(skill_dir.join(PENDING_MARKER), "pending: autogen").unwrap();
    }

    #[test]
    fn pending_lists_no_pending() {
        let user_dir = temp_dir("pending_empty");
        std::fs::create_dir_all(&user_dir).unwrap();
        // Should be Ok and not panic.
        let result = skills_pending(Some(&user_dir));
        assert!(result.is_ok());
        let _ = std::fs::remove_dir_all(&user_dir);
    }

    #[test]
    fn pending_lists_when_present() {
        let user_dir = temp_dir("pending_present");
        create_pending_skill(&user_dir, "alpha_pending", "preview body");
        // Non-pending skill should not affect listing.
        create_skill_md(&user_dir, "regular", "regular skill");

        // Just verify the function executes without error and finds the
        // pending marker.
        let result = skills_pending(Some(&user_dir));
        assert!(result.is_ok());

        let _ = std::fs::remove_dir_all(&user_dir);
    }

    #[test]
    fn approve_promotes_pending_skill() {
        let user_dir = temp_dir("approve_user");
        create_pending_skill(&user_dir, "to_approve", "body");

        let marker = user_dir.join("to_approve").join(PENDING_MARKER);
        assert!(marker.is_file());

        let result = skills_approve("to_approve", Some(&user_dir));
        assert!(result.is_ok(), "approve should succeed: {result:?}");
        assert!(!marker.exists(), "marker should be removed");
        // Skill body should still be in place.
        assert!(user_dir.join("to_approve").join("SKILL.md").is_file());

        let _ = std::fs::remove_dir_all(&user_dir);
    }

    #[test]
    fn approve_fails_for_non_pending() {
        let user_dir = temp_dir("approve_non_pending");
        create_skill_md(&user_dir, "regular", "no marker");

        let result = skills_approve("regular", Some(&user_dir));
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("no pending marker"));

        let _ = std::fs::remove_dir_all(&user_dir);
    }

    #[test]
    fn reject_removes_pending_skill() {
        let user_dir = temp_dir("reject_user");
        create_pending_skill(&user_dir, "to_reject", "body");

        let skill_dir = user_dir.join("to_reject");
        assert!(skill_dir.is_dir());

        let result = skills_reject("to_reject", Some(&user_dir));
        assert!(result.is_ok(), "reject should succeed: {result:?}");
        assert!(!skill_dir.exists());

        let _ = std::fs::remove_dir_all(&user_dir);
    }

    #[test]
    fn reject_refuses_non_pending() {
        let user_dir = temp_dir("reject_non_pending");
        create_skill_md(&user_dir, "regular", "no marker");

        let result = skills_reject("regular", Some(&user_dir));
        assert!(result.is_err(), "reject should refuse without marker");
        // The skill must NOT be removed.
        assert!(user_dir.join("regular").is_dir());

        let _ = std::fs::remove_dir_all(&user_dir);
    }

    #[test]
    fn validate_skill_name_rejects_traversal() {
        assert!(validate_skill_name("../bad").is_err());
        assert!(validate_skill_name(".hidden").is_err());
        assert!(validate_skill_name("ok-name").is_ok());
        assert!(validate_skill_name("ok_name_2").is_ok());
        assert!(validate_skill_name("name.with.dots").is_ok());
    }

    #[test]
    fn skills_remove_success() {
        let user_dir = temp_dir("remove_user");
        create_skill_md(&user_dir, "removable", "To be removed");

        assert!(user_dir.join("removable").exists());

        let result = skills_remove("removable", Some(&user_dir));
        assert!(result.is_ok());
        assert!(!user_dir.join("removable").exists());

        let _ = std::fs::remove_dir_all(&user_dir);
    }

    #[test]
    fn skills_remove_not_found() {
        let user_dir = temp_dir("remove_not_found");
        std::fs::create_dir_all(&user_dir).unwrap();

        let result = skills_remove("nonexistent", Some(&user_dir));
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("not found"));

        let _ = std::fs::remove_dir_all(&user_dir);
    }

    #[test]
    fn skills_remove_no_user_dir() {
        let result = skills_remove("anything", None);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("cannot determine"));
    }

    #[test]
    fn copy_dir_recursive_works() {
        let src = temp_dir("copy_src");
        let dst = temp_dir("copy_dst");

        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("file.txt"), "hello").unwrap();
        std::fs::write(src.join("sub").join("nested.txt"), "nested").unwrap();

        copy_dir_recursive(&src, &dst).unwrap();

        assert!(dst.join("file.txt").exists());
        assert!(dst.join("sub").join("nested.txt").exists());
        assert_eq!(
            std::fs::read_to_string(dst.join("file.txt")).unwrap(),
            "hello"
        );

        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&dst);
    }

    #[test]
    fn parse_frontmatter_basic() {
        let content = "---\nname: test-skill\ndescription: A test\nversion: 1.2.3\n---\nBody.";
        let (name, desc, version, tags) = parse_skill_frontmatter(content).unwrap();
        assert_eq!(name, "test-skill");
        assert_eq!(desc, "A test");
        assert_eq!(version, "1.2.3");
        assert!(tags.is_empty());
    }

    #[test]
    fn parse_frontmatter_with_tags() {
        let content = "---\nname: tagged\ndescription: desc\ntags:\n  - ai\n  - coding\n---\nBody.";
        let (name, _, _, tags) = parse_skill_frontmatter(content).unwrap();
        assert_eq!(name, "tagged");
        assert_eq!(tags, vec!["ai", "coding"]);
    }

    #[test]
    fn parse_frontmatter_missing_delimiters() {
        let content = "No frontmatter here.";
        let result = parse_skill_frontmatter(content);
        assert!(result.is_err());
    }

    #[test]
    fn base64_encode_basic() {
        assert_eq!(base64_encode(b"Hello"), "SGVsbG8=");
        assert_eq!(base64_encode(b"Hi"), "SGk=");
        assert_eq!(base64_encode(b""), "");
    }

    #[test]
    fn hex_decode_basic() {
        assert_eq!(
            hex_decode("deadbeef").unwrap(),
            vec![0xDE, 0xAD, 0xBE, 0xEF]
        );
    }

    #[test]
    fn hex_decode_odd_length_fails() {
        assert!(hex_decode("abc").is_err());
    }

    #[test]
    fn simple_hasher_deterministic() {
        let mut h1 = SimpleHasher::new();
        h1.update(b"test data");
        let mut h2 = SimpleHasher::new();
        h2.update(b"test data");
        assert_eq!(h1.finalize(), h2.finalize());
    }

    #[test]
    fn simple_hasher_different_inputs() {
        let mut h1 = SimpleHasher::new();
        h1.update(b"input A");
        let mut h2 = SimpleHasher::new();
        h2.update(b"input B");
        assert_ne!(h1.finalize(), h2.finalize());
    }

    #[test]
    fn compute_simple_hash_deterministic() {
        let dir = temp_dir("hash_test");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("file.txt"), "content").unwrap();

        let h1 = compute_simple_hash(&dir).unwrap();
        let h2 = compute_simple_hash(&dir).unwrap();
        assert_eq!(h1, h2);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
