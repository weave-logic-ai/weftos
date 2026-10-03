//! Notify-driven hot-reload watcher for identity files (WEFT-329).
//!
//! Watches `<workspace>/.clawft/` for changes to `SOUL.md` and
//! `IDENTITY.md`, debounces rapid events, and calls
//! [`FileIdentityProvider::invalidate`] so the next
//! [`IdentityProvider::current`](super::identity::IdentityProvider::current)
//! re-reads from disk.
//!
//! # Architecture
//!
//! Mirrors [`super::skill_watcher`]:
//! 1. OS events from the [`notify`] crate via an `mpsc` channel.
//! 2. Debounce (default 200ms — identity files are small / rare edits).
//! 3. Filter to identity basenames only (ignore `SOUL.journal`, skills, …).
//! 4. Invalidate the provider cache; do **not** re-read here — the next
//!    turn's `current()` performs the load via [`Platform::fs`].
//!
//! # Feature gate
//!
//! This module is compiled only with the `native` feature (same as
//! `skill_watcher`). The watcher is always-on when started: a single
//! non-recursive directory watch is cheap. Callers that cannot start
//! notify (inotify quota, headless CI) leave the provider in default
//! always-re-read mode.
//!
//! Native-only: the `notify` crate does not compile to wasm.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use clawft_platform::Platform;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use super::identity::FileIdentityProvider;

/// Configuration for the identity file-system watcher.
#[derive(Debug, Clone)]
pub struct IdentityWatcherConfig {
    /// Directory containing `SOUL.md` / `IDENTITY.md`
    /// (typically `<workspace>/.clawft`).
    pub clawft_dir: PathBuf,
    /// Debounce duration for rapid file changes (editor save storms).
    pub debounce: Duration,
}

impl Default for IdentityWatcherConfig {
    fn default() -> Self {
        Self {
            clawft_dir: PathBuf::from(".clawft"),
            debounce: Duration::from_millis(200),
        }
    }
}

impl IdentityWatcherConfig {
    /// Build a config for `<workspace>/.clawft` with default debounce.
    pub fn for_workspace(workspace: impl AsRef<Path>) -> Self {
        Self {
            clawft_dir: workspace.as_ref().join(".clawft"),
            ..Default::default()
        }
    }
}

/// Handle to a running identity watcher. Drop (or [`Self::stop`]) to
/// stop watching. While the handle is active the provider runs in
/// watch mode (cache until invalidate).
pub struct IdentityWatcherHandle {
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
    active: Arc<std::sync::atomic::AtomicBool>,
}

impl IdentityWatcherHandle {
    /// Whether the watcher is still active (not stopped / dropped).
    pub fn is_active(&self) -> bool {
        self.active.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Stop the watcher gracefully.
    pub fn stop(mut self) {
        self.active
            .store(false, std::sync::atomic::Ordering::SeqCst);
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

impl Drop for IdentityWatcherHandle {
    fn drop(&mut self) {
        self.active
            .store(false, std::sync::atomic::Ordering::SeqCst);
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

/// Return `true` when `path` is (or ends with) an identity seed file.
pub fn is_identity_path(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|n| n.to_str()),
        Some("SOUL.md") | Some("IDENTITY.md")
    )
}

fn event_touches_identity(event: &Event) -> bool {
    event.paths.iter().any(|p| is_identity_path(p))
}

/// Start watching `.clawft` for identity file changes.
///
/// Enables watch mode on `provider` and returns a handle that must be
/// kept alive for the duration of the agent loop. Dropping the handle
/// stops the background task (provider stays in watch mode until
/// process exit — subsequent invalidates only happen via
/// [`FileIdentityProvider::invalidate`] if called manually).
///
/// # Errors
///
/// Returns an error if the OS file-system watcher cannot be created.
/// Missing `.clawft` directories are skipped with a debug log (no error).
pub fn start_watching<P: Platform + 'static>(
    config: IdentityWatcherConfig,
    provider: Arc<FileIdentityProvider<P>>,
) -> Result<IdentityWatcherHandle, notify::Error> {
    provider.enable_watch_mode();

    let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel();
    let (event_tx, mut event_rx) = mpsc::channel::<Event>(64);
    let active = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let active_for_task = Arc::clone(&active);

    let mut watcher = RecommendedWatcher::new(
        move |res: notify::Result<Event>| {
            if let Ok(event) = res {
                let _ = event_tx.blocking_send(event);
            }
        },
        notify::Config::default(),
    )?;

    let clawft_dir = config.clawft_dir.clone();
    if clawft_dir.exists() {
        if let Err(e) = watcher.watch(&clawft_dir, RecursiveMode::NonRecursive) {
            warn!(
                path = %clawft_dir.display(),
                error = %e,
                "failed to watch identity directory"
            );
        } else {
            debug!(path = %clawft_dir.display(), "watching identity directory");
        }
    } else {
        // Directory may appear later after `weaver init`. We still run
        // the event loop; without a successful watch, invalidation only
        // happens if something else calls `invalidate`.
        debug!(
            path = %clawft_dir.display(),
            "identity directory does not exist, skipping watch"
        );
    }

    let debounce = config.debounce;
    let provider_for_task = Arc::clone(&provider);

    tokio::spawn(async move {
        let _watcher = watcher;

        let mut pending_invalidate = false;
        let mut debounce_deadline: Option<tokio::time::Instant> = None;

        loop {
            tokio::select! {
                event = event_rx.recv() => {
                    match event {
                        Some(ev) => {
                            match ev.kind {
                                EventKind::Create(_)
                                | EventKind::Modify(_)
                                | EventKind::Remove(_)
                                    if event_touches_identity(&ev) => {
                                        pending_invalidate = true;
                                        debounce_deadline = Some(
                                            tokio::time::Instant::now() + debounce,
                                        );
                                    }
                                _ => {}
                            }
                        }
                        None => break,
                    }
                }
                _ = async {
                    match debounce_deadline {
                        Some(deadline) => tokio::time::sleep_until(deadline).await,
                        None => std::future::pending::<()>().await,
                    }
                }, if pending_invalidate => {
                    pending_invalidate = false;
                    debounce_deadline = None;
                    provider_for_task.invalidate();
                    info!(
                        path = %clawft_dir.display(),
                        "identity cache invalidated after file change"
                    );
                }
                _ = &mut shutdown_rx => {
                    active_for_task.store(false, std::sync::atomic::Ordering::SeqCst);
                    info!("identity watcher shutting down");
                    break;
                }
            }
        }
        active_for_task.store(false, std::sync::atomic::Ordering::SeqCst);
    });

    Ok(IdentityWatcherHandle {
        shutdown_tx: Some(shutdown_tx),
        active,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    use clawft_platform::NativePlatform;

    use crate::agent::identity::{IdentityProvider, IdentitySource};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_dir(prefix: &str) -> PathBuf {
        let id = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        std::env::temp_dir().join(format!("clawft_id_watch_{prefix}_{pid}_{id}"))
    }

    fn seed_identity(clawft: &Path, soul: &str, identity: &str) {
        std::fs::create_dir_all(clawft).unwrap();
        std::fs::write(clawft.join("SOUL.md"), soul).unwrap();
        std::fs::write(clawft.join("IDENTITY.md"), identity).unwrap();
    }

    #[test]
    fn is_identity_path_filters_basenames() {
        assert!(is_identity_path(Path::new("/ws/.clawft/SOUL.md")));
        assert!(is_identity_path(Path::new("/ws/.clawft/IDENTITY.md")));
        assert!(!is_identity_path(Path::new("/ws/.clawft/SOUL.journal")));
        assert!(!is_identity_path(Path::new("/ws/.clawft/skills/x/SKILL.md")));
        assert!(!is_identity_path(Path::new("/ws/.clawft/")));
    }

    #[test]
    fn config_for_workspace_joins_clawft() {
        let cfg = IdentityWatcherConfig::for_workspace("/home/u/proj");
        assert_eq!(cfg.clawft_dir, PathBuf::from("/home/u/proj/.clawft"));
        assert_eq!(cfg.debounce, Duration::from_millis(200));
    }

    #[tokio::test]
    async fn watcher_missing_dir_does_not_error() {
        let platform = Arc::new(NativePlatform::new());
        let provider = Arc::new(FileIdentityProvider::new(
            PathBuf::from("/definitely/does/not/exist/weft329"),
            platform,
        ));
        let config = IdentityWatcherConfig {
            clawft_dir: PathBuf::from("/definitely/does/not/exist/weft329/.clawft"),
            debounce: Duration::from_millis(50),
        };
        let handle = start_watching(config, provider.clone()).expect("start ok");
        assert!(handle.is_active());
        assert!(provider.is_watch_mode());
        handle.stop();
    }

    #[tokio::test]
    async fn watcher_invalidates_on_soul_change() {
        let dir = temp_dir("soul_change");
        let clawft = dir.join(".clawft");
        seed_identity(&clawft, "soul-original", "id-original");

        let platform = Arc::new(NativePlatform::new());
        let provider = Arc::new(FileIdentityProvider::new(&dir, platform));

        let first = provider.current().await.expect("initial load");
        assert_eq!(first.soul, "soul-original");

        let config = IdentityWatcherConfig {
            clawft_dir: clawft.clone(),
            debounce: Duration::from_millis(80),
        };
        let handle = start_watching(config, provider.clone()).expect("start watcher");
        assert!(provider.is_watch_mode());

        // Warm cache under watch mode.
        let warmed = provider.current().await.expect("warm");
        assert_eq!(warmed.soul, "soul-original");
        assert!(!provider.is_stale());

        // Edit SOUL.md on disk.
        std::fs::write(clawft.join("SOUL.md"), "soul-updated").unwrap();

        // Wait for debounce + invalidate.
        let mut saw_stale = false;
        for _ in 0..40 {
            tokio::time::sleep(Duration::from_millis(50)).await;
            if provider.is_stale() {
                saw_stale = true;
                break;
            }
        }
        assert!(
            saw_stale,
            "watcher must mark provider stale after SOUL.md change"
        );

        let reloaded = provider.current().await.expect("reload after invalidate");
        assert_eq!(reloaded.soul, "soul-updated");
        assert_eq!(reloaded.source, IdentitySource::Clawft);
        assert_ne!(reloaded.hash, first.hash);

        handle.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn watcher_ignores_non_identity_files() {
        let dir = temp_dir("ignore_other");
        let clawft = dir.join(".clawft");
        seed_identity(&clawft, "soul-keep", "id-keep");

        let platform = Arc::new(NativePlatform::new());
        let provider = Arc::new(FileIdentityProvider::new(&dir, platform));

        let config = IdentityWatcherConfig {
            clawft_dir: clawft.clone(),
            debounce: Duration::from_millis(80),
        };
        let handle = start_watching(config, provider.clone()).unwrap();

        let _ = provider.current().await.expect("warm");
        assert!(!provider.is_stale());

        // Write a non-identity file in the same directory.
        std::fs::write(clawft.join("SOUL.journal"), "noise").unwrap();
        std::fs::write(clawft.join("notes.txt"), "noise").unwrap();

        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(
            !provider.is_stale(),
            "non-identity files must not invalidate the cache"
        );

        handle.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn watcher_handle_drop_stops() {
        let dir = temp_dir("handle_drop");
        let clawft = dir.join(".clawft");
        seed_identity(&clawft, "s", "i");

        let platform = Arc::new(NativePlatform::new());
        let provider = Arc::new(FileIdentityProvider::new(&dir, platform));
        let config = IdentityWatcherConfig {
            clawft_dir: clawft,
            debounce: Duration::from_millis(50),
        };
        let handle = start_watching(config, provider).unwrap();
        assert!(handle.is_active());
        drop(handle);
        tokio::time::sleep(Duration::from_millis(100)).await;
        let _ = std::fs::remove_dir_all(&dir);
    }
}
