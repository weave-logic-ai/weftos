//! Long-term and session memory management.
//!
//! Manages two markdown files ported from the Python nanobot pattern:
//! - `MEMORY.md` -- long-term facts (append-only, periodically consolidated)
//! - `HISTORY.md` -- session summaries (grep-searchable log)
//!
//! File locations follow the discovery chain:
//! `~/.clawft/workspace/memory/` with fallback to `~/.nanobot/workspace/memory/`.
//!
//! All I/O goes through the [`Platform`] filesystem trait so the module
//! remains WASM-compatible and testable with mock filesystems.

use std::path::PathBuf;
use std::sync::Arc;

use tracing::{debug, warn};

use clawft_platform::Platform;
use clawft_types::{ClawftError, Result};

/// Long-term and session memory store.
///
/// Wraps two markdown files (`MEMORY.md` and `HISTORY.md`) behind the
/// platform filesystem abstraction. Reads are lock-free (each call reads
/// the file); writes go through the platform's append/write operations.
///
/// # Directory resolution
///
/// The constructor resolves the memory directory using the fallback chain:
/// 1. `~/.clawft/workspace/memory/`
/// 2. `~/.nanobot/workspace/memory/` (legacy fallback)
///
/// If neither exists, the `.clawft` path is used and created on first write.
pub struct MemoryStore<P: Platform> {
    memory_path: PathBuf,
    history_path: PathBuf,
    platform: Arc<P>,
}

impl<P: Platform> MemoryStore<P> {
    /// Create a new memory store.
    ///
    /// Resolves the workspace memory directory from the platform home
    /// directory. Prefers `~/.clawft/workspace/memory/`; falls back to
    /// `~/.nanobot/workspace/memory/` if the `.clawft` directory does
    /// not exist but `.nanobot` does.
    ///
    /// # Errors
    ///
    /// Returns [`ClawftError::ConfigInvalid`] if no home directory can
    /// be determined from the platform.
    pub fn new(platform: Arc<P>) -> Result<Self> {
        let home = platform
            .fs()
            .home_dir()
            .ok_or_else(|| ClawftError::ConfigInvalid {
                reason: "could not determine home directory".into(),
            })?;

        let clawft_memory = home.join(".clawft").join("workspace").join("memory");
        let nanobot_memory = home.join(".nanobot").join("workspace").join("memory");

        // Prefer .clawft; fall back to .nanobot only if it already exists
        // and .clawft does not. We use sync existence checks on the parent
        // dirs since home_dir resolution is sync.
        let memory_dir = if nanobot_memory.exists() && !clawft_memory.exists() {
            debug!(path = %nanobot_memory.display(), "using legacy nanobot memory path");
            nanobot_memory
        } else {
            debug!(path = %clawft_memory.display(), "using clawft memory path");
            clawft_memory
        };

        Ok(Self {
            memory_path: memory_dir.join("MEMORY.md"),
            history_path: memory_dir.join("HISTORY.md"),
            platform,
        })
    }

    /// Create a memory store with explicit `MEMORY.md` and `HISTORY.md`
    /// paths.
    ///
    /// Skips the home-directory resolution that [`Self::new`] performs;
    /// useful for hermetic tests (write into a temp dir) and for
    /// embedded callers that want to pin the memory location instead
    /// of inheriting the platform's default. Public so workspace-level
    /// integration tests outside `clawft-core` can compose an isolated
    /// `AgentLoop` without touching the user's `~/.clawft/workspace`.
    pub fn with_paths(
        memory_path: PathBuf,
        history_path: PathBuf,
        platform: Arc<P>,
    ) -> Self {
        Self {
            memory_path,
            history_path,
            platform,
        }
    }

    /// Create a memory store rooted at a workspace overlay.
    ///
    /// Resolves `MEMORY.md` / `HISTORY.md` under
    /// `<workspace_root>/.clawft/memory/` so that a kernel running
    /// inside a workspace persists into that workspace's `.clawft/`
    /// rather than the user's global `~/.clawft/workspace/memory/`.
    /// Mirrors the cwd-relative config overlay (Layer 3 in
    /// `clawft_platform::config_loader::load_config_raw`) so memory
    /// follows the same routing rules as policy.
    ///
    /// This constructor does not touch the home directory and never
    /// fails — the parent dirs are created lazily on first write.
    /// Pair with [`Self::new`] as a fallback when no workspace overlay
    /// is present.
    pub fn for_workspace(workspace_root: &std::path::Path, platform: Arc<P>) -> Self {
        let memory_dir = workspace_root.join(".clawft").join("memory");
        debug!(
            path = %memory_dir.display(),
            "using workspace-scoped memory path"
        );
        Self {
            memory_path: memory_dir.join("MEMORY.md"),
            history_path: memory_dir.join("HISTORY.md"),
            platform,
        }
    }

    /// Read long-term memory (`MEMORY.md`).
    ///
    /// Returns an empty string if the file does not exist yet, matching
    /// the Python nanobot behavior. I/O errors other than "not found"
    /// are propagated.
    pub async fn read_long_term(&self) -> Result<String> {
        self.read_file_or_empty(&self.memory_path).await
    }

    /// Write (overwrite) long-term memory.
    ///
    /// Creates parent directories if they do not exist.
    pub async fn write_long_term(&self, content: &str) -> Result<()> {
        let clean = crate::security::sanitize_content(content);
        self.platform
            .fs()
            .write_string(&self.memory_path, &clean)
            .await
            .map_err(ClawftError::Io)
    }

    /// Append an entry to long-term memory.
    ///
    /// Each entry is terminated with a double newline to form distinct
    /// paragraphs for later search.
    pub async fn append_long_term(&self, entry: &str) -> Result<()> {
        let clean = crate::security::sanitize_content(entry);
        let formatted = format!("{}\n\n", clean.trim_end());
        self.platform
            .fs()
            .append_string(&self.memory_path, &formatted)
            .await
            .map_err(ClawftError::Io)
    }

    /// Read the history file (`HISTORY.md`).
    ///
    /// Returns an empty string if the file does not exist yet.
    pub async fn read_history(&self) -> Result<String> {
        self.read_file_or_empty(&self.history_path).await
    }

    /// Append an entry to the history file.
    ///
    /// Each entry is terminated with a double newline to match the
    /// Python `append_history` behavior.
    pub async fn append_history(&self, entry: &str) -> Result<()> {
        let clean = crate::security::sanitize_content(entry);
        let formatted = format!("{}\n\n", clean.trim_end());
        self.platform
            .fs()
            .append_string(&self.history_path, &formatted)
            .await
            .map_err(ClawftError::Io)
    }

    /// Substring search across both memory files.
    ///
    /// Splits content by double-newline into paragraphs, then returns
    /// paragraphs containing `query` (case-insensitive). Results are
    /// returned in document order, capped at `max_results`.
    pub async fn search(&self, query: &str, max_results: usize) -> Vec<String> {
        if query.is_empty() || max_results == 0 {
            return Vec::new();
        }

        let query_lower = query.to_lowercase();
        let mut results = Vec::new();

        for content in [self.read_long_term().await, self.read_history().await] {
            let text = match content {
                Ok(t) => t,
                Err(e) => {
                    warn!(error = %e, "failed to read memory file during search");
                    continue;
                }
            };
            for paragraph in text.split("\n\n") {
                let trimmed = paragraph.trim();
                if !trimmed.is_empty() && trimmed.to_lowercase().contains(&query_lower) {
                    results.push(trimmed.to_string());
                    if results.len() >= max_results {
                        return results;
                    }
                }
            }
        }

        results
    }

    /// Path to the memory file (for diagnostics / context building).
    pub fn memory_path(&self) -> &PathBuf {
        &self.memory_path
    }

    /// Path to the history file (for diagnostics / context building).
    pub fn history_path(&self) -> &PathBuf {
        &self.history_path
    }

    /// Read a file, returning empty string on "not found".
    async fn read_file_or_empty(&self, path: &std::path::Path) -> Result<String> {
        if !self.platform.fs().exists(path).await {
            return Ok(String::new());
        }
        self.platform
            .fs()
            .read_to_string(path)
            .await
            .map_err(ClawftError::Io)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clawft_platform::NativePlatform;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    /// Generate a unique temp directory for each test.
    fn temp_dir(prefix: &str) -> PathBuf {
        let id = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        std::env::temp_dir().join(format!("clawft_mem_test_{prefix}_{pid}_{id}"))
    }

    /// Create a MemoryStore pointing at a temp directory.
    fn test_store(dir: &std::path::Path) -> MemoryStore<NativePlatform> {
        let platform = Arc::new(NativePlatform::new());
        MemoryStore::with_paths(dir.join("MEMORY.md"), dir.join("HISTORY.md"), platform)
    }

    #[tokio::test]
    async fn read_long_term_returns_empty_when_missing() {
        let dir = temp_dir("empty_lt");
        let store = test_store(&dir);
        let content = store.read_long_term().await.unwrap();
        assert!(content.is_empty());
    }

    #[tokio::test]
    async fn write_and_read_long_term() {
        let dir = temp_dir("write_lt");
        let store = test_store(&dir);

        store
            .write_long_term("fact: the sky is blue")
            .await
            .unwrap();
        let content = store.read_long_term().await.unwrap();
        assert_eq!(content, "fact: the sky is blue");

        // Cleanup
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn append_long_term_adds_paragraphs() {
        let dir = temp_dir("append_lt");
        let store = test_store(&dir);

        store.append_long_term("first entry").await.unwrap();
        store.append_long_term("second entry").await.unwrap();

        let content = store.read_long_term().await.unwrap();
        assert!(content.contains("first entry"));
        assert!(content.contains("second entry"));
        // Entries separated by double newlines
        assert!(content.contains("\n\n"));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn read_history_returns_empty_when_missing() {
        let dir = temp_dir("empty_hist");
        let store = test_store(&dir);
        let content = store.read_history().await.unwrap();
        assert!(content.is_empty());
    }

    #[tokio::test]
    async fn append_and_read_history() {
        let dir = temp_dir("append_hist");
        let store = test_store(&dir);

        store.append_history("session 1 summary").await.unwrap();
        store.append_history("session 2 summary").await.unwrap();

        let content = store.read_history().await.unwrap();
        assert!(content.contains("session 1 summary"));
        assert!(content.contains("session 2 summary"));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn write_long_term_overwrites() {
        let dir = temp_dir("overwrite_lt");
        let store = test_store(&dir);

        store.write_long_term("old content").await.unwrap();
        store.write_long_term("new content").await.unwrap();

        let content = store.read_long_term().await.unwrap();
        assert_eq!(content, "new content");

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn search_finds_matching_paragraphs() {
        let dir = temp_dir("search");
        let store = test_store(&dir);

        store
            .write_long_term("The sky is blue.\n\nGrass is green.\n\nThe ocean is also blue.")
            .await
            .unwrap();

        let results = store.search("blue", 10).await;
        assert_eq!(results.len(), 2);
        assert_eq!(results[0], "The sky is blue.");
        assert_eq!(results[1], "The ocean is also blue.");

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn search_is_case_insensitive() {
        let dir = temp_dir("search_ci");
        let store = test_store(&dir);

        store.write_long_term("Rust is GREAT").await.unwrap();

        let results = store.search("great", 10).await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], "Rust is GREAT");

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn search_respects_max_results() {
        let dir = temp_dir("search_max");
        let store = test_store(&dir);

        store
            .write_long_term("match one\n\nmatch two\n\nmatch three")
            .await
            .unwrap();

        let results = store.search("match", 2).await;
        assert_eq!(results.len(), 2);

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn search_spans_both_files() {
        let dir = temp_dir("search_both");
        let store = test_store(&dir);

        store.write_long_term("memory hit here").await.unwrap();
        store.append_history("history hit here").await.unwrap();

        let results = store.search("hit here", 10).await;
        assert_eq!(results.len(), 2);

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn search_empty_query_returns_empty() {
        let dir = temp_dir("search_empty");
        let store = test_store(&dir);
        store.write_long_term("some content").await.unwrap();

        let results = store.search("", 10).await;
        assert!(results.is_empty());

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn search_zero_max_returns_empty() {
        let dir = temp_dir("search_zero");
        let store = test_store(&dir);
        store.write_long_term("content").await.unwrap();

        let results = store.search("content", 0).await;
        assert!(results.is_empty());

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn search_no_matches() {
        let dir = temp_dir("search_none");
        let store = test_store(&dir);
        store.write_long_term("hello world").await.unwrap();

        let results = store.search("xyz_no_match", 10).await;
        assert!(results.is_empty());

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn append_trims_trailing_whitespace() {
        let dir = temp_dir("trim");
        let store = test_store(&dir);

        store
            .append_long_term("entry with trailing   \n\n")
            .await
            .unwrap();
        let content = store.read_long_term().await.unwrap();
        // Should be trimmed then followed by exactly one double newline
        assert_eq!(content, "entry with trailing\n\n");

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn new_resolves_home_dir() {
        let platform = Arc::new(NativePlatform::new());
        let store = MemoryStore::new(platform);
        assert!(store.is_ok());
        let store = store.unwrap();
        // Memory path should be absolute and end with MEMORY.md
        assert!(store.memory_path().is_absolute());
        assert!(
            store
                .memory_path()
                .file_name()
                .is_some_and(|n| n == "MEMORY.md")
        );
        assert!(
            store
                .history_path()
                .file_name()
                .is_some_and(|n| n == "HISTORY.md")
        );
    }

    #[tokio::test]
    async fn for_workspace_routes_to_workspace_clawft_memory() {
        // WEFT-79: when a workspace root is supplied, MemoryStore must
        // resolve MEMORY.md/HISTORY.md under <workspace>/.clawft/memory/
        // rather than the global ~/.clawft/workspace/memory/.
        let dir = temp_dir("for_ws");
        std::fs::create_dir_all(&dir).unwrap();
        let platform = Arc::new(NativePlatform::new());
        let store = MemoryStore::for_workspace(&dir, platform);

        let expected_dir = dir.join(".clawft").join("memory");
        assert_eq!(store.memory_path(), &expected_dir.join("MEMORY.md"));
        assert_eq!(store.history_path(), &expected_dir.join("HISTORY.md"));

        // First write must create the workspace .clawft/memory/ dir.
        store.write_long_term("workspace fact").await.unwrap();
        assert!(expected_dir.is_dir());
        assert_eq!(
            store.read_long_term().await.unwrap(),
            "workspace fact"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn for_workspace_isolates_from_global_path() {
        // Two stores rooted at different workspaces must not collide.
        let dir_a = temp_dir("for_ws_iso_a");
        let dir_b = temp_dir("for_ws_iso_b");
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();
        let platform = Arc::new(NativePlatform::new());

        let store_a = MemoryStore::for_workspace(&dir_a, platform.clone());
        let store_b = MemoryStore::for_workspace(&dir_b, platform);

        store_a.write_long_term("alpha").await.unwrap();
        store_b.write_long_term("beta").await.unwrap();

        assert_eq!(store_a.read_long_term().await.unwrap(), "alpha");
        assert_eq!(store_b.read_long_term().await.unwrap(), "beta");
        assert_ne!(store_a.memory_path(), store_b.memory_path());

        let _ = std::fs::remove_dir_all(&dir_a);
        let _ = std::fs::remove_dir_all(&dir_b);
    }

    #[tokio::test]
    async fn creates_parent_dirs_on_write() {
        let dir = temp_dir("mkdirs");
        let nested = dir.join("deep").join("nested");
        let platform = Arc::new(NativePlatform::new());
        let store = MemoryStore::with_paths(
            nested.join("MEMORY.md"),
            nested.join("HISTORY.md"),
            platform,
        );

        // Should create all parent dirs
        store.write_long_term("test").await.unwrap();
        assert_eq!(store.read_long_term().await.unwrap(), "test");

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
