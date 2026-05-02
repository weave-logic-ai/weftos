//! File watcher: monitors a directory for changes and triggers re-extraction.
//!
//! Uses polling by default; the `notify` crate can be used when available.
//! Ported from Python `graphify/watch.py`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::GraphifyError;

/// Extensions monitored by the file watcher.
pub const WATCHED_EXTENSIONS: &[&str] = &[
    "py", "ts", "js", "go", "rs", "java", "cpp", "c", "rb", "swift", "kt",
    "cs", "scala", "php", "cc", "cxx", "hpp", "h", "kts",
    "md", "txt", "rst", "pdf",
    "png", "jpg", "jpeg", "webp", "gif", "svg",
];

/// Code-only extensions (changes rebuild without LLM).
pub const CODE_EXTENSIONS: &[&str] = &[
    "py", "ts", "js", "go", "rs", "java", "cpp", "c", "rb", "swift", "kt",
    "cs", "scala", "php", "cc", "cxx", "hpp", "h", "kts",
];

/// Check if a path has a watched extension.
pub fn is_watched(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| WATCHED_EXTENSIONS.contains(&e.to_lowercase().as_str()))
        .unwrap_or(false)
}

/// Check if a path is a code file.
pub fn is_code(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| CODE_EXTENSIONS.contains(&e.to_lowercase().as_str()))
        .unwrap_or(false)
}

fn should_ignore(path: &Path) -> bool {
    for component in path.components() {
        if let std::path::Component::Normal(s) = component {
            let name = s.to_string_lossy();
            if name.starts_with('.') || name == "graphify-out"
                || name == "__pycache__" || name == "node_modules"
            {
                return true;
            }
        }
    }
    false
}

/// Describes a batch of changed files detected by the watcher.
#[derive(Debug, Clone)]
pub struct WatchEvent {
    pub changed: Vec<PathBuf>,
    pub has_non_code: bool,
}

/// Configuration for the file watcher.
#[derive(Debug, Clone)]
pub struct WatchConfig {
    pub root: PathBuf,
    pub debounce_secs: f64,
}

impl Default for WatchConfig {
    fn default() -> Self {
        Self { root: PathBuf::from("."), debounce_secs: 2.0 }
    }
}

/// Polling-based file watcher (no external deps beyond walkdir).
pub fn watch_poll<F>(config: &WatchConfig, mut callback: F) -> Result<(), GraphifyError>
where
    F: FnMut(WatchEvent),
{
    let debounce = Duration::from_secs_f64(config.debounce_secs);
    let mut snapshot = build_snapshot(&config.root)?;
    let mut pending: HashSet<PathBuf> = HashSet::new();
    let mut last_change = Instant::now();

    eprintln!(
        "[graphify watch] Watching {} (polling, debounce {:.1}s) -- Ctrl+C to stop",
        config.root.display(), config.debounce_secs,
    );

    loop {
        std::thread::sleep(Duration::from_millis(500));

        let current = match build_snapshot(&config.root) {
            Ok(s) => s,
            Err(_) => continue,
        };

        for (path, mtime) in &current {
            match snapshot.get(path) {
                Some(old_mtime) if old_mtime != mtime => {
                    pending.insert(path.clone());
                    last_change = Instant::now();
                }
                None => {
                    pending.insert(path.clone());
                    last_change = Instant::now();
                }
                _ => {}
            }
        }

        for path in snapshot.keys() {
            if !current.contains_key(path) {
                pending.insert(path.clone());
                last_change = Instant::now();
            }
        }

        snapshot = current;

        if !pending.is_empty() && last_change.elapsed() >= debounce {
            let changed: Vec<PathBuf> = pending.drain().collect();
            let has_non_code = changed.iter().any(|p| !is_code(p));
            callback(WatchEvent { changed, has_non_code });
        }
    }
}

fn build_snapshot(
    root: &Path,
) -> Result<std::collections::HashMap<PathBuf, std::time::SystemTime>, GraphifyError> {
    let mut map = std::collections::HashMap::new();
    let walker = walkdir::WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            !name.starts_with('.') && name != "graphify-out"
                && name != "__pycache__" && name != "node_modules"
        });

    for entry in walker.filter_map(|e| e.ok()) {
        let path = entry.path();
        if !entry.file_type().is_file() || !is_watched(path) || should_ignore(path) {
            continue;
        }
        if let Ok(meta) = std::fs::metadata(path)
            && let Ok(mtime) = meta.modified() {
                map.insert(path.to_path_buf(), mtime);
            }
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_watched_extensions() {
        assert!(is_watched(Path::new("main.rs")));
        assert!(is_watched(Path::new("script.py")));
        assert!(is_watched(Path::new("readme.md")));
        assert!(!is_watched(Path::new("binary.exe")));
    }

    #[test]
    fn is_code_extensions() {
        assert!(is_code(Path::new("main.rs")));
        assert!(!is_code(Path::new("readme.md")));
        assert!(!is_code(Path::new("image.png")));
    }

    #[test]
    fn should_ignore_hidden_and_output() {
        assert!(should_ignore(Path::new(".git/config")));
        assert!(should_ignore(Path::new("graphify-out/graph.json")));
        assert!(!should_ignore(Path::new("src/main.rs")));
    }

    #[test]
    fn build_snapshot_current_dir() {
        let snap = build_snapshot(Path::new("."));
        assert!(snap.is_ok());
    }
}
