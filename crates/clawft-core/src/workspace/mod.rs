//! Workspace discovery, lifecycle management, and merged config loading.
//!
//! Provides the 4-step workspace discovery algorithm and [`WorkspaceManager`]
//! for creating, listing, loading, and deleting workspaces.
//!
//! Per-agent workspace isolation is in the [`agent`] submodule.
//! Config merge logic is in the [`config`] submodule.

pub mod agent;
mod config;

pub use config::{load_merged_config, load_merged_config_from};

use std::path::{Path, PathBuf};

use clawft_types::workspace::{WorkspaceEntry, WorkspaceRegistry};
use clawft_types::{ClawftError, Result};

// ── Discovery ────────────────────────────────────────────────────────────

/// 4-step workspace discovery algorithm.
///
/// 1. `$CLAWFT_WORKSPACE` environment variable
/// 2. Walk from `cwd` upward looking for a `.clawft/` directory
/// 3. Fall back to `~/.clawft/` (global workspace)
///
/// Returns `None` only if the home directory cannot be determined.
pub fn discover_workspace() -> Option<PathBuf> {
    // Step 1: env var
    if let Ok(ws) = std::env::var("CLAWFT_WORKSPACE") {
        let path = PathBuf::from(&ws);
        if path.join(".clawft").is_dir() {
            return Some(path);
        }
    }

    // Step 2: walk cwd upward
    if let Ok(cwd) = std::env::current_dir() {
        let mut dir: &Path = cwd.as_path();
        loop {
            if dir.join(".clawft").is_dir() {
                return Some(dir.to_path_buf());
            }
            match dir.parent() {
                Some(parent) => dir = parent,
                None => break,
            }
        }
    }

    // Step 3: global default
    #[cfg(feature = "native")]
    { dirs::home_dir().map(|h| h.join(".clawft")) }
    #[cfg(not(feature = "native"))]
    { Some(std::path::PathBuf::from(".clawft")) }
}

// ── Workspace status ─────────────────────────────────────────────────────

/// Summary of a workspace's current state.
pub struct WorkspaceStatus {
    /// Workspace display name.
    pub name: String,

    /// Absolute path to workspace root.
    pub path: PathBuf,

    /// Number of session files found in `.clawft/sessions/`.
    pub session_count: usize,

    /// Whether `.clawft/config.json` exists.
    pub has_config: bool,

    /// Whether `CLAWFT.md` exists at the workspace root.
    pub has_clawft_md: bool,
}

// ── WorkspaceManager ─────────────────────────────────────────────────────

/// The canonical subdirectories created inside `.clawft/`.
const WORKSPACE_SUBDIRS: &[&str] =
    &["sessions", "memory", "skills", "agents", "hooks"];

/// Manages workspace lifecycle (create, list, load, status, delete).
pub struct WorkspaceManager {
    /// Path to the global registry file (`~/.clawft/workspaces.json`).
    pub(crate) registry_path: PathBuf,

    /// In-memory copy of the registry.
    registry: WorkspaceRegistry,
}

impl WorkspaceManager {
    /// Create a new manager, loading the registry from the default path.
    ///
    /// The default registry path is `~/.clawft/workspaces.json`.
    pub fn new() -> Result<Self> {
        #[cfg(feature = "native")]
        let home =
            dirs::home_dir().ok_or_else(|| ClawftError::ConfigInvalid {
                reason: "cannot determine home directory".into(),
            })?;
        #[cfg(not(feature = "native"))]
        let home = std::path::PathBuf::from(".clawft");
        let registry_path = home.join(".clawft").join("workspaces.json");
        let registry = WorkspaceRegistry::load(&registry_path).map_err(|e| {
            ClawftError::ConfigInvalid {
                reason: format!("failed to load workspace registry: {e}"),
            }
        })?;

        Ok(Self {
            registry_path,
            registry,
        })
    }

    /// Create a new manager with an explicit registry path.
    ///
    /// Useful for testing.
    pub fn with_registry_path(registry_path: PathBuf) -> Result<Self> {
        let registry = WorkspaceRegistry::load(&registry_path).map_err(|e| {
            ClawftError::ConfigInvalid {
                reason: format!("failed to load workspace registry: {e}"),
            }
        })?;

        Ok(Self {
            registry_path,
            registry,
        })
    }

    /// Create a new workspace.
    ///
    /// Creates:
    /// 1. `.clawft/` and subdirectories (`sessions`, `memory`, `skills`,
    ///    `agents`, `hooks`)
    /// 2. `.clawft/config.json` with `{}`
    /// 3. `CLAWFT.md` with a starter template
    /// 4. Registers the workspace in the global registry
    ///
    /// Returns the absolute path to the workspace root.
    pub fn create(&mut self, name: &str, parent_dir: &Path) -> Result<PathBuf> {
        let ws_root = parent_dir.join(name);
        let dot_clawft = ws_root.join(".clawft");

        // Create .clawft/ and subdirectories
        for subdir in WORKSPACE_SUBDIRS {
            std::fs::create_dir_all(dot_clawft.join(subdir))?;
        }

        // Create config.json
        std::fs::write(dot_clawft.join("config.json"), "{}\n")?;

        // Create MEMORY.md and HISTORY.md (empty)
        std::fs::write(dot_clawft.join("MEMORY.md"), "")?;
        std::fs::write(dot_clawft.join("HISTORY.md"), "")?;

        // Create CLAWFT.md
        let clawft_md = format!(
            "# {name}\n\n\
             Workspace created by clawft.\n\n\
             ## Configuration\n\n\
             Edit `.clawft/config.json` to customize this workspace.\n"
        );
        std::fs::write(ws_root.join("CLAWFT.md"), clawft_md)?;

        // Register in global registry
        let now = chrono::Utc::now();
        let entry = WorkspaceEntry {
            name: name.into(),
            path: ws_root.clone(),
            last_accessed: Some(now),
            created_at: Some(now),
        };
        self.registry.register(entry);
        self.save_registry()?;

        // Chain event marker for workspace creation.
        crate::chain_event!(
            "workspace",
            crate::chain_event::EVENT_KIND_WORKSPACE_CREATE,
            {
                "name": name,
                "path": ws_root.display()
            }
        );

        Ok(ws_root)
    }

    /// List all registered workspaces.
    pub fn list(&self) -> Vec<&WorkspaceEntry> {
        self.registry.workspaces.iter().collect()
    }

    /// Load a workspace by name or path string.
    ///
    /// Returns the workspace root path if found.
    ///
    /// As a side effect, when the workspace resolves to a registry entry
    /// (whether by name or by absolute path), `last_accessed` is bumped to
    /// `now()` and the registry is persisted atomically. Path-only loads
    /// that miss the registry do not touch state.
    pub fn load(&mut self, name_or_path: &str) -> Result<PathBuf> {
        // Try by name first
        if let Some(entry) = self.registry.find_by_name(name_or_path) {
            let path = entry.path.clone();
            self.touch_last_accessed_by_name(name_or_path)?;
            return Ok(path);
        }

        // Try as a path
        let path = PathBuf::from(name_or_path);
        if path.join(".clawft").is_dir() {
            // If this path is also in the registry, bump its last_accessed.
            if let Some(name) =
                self.registry.find_by_path(&path).map(|e| e.name.clone())
            {
                self.touch_last_accessed_by_name(&name)?;
            }
            return Ok(path);
        }

        Err(ClawftError::ConfigInvalid {
            reason: format!("workspace not found: {name_or_path}"),
        })
    }

    /// Update `last_accessed` to `now()` for the named entry and persist.
    ///
    /// No-op if the name is not registered (called only after a successful
    /// lookup, but defensive for concurrent removal).
    fn touch_last_accessed_by_name(&mut self, name: &str) -> Result<()> {
        let now = chrono::Utc::now();
        let mut changed = false;
        for entry in &mut self.registry.workspaces {
            if entry.name == name {
                entry.last_accessed = Some(now);
                changed = true;
                break;
            }
        }
        if changed {
            self.save_registry()?;
        }
        Ok(())
    }

    /// Get the status of a workspace at the given path.
    pub fn status(&self, path: &Path) -> Result<WorkspaceStatus> {
        let dot_clawft = path.join(".clawft");
        let name = self
            .registry
            .find_by_path(path)
            .map(|e| e.name.clone())
            .unwrap_or_else(|| {
                path.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "unknown".into())
            });

        let sessions_dir = dot_clawft.join("sessions");
        let session_count = if sessions_dir.is_dir() {
            std::fs::read_dir(&sessions_dir)
                .map(|rd| rd.count())
                .unwrap_or(0)
        } else {
            0
        };

        Ok(WorkspaceStatus {
            name,
            path: path.to_path_buf(),
            session_count,
            has_config: dot_clawft.join("config.json").exists(),
            has_clawft_md: path.join("CLAWFT.md").exists(),
        })
    }

    /// Delete a workspace by name.
    ///
    /// Removes the entry from the registry but does NOT delete files
    /// from disk (that is the caller's responsibility).
    pub fn delete(&mut self, name: &str) -> Result<()> {
        if !self.registry.remove_by_name(name) {
            return Err(ClawftError::ConfigInvalid {
                reason: format!("workspace not found: {name}"),
            });
        }
        self.save_registry()?;
        Ok(())
    }

    /// Persist the registry to disk.
    fn save_registry(&self) -> Result<()> {
        self.registry
            .save(&self.registry_path)
            .map_err(|e| ClawftError::ConfigInvalid {
                reason: format!("failed to save workspace registry: {e}"),
            })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Monotonic counter to give each test a unique temp directory.
    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    // ── Discovery tests ──────────────────────────────────────────────

    #[test]
    fn discover_workspace_returns_some() {
        let result = discover_workspace();
        assert!(result.is_some(), "discover_workspace should return Some");
    }

    #[test]
    fn discover_workspace_env_var() {
        let n = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir()
            .join(format!("clawft-test-discover-env-{n}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".clawft")).unwrap();

        let result =
            temp_env::with_var("CLAWFT_WORKSPACE", Some(dir.to_str().unwrap()), || {
                discover_workspace()
            });

        assert_eq!(result, Some(dir.clone()));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn discover_workspace_env_var_invalid_skipped() {
        let result = temp_env::with_var(
            "CLAWFT_WORKSPACE",
            Some("/nonexistent/path/for/test"),
            discover_workspace,
        );

        assert!(result.is_some());
        assert_ne!(
            result.unwrap(),
            PathBuf::from("/nonexistent/path/for/test")
        );
    }

    // ── WorkspaceManager tests ───────────────────────────────────────

    /// Create a unique temp directory and registry path for each test.
    pub(crate) fn temp_registry(label: &str) -> (PathBuf, PathBuf) {
        let n = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir()
            .join(format!("clawft-test-wm-{label}-{n}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let registry_path = dir.join("workspaces.json");
        (dir, registry_path)
    }

    #[test]
    fn workspace_manager_create_directories() {
        let (dir, registry_path) = temp_registry("create-dirs");
        let mut wm =
            WorkspaceManager::with_registry_path(registry_path).unwrap();

        let ws_path = wm.create("test-ws", &dir).unwrap();
        let dot_clawft = ws_path.join(".clawft");

        for subdir in WORKSPACE_SUBDIRS {
            assert!(
                dot_clawft.join(subdir).is_dir(),
                "missing subdir: {subdir}"
            );
        }

        assert!(dot_clawft.join("config.json").exists());
        assert!(dot_clawft.join("MEMORY.md").exists());
        assert!(dot_clawft.join("HISTORY.md").exists());
        assert!(ws_path.join("CLAWFT.md").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn workspace_manager_create_registers_in_registry() {
        let (dir, registry_path) = temp_registry("create-reg");
        let mut wm =
            WorkspaceManager::with_registry_path(registry_path.clone()).unwrap();

        wm.create("reg-test", &dir).unwrap();

        let loaded = WorkspaceRegistry::load(&registry_path).unwrap();
        assert!(loaded.find_by_name("reg-test").is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn workspace_manager_list() {
        let (dir, registry_path) = temp_registry("list");
        let mut wm =
            WorkspaceManager::with_registry_path(registry_path).unwrap();

        assert!(wm.list().is_empty(), "fresh registry should be empty");

        wm.create("ws-a", &dir).unwrap();
        wm.create("ws-b", &dir).unwrap();

        let list = wm.list();
        assert_eq!(list.len(), 2);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn workspace_manager_load_by_name() {
        let (dir, registry_path) = temp_registry("load-name");
        let mut wm =
            WorkspaceManager::with_registry_path(registry_path).unwrap();

        let created = wm.create("load-test", &dir).unwrap();
        let loaded = wm.load("load-test").unwrap();
        assert_eq!(created, loaded);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn workspace_manager_load_by_path() {
        let (dir, registry_path) = temp_registry("load-path");
        let mut wm =
            WorkspaceManager::with_registry_path(registry_path).unwrap();

        let created = wm.create("path-test", &dir).unwrap();
        let loaded = wm.load(created.to_str().unwrap()).unwrap();
        assert_eq!(created, loaded);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn workspace_manager_load_not_found() {
        let (dir, registry_path) = temp_registry("load-notfound");
        let mut wm =
            WorkspaceManager::with_registry_path(registry_path).unwrap();

        let result = wm.load("nonexistent");
        assert!(result.is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn workspace_manager_status() {
        let (dir, registry_path) = temp_registry("status");
        let mut wm =
            WorkspaceManager::with_registry_path(registry_path).unwrap();

        let ws_path = wm.create("status-test", &dir).unwrap();
        let status = wm.status(&ws_path).unwrap();

        assert_eq!(status.name, "status-test");
        assert_eq!(status.path, ws_path);
        assert_eq!(status.session_count, 0);
        assert!(status.has_config);
        assert!(status.has_clawft_md);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn workspace_manager_load_by_name_bumps_last_accessed() {
        let (dir, registry_path) = temp_registry("load-bumps-name");
        let mut wm = WorkspaceManager::with_registry_path(registry_path.clone())
            .unwrap();

        wm.create("recency", &dir).unwrap();
        let before = wm
            .registry
            .find_by_name("recency")
            .and_then(|e| e.last_accessed)
            .expect("create populates last_accessed");

        // Sleep just enough that the timestamp comparison is meaningful.
        std::thread::sleep(std::time::Duration::from_millis(10));
        wm.load("recency").unwrap();

        let after = wm
            .registry
            .find_by_name("recency")
            .and_then(|e| e.last_accessed)
            .expect("load preserves last_accessed");
        assert!(
            after > before,
            "load should advance last_accessed: before={before} after={after}"
        );

        // And the bump must be persisted.
        let on_disk = WorkspaceRegistry::load(&registry_path).unwrap();
        let persisted = on_disk
            .find_by_name("recency")
            .and_then(|e| e.last_accessed)
            .expect("persisted entry has last_accessed");
        assert_eq!(persisted, after);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn workspace_manager_load_by_path_bumps_last_accessed_when_registered() {
        let (dir, registry_path) = temp_registry("load-bumps-path");
        let mut wm = WorkspaceManager::with_registry_path(registry_path)
            .unwrap();

        let ws_path = wm.create("path-recency", &dir).unwrap();
        let before = wm
            .registry
            .find_by_name("path-recency")
            .and_then(|e| e.last_accessed)
            .unwrap();

        std::thread::sleep(std::time::Duration::from_millis(10));
        wm.load(ws_path.to_str().unwrap()).unwrap();

        let after = wm
            .registry
            .find_by_name("path-recency")
            .and_then(|e| e.last_accessed)
            .unwrap();
        assert!(after > before);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn workspace_manager_load_orders_by_recency_after_use() {
        let (dir, registry_path) = temp_registry("load-recency-order");
        let mut wm = WorkspaceManager::with_registry_path(registry_path)
            .unwrap();

        // Create A then B; load A — A should now be more recent than B.
        wm.create("ws-a", &dir).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        wm.create("ws-b", &dir).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        wm.load("ws-a").unwrap();

        let a = wm
            .registry
            .find_by_name("ws-a")
            .and_then(|e| e.last_accessed)
            .unwrap();
        let b = wm
            .registry
            .find_by_name("ws-b")
            .and_then(|e| e.last_accessed)
            .unwrap();
        assert!(
            a > b,
            "ws-a was loaded after ws-b created; a should be newer"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn workspace_manager_delete() {
        let (dir, registry_path) = temp_registry("delete");
        let mut wm =
            WorkspaceManager::with_registry_path(registry_path).unwrap();

        wm.create("del-test", &dir).unwrap();
        assert_eq!(wm.list().len(), 1);

        wm.delete("del-test").unwrap();
        assert!(wm.list().is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn workspace_manager_delete_not_found() {
        let (dir, registry_path) = temp_registry("delete-notfound");
        let mut wm =
            WorkspaceManager::with_registry_path(registry_path).unwrap();

        let result = wm.delete("nonexistent");
        assert!(result.is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
