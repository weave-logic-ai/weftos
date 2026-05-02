//! File discovery, type classification, sensitive file filtering, and manifest.
//!
//! Ports Python `detect.py` -- 280 lines covering file walking, classification,
//! paper detection heuristics, sensitive-file skip patterns, and incremental
//! detection with manifest diffing.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use regex::Regex;
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::entity::FileType;
use crate::GraphifyError;

// ── Extension sets ──────────────────────────────────────────────────────────

/// All 22 code extensions supported by the extractor.
pub static CODE_EXTENSIONS: &[&str] = &[
    "py", "ts", "js", "tsx", "go", "rs", "java", "cpp", "cc", "cxx", "c", "h", "hpp", "rb",
    "swift", "kt", "kts", "cs", "scala", "php", "lua", "toc",
];

pub static DOC_EXTENSIONS: &[&str] = &["md", "txt", "rst"];
pub static PAPER_EXTENSIONS: &[&str] = &["pdf"];
pub static IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "svg"];

/// Corpus size thresholds.
pub const CORPUS_WARN_THRESHOLD: usize = 50_000;
pub const CORPUS_UPPER_THRESHOLD: usize = 500_000;
pub const FILE_COUNT_UPPER: usize = 200;

// ── Skip directories ────────────────────────────────────────────────────────

/// Directories to always skip -- venvs, caches, build artifacts, deps.
static SKIP_DIRS: &[&str] = &[
    "venv",
    ".venv",
    "env",
    ".env",
    "node_modules",
    "__pycache__",
    ".git",
    "dist",
    "build",
    "target",
    "out",
    "site-packages",
    "lib64",
    ".pytest_cache",
    ".mypy_cache",
    ".ruff_cache",
    ".tox",
    ".eggs",
    ".idea",
    ".vscode",
    ".vs",
    ".gradle",
    ".cache",
    ".npm",
    ".yarn",
    "vendor",
    "Pods",
    ".next",
    ".nuxt",
    "coverage",
    ".coverage",
    "htmlcov",
    ".hypothesis",
    "bower_components",
    ".terraform",
    ".serverless",
    ".aws-sam",
    ".bundle",
];

fn is_noise_dir(name: &str) -> bool {
    if SKIP_DIRS.contains(&name) {
        return true;
    }
    if name.ends_with("_venv") || name.ends_with("_env") {
        return true;
    }
    if name.ends_with(".egg-info") {
        return true;
    }
    false
}

// ── Sensitive file patterns ─────────────────────────────────────────────────

fn build_sensitive_patterns() -> Vec<Regex> {
    vec![
        Regex::new(r"(?i)(^|[\\/])\.(env|envrc)(\.|$)").unwrap(),
        Regex::new(r"(?i)\.(pem|key|p12|pfx|cert|crt|der|p8)$").unwrap(),
        Regex::new(r"(?i)(credential|secret|passwd|password|token|private_key)").unwrap(),
        Regex::new(r"(id_rsa|id_dsa|id_ecdsa|id_ed25519)(\.pub)?$").unwrap(),
        Regex::new(r"(?i)(\.netrc|\.pgpass|\.htpasswd)$").unwrap(),
        Regex::new(r"(?i)(aws_credentials|gcloud_credentials|service.account)").unwrap(),
    ]
}

/// Return true if this file likely contains secrets and should be skipped.
pub fn is_sensitive(path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let full = path.to_string_lossy().to_string();

    let patterns = build_sensitive_patterns();
    patterns
        .iter()
        .any(|p| p.is_match(&name) || p.is_match(&full))
}

// ── Paper detection ─────────────────────────────────────────────────────────

fn build_paper_signals() -> Vec<Regex> {
    vec![
        Regex::new(r"(?i)\barxiv\b").unwrap(),
        Regex::new(r"(?i)\bdoi\s*:").unwrap(),
        Regex::new(r"(?i)\babstract\b").unwrap(),
        Regex::new(r"(?i)\bproceedings\b").unwrap(),
        Regex::new(r"(?i)\bjournal\b").unwrap(),
        Regex::new(r"(?i)\bpreprint\b").unwrap(),
        Regex::new(r"\\cite\{").unwrap(),
        Regex::new(r"\[\d+\]").unwrap(),
        Regex::new(r"\[\n\d+\n\]").unwrap(),
        Regex::new(r"(?i)eq\.\s*\d+|equation\s+\d+").unwrap(),
        Regex::new(r"\d{4}\.\d{4,5}").unwrap(),
        Regex::new(r"(?i)\bwe propose\b").unwrap(),
        Regex::new(r"(?i)\bliterature\b").unwrap(),
    ]
}

const PAPER_SIGNAL_THRESHOLD: usize = 3;

/// Heuristic: does this text file read like an academic paper?
pub fn looks_like_paper(path: &Path) -> bool {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => return false,
    };
    let sample: &str = if text.len() > 3000 {
        &text[..3000]
    } else {
        &text
    };

    let signals = build_paper_signals();
    let hits = signals.iter().filter(|p| p.is_match(sample)).count();
    hits >= PAPER_SIGNAL_THRESHOLD
}

// ── Classification ──────────────────────────────────────────────────────────

/// Classify a file by extension, with paper-detection heuristic for doc files.
pub fn classify_file(path: &Path) -> Option<FileType> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    if CODE_EXTENSIONS.contains(&ext.as_str()) {
        return Some(FileType::Code);
    }
    if PAPER_EXTENSIONS.contains(&ext.as_str()) {
        return Some(FileType::Paper);
    }
    if IMAGE_EXTENSIONS.contains(&ext.as_str()) {
        return Some(FileType::Image);
    }
    if DOC_EXTENSIONS.contains(&ext.as_str()) {
        if looks_like_paper(path) {
            return Some(FileType::Paper);
        }
        return Some(FileType::Document);
    }
    None
}

/// Count words in a file (approximate, splitting on whitespace).
pub fn count_words(path: &Path) -> usize {
    match std::fs::read_to_string(path) {
        Ok(text) => text.split_whitespace().count(),
        Err(_) => 0,
    }
}

// ── Detection result ────────────────────────────────────────────────────────

/// Result of detecting files in a directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectionResult {
    pub files: HashMap<String, Vec<String>>,
    pub total_files: usize,
    pub total_words: usize,
    pub needs_graph: bool,
    pub warning: Option<String>,
    pub skipped_sensitive: Vec<String>,
}

/// Walk a directory and classify all files.
pub fn detect(root: &Path) -> Result<DetectionResult, GraphifyError> {
    let mut files: HashMap<String, Vec<String>> = HashMap::new();
    files.insert("code".into(), Vec::new());
    files.insert("document".into(), Vec::new());
    files.insert("paper".into(), Vec::new());
    files.insert("image".into(), Vec::new());

    let mut total_words: usize = 0;
    let mut skipped_sensitive: Vec<String> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();

    let memory_dir = root.join("graphify-out").join("memory");

    // Walk root, pruning noise dirs.
    // Note: depth 0 is the root itself, which we never skip.
    let walker = WalkDir::new(root).follow_links(false).into_iter();
    for entry in walker.filter_entry(|e| {
        if e.depth() == 0 {
            return true; // Never skip the root
        }
        let name = e.file_name().to_string_lossy();
        if e.file_type().is_dir() {
            !name.starts_with('.') && !is_noise_dir(&name)
        } else {
            true
        }
    }) {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path().to_path_buf();
        if !seen.insert(path.clone()) {
            continue;
        }

        // Check sensitive FIRST (before hidden-file skip) so .env gets counted
        if is_sensitive(&path) {
            skipped_sensitive.push(path.to_string_lossy().to_string());
            continue;
        }

        // Skip hidden files
        if path
            .file_name()
            .map(|n| n.to_string_lossy().starts_with('.'))
            .unwrap_or(false)
        {
            continue;
        }

        if let Some(ftype) = classify_file(&path) {
            let key = match ftype {
                FileType::Code => "code",
                FileType::Document => "document",
                FileType::Paper => "paper",
                FileType::Image => "image",
                _ => continue,
            };
            total_words += count_words(&path);
            files
                .get_mut(key)
                .unwrap()
                .push(path.to_string_lossy().to_string());
        }
    }

    // Also scan memory dir if it exists
    if memory_dir.exists() {
        let mem_walker = WalkDir::new(&memory_dir).follow_links(false).into_iter();
        for entry in mem_walker.flatten() {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path().to_path_buf();
            if !seen.insert(path.clone()) {
                continue;
            }
            if is_sensitive(&path) {
                skipped_sensitive.push(path.to_string_lossy().to_string());
                continue;
            }
            if let Some(ftype) = classify_file(&path) {
                let key = match ftype {
                    FileType::Code => "code",
                    FileType::Document => "document",
                    FileType::Paper => "paper",
                    FileType::Image => "image",
                    _ => continue,
                };
                total_words += count_words(&path);
                files
                    .get_mut(key)
                    .unwrap()
                    .push(path.to_string_lossy().to_string());
            }
        }
    }

    let total_files: usize = files.values().map(|v| v.len()).sum();
    let needs_graph = total_words >= CORPUS_WARN_THRESHOLD;

    let warning = if !needs_graph {
        Some(format!(
            "Corpus is ~{total_words} words - fits in a single context window. \
             You may not need a graph."
        ))
    } else if total_words >= CORPUS_UPPER_THRESHOLD || total_files >= FILE_COUNT_UPPER {
        Some(format!(
            "Large corpus: {total_files} files / ~{total_words} words. \
             Semantic extraction will be expensive (many Claude tokens). \
             Consider running on a subfolder, or use --no-semantic to run AST-only."
        ))
    } else {
        None
    };

    Ok(DetectionResult {
        files,
        total_files,
        total_words,
        needs_graph,
        warning,
        skipped_sensitive,
    })
}

// ── Manifest for incremental detection ──────────────────────────────────────

/// Manifest: file path -> modification time.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Manifest {
    pub entries: HashMap<String, f64>,
}

impl Manifest {
    pub fn load(manifest_path: &Path) -> Self {
        match std::fs::read_to_string(manifest_path) {
            Ok(text) => {
                // The manifest is stored as a bare HashMap, not a Manifest struct
                match serde_json::from_str::<HashMap<String, f64>>(&text) {
                    Ok(entries) => Self { entries },
                    Err(_) => Self::default(),
                }
            }
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self, manifest_path: &Path) -> Result<(), GraphifyError> {
        if let Some(parent) = manifest_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(&self.entries)
            .map_err(|e| GraphifyError::CacheError(e.to_string()))?;
        std::fs::write(manifest_path, json)?;
        Ok(())
    }

    pub fn from_detection(detection: &DetectionResult) -> Self {
        let mut entries = HashMap::new();
        for file_list in detection.files.values() {
            for f in file_list {
                if let Ok(meta) = std::fs::metadata(f)
                    && let Ok(mtime) = meta.modified() {
                        let secs = mtime
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs_f64();
                        entries.insert(f.clone(), secs);
                    }
            }
        }
        Self { entries }
    }
}

/// Result of incremental detection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncrementalDetection {
    pub full: DetectionResult,
    pub new_files: HashMap<String, Vec<String>>,
    pub unchanged_files: HashMap<String, Vec<String>>,
    pub deleted_files: Vec<String>,
    pub new_total: usize,
}

/// Incremental detection: compare current files against a stored manifest.
pub fn detect_incremental(
    root: &Path,
    manifest: &Manifest,
) -> Result<IncrementalDetection, GraphifyError> {
    let full = detect(root)?;

    if manifest.entries.is_empty() {
        let new_total = full.total_files;
        let new_files = full.files.clone();
        let unchanged_files: HashMap<String, Vec<String>> = full
            .files
            .keys()
            .map(|k| (k.clone(), Vec::new()))
            .collect();
        return Ok(IncrementalDetection {
            full,
            new_files,
            unchanged_files,
            deleted_files: Vec::new(),
            new_total,
        });
    }

    let mut new_files: HashMap<String, Vec<String>> = HashMap::new();
    let mut unchanged_files: HashMap<String, Vec<String>> = HashMap::new();

    for (ftype, file_list) in &full.files {
        let new_list = new_files.entry(ftype.clone()).or_default();
        let unchanged_list = unchanged_files.entry(ftype.clone()).or_default();

        for f in file_list {
            let stored_mtime = manifest.entries.get(f.as_str());
            let current_mtime = std::fs::metadata(f)
                .ok()
                .and_then(|m| m.modified().ok())
                .map(|t| {
                    t.duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs_f64()
                })
                .unwrap_or(0.0);

            match stored_mtime {
                Some(&prev) if current_mtime <= prev => {
                    unchanged_list.push(f.clone());
                }
                _ => {
                    new_list.push(f.clone());
                }
            }
        }
    }

    let current_files: HashSet<&str> = full
        .files
        .values()
        .flat_map(|v| v.iter().map(|s| s.as_str()))
        .collect();
    let deleted_files: Vec<String> = manifest
        .entries
        .keys()
        .filter(|k| !current_files.contains(k.as_str()))
        .cloned()
        .collect();

    let new_total: usize = new_files.values().map(|v| v.len()).sum();

    Ok(IncrementalDetection {
        full,
        new_files,
        unchanged_files,
        deleted_files,
        new_total,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn classify_code_extensions() {
        let tmp = TempDir::new().unwrap();
        let py = tmp.path().join("test.py");
        fs::write(&py, "print('hello')").unwrap();
        assert_eq!(classify_file(&py), Some(FileType::Code));

        let rs = tmp.path().join("test.rs");
        fs::write(&rs, "fn main() {}").unwrap();
        assert_eq!(classify_file(&rs), Some(FileType::Code));
    }

    #[test]
    fn classify_doc_vs_paper() {
        let tmp = TempDir::new().unwrap();

        let doc = tmp.path().join("readme.md");
        fs::write(&doc, "# My Project\nSome docs").unwrap();
        assert_eq!(classify_file(&doc), Some(FileType::Document));

        let paper = tmp.path().join("paper.md");
        fs::write(
            &paper,
            "# Abstract\narxiv preprint 2024.12345\n\
             We propose a novel approach. From the literature [1] [2] [3]\n\
             doi: 10.1234/test proceedings journal",
        )
        .unwrap();
        assert_eq!(classify_file(&paper), Some(FileType::Paper));
    }

    #[test]
    fn classify_pdf_as_paper() {
        let tmp = TempDir::new().unwrap();
        let pdf = tmp.path().join("test.pdf");
        fs::write(&pdf, "fake pdf").unwrap();
        assert_eq!(classify_file(&pdf), Some(FileType::Paper));
    }

    #[test]
    fn classify_image() {
        let tmp = TempDir::new().unwrap();
        let png = tmp.path().join("test.png");
        fs::write(&png, "fake png").unwrap();
        assert_eq!(classify_file(&png), Some(FileType::Image));
    }

    #[test]
    fn classify_unknown() {
        let tmp = TempDir::new().unwrap();
        let xyz = tmp.path().join("test.xyz");
        fs::write(&xyz, "unknown").unwrap();
        assert_eq!(classify_file(&xyz), None);
    }

    #[test]
    fn sensitive_files_detected() {
        assert!(is_sensitive(Path::new(".env")));
        assert!(is_sensitive(Path::new("id_rsa")));
        assert!(is_sensitive(Path::new("credentials.json")));
        assert!(is_sensitive(Path::new("server.key")));
        assert!(is_sensitive(Path::new("private_key.pem")));
        assert!(!is_sensitive(Path::new("main.py")));
        assert!(!is_sensitive(Path::new("config.toml")));
    }

    #[test]
    fn noise_dirs_detected() {
        assert!(is_noise_dir("node_modules"));
        assert!(is_noise_dir("__pycache__"));
        assert!(is_noise_dir(".git"));
        assert!(is_noise_dir("target"));
        assert!(is_noise_dir("my_venv"));
        assert!(is_noise_dir("foo.egg-info"));
        assert!(!is_noise_dir("src"));
        assert!(!is_noise_dir("lib"));
    }

    #[test]
    fn detect_basic_tree() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("main.py"), "print('hello')").unwrap();
        fs::write(tmp.path().join("readme.md"), "# Docs").unwrap();
        fs::create_dir_all(tmp.path().join("node_modules")).unwrap();
        fs::write(
            tmp.path().join("node_modules/junk.js"),
            "// should be skipped",
        )
        .unwrap();

        let result = detect(tmp.path()).unwrap();
        assert_eq!(result.files["code"].len(), 1);
        assert_eq!(result.files["document"].len(), 1);
        assert_eq!(result.total_files, 2);
    }

    #[test]
    fn detect_skips_sensitive() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("main.py"), "x = 1").unwrap();
        fs::write(tmp.path().join(".env"), "SECRET=foo").unwrap();

        let result = detect(tmp.path()).unwrap();
        assert_eq!(result.files["code"].len(), 1);
        assert_eq!(result.skipped_sensitive.len(), 1);
    }

    #[test]
    fn manifest_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let manifest_path = tmp.path().join("manifest.json");

        let mut m = Manifest::default();
        m.entries.insert("foo.py".into(), 12345.0);
        m.save(&manifest_path).unwrap();

        let loaded = Manifest::load(&manifest_path);
        assert_eq!(loaded.entries.get("foo.py"), Some(&12345.0));
    }
}
