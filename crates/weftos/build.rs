//! Embeds the repo's `agents/` package tree into the `weftos` binary so a
//! released `weftos init --claude|--grok|--codex` carries the agent set of its
//! own release (agent-directory ADR, D3). Also stamps the git commit.
//!
//! Only package directories (a top-level dir with `weftos-package.yaml`) and
//! `teams/` are embedded; `evals/` and dotfiles are skipped. When the crate is
//! built outside the repo (crates.io tarball) the table is empty and
//! `weftos init` needs `--from <dir>`.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let repo_root = manifest_dir.join("../..");
    let agents_dir = repo_root.join("agents");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("embedded_agents.rs");

    let mut files: Vec<(String, PathBuf)> = Vec::new();
    if agents_dir.is_dir() {
        println!("cargo:rerun-if-changed={}", agents_dir.display());
        collect_agents(&agents_dir, &mut files);
    }
    files.sort();

    let mut src = String::from("pub static EMBEDDED_AGENTS: &[(&str, &[u8])] = &[\n");
    for (rel, abs) in &files {
        let abs = abs.canonicalize().unwrap_or_else(|_| abs.clone());
        src.push_str(&format!(
            "    ({rel:?}, include_bytes!({:?})),\n",
            abs.display().to_string()
        ));
    }
    src.push_str("];\n");
    fs::File::create(&out)
        .unwrap()
        .write_all(src.as_bytes())
        .unwrap();

    println!("cargo:rerun-if-env-changed=WEFTOS_GIT_COMMIT");
    let commit = std::env::var("WEFTOS_GIT_COMMIT")
        .ok()
        .filter(|c| !c.is_empty())
        .or_else(|| git(&repo_root, &["rev-parse", "HEAD"]))
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=WEFTOS_GIT_COMMIT={commit}");
    watch_git_head(&repo_root);
}

/// Keep in sync with `src/init/source.rs`.
const LEGACY_DIRS: &[&str] = &[
    "clawft",
    "code-reviewer",
    "weftos",
    "weftos-ecc",
    "weftos-kernel",
    "weftos-mesh",
];

fn collect_agents(agents_dir: &Path, files: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = fs::read_dir(agents_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if !path.is_dir() || name.starts_with('.') || LEGACY_DIRS.contains(&name.as_str()) {
            continue;
        }
        if name == "teams" || path.join("weftos-package.yaml").is_file() {
            walk(&path, &name, files);
        }
    }
}

fn walk(dir: &Path, rel: &str, files: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || name == "evals" {
            continue;
        }
        let path = entry.path();
        let child = format!("{rel}/{name}");
        if path.is_dir() {
            walk(&path, &child, files);
        } else if path.is_file() {
            files.push((child, path));
        }
    }
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// Rebuild when HEAD moves so the stamped commit stays current.
fn watch_git_head(root: &Path) {
    let Some(git_dir) = git(root, &["rev-parse", "--absolute-git-dir"]) else {
        return;
    };
    let git_dir = PathBuf::from(git_dir);
    println!("cargo:rerun-if-changed={}", git_dir.join("HEAD").display());
    if let Some(head_ref) = git(root, &["symbolic-ref", "-q", "HEAD"]) {
        let common = git(root, &["rev-parse", "--git-common-dir"])
            .map(|c| {
                if Path::new(&c).is_absolute() {
                    PathBuf::from(c)
                } else {
                    root.join(c)
                }
            })
            .unwrap_or_else(|| git_dir.clone());
        let ref_path = common.join(&head_ref);
        if ref_path.exists() {
            println!("cargo:rerun-if-changed={}", ref_path.display());
        }
        println!(
            "cargo:rerun-if-changed={}",
            common.join("packed-refs").display()
        );
    }
}
