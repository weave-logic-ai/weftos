//! File-system watcher for automatic skill hot-reload.
//!
//! Watches skill directories for changes and triggers [`SkillRegistry`] rebuilds
//! with debouncing to avoid redundant work during rapid file operations.
//!
//! # Architecture
//!
//! The watcher runs as a background `tokio::spawn` task that:
//! 1. Receives events from the [`notify`] crate via an `mpsc` channel.
//! 2. Debounces rapid changes (default: 500ms).
//! 3. Acquires a write lock on [`SharedSkillRegistry`] only during rebuild.
//! 4. Releases the write lock immediately after rebuild completes.
//!
//! # Concurrency
//!
//! - Reads: Every agent loop iteration reads skills (high frequency).
//! - Writes: Only on file-system changes (low frequency).
//! - [`tokio::sync::RwLock`] is appropriate: many concurrent readers, rare
//!   exclusive writers. Write starvation is not a concern given low write
//!   frequency.

use std::path::PathBuf;
use std::time::Duration;

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use clawft_types::skill::SkillDefinition;

use super::skills_v2::SharedSkillRegistry;

/// Configuration for the skill file-system watcher.
#[derive(Debug, Clone)]
pub struct SkillWatcherConfig {
    /// Workspace skill directory (highest priority).
    pub workspace_dir: Option<PathBuf>,
    /// User skill directory (medium priority).
    pub user_dir: Option<PathBuf>,
    /// Additional directories to watch (e.g. plugin-shipped skill dirs).
    pub extra_dirs: Vec<PathBuf>,
    /// Debounce duration for rapid file changes.
    pub debounce: Duration,
    /// Built-in skills (lowest priority, compiled into binary).
    pub builtin_skills: Vec<SkillDefinition>,
    /// Whether workspace skills are trusted.
    pub trust_workspace: bool,
}

impl Default for SkillWatcherConfig {
    fn default() -> Self {
        Self {
            workspace_dir: None,
            user_dir: None,
            extra_dirs: Vec::new(),
            debounce: Duration::from_millis(500),
            builtin_skills: Vec::new(),
            trust_workspace: false,
        }
    }
}

impl SkillWatcherConfig {
    /// Collect all directories that should be watched.
    fn watch_dirs(&self) -> Vec<&PathBuf> {
        let mut dirs = Vec::new();
        if let Some(ref d) = self.workspace_dir {
            dirs.push(d);
        }
        if let Some(ref d) = self.user_dir {
            dirs.push(d);
        }
        for d in &self.extra_dirs {
            dirs.push(d);
        }
        dirs
    }
}

/// Handle to a running skill watcher. Drop to stop watching.
pub struct SkillWatcherHandle {
    /// Sends a shutdown signal to the watcher task.
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

impl SkillWatcherHandle {
    /// Stop the watcher gracefully.
    pub fn stop(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

impl Drop for SkillWatcherHandle {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

/// Start watching skill directories for changes.
///
/// Returns a [`SkillWatcherHandle`] that can be used to stop watching.
/// The watcher runs in a background tokio task and will automatically
/// rebuild the registry when files change.
///
/// # Errors
///
/// Returns an error if the file-system watcher cannot be created.
pub fn start_watching(
    config: SkillWatcherConfig,
    registry: SharedSkillRegistry,
) -> Result<SkillWatcherHandle, notify::Error> {
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel();
    let (event_tx, mut event_rx) = mpsc::channel::<Event>(100);

    // Create OS file watcher.
    let mut watcher = RecommendedWatcher::new(
        move |res: notify::Result<Event>| {
            if let Ok(event) = res {
                let _ = event_tx.blocking_send(event);
            }
        },
        notify::Config::default(),
    )?;

    // Watch all configured directories that exist.
    for dir in config.watch_dirs() {
        if dir.exists() {
            if let Err(e) = watcher.watch(dir, RecursiveMode::Recursive) {
                warn!(
                    path = %dir.display(),
                    error = %e,
                    "failed to watch skill directory"
                );
            } else {
                debug!(path = %dir.display(), "watching skill directory");
            }
        } else {
            debug!(path = %dir.display(), "skill directory does not exist, skipping watch");
        }
    }

    let debounce = config.debounce;
    let ws_dir = config.workspace_dir.clone();
    let user_dir = config.user_dir.clone();
    let builtins = config.builtin_skills.clone();
    let trust = config.trust_workspace;

    // Spawn the event processing loop.
    tokio::spawn(async move {
        // Keep the watcher alive for the duration of the task.
        let _watcher = watcher;

        let mut pending_rebuild = false;
        let mut debounce_deadline: Option<tokio::time::Instant> = None;

        loop {
            tokio::select! {
                // Receive file-system events.
                event = event_rx.recv() => {
                    match event {
                        Some(ev) => {
                            match ev.kind {
                                EventKind::Create(_)
                                | EventKind::Modify(_)
                                | EventKind::Remove(_) => {
                                    pending_rebuild = true;
                                    debounce_deadline = Some(
                                        tokio::time::Instant::now() + debounce,
                                    );
                                }
                                _ => {}
                            }
                        }
                        None => {
                            // Channel closed, exit.
                            break;
                        }
                    }
                }
                // Debounce timer fires.
                _ = async {
                    match debounce_deadline {
                        Some(deadline) => tokio::time::sleep_until(deadline).await,
                        None => std::future::pending::<()>().await,
                    }
                }, if pending_rebuild => {
                    pending_rebuild = false;
                    debounce_deadline = None;

                    let mut reg = registry.write().await;
                    match reg.rebuild(
                        ws_dir.as_deref(),
                        user_dir.as_deref(),
                        builtins.clone(),
                        trust,
                    ).await {
                        Ok(()) => {
                            info!(
                                count = reg.len(),
                                "skill registry reloaded after file change"
                            );
                        }
                        Err(e) => {
                            warn!(error = %e, "skill registry rebuild failed");
                        }
                    }
                }
                // Shutdown signal.
                _ = &mut shutdown_rx => {
                    info!("skill watcher shutting down");
                    break;
                }
            }
        }
    });

    Ok(SkillWatcherHandle {
        shutdown_tx: Some(shutdown_tx),
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    use tokio::sync::RwLock;

    use super::super::skills_v2::SkillRegistry;
    use clawft_types::skill::SkillDefinition;

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_dir(prefix: &str) -> PathBuf {
        let id = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        std::env::temp_dir().join(format!("clawft_watcher_{prefix}_{pid}_{id}"))
    }

    fn create_skill_md(dir: &std::path::Path, name: &str, desc: &str) {
        let skill_dir = dir.join(name);
        std::fs::create_dir_all(&skill_dir).unwrap();
        let content =
            format!("---\nname: {name}\ndescription: {desc}\n---\n\nInstructions for {name}.");
        std::fs::write(skill_dir.join("SKILL.md"), content).unwrap();
    }

    #[tokio::test]
    async fn test_watcher_config_defaults() {
        let config = SkillWatcherConfig::default();
        assert_eq!(config.debounce, Duration::from_millis(500));
        assert!(config.workspace_dir.is_none());
        assert!(config.user_dir.is_none());
        assert!(config.extra_dirs.is_empty());
        assert!(config.builtin_skills.is_empty());
        assert!(!config.trust_workspace);
    }

    #[tokio::test]
    async fn test_watcher_config_watch_dirs() {
        let config = SkillWatcherConfig {
            workspace_dir: Some(PathBuf::from("/ws/skills")),
            user_dir: Some(PathBuf::from("/user/skills")),
            extra_dirs: vec![PathBuf::from("/extra/skills")],
            ..Default::default()
        };
        assert_eq!(config.watch_dirs().len(), 3);
    }

    #[tokio::test]
    async fn test_watcher_handle_drop_sends_shutdown() {
        let dir = temp_dir("handle_drop");
        std::fs::create_dir_all(&dir).unwrap();
        create_skill_md(&dir, "drop_test", "Drop test skill");

        let registry = Arc::new(RwLock::new(
            SkillRegistry::discover(Some(&dir), None, vec![])
                .await
                .unwrap(),
        ));

        let config = SkillWatcherConfig {
            workspace_dir: Some(dir.clone()),
            trust_workspace: true,
            debounce: Duration::from_millis(50),
            ..Default::default()
        };

        let handle = start_watching(config, registry).unwrap();
        // Dropping the handle should send shutdown
        drop(handle);

        // Give the task a moment to receive the signal
        tokio::time::sleep(Duration::from_millis(100)).await;

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_watcher_detects_new_skill() {
        let dir = temp_dir("detect_new");
        std::fs::create_dir_all(&dir).unwrap();
        create_skill_md(&dir, "original", "Original skill");

        let registry = Arc::new(RwLock::new(
            SkillRegistry::discover(Some(&dir), None, vec![])
                .await
                .unwrap(),
        ));

        {
            let reg = registry.read().await;
            assert_eq!(reg.len(), 1);
            assert!(reg.get("original").is_some());
        }

        let config = SkillWatcherConfig {
            workspace_dir: Some(dir.clone()),
            trust_workspace: true,
            debounce: Duration::from_millis(100),
            ..Default::default()
        };

        let handle = start_watching(config, registry.clone()).unwrap();

        // Add a new skill
        create_skill_md(&dir, "added", "Added skill");

        // Wait for debounce + rebuild
        tokio::time::sleep(Duration::from_millis(500)).await;

        {
            let reg = registry.read().await;
            assert_eq!(reg.len(), 2);
            assert!(reg.get("original").is_some());
            assert!(reg.get("added").is_some());
        }

        handle.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_watcher_detects_skill_removal() {
        let dir = temp_dir("detect_remove");
        std::fs::create_dir_all(&dir).unwrap();
        create_skill_md(&dir, "alpha", "Alpha");
        create_skill_md(&dir, "beta", "Beta");

        let registry = Arc::new(RwLock::new(
            SkillRegistry::discover(Some(&dir), None, vec![])
                .await
                .unwrap(),
        ));

        {
            let reg = registry.read().await;
            assert_eq!(reg.len(), 2);
        }

        let config = SkillWatcherConfig {
            workspace_dir: Some(dir.clone()),
            trust_workspace: true,
            debounce: Duration::from_millis(100),
            ..Default::default()
        };

        let handle = start_watching(config, registry.clone()).unwrap();

        // Remove one skill
        std::fs::remove_dir_all(dir.join("beta")).unwrap();

        // Wait for debounce + rebuild
        tokio::time::sleep(Duration::from_millis(500)).await;

        {
            let reg = registry.read().await;
            assert_eq!(reg.len(), 1);
            assert!(reg.get("alpha").is_some());
            assert!(reg.get("beta").is_none());
        }

        handle.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_watcher_detects_skill_modification() {
        let dir = temp_dir("detect_modify");
        std::fs::create_dir_all(&dir).unwrap();
        create_skill_md(&dir, "mutable", "Original description");

        let registry = Arc::new(RwLock::new(
            SkillRegistry::discover(Some(&dir), None, vec![])
                .await
                .unwrap(),
        ));

        {
            let reg = registry.read().await;
            assert_eq!(
                reg.get("mutable").unwrap().description,
                "Original description"
            );
        }

        let config = SkillWatcherConfig {
            workspace_dir: Some(dir.clone()),
            trust_workspace: true,
            debounce: Duration::from_millis(100),
            ..Default::default()
        };

        let handle = start_watching(config, registry.clone()).unwrap();

        // Modify the skill
        let modified =
            "---\nname: mutable\ndescription: Updated description\n---\n\nUpdated instructions.";
        std::fs::write(dir.join("mutable").join("SKILL.md"), modified).unwrap();

        // Wait for debounce + rebuild
        tokio::time::sleep(Duration::from_millis(500)).await;

        {
            let reg = registry.read().await;
            assert_eq!(
                reg.get("mutable").unwrap().description,
                "Updated description"
            );
        }

        handle.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_watcher_missing_dir_does_not_error() {
        let registry = Arc::new(RwLock::new(
            SkillRegistry::discover(None, None, vec![]).await.unwrap(),
        ));

        let config = SkillWatcherConfig {
            workspace_dir: Some(PathBuf::from("/definitely/does/not/exist")),
            debounce: Duration::from_millis(50),
            ..Default::default()
        };

        // Should not panic or error
        let handle = start_watching(config, registry);
        assert!(handle.is_ok());
        handle.unwrap().stop();
    }

    #[tokio::test]
    async fn test_registry_upsert_and_remove() {
        let mut registry = SkillRegistry::discover(None, None, vec![]).await.unwrap();
        assert!(registry.is_empty());

        // Upsert a skill
        let skill = SkillDefinition::new("dynamic", "A dynamically added skill");
        let prev = registry.upsert(skill);
        assert!(prev.is_none());
        assert_eq!(registry.len(), 1);
        assert!(registry.get("dynamic").is_some());

        // Upsert again (replace)
        let updated = SkillDefinition::new("dynamic", "Updated description");
        let prev = registry.upsert(updated);
        assert!(prev.is_some());
        assert_eq!(prev.unwrap().description, "A dynamically added skill");
        assert_eq!(
            registry.get("dynamic").unwrap().description,
            "Updated description"
        );

        // Remove
        let removed = registry.remove("dynamic");
        assert!(removed.is_some());
        assert!(registry.is_empty());

        // Remove nonexistent
        let removed = registry.remove("nonexistent");
        assert!(removed.is_none());
    }

    #[tokio::test]
    async fn test_registry_rebuild() {
        let dir = temp_dir("rebuild");
        std::fs::create_dir_all(&dir).unwrap();
        create_skill_md(&dir, "first", "First skill");

        let mut registry = SkillRegistry::discover(Some(&dir), None, vec![])
            .await
            .unwrap();
        assert_eq!(registry.len(), 1);

        // Add another skill to disk
        create_skill_md(&dir, "second", "Second skill");

        // Rebuild
        registry
            .rebuild(Some(&dir), None, vec![], true)
            .await
            .unwrap();
        assert_eq!(registry.len(), 2);
        assert!(registry.get("first").is_some());
        assert!(registry.get("second").is_some());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_precedence_on_reload() {
        let ws_dir = temp_dir("prec_ws");
        let user_dir = temp_dir("prec_user");

        std::fs::create_dir_all(&ws_dir).unwrap();
        std::fs::create_dir_all(&user_dir).unwrap();

        // Both have a skill with the same name
        create_skill_md(&ws_dir, "shared", "Workspace version");
        create_skill_md(&user_dir, "shared", "User version");

        let registry = Arc::new(RwLock::new(
            SkillRegistry::discover(Some(&ws_dir), Some(&user_dir), vec![])
                .await
                .unwrap(),
        ));

        {
            let reg = registry.read().await;
            // Workspace takes priority
            assert_eq!(reg.get("shared").unwrap().description, "Workspace version");
        }

        let config = SkillWatcherConfig {
            workspace_dir: Some(ws_dir.clone()),
            user_dir: Some(user_dir.clone()),
            trust_workspace: true,
            debounce: Duration::from_millis(100),
            ..Default::default()
        };

        let handle = start_watching(config, registry.clone()).unwrap();

        // Remove the workspace skill -- user version should be revealed
        std::fs::remove_dir_all(ws_dir.join("shared")).unwrap();

        tokio::time::sleep(Duration::from_millis(500)).await;

        {
            let reg = registry.read().await;
            assert_eq!(reg.get("shared").unwrap().description, "User version");
        }

        handle.stop();
        let _ = std::fs::remove_dir_all(&ws_dir);
        let _ = std::fs::remove_dir_all(&user_dir);
    }
}
