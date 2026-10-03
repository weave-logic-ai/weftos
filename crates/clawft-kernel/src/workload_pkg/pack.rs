//! `cog-pack`: build a cog package directory from a cog source directory
//! (`vendor/cogs/src/cogs/<id>`, holding the unmodified `cog.toml`) plus
//! one binary per arch (from our fork build or upstream released binaries).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::cognitum::COGNITUM_RECORD_KIND;
use super::manifest::{
    AttestationRef, COG_ARCHES, CogPackageBody, FileRef, KIND_COG, MANIFEST_FILE, ManifestEnvelope,
    ManifestError, PackageSource, binary_path,
};
use super::verify::MAX_FILE_BYTES;

/// Largest `cog.toml` accepted.
const MAX_COG_TOML_BYTES: u64 = 256 * 1024;
/// Path of a packaged Cognitum release record.
pub const COGNITUM_RECORD_PATH: &str = "attestations/cognitum-release-record.json";
/// Attestation kind stamped when the cog dir's `provenance.json` says the
/// binary is a licensed Cognitum install (`trust = "cognitum-sha256"`).
/// Unlike [`COGNITUM_RECORD_KIND`] it is not a signed release record and is
/// never parsed as one; it only marks the package Cognitum-origin.
pub const COGNITUM_PROVENANCE_KIND: &str = "cognitum.install.provenance.v1";
/// Path of the stamped provenance attestation.
pub const COGNITUM_PROVENANCE_PATH: &str = "attestations/cognitum-provenance.json";
/// `provenance.json` in an installed cog dir (written by `weaver cog install`).
const PROVENANCE_FILE: &str = "provenance.json";
/// `provenance.json` trust value for a licensed Cognitum binary.
const TRUST_COGNITUM_SHA256: &str = "cognitum-sha256";

/// Inputs to [`pack_cog`].
#[derive(Debug, Clone, Default)]
pub struct CogPackInput {
    /// Directory holding `cog.toml`.
    pub cog_dir: PathBuf,
    /// `(arch, binary path)` pairs.
    pub binaries: Vec<(String, PathBuf)>,
    /// Provenance.
    pub source: PackageSource,
    /// Optional Cognitum ADR-154/155 release record to carry.
    pub cognitum_record: Option<PathBuf>,
    /// Sign the package as redistributable (see
    /// [`CogPackageBody::redistributable`]). Default for a packer: false.
    pub redistributable: bool,
    /// Explicit `provenance.json` describing the binaries. Otherwise the cog
    /// dir's is read, and one next to each binary (where `weaver cog install`
    /// writes it).
    pub provenance: Option<PathBuf>,
    /// Allow `redistributable` with no `provenance.json` anywhere. Without it,
    /// sharing a package needs evidence the binary is not a licensed Cognitum
    /// one.
    pub allow_no_provenance: bool,
}

/// Packing failure.
#[derive(Debug, thiserror::Error)]
pub enum PackError {
    /// Bad inputs.
    #[error("pack input: {0}")]
    Input(String),
    /// Filesystem error.
    #[error("pack io on {path}: {msg}")]
    Io {
        /// Path involved.
        path: String,
        /// Error text.
        msg: String,
    },
    /// Manifest construction failed.
    #[error(transparent)]
    Manifest(#[from] ManifestError),
}

fn io_err(path: &Path, e: impl std::fmt::Display) -> PackError {
    PackError::Io {
        path: path.display().to_string(),
        msg: e.to_string(),
    }
}

fn read_limited(path: &Path, max: u64) -> Result<Vec<u8>, PackError> {
    let meta = std::fs::metadata(path).map_err(|e| io_err(path, e))?;
    if !meta.is_file() {
        return Err(io_err(path, "not a regular file"));
    }
    if meta.len() > max {
        return Err(io_err(path, format!("larger than {max} bytes")));
    }
    std::fs::read(path).map_err(|e| io_err(path, e))
}

/// `[cog].id` and `[cog].version` from `cog.toml` text.
pub fn cog_identity(toml_text: &str) -> Result<(String, String), PackError> {
    let doc: toml::Value =
        toml::from_str(toml_text).map_err(|e| PackError::Input(format!("cog.toml: {e}")))?;
    let field = |k: &str| {
        doc.get("cog")
            .and_then(|c| c.get(k))
            .and_then(toml::Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| PackError::Input(format!("cog.toml: [cog].{k} missing")))
    };
    Ok((field("id")?, field("version")?))
}

/// Write `content` to `out/rel` (new file only) and return its [`FileRef`].
fn place(out: &Path, rel: &str, content: &[u8], executable: bool) -> Result<FileRef, PackError> {
    use std::io::Write;
    let dest = out.join(rel);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| io_err(parent, e))?;
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(if executable { 0o755 } else { 0o644 });
    }
    #[cfg(not(unix))]
    let _ = executable;
    let mut f = opts.open(&dest).map_err(|e| io_err(&dest, e))?;
    f.write_all(content).map_err(|e| io_err(&dest, e))?;
    Ok(FileRef {
        path: rel.to_string(),
        size: content.len() as u64,
        blake3: blake3::hash(content).to_hex().to_string(),
    })
}

/// A `provenance.json` found for the package.
struct FoundProvenance {
    /// Arch of the `--bin` it sits next to; `None` for the cog dir's or an explicit one.
    beside: Option<String>,
    trust: Option<String>,
    sha256: Option<String>,
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// Every `provenance.json` that describes this package: the explicit one,
/// the cog dir's, and one beside each binary. A file that exists but cannot
/// be read or parsed is an error, since ignoring it could let a licensed
/// binary be packed as shareable.
fn find_provenance(input: &CogPackInput) -> Result<Vec<FoundProvenance>, PackError> {
    let mut candidates: Vec<(Option<String>, PathBuf)> = Vec::new();
    if let Some(p) = &input.provenance {
        if !p.is_file() {
            return Err(PackError::Input(format!("--provenance {}: no such file", p.display())));
        }
        candidates.push((None, p.clone()));
    }
    candidates.push((None, input.cog_dir.join(PROVENANCE_FILE)));
    for (arch, bin) in &input.binaries {
        if let Some(dir) = bin.parent() {
            candidates.push((Some(arch.clone()), dir.join(PROVENANCE_FILE)));
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for (beside, path) in candidates {
        if !path.is_file() {
            continue;
        }
        let canon = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        // A file seen again beside a binary keeps that association.
        if !seen.insert((canon, beside.clone())) {
            continue;
        }
        let bytes = read_limited(&path, 64 * 1024)?;
        let doc: Value = serde_json::from_slice(&bytes)
            .map_err(|e| PackError::Input(format!("{}: {e}", path.display())))?;
        let get = |k: &str| doc.get(k).and_then(Value::as_str).map(str::to_string);
        out.push(FoundProvenance { beside, trust: get("trust"), sha256: get("sha256") });
    }
    Ok(out)
}

/// Decide what provenance means for this pack. Returns the bytes of a
/// Cognitum stamp to carry when any provenance marks a licensed Cognitum
/// install. With `redistributable`, refuses a Cognitum install, and refuses
/// when there is no provenance (unless `allow_no_provenance`), when the
/// provenance names no recognised non-Cognitum trust or no sha256, or when its
/// sha256 does not match the binary it describes.
fn provenance_decision(
    input: &CogPackInput,
    id: &str,
    version: &str,
    bin_sha: &BTreeMap<String, String>,
) -> Result<Option<Vec<u8>>, PackError> {
    let found = find_provenance(input)?;
    let cognitum = found.iter().find(|f| f.trust.as_deref() == Some(TRUST_COGNITUM_SHA256));
    if let Some(c) = cognitum {
        if input.redistributable {
            return Err(PackError::Input(format!(
                "{PROVENANCE_FILE} marks {id}@{version} as a licensed Cognitum install \
                 (trust = {TRUST_COGNITUM_SHA256}); --redistributable is refused"
            )));
        }
        let stamp = serde_json::json!({
            "trust": TRUST_COGNITUM_SHA256,
            "cog_id": id,
            "version": version,
            "sha256": c.sha256,
        });
        return serde_json::to_vec_pretty(&stamp)
            .map(Some)
            .map_err(|e| PackError::Input(format!("{PROVENANCE_FILE}: {e}")));
    }
    if !input.redistributable {
        return Ok(None);
    }
    if found.is_empty() {
        if input.allow_no_provenance {
            return Ok(None);
        }
        return Err(PackError::Input(format!(
            "--redistributable needs a {PROVENANCE_FILE} (--provenance, in the cog dir, or beside a --bin) \
             showing {id}@{version} is not a licensed Cognitum binary; for your own build from source \
             pass --no-provenance-ok"
        )));
    }
    for f in &found {
        let (Some(_), Some(want)) = (&f.trust, &f.sha256) else {
            return Err(PackError::Input(format!(
                "{PROVENANCE_FILE} has no trust or sha256, so it cannot show the binary is shareable; --redistributable is refused"
            )));
        };
        let want = want.to_ascii_lowercase();
        let matches = match &f.beside {
            Some(arch) => bin_sha.get(arch).is_some_and(|h| *h == want),
            None => bin_sha.values().any(|h| *h == want),
        };
        if !matches {
            return Err(PackError::Input(format!(
                "{PROVENANCE_FILE} sha256 {want} does not match the binary being packed; --redistributable is refused"
            )));
        }
    }
    Ok(None)
}

/// Build an unsigned cog package in `out_dir` (which must not exist or be
/// empty) and return its envelope. Sign it with
/// [`super::sign::sign_envelope`] and persist with [`write_manifest`].
pub fn pack_cog(input: &CogPackInput, out_dir: &Path) -> Result<ManifestEnvelope, PackError> {
    if input.binaries.is_empty() {
        return Err(PackError::Input(
            "at least one --bin <arch>=<path> is required".into(),
        ));
    }
    let mut seen = BTreeMap::new();
    for (arch, path) in &input.binaries {
        if !COG_ARCHES.contains(&arch.as_str()) {
            return Err(PackError::Input(format!(
                "unknown arch {arch:?} (expected one of {COG_ARCHES:?})"
            )));
        }
        if seen.insert(arch.clone(), path.clone()).is_some() {
            return Err(PackError::Input(format!("arch {arch} given twice")));
        }
    }
    if out_dir.exists() {
        let mut entries = std::fs::read_dir(out_dir).map_err(|e| io_err(out_dir, e))?;
        if entries.next().is_some() {
            return Err(PackError::Input(format!(
                "output directory {} is not empty",
                out_dir.display()
            )));
        }
    }
    std::fs::create_dir_all(out_dir).map_err(|e| io_err(out_dir, e))?;

    let toml_path = input.cog_dir.join("cog.toml");
    let toml_bytes = read_limited(&toml_path, MAX_COG_TOML_BYTES)?;
    let toml_text = std::str::from_utf8(&toml_bytes)
        .map_err(|_| PackError::Input("cog.toml is not UTF-8".into()))?;
    let (id, version) = cog_identity(toml_text)?;
    let cog_toml = place(out_dir, "cog.toml", &toml_bytes, false)?;

    let mut binaries = BTreeMap::new();
    let mut bin_sha = BTreeMap::new();
    for (arch, path) in &seen {
        let content = read_limited(path, MAX_FILE_BYTES)?;
        bin_sha.insert(arch.clone(), sha256_hex(&content));
        binaries.insert(
            arch.clone(),
            place(out_dir, &binary_path(arch, &id), &content, true)?,
        );
    }

    let mut attestations = Vec::new();
    if let Some(record_path) = &input.cognitum_record {
        let content = read_limited(record_path, 256 * 1024)?;
        serde_json::from_slice::<Value>(&content)
            .map_err(|e| PackError::Input(format!("cognitum record is not JSON: {e}")))?;
        attestations.push(AttestationRef {
            kind: COGNITUM_RECORD_KIND.to_string(),
            file: place(out_dir, COGNITUM_RECORD_PATH, &content, false)?,
        });
    }

    if let Some(stamp) = provenance_decision(input, &id, &version, &bin_sha)? {
        attestations.push(AttestationRef {
            kind: COGNITUM_PROVENANCE_KIND.to_string(),
            file: place(out_dir, COGNITUM_PROVENANCE_PATH, &stamp, false)?,
        });
    }

    let body = CogPackageBody {
        id,
        version,
        cog_toml,
        binaries,
        source: input.source.clone(),
        attestations,
        redistributable: input.redistributable,
    };
    body.validate()?;
    let value = serde_json::to_value(&body).map_err(|e| ManifestError::Parse(e.to_string()))?;
    Ok(ManifestEnvelope::new(KIND_COG, value))
}

/// Write (or overwrite) `out_dir/cogpkg.json`.
pub fn write_manifest(out_dir: &Path, envelope: &ManifestEnvelope) -> Result<PathBuf, PackError> {
    let path = out_dir.join(MANIFEST_FILE);
    std::fs::write(&path, envelope.to_pretty_json()?).map_err(|e| io_err(&path, e))?;
    Ok(path)
}
