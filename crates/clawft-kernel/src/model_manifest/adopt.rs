//! Adopt-in-place scanning (ADR-101 section 3): hash model files where they
//! lie and describe them as a [`ModelPackageBody`]. Nothing is copied or
//! moved; the result records absolute locations only in the local registry,
//! never in the signed body (which carries paths relative to the root).
//!
//! Recognised layouts: an HF cache snapshot or MLX quant directory (a tree
//! of weight files plus tokenizer files, symlinks followed), a single GGUF
//! file, and Ollama blobs addressed through an Ollama manifest.

use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::Deserialize;

use super::body::{ModelError, ModelFormat, ModelPackageBody, ModelSource, MAX_SHARDS};
use crate::workload_pkg::manifest::{valid_token, validate_rel_path};
use crate::workload_pkg::FileRef;

/// Most directory levels scanned below the root.
const MAX_DEPTH: usize = 4;
/// Most entries visited in one scan (guards a mis-pointed root).
const MAX_VISITED: usize = 4096;
/// Extensions treated as weight shards.
const WEIGHT_EXTS: &[&str] = &["safetensors", "gguf", "bin", "pt", "pth", "npz", "ckpt"];
/// Tokenizer files, first match wins.
const TOKENIZER_FILES: &[&str] = &["tokenizer.json", "tokenizer.model"];
/// Chat template files, or the config that carries the template.
const TEMPLATE_FILES: &[&str] = &["chat_template.jinja", "chat_template.json", "tokenizer_config.json"];

/// What a file is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileRole {
    /// A weight shard.
    Shard,
    /// The tokenizer.
    Tokenizer,
    /// The chat template (or its carrier).
    Template,
}

/// A file as found: relative path, size, mtime and content hash.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, Deserialize)]
pub struct ScannedFile {
    /// Path relative to the root (forward slashes).
    pub path: String,
    /// Role.
    pub role: FileRole,
    /// Size in bytes.
    pub size: u64,
    /// Modification time, nanoseconds since the epoch (0 when unknown).
    pub mtime_ns: u64,
    /// BLAKE3 hex.
    pub blake3: String,
}

/// Operator-supplied description of what is being adopted.
#[derive(Debug, Clone)]
pub struct AdoptInput {
    /// Model name.
    pub name: String,
    /// Layout.
    pub format: ModelFormat,
    /// Provenance.
    pub source: ModelSource,
    /// The operator explicitly opts in to sharing the weights.
    pub redistributable: bool,
}

/// A scanned model, ready to be attested and registered.
#[derive(Debug, Clone)]
pub struct ScannedModel {
    /// Canonical model root.
    pub root: PathBuf,
    /// The describing body.
    pub body: ModelPackageBody,
    /// Every hashed file with its stat stamp.
    pub files: Vec<ScannedFile>,
}

pub(crate) fn io_err(path: &Path, e: impl std::fmt::Display) -> ModelError {
    ModelError::Io {
        path: path.display().to_string(),
        msg: e.to_string(),
    }
}

/// Stat stamp of a file: `(size, mtime_ns)`. Follows symlinks.
pub fn stamp(path: &Path) -> std::io::Result<(u64, u64)> {
    let m = fs::metadata(path)?;
    let mtime = m
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
        .unwrap_or(0);
    Ok((m.len(), mtime))
}

/// Stream-hash a file with BLAKE3 (1 MiB buffer; never read whole).
pub fn hash_file(path: &Path) -> std::io::Result<String> {
    let mut f = File::open(path)?;
    let mut h = blake3::Hasher::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().to_hex().to_string())
}

fn scan_file_entry(root: &Path, rel: &str, role: FileRole) -> Result<ScannedFile, ModelError> {
    let abs = root.join(rel);
    let (size, mtime_ns) = stamp(&abs).map_err(|e| io_err(&abs, e))?;
    let blake3 = hash_file(&abs).map_err(|e| io_err(&abs, e))?;
    Ok(ScannedFile {
        path: rel.to_string(),
        role,
        size,
        mtime_ns,
        blake3,
    })
}

fn is_weight(name: &str) -> bool {
    name.rsplit_once('.')
        .is_some_and(|(_, ext)| WEIGHT_EXTS.contains(&ext.to_ascii_lowercase().as_str()))
}

fn walk(root: &Path, dir: &Path, depth: usize, found: &mut Vec<String>, visited: &mut usize) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        *visited += 1;
        if *visited > MAX_VISITED {
            return;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || !valid_token(&name, 128) {
            continue;
        }
        let path = entry.path();
        // `metadata` follows symlinks (HF snapshots link into blobs/).
        let Ok(meta) = fs::metadata(&path) else { continue };
        if meta.is_dir() {
            if depth < MAX_DEPTH {
                walk(root, &path, depth + 1, found, visited);
            }
        } else if meta.is_file()
            && let Ok(rel) = path.strip_prefix(root)
        {
            found.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
}

fn finish(
    root: PathBuf,
    input: AdoptInput,
    shards: Vec<ScannedFile>,
    tokenizer: Option<ScannedFile>,
    template: Option<ScannedFile>,
) -> Result<ScannedModel, ModelError> {
    if shards.is_empty() {
        return Err(ModelError::Scan(format!(
            "no model weight files ({}) found under {}",
            WEIGHT_EXTS.join(", "),
            root.display()
        )));
    }
    if shards.len() > MAX_SHARDS {
        return Err(ModelError::Scan(format!(
            "{} weight files exceeds the {MAX_SHARDS} shard limit",
            shards.len()
        )));
    }
    let mut body = ModelPackageBody {
        name: input.name,
        format: input.format,
        shards: shards
            .iter()
            .map(|s| FileRef {
                path: s.path.clone(),
                size: s.size,
                blake3: s.blake3.clone(),
            })
            .collect(),
        tokenizer_blake3: tokenizer.as_ref().map(|t| t.blake3.clone()),
        template_blake3: template.as_ref().map(|t| t.blake3.clone()),
        source: input.source,
        redistributable: input.redistributable,
    };
    body.shards.sort_by(|a, b| a.path.cmp(&b.path));
    body.validate()?;
    let mut files = shards;
    files.extend(tokenizer);
    files.extend(template);
    Ok(ScannedModel { root, body, files })
}

/// Scan a model directory in place: weight files become shards, a tokenizer
/// and a template file (when present) are hashed too.
pub fn scan_dir(root: &Path, input: AdoptInput) -> Result<ScannedModel, ModelError> {
    let root = root.canonicalize().map_err(|e| io_err(root, e))?;
    if !root.is_dir() {
        return Err(ModelError::Scan(format!("{} is not a directory", root.display())));
    }
    let mut found = Vec::new();
    walk(&root, &root, 0, &mut found, &mut 0);
    found.sort();
    let mut shards = Vec::new();
    for rel in found.iter().filter(|r| is_weight(r.rsplit('/').next().unwrap_or(r))) {
        shards.push(scan_file_entry(&root, rel, FileRole::Shard)?);
    }
    let pick = |names: &[&str], role| -> Result<Option<ScannedFile>, ModelError> {
        names
            .iter()
            .find(|n| found.iter().any(|f| f == *n))
            .map(|n| scan_file_entry(&root, n, role))
            .transpose()
    };
    let tokenizer = pick(TOKENIZER_FILES, FileRole::Tokenizer)?;
    let template = pick(TEMPLATE_FILES, FileRole::Template)?;
    finish(root, input, shards, tokenizer, template)
}

/// Scan one weight file (a GGUF) in place. The root is its directory and
/// the single shard is its file name.
pub fn scan_file(path: &Path, input: AdoptInput) -> Result<ScannedModel, ModelError> {
    let abs = path.canonicalize().map_err(|e| io_err(path, e))?;
    let name = abs
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| ModelError::Scan("file name is not valid UTF-8".into()))?
        .to_string();
    let root = abs
        .parent()
        .ok_or_else(|| ModelError::Scan("file has no parent directory".into()))?
        .to_path_buf();
    validate_rel_path(&name).map_err(|e| ModelError::Scan(e.to_string()))?;
    let shard = scan_file_entry(&root, &name, FileRole::Shard)?;
    finish(root, input, vec![shard], None, None)
}

#[derive(Deserialize)]
struct OllamaManifest {
    layers: Vec<OllamaLayer>,
}

#[derive(Deserialize)]
struct OllamaLayer {
    #[serde(rename = "mediaType")]
    media_type: String,
    digest: String,
    size: u64,
}

fn ollama_blob_rel(digest: &str) -> Result<String, ModelError> {
    let hex = digest
        .strip_prefix("sha256:")
        .filter(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(|| ModelError::Scan(format!("bad ollama digest {digest:?}")))?;
    Ok(format!("blobs/sha256-{}", hex.to_ascii_lowercase()))
}

/// Scan an Ollama model in place through its manifest under `models_dir`
/// (`manifests/registry.ollama.ai/library/<name>/<tag>`). The model layer
/// becomes the shard; the template layer, when present, the template hash.
/// `input.source.ollama_tag` is set to `name:tag`.
pub fn scan_ollama(
    models_dir: &Path,
    name: &str,
    tag: &str,
    mut input: AdoptInput,
) -> Result<ScannedModel, ModelError> {
    for part in [name, tag] {
        if !valid_token(part, 64) {
            return Err(ModelError::Scan(format!("bad ollama name or tag {part:?}")));
        }
    }
    let root = models_dir.canonicalize().map_err(|e| io_err(models_dir, e))?;
    let mpath = root
        .join("manifests/registry.ollama.ai/library")
        .join(name)
        .join(tag);
    let bytes = fs::read(&mpath).map_err(|e| io_err(&mpath, e))?;
    if bytes.len() > 1024 * 1024 {
        return Err(ModelError::Scan("ollama manifest too large".into()));
    }
    let m: OllamaManifest = serde_json::from_slice(&bytes)
        .map_err(|e| ModelError::Scan(format!("ollama manifest: {e}")))?;
    let mut shards = Vec::new();
    let mut template = None;
    for layer in &m.layers {
        let (role, slot) = match layer.media_type.as_str() {
            "application/vnd.ollama.image.model" => (FileRole::Shard, true),
            "application/vnd.ollama.image.template" => (FileRole::Template, false),
            _ => continue,
        };
        let rel = ollama_blob_rel(&layer.digest)?;
        let f = scan_file_entry(&root, &rel, role)?;
        if f.size != layer.size {
            return Err(ModelError::HashMismatch {
                path: rel,
                expected: format!("{} bytes", layer.size),
                actual: format!("{} bytes", f.size),
            });
        }
        if slot {
            shards.push(f);
        } else {
            template = Some(f);
        }
    }
    input.format = ModelFormat::Ollama;
    input.source.ollama_tag = Some(format!("{name}:{tag}"));
    finish(root, input, shards, None, template)
}
