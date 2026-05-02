//! BLAKE3-based content cache for incremental extraction.
//!
//! Cache location: `.weftos/graphify-cache/` (differs from Python's
//! `graphify-out/cache/`). Each entry is keyed by `BLAKE3(file_content)` and
//! stored as a JSON file. Writes are atomic (write to `.tmp`, then rename).

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::model::ExtractionResult;

/// Version tag embedded in cache entries. When the extractor changes in a
/// backward-incompatible way, bump this to invalidate all cached results.
const EXTRACTOR_VERSION: u32 = 1;

/// A cached extraction entry.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct CacheEntry {
    /// BLAKE3 hash of the source file content (hex).
    content_hash: String,
    /// The extraction result.
    result: ExtractionResult,
    /// Unix timestamp (seconds) when the extraction was performed.
    extracted_at: u64,
    /// Extractor version that produced this result.
    extractor_version: u32,
}

/// BLAKE3-based content cache for skipping unchanged files on re-extraction.
pub struct ContentCache {
    cache_dir: PathBuf,
}

impl ContentCache {
    /// Create a new cache rooted at `project_root/.weftos/graphify-cache/`.
    pub fn new(project_root: &Path) -> io::Result<Self> {
        let cache_dir = project_root.join(".weftos").join("graphify-cache");
        fs::create_dir_all(&cache_dir)?;
        Ok(Self { cache_dir })
    }

    /// Create a cache at a custom directory (useful for testing).
    pub fn with_dir(cache_dir: PathBuf) -> io::Result<Self> {
        fs::create_dir_all(&cache_dir)?;
        Ok(Self { cache_dir })
    }

    /// Compute the BLAKE3 hash of a file's contents (hex string).
    fn file_hash(path: &Path) -> io::Result<String> {
        let content = fs::read(path)?;
        let hash = blake3::hash(&content);
        Ok(hash.to_hex().to_string())
    }

    /// Path to the cache entry JSON file for a given content hash.
    fn entry_path(&self, content_hash: &str) -> PathBuf {
        self.cache_dir.join(format!("{content_hash}.json"))
    }

    /// Look up a cached extraction for the given source file.
    ///
    /// Returns `None` if:
    /// - The file cannot be read.
    /// - No cache entry exists for the current content hash.
    /// - The cache entry was produced by a different extractor version.
    pub fn get(&self, path: &Path) -> Option<ExtractionResult> {
        let hash = Self::file_hash(path).ok()?;
        let entry_path = self.entry_path(&hash);
        let data = fs::read_to_string(&entry_path).ok()?;
        let entry: CacheEntry = serde_json::from_str(&data).ok()?;

        // Validate extractor version.
        if entry.extractor_version != EXTRACTOR_VERSION {
            return None;
        }
        // Validate content hash (belt-and-suspenders).
        if entry.content_hash != hash {
            return None;
        }

        Some(entry.result)
    }

    /// Store an extraction result for the given source file.
    ///
    /// Uses atomic write: writes to a `.tmp` file then renames.
    pub fn put(&self, path: &Path, result: &ExtractionResult) -> io::Result<()> {
        let hash = Self::file_hash(path)?;
        let entry = CacheEntry {
            content_hash: hash.clone(),
            result: result.clone(),
            extracted_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            extractor_version: EXTRACTOR_VERSION,
        };

        let json = serde_json::to_string(&entry)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        let entry_path = self.entry_path(&hash);
        let tmp_path = entry_path.with_extension("tmp");

        fs::write(&tmp_path, json)?;
        fs::rename(&tmp_path, &entry_path)?;

        Ok(())
    }

    /// Remove cache entries for files that no longer exist.
    ///
    /// `live_paths` should be the set of currently existing source file paths.
    /// Returns the number of stale entries removed.
    pub fn gc(&self, live_paths: &[PathBuf]) -> usize {
        let live_hashes: HashSet<String> = live_paths
            .iter()
            .filter_map(|p| Self::file_hash(p).ok())
            .collect();

        let mut removed = 0;
        if let Ok(entries) = fs::read_dir(&self.cache_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("json")
                    && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
                        && !live_hashes.contains(stem)
                            && fs::remove_file(&path).is_ok() {
                                removed += 1;
                            }
            }
        }
        removed
    }

    /// Remove all cache entries.
    pub fn clear(&self) -> io::Result<()> {
        if let Ok(entries) = fs::read_dir(&self.cache_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("json") {
                    fs::remove_file(&path)?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ExtractionResult;
    use tempfile::TempDir;

    fn make_result() -> ExtractionResult {
        ExtractionResult {
            source_file: "test.py".into(),
            entities: vec![],
            relationships: vec![],
            hyperedges: vec![],
            input_tokens: 0,
            output_tokens: 0,
            errors: vec![],
        }
    }

    #[test]
    fn cache_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let cache = ContentCache::with_dir(tmp.path().join("cache")).unwrap();

        // Write a source file.
        let src = tmp.path().join("test.py");
        fs::write(&src, "print('hello')").unwrap();

        let result = make_result();
        cache.put(&src, &result).unwrap();

        // Read it back.
        let cached = cache.get(&src).unwrap();
        assert_eq!(cached.source_file, "test.py");
    }

    #[test]
    fn cache_miss_on_content_change() {
        let tmp = TempDir::new().unwrap();
        let cache = ContentCache::with_dir(tmp.path().join("cache")).unwrap();

        let src = tmp.path().join("test.py");
        fs::write(&src, "v1").unwrap();
        cache.put(&src, &make_result()).unwrap();

        // Modify the file.
        fs::write(&src, "v2").unwrap();
        assert!(cache.get(&src).is_none());
    }

    #[test]
    fn gc_removes_stale() {
        let tmp = TempDir::new().unwrap();
        let cache = ContentCache::with_dir(tmp.path().join("cache")).unwrap();

        let src1 = tmp.path().join("a.py");
        let src2 = tmp.path().join("b.py");
        fs::write(&src1, "aaa").unwrap();
        fs::write(&src2, "bbb").unwrap();
        cache.put(&src1, &make_result()).unwrap();
        cache.put(&src2, &make_result()).unwrap();

        // Delete src2.
        fs::remove_file(&src2).unwrap();

        let removed = cache.gc(&[src1]);
        assert_eq!(removed, 1);
    }

    #[test]
    fn clear_removes_all() {
        let tmp = TempDir::new().unwrap();
        let cache = ContentCache::with_dir(tmp.path().join("cache")).unwrap();

        let src = tmp.path().join("test.py");
        fs::write(&src, "content").unwrap();
        cache.put(&src, &make_result()).unwrap();

        cache.clear().unwrap();
        assert!(cache.get(&src).is_none());
    }
}
