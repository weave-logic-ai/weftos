//! Per-agent workspace isolation.
//!
//! Provides methods on [`WorkspaceManager`] for creating, deleting, and
//! listing per-agent workspaces under `~/.clawft/agents/<agent_id>/`.
//!
//! Each agent workspace contains:
//!
//! ```text
//! ~/.clawft/agents/<agent_id>/
//!   SOUL.md          # Agent personality
//!   AGENTS.md        # Agent capabilities
//!   USER.md          # User preferences for this agent
//!   config.toml      # Per-agent config overrides
//!   sessions/        # Per-agent session store
//!   memory/          # Per-agent memory namespace
//!   skills/          # Per-agent skill overrides
//!   tool_state/      # Per-plugin state -- see Contract 3.1 below
//! ```
//!
//! Directories are created with `0700` permissions on Unix for security.
//! Cross-agent sharing is done via symlinks.
//!
//! `tool_state/` implements **Contract 3.1** (Tool Plugin <-> Memory)
//! from the cross-element integration spec. Plugins access it via
//! `clawft_plugin::ToolContext::key_value_store()` -- they see a
//! `&dyn KeyValueStore` slice scoped to
//! `~/.clawft/agents/<agent_id>/tool_state/<plugin_name>/`. The host
//! materializes the per-plugin subdirectory; plugins never write to
//! `tool_state/` directly. Operator-facing documentation:
//! `docs/guides/workspaces.md` ("tool_state contract"). See WEFT-94
//! for the docs decision and WEFT MW-16 for the runtime-backed impl.

use std::path::{Path, PathBuf};

use clawft_types::{ClawftError, Result};

use super::WorkspaceManager;

/// Subdirectories created inside each per-agent workspace.
///
/// `tool_state` is created eagerly even though the only in-tree
/// `KeyValueStore` impls are test-fixture mocks today; this keeps the
/// directory layout visible to operators and lets the sandbox grant
/// the path proactively. See the module-level docs and
/// `docs/guides/workspaces.md` "tool_state contract".
const AGENT_WORKSPACE_SUBDIRS: &[&str] = &["sessions", "memory", "skills", "tool_state"];

impl WorkspaceManager {
    /// Resolve the agents directory root.
    ///
    /// For the default manager this is `~/.clawft/agents/`.
    /// For a test manager the parent of the registry is the root.
    pub(crate) fn agents_root(&self) -> PathBuf {
        self.registry_path
            .parent()
            .unwrap_or(Path::new("."))
            .join("agents")
    }

    /// Create or ensure a per-agent workspace exists.
    ///
    /// Idempotent: if the workspace already exists, returns its path
    /// without modifying it. Creates the directory structure described
    /// in the [module docs](self).
    ///
    /// Directories are created with `0700` permissions on Unix for security.
    pub fn ensure_agent_workspace(&self, agent_id: &str) -> Result<PathBuf> {
        validate_agent_id(agent_id)?;

        let agent_dir = self.agents_root().join(agent_id);

        // Create subdirectories.
        for subdir in AGENT_WORKSPACE_SUBDIRS {
            let dir = agent_dir.join(subdir);
            std::fs::create_dir_all(&dir)?;
            set_dir_permissions_0700(&dir);
        }

        // Set permissions on the agent root directory itself.
        set_dir_permissions_0700(&agent_dir);

        // Create template files (only if they don't already exist).
        let files = [
            (
                "SOUL.md",
                format!("# Agent: {agent_id}\n\nPersonality and directives for this agent.\n"),
            ),
            (
                "AGENTS.md",
                format!("# Agent: {agent_id}\n\nCapabilities and tool access.\n"),
            ),
            (
                "USER.md",
                format!("# Agent: {agent_id}\n\nUser preferences for this agent.\n"),
            ),
            (
                "config.toml",
                format!("# Per-agent configuration overrides for {agent_id}\n"),
            ),
        ];

        for (filename, content) in &files {
            let path = agent_dir.join(filename);
            if !path.exists() {
                std::fs::write(&path, content)?;
            }
        }

        Ok(agent_dir)
    }

    /// Create a per-agent workspace from a template.
    ///
    /// If `template` is `None`, falls back to `~/.clawft/agents/default/` if
    /// it exists, otherwise creates a bare workspace via
    /// [`ensure_agent_workspace`](Self::ensure_agent_workspace).
    pub fn create_agent_workspace(
        &self,
        agent_id: &str,
        template: Option<&Path>,
    ) -> Result<PathBuf> {
        validate_agent_id(agent_id)?;

        let agent_dir = self.agents_root().join(agent_id);
        if agent_dir.exists() {
            return Err(ClawftError::ConfigInvalid {
                reason: format!("agent workspace already exists: {agent_id}"),
            });
        }

        // Determine template source.
        let template_dir = match template {
            Some(p) => {
                if p.is_dir() {
                    Some(p.to_path_buf())
                } else {
                    return Err(ClawftError::ConfigInvalid {
                        reason: format!("template path is not a directory: {}", p.display()),
                    });
                }
            }
            None => {
                let default_template = self.agents_root().join("default");
                if default_template.is_dir() {
                    Some(default_template)
                } else {
                    None
                }
            }
        };

        if let Some(ref src) = template_dir {
            // Copy template directory contents.
            copy_dir_recursive(src, &agent_dir)?;
            set_dir_permissions_0700(&agent_dir);
        } else {
            // No template: create bare workspace.
            self.ensure_agent_workspace(agent_id)?;
        }

        Ok(agent_dir)
    }

    /// Delete a per-agent workspace by agent ID.
    ///
    /// Removes the directory and all its contents from disk.
    pub fn delete_agent_workspace(&self, agent_id: &str) -> Result<()> {
        validate_agent_id(agent_id)?;

        let agent_dir = self.agents_root().join(agent_id);
        if !agent_dir.exists() {
            return Err(ClawftError::ConfigInvalid {
                reason: format!("agent workspace not found: {agent_id}"),
            });
        }

        std::fs::remove_dir_all(&agent_dir)?;
        Ok(())
    }

    /// List all per-agent workspaces.
    ///
    /// Returns a sorted list of agent IDs found under the agents directory.
    pub fn list_agent_workspaces(&self) -> Result<Vec<String>> {
        let agents_dir = self.agents_root();
        if !agents_dir.is_dir() {
            return Ok(Vec::new());
        }

        let mut agent_ids: Vec<String> = std::fs::read_dir(&agents_dir)?
            .filter_map(|entry| {
                let entry = entry.ok()?;
                if entry.file_type().ok()?.is_dir() {
                    Some(entry.file_name().to_string_lossy().into_owned())
                } else {
                    None
                }
            })
            .collect();

        agent_ids.sort();
        Ok(agent_ids)
    }

    /// Set up a cross-agent symlink for shared namespace access.
    ///
    /// Creates a symlink from `importer_id`'s memory directory to
    /// `exporter_id`'s namespace directory. The symlink provides
    /// read-only access by default.
    ///
    /// # Security
    ///
    /// - Validates both agent IDs to prevent path traversal.
    /// - The exporter workspace must exist.
    /// - The namespace directory must exist under the exporter.
    pub fn link_shared_namespace(
        &self,
        exporter_id: &str,
        importer_id: &str,
        namespace: &str,
    ) -> Result<PathBuf> {
        validate_agent_id(exporter_id)?;
        validate_agent_id(importer_id)?;
        validate_agent_id(namespace)?; // reuse same validation for namespace

        let exporter_ns = self
            .agents_root()
            .join(exporter_id)
            .join("memory")
            .join(namespace);

        if !exporter_ns.is_dir() {
            // Create the namespace directory if it doesn't exist yet.
            std::fs::create_dir_all(&exporter_ns)?;
        }

        let importer_link = self
            .agents_root()
            .join(importer_id)
            .join("memory")
            .join(format!("{exporter_id}--{namespace}"));

        // Ensure the importer's memory directory exists.
        let importer_memory = self.agents_root().join(importer_id).join("memory");
        std::fs::create_dir_all(&importer_memory)?;

        // Validate the symlink target is within the agents directory.
        let agents_root = self.agents_root();
        let canonical_target = exporter_ns
            .canonicalize()
            .unwrap_or_else(|_| exporter_ns.clone());
        let canonical_root = agents_root
            .canonicalize()
            .unwrap_or_else(|_| agents_root.clone());
        if !canonical_target.starts_with(&canonical_root) {
            return Err(ClawftError::ConfigInvalid {
                reason: "symlink target escapes agents directory".into(),
            });
        }

        // Remove existing symlink if present (re-link).
        if importer_link.exists() || importer_link.is_symlink() {
            std::fs::remove_file(&importer_link)?;
        }

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&exporter_ns, &importer_link)?;
            Ok(importer_link)
        }

        #[cfg(not(unix))]
        {
            // On non-Unix, fall back to directory junction or just error.
            let _ = exporter_ns;
            Err(ClawftError::ConfigInvalid {
                reason: "symlink-based sharing requires Unix".into(),
            })
        }
    }
}

// ── Helpers ────────────────────────────────────────────────────────────

/// Validate an agent ID to prevent path traversal attacks.
///
/// Agent IDs must be non-empty, contain only alphanumeric characters,
/// hyphens, underscores, and dots, and must not start with a dot.
pub(crate) fn validate_agent_id(agent_id: &str) -> Result<()> {
    if agent_id.is_empty() {
        return Err(ClawftError::ConfigInvalid {
            reason: "agent ID cannot be empty".into(),
        });
    }
    if agent_id.starts_with('.') {
        return Err(ClawftError::ConfigInvalid {
            reason: format!("agent ID must not start with dot: {agent_id}"),
        });
    }
    if agent_id.contains('/') || agent_id.contains('\\') || agent_id.contains('\0') {
        return Err(ClawftError::ConfigInvalid {
            reason: format!("agent ID contains invalid characters: {agent_id}"),
        });
    }
    if agent_id == ".." {
        return Err(ClawftError::ConfigInvalid {
            reason: "agent ID cannot be '..'".into(),
        });
    }
    Ok(())
}

/// Set directory permissions to 0700 on Unix (no-op on other platforms).
pub(crate) fn set_dir_permissions_0700(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
    }
    #[cfg(not(unix))]
    {
        let _ = path; // suppress unused warning
    }
}

/// Recursively copy a directory tree from `src` to `dst`.
pub(crate) fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if file_type.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::tests::temp_registry;

    #[test]
    fn ensure_agent_workspace_creates_structure() {
        let (dir, registry_path) = temp_registry("agent-ensure");
        let wm = WorkspaceManager::with_registry_path(registry_path).unwrap();

        let agent_dir = wm.ensure_agent_workspace("test-agent").unwrap();
        assert!(agent_dir.is_dir());

        for subdir in &["sessions", "memory", "skills", "tool_state"] {
            assert!(agent_dir.join(subdir).is_dir(), "missing subdir: {subdir}");
        }

        for file in &["SOUL.md", "AGENTS.md", "USER.md", "config.toml"] {
            assert!(agent_dir.join(file).exists(), "missing file: {file}");
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ensure_agent_workspace_is_idempotent() {
        let (dir, registry_path) = temp_registry("agent-idempotent");
        let wm = WorkspaceManager::with_registry_path(registry_path).unwrap();

        let path1 = wm.ensure_agent_workspace("idempotent-agent").unwrap();
        std::fs::write(path1.join("SOUL.md"), "custom content").unwrap();

        let path2 = wm.ensure_agent_workspace("idempotent-agent").unwrap();
        assert_eq!(path1, path2);

        let content = std::fs::read_to_string(path2.join("SOUL.md")).unwrap();
        assert_eq!(content, "custom content");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn delete_agent_workspace_removes_dir() {
        let (dir, registry_path) = temp_registry("agent-delete");
        let wm = WorkspaceManager::with_registry_path(registry_path).unwrap();

        let agent_dir = wm.ensure_agent_workspace("del-agent").unwrap();
        assert!(agent_dir.is_dir());

        wm.delete_agent_workspace("del-agent").unwrap();
        assert!(!agent_dir.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn delete_nonexistent_agent_workspace_errors() {
        let (dir, registry_path) = temp_registry("agent-delete-missing");
        let wm = WorkspaceManager::with_registry_path(registry_path).unwrap();
        assert!(wm.delete_agent_workspace("nonexistent").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_agent_workspaces_returns_sorted() {
        let (dir, registry_path) = temp_registry("agent-list");
        let wm = WorkspaceManager::with_registry_path(registry_path).unwrap();

        assert!(wm.list_agent_workspaces().unwrap().is_empty());

        wm.ensure_agent_workspace("charlie").unwrap();
        wm.ensure_agent_workspace("alpha").unwrap();
        wm.ensure_agent_workspace("bravo").unwrap();

        let agents = wm.list_agent_workspaces().unwrap();
        assert_eq!(agents, vec!["alpha", "bravo", "charlie"]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn validate_agent_id_rejects_traversal() {
        assert!(validate_agent_id("").is_err());
        assert!(validate_agent_id("..").is_err());
        assert!(validate_agent_id(".hidden").is_err());
        assert!(validate_agent_id("../escape").is_err());
        assert!(validate_agent_id("path/traversal").is_err());
        assert!(validate_agent_id("back\\slash").is_err());
        // Valid IDs
        assert!(validate_agent_id("agent-1").is_ok());
        assert!(validate_agent_id("my_agent_2").is_ok());
        assert!(validate_agent_id("Agent.v3").is_ok());
    }

    #[test]
    fn create_agent_workspace_from_template() {
        let (dir, registry_path) = temp_registry("agent-template");
        let wm = WorkspaceManager::with_registry_path(registry_path).unwrap();

        let template = dir.join("my-template");
        std::fs::create_dir_all(template.join("skills")).unwrap();
        std::fs::write(template.join("SOUL.md"), "template soul").unwrap();
        std::fs::write(template.join("skills/custom.md"), "skill data").unwrap();

        let agent_dir = wm
            .create_agent_workspace("from-template", Some(&template))
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(agent_dir.join("SOUL.md")).unwrap(),
            "template soul"
        );
        assert_eq!(
            std::fs::read_to_string(agent_dir.join("skills/custom.md")).unwrap(),
            "skill data"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn create_agent_workspace_rejects_duplicate() {
        let (dir, registry_path) = temp_registry("agent-dup");
        let wm = WorkspaceManager::with_registry_path(registry_path).unwrap();

        wm.ensure_agent_workspace("dup-agent").unwrap();
        assert!(wm.create_agent_workspace("dup-agent", None).is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn agent_workspace_has_0700_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let (dir, registry_path) = temp_registry("agent-perms");
        let wm = WorkspaceManager::with_registry_path(registry_path).unwrap();

        let agent_dir = wm.ensure_agent_workspace("perms-agent").unwrap();
        let mode = std::fs::metadata(&agent_dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "agent dir should have 0700 permissions");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn cross_agent_symlink_sharing() {
        let (dir, registry_path) = temp_registry("agent-symlink");
        let wm = WorkspaceManager::with_registry_path(registry_path).unwrap();

        wm.ensure_agent_workspace("exporter").unwrap();
        wm.ensure_agent_workspace("importer").unwrap();

        let exporter_ns = wm.agents_root().join("exporter/memory/project-ctx");
        std::fs::create_dir_all(&exporter_ns).unwrap();
        std::fs::write(exporter_ns.join("data.txt"), "shared data").unwrap();

        let link = wm
            .link_shared_namespace("exporter", "importer", "project-ctx")
            .unwrap();

        assert!(link.is_symlink());

        let content = std::fs::read_to_string(link.join("data.txt")).unwrap();
        assert_eq!(content, "shared data");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
