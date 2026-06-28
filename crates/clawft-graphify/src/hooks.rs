//! Git hook integration: install/uninstall graphify post-commit and
//! post-checkout hooks that call `weaver graphify rebuild`.
//!
//! Ported from Python `graphify/hooks.py`.

use std::path::{Path, PathBuf};

use regex::Regex;

use crate::GraphifyError;

const HOOK_MARKER_START: &str = "# graphify-hook-start";
const HOOK_MARKER_END: &str = "# graphify-hook-end";
const CHECKOUT_MARKER_START: &str = "# graphify-checkout-hook-start";
const CHECKOUT_MARKER_END: &str = "# graphify-checkout-hook-end";

const POST_COMMIT_SCRIPT: &str = r#"# graphify-hook-start
# Auto-rebuilds the knowledge graph after each commit.
# Installed by: weaver graphify hooks install

CHANGED=$(git diff --name-only HEAD~1 HEAD 2>/dev/null || git diff --name-only HEAD 2>/dev/null)
if [ -z "$CHANGED" ]; then
    exit 0
fi

CODE_CHANGED=$(echo "$CHANGED" | grep -E '\.(py|ts|js|go|rs|java|cpp|c|rb|swift|kt|cs|scala|php|cc|cxx|hpp|h|kts)$' || true)
if [ -z "$CODE_CHANGED" ]; then
    exit 0
fi

echo "[graphify hook] Code files changed -- rebuilding graph..."
weaver graphify rebuild 2>/dev/null || echo "[graphify hook] Rebuild failed (weaver not in PATH?)"
# graphify-hook-end
"#;

const POST_CHECKOUT_SCRIPT: &str = r#"# graphify-checkout-hook-start
# Auto-rebuilds the knowledge graph when switching branches.
# Installed by: weaver graphify hooks install

PREV_HEAD=$1
NEW_HEAD=$2
BRANCH_SWITCH=$3

if [ "$BRANCH_SWITCH" != "1" ]; then
    exit 0
fi

if [ ! -d "graphify-out" ]; then
    exit 0
fi

echo "[graphify] Branch switched -- rebuilding knowledge graph..."
weaver graphify rebuild 2>/dev/null || echo "[graphify] Rebuild failed (weaver not in PATH?)"
# graphify-checkout-hook-end
"#;

/// Walk up the directory tree to find the `.git` directory.
pub fn find_git_root(start: &Path) -> Option<PathBuf> {
    let mut current = if start.is_absolute() {
        start.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(start)
    };
    loop {
        if current.join(".git").exists() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

fn install_hook(
    hooks_dir: &Path,
    name: &str,
    script: &str,
    marker: &str,
) -> Result<String, GraphifyError> {
    let hook_path = hooks_dir.join(name);

    if hook_path.exists() {
        let content = std::fs::read_to_string(&hook_path)?;
        if content.contains(marker) {
            return Ok(format!("already installed at {}", hook_path.display()));
        }
        let new_content = format!("{}\n\n{}", content.trim_end(), script);
        std::fs::write(&hook_path, new_content)?;
        return Ok(format!(
            "appended to existing {name} hook at {}",
            hook_path.display()
        ));
    }

    let content = format!("#!/bin/bash\n{script}");
    std::fs::write(&hook_path, &content)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook_path, std::fs::Permissions::from_mode(0o755))?;
    }

    Ok(format!("installed at {}", hook_path.display()))
}

fn uninstall_hook(
    hooks_dir: &Path,
    name: &str,
    marker_start: &str,
    marker_end: &str,
) -> Result<String, GraphifyError> {
    let hook_path = hooks_dir.join(name);

    if !hook_path.exists() {
        return Ok(format!("no {name} hook found -- nothing to remove."));
    }

    let content = std::fs::read_to_string(&hook_path)?;
    if !content.contains(marker_start) {
        return Ok(format!(
            "graphify hook not found in {name} -- nothing to remove."
        ));
    }

    let pattern = format!(
        r"{}.*?{}\n?",
        regex::escape(marker_start),
        regex::escape(marker_end)
    );
    let re = Regex::new(&pattern).map_err(|e| GraphifyError::HookError(format!("regex: {e}")))?;
    let new_content = re.replace(&content, "").trim().to_string();

    if new_content.is_empty() || new_content == "#!/bin/bash" {
        std::fs::remove_file(&hook_path)?;
        return Ok(format!("removed {name} hook at {}", hook_path.display()));
    }

    std::fs::write(&hook_path, format!("{new_content}\n"))?;
    Ok(format!(
        "graphify removed from {name} at {} (other content preserved)",
        hook_path.display()
    ))
}

/// Chain event kind for hook install / uninstall.
pub const EVENT_KIND_GRAPHIFY_HOOK: &str = "graphify.hook";

/// Install graphify post-commit and post-checkout hooks.
pub fn install_hooks(repo_root: &Path) -> Result<String, GraphifyError> {
    let root = find_git_root(repo_root).ok_or_else(|| {
        GraphifyError::HookError(format!(
            "no git repository found at or above {}",
            repo_root.display()
        ))
    })?;

    let hooks_dir = root.join(".git").join("hooks");
    std::fs::create_dir_all(&hooks_dir)?;

    let commit_msg = install_hook(
        &hooks_dir,
        "post-commit",
        POST_COMMIT_SCRIPT,
        HOOK_MARKER_START,
    )?;
    let checkout_msg = install_hook(
        &hooks_dir,
        "post-checkout",
        POST_CHECKOUT_SCRIPT,
        CHECKOUT_MARKER_START,
    )?;

    // Chain event marker -- daemon subscriber forwards to ExoChain.
    tracing::info!(
        target: "chain_event",
        source = "graphify",
        kind = EVENT_KIND_GRAPHIFY_HOOK,
        repo_root = %root.display(),
        action = "install",
        "chain"
    );

    Ok(format!(
        "post-commit: {commit_msg}\npost-checkout: {checkout_msg}"
    ))
}

/// Remove graphify post-commit and post-checkout hooks.
pub fn uninstall_hooks(repo_root: &Path) -> Result<String, GraphifyError> {
    let root = find_git_root(repo_root).ok_or_else(|| {
        GraphifyError::HookError(format!(
            "no git repository found at or above {}",
            repo_root.display()
        ))
    })?;

    let hooks_dir = root.join(".git").join("hooks");
    let commit_msg = uninstall_hook(
        &hooks_dir,
        "post-commit",
        HOOK_MARKER_START,
        HOOK_MARKER_END,
    )?;
    let checkout_msg = uninstall_hook(
        &hooks_dir,
        "post-checkout",
        CHECKOUT_MARKER_START,
        CHECKOUT_MARKER_END,
    )?;

    // Chain event marker -- daemon subscriber forwards to ExoChain.
    tracing::info!(
        target: "chain_event",
        source = "graphify",
        kind = EVENT_KIND_GRAPHIFY_HOOK,
        repo_root = %root.display(),
        action = "uninstall",
        "chain"
    );

    Ok(format!(
        "post-commit: {commit_msg}\npost-checkout: {checkout_msg}"
    ))
}

/// Check the installation status of graphify hooks.
pub fn hook_status(repo_root: &Path) -> Result<String, GraphifyError> {
    let root = match find_git_root(repo_root) {
        Some(r) => r,
        None => return Ok("Not in a git repository.".into()),
    };

    let hooks_dir = root.join(".git").join("hooks");

    let check = |name: &str, marker: &str| -> String {
        let path = hooks_dir.join(name);
        if !path.exists() {
            return "not installed".into();
        }
        match std::fs::read_to_string(&path) {
            Ok(content) if content.contains(marker) => "installed".into(),
            Ok(_) => "not installed (hook exists but graphify not found)".into(),
            Err(_) => "error reading hook".into(),
        }
    };

    let commit = check("post-commit", HOOK_MARKER_START);
    let checkout = check("post-checkout", CHECKOUT_MARKER_START);
    Ok(format!("post-commit: {commit}\npost-checkout: {checkout}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_git_root_from_subdir() {
        let _ = find_git_root(Path::new("."));
    }

    #[test]
    fn install_and_uninstall_hooks() {
        let temp = std::env::temp_dir().join("graphify_test_hooks");
        let git_dir = temp.join(".git").join("hooks");
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&git_dir).unwrap();

        let msg = install_hook(
            &git_dir,
            "post-commit",
            POST_COMMIT_SCRIPT,
            HOOK_MARKER_START,
        )
        .unwrap();
        assert!(msg.contains("installed"));

        let hook_path = git_dir.join("post-commit");
        assert!(hook_path.exists());
        let content = std::fs::read_to_string(&hook_path).unwrap();
        assert!(content.contains(HOOK_MARKER_START));
        assert!(content.contains("weaver graphify rebuild"));

        let msg2 = install_hook(
            &git_dir,
            "post-commit",
            POST_COMMIT_SCRIPT,
            HOOK_MARKER_START,
        )
        .unwrap();
        assert!(msg2.contains("already installed"));

        let msg3 =
            uninstall_hook(&git_dir, "post-commit", HOOK_MARKER_START, HOOK_MARKER_END).unwrap();
        assert!(msg3.contains("removed"));

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn hook_status_no_repo() {
        let status = hook_status(Path::new("/nonexistent")).unwrap();
        assert!(status.contains("Not in a git repository"));
    }
}
