//! The artifact cache: files named by BLAKE3 under `cache/`, 256 MiB LRU.
//! Entries under an active grant are pinned and never evicted.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::fsio;

/// Why an artifact could not be cached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheError {
    /// The artifact alone is larger than the cache.
    TooLarge,
    /// Everything that could be evicted is pinned.
    Full,
    /// Disk error.
    Io(String),
}

/// An LRU file cache.
#[derive(Debug)]
pub struct Cache {
    dir: PathBuf,
    cap: u64,
    /// Least recently used first.
    order: Vec<String>,
    sizes: HashMap<String, u64>,
}

fn is_hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl Cache {
    /// Open (and index) `dir`. Stray files are ignored.
    pub fn open(dir: &Path, cap: u64) -> Result<Self, CacheError> {
        fsio::ensure_private_dir(dir).map_err(|e| CacheError::Io(e.to_string()))?;
        let mut found: Vec<(std::time::SystemTime, String, u64)> = Vec::new();
        for ent in std::fs::read_dir(dir).map_err(|e| CacheError::Io(e.to_string()))?.flatten() {
            let name = ent.file_name().to_string_lossy().into_owned();
            if !is_hash(&name) {
                continue;
            }
            if let Ok(m) = ent.metadata() {
                found.push((m.modified().unwrap_or(std::time::UNIX_EPOCH), name, m.len()));
            }
        }
        found.sort();
        let mut c = Self { dir: dir.to_path_buf(), cap, order: Vec::new(), sizes: HashMap::new() };
        for (_, name, len) in found {
            c.order.push(name.clone());
            c.sizes.insert(name, len);
        }
        Ok(c)
    }

    /// Path of a cached artifact, marking it recently used.
    pub fn get(&mut self, blake3: &str) -> Option<(PathBuf, u64)> {
        let len = *self.sizes.get(blake3)?;
        self.order.retain(|h| h != blake3);
        self.order.push(blake3.to_string());
        Some((self.dir.join(blake3), len))
    }

    /// True when `blake3` is cached.
    pub fn contains(&self, blake3: &str) -> bool {
        self.sizes.contains_key(blake3)
    }

    /// Total bytes held.
    pub fn used(&self) -> u64 {
        self.sizes.values().sum()
    }

    /// Store `bytes` under `blake3`, evicting unpinned entries oldest first.
    pub fn put(&mut self, blake3: &str, bytes: &[u8], pinned: &HashSet<String>) -> Result<(), CacheError> {
        let need = bytes.len() as u64;
        if need > self.cap {
            return Err(CacheError::TooLarge);
        }
        if self.contains(blake3) {
            self.get(blake3);
            return Ok(());
        }
        while self.used() + need > self.cap {
            let victim = self.order.iter().find(|h| !pinned.contains(*h)).cloned();
            let Some(v) = victim else { return Err(CacheError::Full) };
            let _ = std::fs::remove_file(self.dir.join(&v));
            self.order.retain(|h| h != &v);
            self.sizes.remove(&v);
        }
        fsio::write_atomic(&self.dir.join(blake3), bytes).map_err(|e| CacheError::Io(e.to_string()))?;
        self.order.push(blake3.to_string());
        self.sizes.insert(blake3.to_string(), need);
        Ok(())
    }
}
