//! Browser filesystem implementation using an in-memory store.
//!
//! Since the Origin Private File System (OPFS) web-sys bindings are not yet
//! stable and require additional feature flags that may not compile cleanly,
//! this implementation uses an in-memory filesystem backed by a `HashMap`.
//!
//! This is acceptable for the current stub/MVP phase. Files are scoped to the
//! lifetime of the [`BrowserFileSystem`] instance and do not persist across
//! page reloads.

use async_trait::async_trait;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::fs::FileSystem;

/// In-memory filesystem for browser/WASM targets.
///
/// Stores file contents in a `HashMap<PathBuf, String>`. Directories are
/// tracked implicitly: a directory "exists" if any file has it as a prefix,
/// or if it was explicitly created via [`create_dir_all`].
pub struct BrowserFileSystem {
    /// File contents keyed by their absolute paths.
    files: Mutex<HashMap<PathBuf, String>>,
    /// Explicitly created directories (those without files yet).
    dirs: Mutex<Vec<PathBuf>>,
}

impl BrowserFileSystem {
    /// Create a new empty in-memory filesystem.
    pub fn new() -> Self {
        Self {
            files: Mutex::new(HashMap::new()),
            dirs: Mutex::new(Vec::new()),
        }
    }
}

impl Default for BrowserFileSystem {
    fn default() -> Self {
        Self::new()
    }
}

/// Normalize a path to a canonical form for consistent lookups.
fn normalize(path: &Path) -> PathBuf {
    // Use the path components to rebuild without redundant separators.
    let mut result = PathBuf::new();
    for component in path.components() {
        result.push(component);
    }
    result
}

#[async_trait(?Send)]
impl FileSystem for BrowserFileSystem {
    async fn read_to_string(&self, path: &Path) -> std::io::Result<String> {
        let key = normalize(path);
        self.files
            .lock()
            .expect("BrowserFileSystem mutex poisoned")
            .get(&key)
            .cloned()
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("file not found: {}", key.display()),
                )
            })
    }

    async fn write_string(&self, path: &Path, content: &str) -> std::io::Result<()> {
        let key = normalize(path);
        // Ensure parent directories are implicitly created.
        if let Some(parent) = key.parent() {
            self.create_dir_all(parent).await?;
        }
        self.files
            .lock()
            .expect("BrowserFileSystem mutex poisoned")
            .insert(key, content.to_string());
        Ok(())
    }

    async fn append_string(&self, path: &Path, content: &str) -> std::io::Result<()> {
        let key = normalize(path);
        if let Some(parent) = key.parent() {
            self.create_dir_all(parent).await?;
        }
        let mut files = self.files.lock().expect("BrowserFileSystem mutex poisoned");
        let entry = files.entry(key).or_default();
        entry.push_str(content);
        Ok(())
    }

    async fn exists(&self, path: &Path) -> bool {
        let key = normalize(path);
        let files = self.files.lock().expect("BrowserFileSystem mutex poisoned");
        if files.contains_key(&key) {
            return true;
        }
        // Check if it's a known directory.
        let dirs = self
            .dirs
            .lock()
            .expect("BrowserFileSystem dirs mutex poisoned");
        if dirs.iter().any(|d| d == &key) {
            return true;
        }
        // Check if any file is underneath this path (implicit directory).
        files.keys().any(|k| k.starts_with(&key) && k != &key)
    }

    async fn list_dir(&self, path: &Path) -> std::io::Result<Vec<PathBuf>> {
        let dir = normalize(path);
        let files = self.files.lock().expect("BrowserFileSystem mutex poisoned");

        let mut entries = std::collections::BTreeSet::new();
        for key in files.keys() {
            if let Ok(suffix) = key.strip_prefix(&dir) {
                // Direct children: the first component of the remaining suffix.
                if let Some(first) = suffix.components().next() {
                    entries.insert(dir.join(first));
                }
            }
        }

        // Also check explicit subdirectories.
        let dirs = self
            .dirs
            .lock()
            .expect("BrowserFileSystem dirs mutex poisoned");
        for d in dirs.iter() {
            if let Ok(suffix) = d.strip_prefix(&dir) {
                if let Some(first) = suffix.components().next() {
                    let child = dir.join(first);
                    if child != dir {
                        entries.insert(child);
                    }
                }
            }
        }

        if entries.is_empty() && !self.exists(path).await {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("directory not found: {}", dir.display()),
            ));
        }

        Ok(entries.into_iter().collect())
    }

    async fn create_dir_all(&self, path: &Path) -> std::io::Result<()> {
        let key = normalize(path);
        let mut dirs = self
            .dirs
            .lock()
            .expect("BrowserFileSystem dirs mutex poisoned");
        // Add this directory and all ancestor directories.
        let mut current = PathBuf::new();
        for component in key.components() {
            current.push(component);
            if !dirs.contains(&current) {
                dirs.push(current.clone());
            }
        }
        Ok(())
    }

    async fn remove_file(&self, path: &Path) -> std::io::Result<()> {
        let key = normalize(path);
        self.files
            .lock()
            .expect("BrowserFileSystem mutex poisoned")
            .remove(&key)
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("file not found: {}", key.display()),
                )
            })?;
        Ok(())
    }

    fn home_dir(&self) -> Option<PathBuf> {
        // Return a virtual home directory for browser contexts.
        Some(PathBuf::from("/clawft"))
    }
}
